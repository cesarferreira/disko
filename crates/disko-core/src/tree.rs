//! The neutral data the scanner produces. No formatting, no colours — the TUI,
//! `--json` and any other consumer all read this same shape.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::size::SizeKind;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    Directory,
    File,
    Symlink,
    Other,
}

/// How much of the truth an entry's sizes represent.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanState {
    /// Fully walked.
    Complete,
    /// Walked, but something underneath was denied, cancelled or depth-capped.
    Partial,
    /// `read_dir` failed — almost always a permission error.
    Denied,
    /// The user stopped the scan before this subtree finished.
    Cancelled,
    /// Deliberately not descended into (other filesystem, depth limit).
    Skipped,
}

impl ScanState {
    /// Worst-of, so a single denied leaf marks its ancestors partial.
    fn merge(self, child: ScanState) -> ScanState {
        match (self, child) {
            (ScanState::Complete, ScanState::Complete) => ScanState::Complete,
            (ScanState::Cancelled, _) | (_, ScanState::Cancelled) => ScanState::Cancelled,
            (ScanState::Denied, _) => ScanState::Denied,
            _ => ScanState::Partial,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiskEntry {
    pub path: PathBuf,
    /// Sum of file lengths, as `ls` reports.
    pub apparent_size: u64,
    /// Blocks actually occupied, as `du` reports.
    pub allocated_size: u64,
    pub entry_type: EntryType,
    /// Number of filesystem entries in this subtree, including this one.
    pub items: u64,
    /// Newest modification time anywhere in this subtree, in seconds since the
    /// Unix epoch. This is what answers "when was this last actually used" —
    /// a 30 GB cache nobody has touched in four months is a very different
    /// proposition from one that is warm.
    #[serde(default)]
    pub modified: u64,
    pub children: Vec<DiskEntry>,
    pub scan_state: ScanState,
}

impl DiskEntry {
    pub fn new(path: PathBuf, entry_type: EntryType) -> Self {
        Self {
            path,
            apparent_size: 0,
            allocated_size: 0,
            entry_type,
            items: 1,
            modified: 0,
            children: Vec::new(),
            scan_state: ScanState::Complete,
        }
    }

    /// The last path component, falling back to the whole path for roots like
    /// `/` which have no file name.
    pub fn name(&self) -> Cow<'_, str> {
        match self.path.file_name() {
            Some(name) => name.to_string_lossy(),
            None => self.path.to_string_lossy(),
        }
    }

    /// This entry's own figures, with an empty child list.
    ///
    /// Anything building a reduced copy of a tree wants this rather than
    /// `clone()`: cloning deep-copies every descendant, so a caller that only
    /// means to keep a couple of levels still pays for the whole subtree first
    /// and then throws it away.
    pub fn without_children(&self) -> DiskEntry {
        DiskEntry {
            path: self.path.clone(),
            apparent_size: self.apparent_size,
            allocated_size: self.allocated_size,
            entry_type: self.entry_type,
            items: self.items,
            modified: self.modified,
            children: Vec::new(),
            scan_state: self.scan_state,
        }
    }

    pub fn size(&self, kind: SizeKind) -> u64 {
        match kind {
            SizeKind::Allocated => self.allocated_size,
            SizeKind::Apparent => self.apparent_size,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.entry_type == EntryType::Directory
    }

    /// Roll a finished child's totals into this entry.
    pub fn absorb(&mut self, child: &DiskEntry) {
        self.apparent_size += child.apparent_size;
        self.allocated_size += child.allocated_size;
        self.items += child.items;
        self.modified = self.modified.max(child.modified);
        self.scan_state = self.scan_state.merge(child.scan_state);
    }

    pub fn child(&self, name: &str) -> Option<&DiskEntry> {
        self.children.iter().find(|c| c.name() == name)
    }

    /// Walk down to `path`, which must live under this entry. Returns `None`
    /// if any component is missing (a file that was deleted mid-session, or a
    /// subtree the depth limit pruned).
    pub fn resolve(&self, path: &Path) -> Option<&DiskEntry> {
        let relative = path.strip_prefix(&self.path).ok()?;
        let mut node = self;
        for component in relative.components() {
            let name = component.as_os_str().to_string_lossy();
            node = node.child(&name)?;
        }
        Some(node)
    }

    /// Remove `path` from this tree, subtracting its totals from every
    /// ancestor on the way back up.
    ///
    /// Lets the display correct itself the instant something is deleted,
    /// without paying for a full rescan of a tree that may have taken minutes
    /// to build.
    pub fn remove(&mut self, path: &Path) -> Option<DiskEntry> {
        // A tree cannot remove itself, and nothing outside it is its business.
        if path == self.path || !path.starts_with(&self.path) {
            return None;
        }

        if let Some(index) = self.children.iter().position(|child| child.path == path) {
            let removed = self.children.remove(index);
            self.subtract(&removed);
            return Some(removed);
        }

        for child in &mut self.children {
            if path.starts_with(&child.path)
                && let Some(removed) = child.remove(path)
            {
                self.subtract(&removed);
                return Some(removed);
            }
        }
        None
    }

    fn subtract(&mut self, child: &DiskEntry) {
        self.apparent_size = self.apparent_size.saturating_sub(child.apparent_size);
        self.allocated_size = self.allocated_size.saturating_sub(child.allocated_size);
        self.items = self.items.saturating_sub(child.items);
    }

    /// Sorts every level largest-first. The TUI re-sorts by other keys itself;
    /// this is the ordering `--json` and `--plain` ship with.
    pub fn sort_by_size(&mut self, kind: SizeKind) {
        self.children.sort_by(|a, b| {
            b.size(kind)
                .cmp(&a.size(kind))
                .then_with(|| a.name().cmp(&b.name()))
        });
        for child in &mut self.children {
            child.sort_by_size(kind);
        }
    }

    /// Every descendant exactly `depth` levels below this entry.
    ///
    /// Used for the "Largest items" list: children answer *what* is big, but
    /// grandchildren answer *where to look next* — `~/Downloads` is actionable
    /// in a way that `Users` is not.
    pub fn descendants_at_depth(&self, depth: usize) -> Vec<&DiskEntry> {
        let mut out = Vec::new();
        self.collect_at_depth(depth, &mut out);
        out
    }

    fn collect_at_depth<'a>(&'a self, depth: usize, out: &mut Vec<&'a DiskEntry>) {
        if depth == 0 {
            out.push(self);
            return;
        }
        for child in &self.children {
            child.collect_at_depth(depth - 1, out);
        }
    }

    /// The `n` biggest entries `depth` levels down, largest first. Falls back
    /// to shallower levels when the tree is not that deep.
    pub fn largest_at_depth(&self, depth: usize, n: usize, kind: SizeKind) -> Vec<&DiskEntry> {
        let mut candidates = self.descendants_at_depth(depth);
        for shallower in (1..depth).rev() {
            if !candidates.is_empty() {
                break;
            }
            candidates = self.descendants_at_depth(shallower);
        }
        candidates.sort_by_key(|entry| std::cmp::Reverse(entry.size(kind)));
        candidates.truncate(n);
        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(path: &str, size: u64, children: Vec<DiskEntry>) -> DiskEntry {
        let mut entry = DiskEntry::new(PathBuf::from(path), EntryType::Directory);
        entry.allocated_size = size;
        entry.apparent_size = size;
        entry.children = children;
        entry
    }

    fn tree() -> DiskEntry {
        dir(
            "/root",
            100,
            vec![
                dir("/root/a", 70, vec![dir("/root/a/deep", 65, vec![])]),
                dir("/root/b", 30, vec![dir("/root/b/deep", 10, vec![])]),
            ],
        )
    }

    #[test]
    fn resolves_nested_paths() {
        let root = tree();
        assert_eq!(
            root.resolve(Path::new("/root/a/deep")).unwrap().name(),
            "deep"
        );
        assert_eq!(root.resolve(Path::new("/root")).unwrap().name(), "root");
        assert!(root.resolve(Path::new("/root/missing")).is_none());
        assert!(root.resolve(Path::new("/elsewhere")).is_none());
    }

    #[test]
    fn largest_at_depth_looks_past_direct_children() {
        let root = tree();
        let largest = root.largest_at_depth(2, 2, SizeKind::Allocated);
        let names: Vec<_> = largest
            .iter()
            .map(|e| e.path.display().to_string())
            .collect();
        assert_eq!(names, vec!["/root/a/deep", "/root/b/deep"]);
    }

    #[test]
    fn largest_at_depth_falls_back_when_tree_is_shallow() {
        let root = dir("/root", 10, vec![dir("/root/a", 5, vec![])]);
        let largest = root.largest_at_depth(3, 5, SizeKind::Allocated);
        assert_eq!(largest.len(), 1);
        assert_eq!(largest[0].name(), "a");
    }

    #[test]
    fn removing_an_entry_corrects_every_ancestor() {
        let mut root = tree();
        let before = root.allocated_size;

        let removed = root.remove(Path::new("/root/a/deep")).unwrap();

        assert_eq!(removed.allocated_size, 65);
        assert_eq!(root.allocated_size, before - 65);
        assert_eq!(root.child("a").unwrap().allocated_size, 70 - 65);
        assert!(root.resolve(Path::new("/root/a/deep")).is_none());
    }

    #[test]
    fn removing_a_whole_branch_takes_its_children_with_it() {
        let mut root = tree();
        let removed = root.remove(Path::new("/root/a")).unwrap();

        assert_eq!(removed.children.len(), 1);
        assert_eq!(root.allocated_size, 100 - 70);
        assert!(root.child("a").is_none());
    }

    #[test]
    fn a_tree_refuses_to_remove_itself_or_a_stranger() {
        let mut root = tree();
        assert!(root.remove(Path::new("/root")).is_none());
        assert!(root.remove(Path::new("/elsewhere")).is_none());
        assert!(root.remove(Path::new("/root/missing")).is_none());
        assert_eq!(
            root.allocated_size, 100,
            "nothing should have been subtracted"
        );
    }

    #[test]
    fn a_copy_without_children_keeps_every_figure() {
        let root = tree();
        let bare = root.child("a").unwrap().without_children();

        assert_eq!(bare.path, PathBuf::from("/root/a"));
        assert_eq!(bare.allocated_size, 70);
        assert_eq!(bare.apparent_size, 70);
        assert_eq!(bare.entry_type, EntryType::Directory);
        // The subtree is what is left behind, and only the subtree.
        assert!(bare.children.is_empty());
    }

    #[test]
    fn scan_state_degrades_to_partial() {
        let mut parent = DiskEntry::new(PathBuf::from("/p"), EntryType::Directory);
        let mut denied = DiskEntry::new(PathBuf::from("/p/c"), EntryType::Directory);
        denied.scan_state = ScanState::Denied;
        parent.absorb(&denied);
        assert_eq!(parent.scan_state, ScanState::Partial);
    }
}
