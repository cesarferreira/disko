//! The neutral data the scanner produces. No formatting, no colours — the TUI,
//! `--json` and any other consumer all read this same shape.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
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

#[derive(Clone, Debug)]
pub struct DiskEntry {
    /// What this entry is called — and, on the root of a scan, the whole path
    /// it was given, since the root is the one entry with no parent to ask.
    ///
    /// Holding a full path on every entry means storing each directory's name
    /// again for every single file beneath it. On a home directory with two
    /// million entries in it that came to 481 MB of paths where the names
    /// alone are 75 MB. Anything needing a full path builds it on the way
    /// down, which is how the tree is walked anyway.
    ///
    /// Read it through [`DiskEntry::name_os`] rather than directly: that
    /// returns the last component either way, so the root and everything under
    /// it can be treated alike.
    name: Box<OsStr>,
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
    pub modified: u64,
    pub children: Vec<DiskEntry>,
    pub scan_state: ScanState,
}

impl DiskEntry {
    /// Takes either a bare name, for an entry inside a scan, or a whole path,
    /// for the root of one. Both read back the same through [`Self::name_os`].
    pub fn new(name: impl Into<OsString>, entry_type: EntryType) -> Self {
        Self {
            name: name.into().into_boxed_os_str(),
            apparent_size: 0,
            allocated_size: 0,
            entry_type,
            items: 1,
            modified: 0,
            children: Vec::new(),
            scan_state: ScanState::Complete,
        }
    }

    /// The last component of what this entry stores.
    ///
    /// A bare name has no separators in it, so taking its file name gives it
    /// straight back; a root's whole path yields the directory it points at.
    /// That is what lets both be stored in the same field.
    pub fn name_os(&self) -> &OsStr {
        // `file_name` is `None` only for `/`, `.` and `..`, none of which
        // `read_dir` produces and none of which survive canonicalising a root.
        Path::new(&self.name).file_name().unwrap_or(&self.name)
    }

    /// The last path component, falling back to the whole path for roots like
    /// `/` which have no file name.
    pub fn name(&self) -> Cow<'_, str> {
        self.name_os().to_string_lossy()
    }

    /// The path this entry stands for — meaningful only on the root of a scan,
    /// which is the one entry that stores a whole path. Everything below it
    /// gets its path from the walk that reached it.
    pub fn root_path(&self) -> &Path {
        Path::new(&self.name)
    }

    /// This entry's own figures, with an empty child list.
    ///
    /// Anything building a reduced copy of a tree wants this rather than
    /// `clone()`: cloning deep-copies every descendant, so a caller that only
    /// means to keep a couple of levels still pays for the whole subtree first
    /// and then throws it away.
    pub fn without_children(&self) -> DiskEntry {
        DiskEntry {
            name: self.name.clone(),
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

    fn child_named(&self, name: &OsStr) -> Option<&DiskEntry> {
        self.children.iter().find(|c| c.name_os() == name)
    }

    /// Walk down to `path`, which must live under this entry. Returns `None`
    /// if any component is missing (a file that was deleted mid-session, or a
    /// subtree the depth limit pruned).
    pub fn resolve(&self, path: &Path) -> Option<&DiskEntry> {
        let relative = path.strip_prefix(self.root_path()).ok()?;
        let mut node = self;
        for component in relative.components() {
            node = node.child_named(component.as_os_str())?;
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
        let relative = path.strip_prefix(self.root_path()).ok()?;
        let names: Vec<&OsStr> = relative.components().map(|c| c.as_os_str()).collect();
        self.remove_named(&names)
    }

    fn remove_named(&mut self, names: &[&OsStr]) -> Option<DiskEntry> {
        let (name, rest) = names.split_first()?;
        let index = self.children.iter().position(|c| c.name_os() == *name)?;

        let removed = if rest.is_empty() {
            self.children.remove(index)
        } else {
            // Nothing is subtracted unless the whole path was there to remove.
            self.children[index].remove_named(rest)?
        };
        self.subtract(&removed);
        Some(removed)
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

    /// Every descendant exactly `depth` levels below this entry, each with the
    /// path that reached it — built on the way down, since no entry below the
    /// root carries one.
    ///
    /// Used for the "Largest items" list: children answer *what* is big, but
    /// grandchildren answer *where to look next* — `~/Downloads` is actionable
    /// in a way that `Users` is not.
    pub fn descendants_at_depth(&self, base: &Path, depth: usize) -> Vec<(PathBuf, &DiskEntry)> {
        let mut out = Vec::new();
        self.collect_at_depth(base, depth, &mut out);
        out
    }

    fn collect_at_depth<'a>(
        &'a self,
        base: &Path,
        depth: usize,
        out: &mut Vec<(PathBuf, &'a DiskEntry)>,
    ) {
        if depth == 0 {
            out.push((base.to_path_buf(), self));
            return;
        }
        for child in &self.children {
            child.collect_at_depth(&base.join(child.name_os()), depth - 1, out);
        }
    }

    /// The `n` biggest entries `depth` levels down, largest first. Falls back
    /// to shallower levels when the tree is not that deep.
    pub fn largest_at_depth(
        &self,
        base: &Path,
        depth: usize,
        n: usize,
        kind: SizeKind,
    ) -> Vec<(PathBuf, &DiskEntry)> {
        let mut candidates = self.descendants_at_depth(base, depth);
        for shallower in (1..depth).rev() {
            if !candidates.is_empty() {
                break;
            }
            candidates = self.descendants_at_depth(base, shallower);
        }
        candidates.sort_by_key(|(_, entry)| std::cmp::Reverse(entry.size(kind)));
        candidates.truncate(n);
        candidates
    }
}

/// Every format disko publishes — `--json` and the snapshots behind `diff` —
/// gives each entry its full path, and did so before entries stopped carrying
/// one. Serialising rebuilds those paths on the way down and parsing takes them
/// apart again, so the shape on the wire is exactly what it always was.
mod wire {
    use super::*;
    use serde::de::Deserializer;
    use serde::ser::{SerializeSeq, SerializeStruct, Serializer};

    /// An entry together with the path that reached it.
    struct Pathed<'a> {
        path: &'a Path,
        entry: &'a DiskEntry,
    }

    struct Children<'a> {
        base: &'a Path,
        entries: &'a [DiskEntry],
    }

    impl Serialize for Children<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut seq = serializer.serialize_seq(Some(self.entries.len()))?;
            for entry in self.entries {
                // Transient: one path at a time, dropped before the next, so
                // writing a two-million entry tree still holds only one copy.
                let path = self.base.join(entry.name_os());
                seq.serialize_element(&Pathed { path: &path, entry })?;
            }
            seq.end()
        }
    }

    impl Serialize for Pathed<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let entry = self.entry;
            let mut out = serializer.serialize_struct("DiskEntry", 8)?;
            out.serialize_field("path", self.path)?;
            out.serialize_field("apparent_size", &entry.apparent_size)?;
            out.serialize_field("allocated_size", &entry.allocated_size)?;
            out.serialize_field("entry_type", &entry.entry_type)?;
            out.serialize_field("items", &entry.items)?;
            out.serialize_field("modified", &entry.modified)?;
            out.serialize_field(
                "children",
                &Children {
                    base: self.path,
                    entries: &entry.children,
                },
            )?;
            out.serialize_field("scan_state", &entry.scan_state)?;
            out.end()
        }
    }

    /// Only ever the root of a tree is serialised — a snapshot's tree, or the
    /// root of a `--json` scan — and the root is the entry that knows its own
    /// whole path.
    impl Serialize for DiskEntry {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Pathed {
                path: self.root_path(),
                entry: self,
            }
            .serialize(serializer)
        }
    }

    #[derive(Deserialize)]
    struct Wire {
        path: PathBuf,
        apparent_size: u64,
        allocated_size: u64,
        entry_type: EntryType,
        items: u64,
        #[serde(default)]
        modified: u64,
        children: Vec<Wire>,
        scan_state: ScanState,
    }

    impl Wire {
        fn into_entry(self, is_root: bool) -> DiskEntry {
            // The root keeps the whole path it was written with; everything
            // below it needs only the last component back.
            let name: OsString = match self.path.file_name() {
                Some(name) if !is_root => name.to_os_string(),
                _ => self.path.into_os_string(),
            };
            DiskEntry {
                name: name.into_boxed_os_str(),
                apparent_size: self.apparent_size,
                allocated_size: self.allocated_size,
                entry_type: self.entry_type,
                items: self.items,
                modified: self.modified,
                children: self
                    .children
                    .into_iter()
                    .map(|child| child.into_entry(false))
                    .collect(),
                scan_state: self.scan_state,
            }
        }
    }

    impl<'de> Deserialize<'de> for DiskEntry {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Ok(Wire::deserialize(deserializer)?.into_entry(true))
        }
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
        let largest = root.largest_at_depth(Path::new("/root"), 2, 2, SizeKind::Allocated);
        let names: Vec<_> = largest
            .iter()
            .map(|(path, _)| path.display().to_string())
            .collect();
        assert_eq!(names, vec!["/root/a/deep", "/root/b/deep"]);
    }

    #[test]
    fn largest_at_depth_falls_back_when_tree_is_shallow() {
        let root = dir("/root", 10, vec![dir("/root/a", 5, vec![])]);
        let largest = root.largest_at_depth(Path::new("/root"), 3, 5, SizeKind::Allocated);
        assert_eq!(largest.len(), 1);
        assert_eq!(largest[0].1.name(), "a");
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

        assert_eq!(bare.name_os(), "a");
        assert_eq!(bare.allocated_size, 70);
        assert_eq!(bare.apparent_size, 70);
        assert_eq!(bare.entry_type, EntryType::Directory);
        // The subtree is what is left behind, and only the subtree.
        assert!(bare.children.is_empty());
    }

    /// `/root` -> `a` -> `deep.txt`, built the way a scan builds one: the root
    /// with a whole path, everything under it with a name.
    fn named_tree() -> DiskEntry {
        let mut root = DiskEntry::new("/root", EntryType::Directory);
        let mut a = DiskEntry::new("a", EntryType::Directory);
        let mut leaf = DiskEntry::new("deep.txt", EntryType::File);
        leaf.allocated_size = 40;
        a.allocated_size = 40;
        a.children.push(leaf);
        root.allocated_size = 40;
        root.children.push(a);
        root
    }

    #[test]
    fn the_wire_format_still_gives_every_entry_its_full_path() {
        let json: serde_json::Value = serde_json::to_value(named_tree()).unwrap();

        // `--json` and every snapshot ever written say `path`, and say it in
        // full. Entries stopped carrying one; the output must not notice.
        assert_eq!(json["path"], "/root");
        assert_eq!(json["children"][0]["path"], "/root/a");
        assert_eq!(
            json["children"][0]["children"][0]["path"],
            "/root/a/deep.txt"
        );
        assert_eq!(json["allocated_size"], 40);
        assert_eq!(json["entry_type"], "directory");
        assert_eq!(json["children"][0]["children"][0]["entry_type"], "file");
    }

    #[test]
    fn a_tree_survives_a_round_trip_through_the_wire_format() {
        let json = serde_json::to_string(&named_tree()).unwrap();
        let back: DiskEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(back.root_path(), Path::new("/root"));
        // The root got its whole path back and the rest got their names.
        assert_eq!(back.child("a").unwrap().name_os(), "a");
        assert_eq!(
            back.resolve(Path::new("/root/a/deep.txt")).unwrap().name(),
            "deep.txt"
        );
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn a_snapshot_written_by_an_older_disko_still_loads() {
        // Verbatim shape of a stored tree from before entries dropped their
        // paths, down to `modified` being absent.
        let stored = r#"{
            "path": "/root",
            "apparent_size": 40,
            "allocated_size": 40,
            "entry_type": "directory",
            "items": 3,
            "children": [
                {
                    "path": "/root/a",
                    "apparent_size": 40,
                    "allocated_size": 40,
                    "entry_type": "directory",
                    "items": 2,
                    "children": [],
                    "scan_state": "complete"
                }
            ],
            "scan_state": "complete"
        }"#;

        let tree: DiskEntry = serde_json::from_str(stored).unwrap();
        assert_eq!(tree.root_path(), Path::new("/root"));
        assert_eq!(tree.allocated_size, 40);
        assert_eq!(tree.modified, 0);
        assert_eq!(tree.resolve(Path::new("/root/a")).unwrap().items, 2);
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
