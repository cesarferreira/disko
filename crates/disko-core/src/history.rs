//! Snapshots on disk, so disko can answer "what changed since last time".
//!
//! Every full scan records one, which is what makes `disko diff` work without
//! anyone having set anything up first: by the time you think to ask what
//! happened, the evidence already exists.
//!
//! Snapshots are pruned before they are written — deep enough and coarse
//! enough to attribute growth, small enough that keeping months of them costs
//! a few megabytes.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::tree::DiskEntry;

/// How deep a stored tree goes. Growth attribution rarely needs more: below
/// this the answer is "this directory", not "this file".
const STORAGE_DEPTH: usize = 8;

/// Entries smaller than this are dropped from a snapshot. Ten thousandths of
/// the root, floored at 1 MB, so a 500 GB disk keeps everything above 50 MB
/// and a small project keeps everything above 1 MB.
const MIN_STORED_FRACTION: u64 = 10_000;
const MIN_STORED_BYTES: u64 = 1_000_000;

/// Snapshots kept per scanned root before the oldest are dropped.
const RETAIN: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub root: PathBuf,
    /// Seconds since the Unix epoch.
    pub taken_at: u64,
    /// Totals before pruning, so a snapshot's headline number is exact even
    /// though its tree is abridged.
    pub total_allocated: u64,
    pub total_apparent: u64,
    pub items: u64,
    /// Entries below this many bytes were dropped before storing. A diff needs
    /// it to tell "this is new" from "this was too small to record".
    #[serde(default)]
    pub floor: u64,
    pub tree: DiskEntry,
}

impl Snapshot {
    /// Seconds between this snapshot and `now`.
    pub fn age(&self, now: u64) -> u64 {
        now.saturating_sub(self.taken_at)
    }
}

/// A snapshot's identity without its tree, for listing without parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotId {
    pub taken_at: u64,
    path: PathBuf,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Where snapshots live: `~/.local/share/disko` on Linux,
/// `~/Library/Application Support/disko` on macOS.
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open() -> Result<Self> {
        let root = dirs::data_dir()
            .context("no data directory on this system")?
            .join("disko")
            .join("snapshots");
        fs::create_dir_all(&root).with_context(|| format!("cannot create {}", root.display()))?;
        Ok(Self { root })
    }

    /// A store rooted anywhere, for tests and for `--snapshot-dir`.
    pub fn at(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).with_context(|| format!("cannot create {}", root.display()))?;
        Ok(Self { root })
    }

    pub fn location(&self) -> &Path {
        &self.root
    }

    /// One directory per scanned root. The hash keeps the name short and
    /// filesystem-safe; the path itself is stored inside each snapshot.
    fn dir_for(&self, scanned_root: &Path) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        scanned_root.hash(&mut hasher);
        self.root.join(format!("{:016x}", hasher.finish()))
    }

    /// Store `tree` as the state of the world right now.
    pub fn record(&self, tree: &DiskEntry) -> Result<Snapshot> {
        self.record_at(tree, now())
    }

    pub fn record_at(&self, tree: &DiskEntry, taken_at: u64) -> Result<Snapshot> {
        let snapshot = Snapshot {
            root: tree.root_path().to_path_buf(),
            taken_at,
            total_allocated: tree.allocated_size,
            total_apparent: tree.apparent_size,
            items: tree.items,
            floor: storage_floor(tree),
            tree: prune_for_storage(tree),
        };

        let dir = self.dir_for(tree.root_path());
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;

        let file = dir.join(format!("{taken_at}.json"));
        let json = serde_json::to_vec(&snapshot)?;
        fs::write(&file, json).with_context(|| format!("cannot write {}", file.display()))?;

        self.prune(tree.root_path())?;
        Ok(snapshot)
    }

    /// Every snapshot of `scanned_root`, oldest first.
    pub fn list(&self, scanned_root: &Path) -> Vec<SnapshotId> {
        let dir = self.dir_for(scanned_root);
        let Ok(entries) = fs::read_dir(&dir) else {
            return Vec::new();
        };

        let mut ids: Vec<SnapshotId> = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();
                let taken_at = path.file_stem()?.to_str()?.parse().ok()?;
                Some(SnapshotId { taken_at, path })
            })
            .collect();
        ids.sort_by_key(|id| id.taken_at);
        ids
    }

    pub fn load(&self, id: &SnapshotId) -> Result<Snapshot> {
        let bytes =
            fs::read(&id.path).with_context(|| format!("cannot read {}", id.path.display()))?;
        // A snapshot written by an older disko is not worth failing over.
        serde_json::from_slice(&bytes)
            .with_context(|| format!("cannot parse {}", id.path.display()))
    }

    /// The most recent snapshot taken at or before `at`.
    ///
    /// "Since 7 days ago" means the last picture of the world before that
    /// moment, not the first one after it.
    pub fn latest_before(&self, scanned_root: &Path, at: u64) -> Option<Snapshot> {
        self.list(scanned_root)
            .iter()
            .rev()
            .find(|id| id.taken_at <= at)
            .and_then(|id| self.load(id).ok())
    }

    /// The newest snapshot, whenever it was taken.
    pub fn latest(&self, scanned_root: &Path) -> Option<Snapshot> {
        self.list(scanned_root)
            .last()
            .and_then(|id| self.load(id).ok())
    }

    /// The root of whichever scan was recorded most recently, anywhere.
    ///
    /// This is what lets `disko diff` with no arguments mean "the thing I
    /// just looked at".
    pub fn most_recent_root(&self) -> Option<PathBuf> {
        let entries = fs::read_dir(&self.root).ok()?;
        let mut newest: Option<(u64, PathBuf)> = None;

        for dir in entries.filter_map(|entry| entry.ok()) {
            let Ok(files) = fs::read_dir(dir.path()) else {
                continue;
            };
            let latest = files
                .filter_map(|file| file.ok())
                .filter_map(|file| {
                    let path = file.path();
                    let taken_at: u64 = path.file_stem()?.to_str()?.parse().ok()?;
                    Some((taken_at, path))
                })
                .max_by_key(|(taken_at, _)| *taken_at);

            if let Some((taken_at, path)) = latest
                && newest.as_ref().is_none_or(|(best, _)| taken_at > *best)
                && let Ok(snapshot) = self.load(&SnapshotId { taken_at, path })
            {
                newest = Some((taken_at, snapshot.root));
            }
        }

        newest.map(|(_, root)| root)
    }

    /// Every snapshot, loaded, oldest first — for plotting a path's history.
    pub fn load_all(&self, scanned_root: &Path) -> Vec<Snapshot> {
        self.list(scanned_root)
            .iter()
            .filter_map(|id| self.load(id).ok())
            .collect()
    }

    fn prune(&self, scanned_root: &Path) -> Result<()> {
        let ids = self.list(scanned_root);
        if ids.len() <= RETAIN {
            return Ok(());
        }
        for id in &ids[..ids.len() - RETAIN] {
            let _ = fs::remove_file(&id.path);
        }
        Ok(())
    }

    /// Forget everything about `scanned_root`.
    pub fn forget(&self, scanned_root: &Path) -> Result<()> {
        let dir = self.dir_for(scanned_root);
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| format!("cannot remove {}", dir.display()))?;
        }
        Ok(())
    }

    /// Bytes currently held by all snapshots.
    pub fn disk_usage(&self) -> u64 {
        fn walk(dir: &Path) -> u64 {
            let Ok(entries) = fs::read_dir(dir) else {
                return 0;
            };
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| match entry.metadata() {
                    Ok(meta) if meta.is_dir() => walk(&entry.path()),
                    Ok(meta) => meta.len(),
                    Err(_) => 0,
                })
                .sum()
        }
        walk(&self.root)
    }
}

/// The smallest entry a snapshot of this tree will keep.
pub fn storage_floor(tree: &DiskEntry) -> u64 {
    (tree.allocated_size / MIN_STORED_FRACTION).max(MIN_STORED_BYTES)
}

/// Trim a tree down to what a diff actually needs.
pub fn prune_for_storage(tree: &DiskEntry) -> DiskEntry {
    prune(tree, storage_floor(tree), STORAGE_DEPTH)
}

fn prune(entry: &DiskEntry, floor: u64, depth: usize) -> DiskEntry {
    // Deliberately `without_children` rather than `clone`: this walks the whole
    // tree, and cloning each node would deep-copy the very subtree the next
    // line is about to replace. Pruning a two-million entry home directory down
    // to the couple of thousand nodes worth storing used to allocate 1.4 GB to
    // do it, none of which the allocator gave back.
    let mut copy = entry.without_children();
    if depth > 0 {
        copy.children = entry
            .children
            .iter()
            .filter(|child| child.allocated_size >= floor)
            .map(|child| prune(child, floor, depth - 1))
            .collect();
    }
    copy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::EntryType;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!("disko-history-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn entry(path: &str, size: u64, children: Vec<DiskEntry>) -> DiskEntry {
        let mut entry = DiskEntry::new(PathBuf::from(path), EntryType::Directory);
        entry.allocated_size = size;
        entry.apparent_size = size;
        entry.children = children;
        entry
    }

    #[test]
    fn a_recorded_snapshot_comes_back_intact() {
        let dir = TempDir::new("roundtrip");
        let store = Store::at(&dir.0).unwrap();
        let tree = entry(
            "/data",
            5_000_000,
            vec![entry("/data/big", 4_000_000, vec![])],
        );

        store.record_at(&tree, 1000).unwrap();
        let loaded = store.latest(Path::new("/data")).unwrap();

        assert_eq!(loaded.taken_at, 1000);
        assert_eq!(loaded.root, PathBuf::from("/data"));
        assert_eq!(loaded.total_allocated, 5_000_000);
        assert_eq!(loaded.tree.children.len(), 1);
    }

    #[test]
    fn snapshots_of_different_roots_do_not_mix() {
        let dir = TempDir::new("roots");
        let store = Store::at(&dir.0).unwrap();

        store
            .record_at(&entry("/a", 1_000_000, vec![]), 100)
            .unwrap();
        store
            .record_at(&entry("/b", 2_000_000, vec![]), 200)
            .unwrap();

        assert_eq!(store.list(Path::new("/a")).len(), 1);
        assert_eq!(
            store.latest(Path::new("/b")).unwrap().total_allocated,
            2_000_000
        );
    }

    #[test]
    fn latest_before_picks_the_last_snapshot_that_predates_the_cutoff() {
        let dir = TempDir::new("before");
        let store = Store::at(&dir.0).unwrap();
        for (at, size) in [(100u64, 1u64), (200, 2), (300, 3)] {
            store
                .record_at(&entry("/data", size * 1_000_000, vec![]), at)
                .unwrap();
        }

        assert_eq!(
            store
                .latest_before(Path::new("/data"), 250)
                .unwrap()
                .taken_at,
            200
        );
        assert_eq!(
            store
                .latest_before(Path::new("/data"), 300)
                .unwrap()
                .taken_at,
            300
        );
        // Nothing that old exists yet.
        assert!(store.latest_before(Path::new("/data"), 50).is_none());
    }

    #[test]
    fn old_snapshots_are_pruned_but_the_recent_ones_survive() {
        let dir = TempDir::new("prune");
        let store = Store::at(&dir.0).unwrap();
        for at in 1..=(RETAIN as u64 + 10) {
            store
                .record_at(&entry("/data", at * 1_000_000, vec![]), at)
                .unwrap();
        }

        let kept = store.list(Path::new("/data"));
        assert_eq!(kept.len(), RETAIN);
        assert_eq!(kept.last().unwrap().taken_at, RETAIN as u64 + 10);
    }

    #[test]
    fn pruning_for_storage_drops_noise_and_caps_depth() {
        let mut deep = entry("/data/deep", 900_000_000, vec![]);
        for level in 0..12 {
            deep = entry(&format!("/data/level{level}"), 900_000_000, vec![deep]);
        }
        let tree = entry(
            "/data",
            1_000_000_000,
            vec![deep, entry("/data/noise", 500, vec![])],
        );

        let pruned = prune_for_storage(&tree);

        // The tiny entry is below the floor (1 GB / 10_000 = 100 KB).
        assert_eq!(pruned.children.len(), 1);
        // Totals on the surviving nodes are untouched.
        assert_eq!(pruned.allocated_size, 1_000_000_000);

        let mut depth = 0;
        let mut node = &pruned;
        while let Some(child) = node.children.first() {
            depth += 1;
            node = child;
        }
        assert_eq!(depth, STORAGE_DEPTH);
    }

    #[test]
    fn forgetting_a_root_removes_its_snapshots() {
        let dir = TempDir::new("forget");
        let store = Store::at(&dir.0).unwrap();
        store
            .record_at(&entry("/data", 1_000_000, vec![]), 1)
            .unwrap();

        store.forget(Path::new("/data")).unwrap();
        assert!(store.list(Path::new("/data")).is_empty());
    }

    #[test]
    fn an_unknown_root_has_no_history_rather_than_an_error() {
        let dir = TempDir::new("empty");
        let store = Store::at(&dir.0).unwrap();

        assert!(store.list(Path::new("/never-scanned")).is_empty());
        assert!(store.latest(Path::new("/never-scanned")).is_none());
    }
}
