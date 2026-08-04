//! Comparing two scans, and working out *which directory to blame*.
//!
//! A raw diff of two trees says "your home directory grew 82 GB", which you
//! already knew. The useful answer names the deepest directory that still
//! accounts for the growth — `~/Library/Developer/Xcode/DerivedData`, not
//! `~/Library`, and not the two hundred per-project folders inside it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::size::SizeKind;
use crate::tree::DiskEntry;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    /// Not present in the earlier scan.
    Added,
    Grown,
    Shrunk,
    /// Present earlier, gone now.
    Removed,
    Unchanged,
}

#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub path: PathBuf,
    pub before: u64,
    pub after: u64,
    pub delta: i64,
    pub kind: ChangeKind,
    /// Newest modification time in this subtree, from the later scan.
    pub modified: u64,
    /// True when `before` is an upper bound rather than a measurement: the
    /// entry sat below the snapshot's storage floor, so it may have existed at
    /// a smaller size instead of being genuinely new.
    pub before_is_bound: bool,
}

impl Change {
    pub fn is_growth(&self) -> bool {
        self.delta > 0
    }

    /// Share of a total change this entry accounts for, 0.0..=1.0.
    pub fn share_of(&self, total: i64) -> f64 {
        if total == 0 {
            return 0.0;
        }
        (self.delta as f64 / total as f64).abs().clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ChangeNode {
    pub change: Change,
    pub children: Vec<ChangeNode>,
}

impl ChangeNode {
    pub fn find(&self, path: &Path) -> Option<&ChangeNode> {
        if self.change.path == path {
            return Some(self);
        }
        // Only descend the branch that could contain it.
        if !path.starts_with(&self.change.path) {
            return None;
        }
        self.children.iter().find_map(|child| child.find(path))
    }

    fn max_abs_delta(&self) -> i64 {
        self.children
            .iter()
            .map(ChangeNode::max_abs_delta)
            .chain(std::iter::once(self.change.delta.abs()))
            .max()
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Diff {
    pub root: PathBuf,
    pub before_at: u64,
    pub after_at: u64,
    pub before_total: u64,
    pub after_total: u64,
    pub tree: ChangeNode,
}

/// A mover has to be at least this big a slice of the largest single movement
/// before disko descends into it looking for a more specific culprit.
const SIGNIFICANCE: f64 = 0.05;

/// ...and at least this many bytes, so a quiet week does not produce a list of
/// log files that grew by a kilobyte.
const MIN_ATTRIBUTION: i64 = 10_000_000;

impl Diff {
    pub fn total_delta(&self) -> i64 {
        self.tree.change.delta
    }

    pub fn elapsed(&self) -> u64 {
        self.after_at.saturating_sub(self.before_at)
    }

    /// The directories that grew, most first, each named as specifically as
    /// the evidence supports.
    pub fn growth(&self, limit: usize) -> Vec<&Change> {
        self.attributed(limit, true)
    }

    /// The same, for space that was freed.
    pub fn shrinkage(&self, limit: usize) -> Vec<&Change> {
        self.attributed(limit, false)
    }

    pub fn change_for(&self, path: &Path) -> Option<&Change> {
        self.tree.find(path).map(|node| &node.change)
    }

    /// The same as [`growth`](Self::growth), but you choose where the trail
    /// stops. `growth` stops at recognised caches; a caller wanting the exact
    /// file can pass `|_| false`.
    pub fn growth_stopping_at(&self, limit: usize, stop: &dyn Fn(&Path) -> bool) -> Vec<&Change> {
        self.attributed_with(limit, true, stop)
    }

    fn attributed(&self, limit: usize, growing: bool) -> Vec<&Change> {
        // Once growth reaches something with a name — "Gradle caches" — that
        // is the answer. Descending further only trades a useful label for an
        // arbitrary file inside it.
        self.attributed_with(limit, growing, &|path| {
            crate::categories::classify(path).is_some()
        })
    }

    fn attributed_with(
        &self,
        limit: usize,
        growing: bool,
        stop: &dyn Fn(&Path) -> bool,
    ) -> Vec<&Change> {
        // Scale the floor to the biggest thing that moved rather than to the
        // net change: a week where one cache grew 40 GB and another shrank
        // 40 GB nets to zero but is not a quiet week.
        let floor = ((self.tree.max_abs_delta() as f64 * SIGNIFICANCE) as i64).max(MIN_ATTRIBUTION);

        let mut out = Vec::new();
        collect(&self.tree, floor, growing, stop, &mut out);
        out.retain(|change| {
            if growing {
                change.delta >= floor
            } else {
                change.delta <= -floor
            }
        });
        out.sort_by_key(|change| if growing { -change.delta } else { change.delta });
        out.truncate(limit);
        out
    }
}

/// Walk down while some child still explains the movement; emit the node where
/// the trail goes cold, or where it reaches something already worth naming.
fn collect<'a>(
    node: &'a ChangeNode,
    floor: i64,
    growing: bool,
    stop: &dyn Fn(&Path) -> bool,
    out: &mut Vec<&'a Change>,
) {
    if stop(&node.change.path) {
        out.push(&node.change);
        return;
    }

    let qualifies = |candidate: &ChangeNode| {
        if growing {
            candidate.change.delta >= floor
        } else {
            candidate.change.delta <= -floor
        }
    };

    let mut descended = false;
    for child in node.children.iter().filter(|child| qualifies(child)) {
        descended = true;
        collect(child, floor, growing, stop, out);
    }

    // Nothing below is big enough to name: this directory is the answer.
    if !descended {
        out.push(&node.change);
    }
}

/// Compare two scans of the same root.
///
/// `before_floor` is the snapshot storage floor that produced `before`; it is
/// what lets the result distinguish "this is new" from "this was too small to
/// record".
pub fn diff(
    before: &DiskEntry,
    after: &DiskEntry,
    kind: SizeKind,
    before_at: u64,
    after_at: u64,
    before_floor: u64,
) -> Diff {
    Diff {
        root: after.path.clone(),
        before_at,
        after_at,
        before_total: before.size(kind),
        after_total: after.size(kind),
        tree: compare(Some(before), Some(after), &after.path, kind, before_floor),
    }
}

fn compare(
    before: Option<&DiskEntry>,
    after: Option<&DiskEntry>,
    path: &Path,
    kind: SizeKind,
    floor: u64,
) -> ChangeNode {
    let before_size = before.map(|entry| entry.size(kind)).unwrap_or(0);
    let after_size = after.map(|entry| entry.size(kind)).unwrap_or(0);
    let delta = after_size as i64 - before_size as i64;

    let change = Change {
        path: path.to_path_buf(),
        before: before_size,
        after: after_size,
        delta,
        kind: classify(before.is_some(), after.is_some(), delta),
        modified: after.map(|entry| entry.modified).unwrap_or(0),
        before_is_bound: before.is_none() && floor > 0,
    };

    // Pair children up by name; a name on one side only is an addition or a
    // removal.
    let mut pairs: BTreeMap<&std::ffi::OsStr, (Option<&DiskEntry>, Option<&DiskEntry>)> =
        BTreeMap::new();
    for entry in before.into_iter().flat_map(|entry| entry.children.iter()) {
        if let Some(name) = entry.path.file_name() {
            pairs.entry(name).or_default().0 = Some(entry);
        }
    }
    for entry in after.into_iter().flat_map(|entry| entry.children.iter()) {
        if let Some(name) = entry.path.file_name() {
            pairs.entry(name).or_default().1 = Some(entry);
        }
    }

    let children = pairs
        .into_iter()
        .map(|(name, (old, new))| {
            let child_path = new
                .map(|entry| entry.path.clone())
                .unwrap_or_else(|| path.join(name));
            compare(old, new, &child_path, kind, floor)
        })
        .collect();

    ChangeNode { change, children }
}

fn classify(existed: bool, exists: bool, delta: i64) -> ChangeKind {
    match (existed, exists) {
        (false, true) => ChangeKind::Added,
        (true, false) => ChangeKind::Removed,
        _ if delta > 0 => ChangeKind::Grown,
        _ if delta < 0 => ChangeKind::Shrunk,
        _ => ChangeKind::Unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::EntryType;

    const GB: u64 = 1_000_000_000;

    fn dir(path: &str, size: u64, children: Vec<DiskEntry>) -> DiskEntry {
        let mut entry = DiskEntry::new(PathBuf::from(path), EntryType::Directory);
        entry.allocated_size = size;
        entry.apparent_size = size;
        entry.children = children;
        entry
    }

    fn compare_trees(before: &DiskEntry, after: &DiskEntry) -> Diff {
        diff(before, after, SizeKind::Allocated, 1000, 2000, 0)
    }

    #[test]
    fn totals_and_elapsed_time_come_through() {
        let before = dir("/home", 100 * GB, vec![]);
        let after = dir("/home", 182 * GB, vec![]);

        let diff = compare_trees(&before, &after);

        assert_eq!(diff.total_delta(), 82 * GB as i64);
        assert_eq!(diff.before_total, 100 * GB);
        assert_eq!(diff.after_total, 182 * GB);
        assert_eq!(diff.elapsed(), 1000);
    }

    /// The headline case: growth spread over four directories should name all
    /// four, not their common parent.
    #[test]
    fn growth_is_attributed_to_each_directory_that_owns_it() {
        let before = dir(
            "/home",
            100 * GB,
            vec![dir("/home/Library", 10 * GB, vec![])],
        );
        let after = dir(
            "/home",
            182 * GB,
            vec![
                dir("/home/Library", 56 * GB, vec![]),
                dir("/home/.gradle", 18 * GB, vec![]),
                dir("/home/Downloads", 11 * GB, vec![]),
                dir("/home/.docker", 7 * GB, vec![]),
            ],
        );

        let diff = compare_trees(&before, &after);
        let growth = diff.growth(10);

        let named: Vec<String> = growth
            .iter()
            .map(|change| change.path.display().to_string())
            .collect();
        assert_eq!(
            named,
            [
                "/home/Library",
                "/home/.gradle",
                "/home/Downloads",
                "/home/.docker"
            ]
        );
        assert_eq!(growth[0].delta, 46 * GB as i64);
    }

    /// When growth concentrates in one deep directory, follow it all the way
    /// down rather than stopping at the top.
    #[test]
    fn a_single_dominant_branch_is_followed_to_its_source() {
        let before = dir("/home", 10 * GB, vec![]);
        let after = dir(
            "/home",
            56 * GB,
            vec![dir(
                "/home/Library",
                46 * GB,
                vec![dir(
                    "/home/Library/Developer",
                    46 * GB,
                    vec![dir("/home/Library/Developer/DerivedData", 46 * GB, vec![])],
                )],
            )],
        );

        let growth = compare_trees(&before, &after);
        let growth = growth.growth(5);

        assert_eq!(growth.len(), 1);
        assert_eq!(
            growth[0].path,
            PathBuf::from("/home/Library/Developer/DerivedData")
        );
    }

    /// ...but stop before fragmenting into hundreds of equally small children.
    #[test]
    fn diffuse_growth_stops_at_the_directory_that_contains_it() {
        let projects: Vec<DiskEntry> = (0..40)
            .map(|index| dir(&format!("/home/cache/p{index}"), GB, vec![]))
            .collect();
        let before = dir("/home", 10 * GB, vec![]);
        let after = dir(
            "/home",
            50 * GB,
            vec![dir("/home/cache", 40 * GB, projects)],
        );

        let growth = compare_trees(&before, &after);
        let growth = growth.growth(50);

        assert_eq!(growth.len(), 1, "expected one culprit, got {growth:#?}");
        assert_eq!(growth[0].path, PathBuf::from("/home/cache"));
    }

    /// A directory disko can name — "Gradle caches" — is a better answer than
    /// whichever blob inside it happens to be biggest.
    #[test]
    fn attribution_stops_at_something_worth_naming() {
        let before = dir("/home", 10 * GB, vec![]);
        let after = dir(
            "/home",
            40 * GB,
            vec![dir(
                "/home/cache",
                30 * GB,
                vec![dir("/home/cache/blob", 30 * GB, vec![])],
            )],
        );
        let computed = compare_trees(&before, &after);

        let named = computed.growth_stopping_at(5, &|path| path.ends_with("cache"));
        assert_eq!(named.len(), 1);
        assert_eq!(named[0].path, PathBuf::from("/home/cache"));

        // Without a stop rule the trail runs all the way to the blob.
        let exact = computed.growth_stopping_at(5, &|_| false);
        assert_eq!(exact[0].path, PathBuf::from("/home/cache/blob"));
    }

    #[test]
    fn shrinkage_is_reported_separately_from_growth() {
        let before = dir(
            "/home",
            60 * GB,
            vec![
                dir("/home/keep", 10 * GB, vec![]),
                dir("/home/gone", 50 * GB, vec![]),
            ],
        );
        let after = dir("/home", 10 * GB, vec![dir("/home/keep", 10 * GB, vec![])]);

        let diff = compare_trees(&before, &after);

        assert!(diff.growth(10).is_empty());
        let freed = diff.shrinkage(10);
        assert_eq!(freed.len(), 1);
        assert_eq!(freed[0].path, PathBuf::from("/home/gone"));
        assert_eq!(freed[0].kind, ChangeKind::Removed);
        assert_eq!(freed[0].delta, -(50 * GB as i64));
    }

    /// A week that nets to zero because one cache exploded while another was
    /// cleared is not a quiet week.
    #[test]
    fn offsetting_movements_are_both_reported() {
        let before = dir(
            "/home",
            50 * GB,
            vec![
                dir("/home/old", 40 * GB, vec![]),
                dir("/home/new", 10 * GB, vec![]),
            ],
        );
        let after = dir(
            "/home",
            50 * GB,
            vec![
                dir("/home/old", 0, vec![]),
                dir("/home/new", 50 * GB, vec![]),
            ],
        );

        let diff = compare_trees(&before, &after);

        assert_eq!(diff.total_delta(), 0);
        assert_eq!(diff.growth(5).len(), 1);
        assert_eq!(diff.shrinkage(5).len(), 1);
    }

    #[test]
    fn a_quiet_period_reports_nothing() {
        let before = dir("/home", 100 * GB, vec![dir("/home/stuff", 50 * GB, vec![])]);
        let mut after = before.clone();
        // A few kilobytes of log churn is not news.
        after.allocated_size += 4096;

        let diff = compare_trees(&before, &after);

        assert!(diff.growth(10).is_empty());
        assert!(diff.shrinkage(10).is_empty());
    }

    #[test]
    fn new_directories_are_marked_added() {
        let before = dir("/home", 10 * GB, vec![]);
        let after = dir("/home", 30 * GB, vec![dir("/home/fresh", 20 * GB, vec![])]);

        let growth = compare_trees(&before, &after);
        let growth = growth.growth(5);

        assert_eq!(growth[0].kind, ChangeKind::Added);
        assert_eq!(growth[0].before, 0);
        assert!(
            !growth[0].before_is_bound,
            "an unpruned scan knows it was absent"
        );
    }

    /// A pruned snapshot cannot tell "new" from "was below the noise floor",
    /// and says so instead of guessing.
    #[test]
    fn growth_out_of_a_pruned_snapshot_is_flagged_as_a_bound() {
        let before = dir("/home", 10 * GB, vec![]);
        let after = dir("/home", 30 * GB, vec![dir("/home/fresh", 20 * GB, vec![])]);

        let diff = diff(&before, &after, SizeKind::Allocated, 0, 1, 1_000_000);
        let growth = diff.growth(5);

        assert!(growth[0].before_is_bound);
    }

    #[test]
    fn a_change_can_be_looked_up_by_path() {
        let before = dir("/home", 10 * GB, vec![dir("/home/a", 5 * GB, vec![])]);
        let after = dir("/home", 20 * GB, vec![dir("/home/a", 15 * GB, vec![])]);

        let diff = compare_trees(&before, &after);

        assert_eq!(
            diff.change_for(Path::new("/home/a")).unwrap().delta,
            10 * GB as i64
        );
        assert!(diff.change_for(Path::new("/home/nope")).is_none());
    }

    #[test]
    fn shares_are_relative_to_the_whole_change() {
        let before = dir("/home", 0, vec![]);
        let after = dir("/home", 100, vec![dir("/home/half", 50, vec![])]);
        let diff = compare_trees(&before, &after);

        let change = diff.change_for(Path::new("/home/half")).unwrap();
        assert!((change.share_of(100) - 0.5).abs() < 1e-9);
        assert_eq!(change.share_of(0), 0.0);
    }
}
