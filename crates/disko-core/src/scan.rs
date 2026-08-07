//! Parallel directory scanning with cancellation and live progress.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::tree::{DiskEntry, EntryType, ScanState};

#[derive(Clone, Debug)]
pub struct ScanOptions {
    /// Do not cross into other mounted filesystems.
    pub one_file_system: bool,
    /// Keep children only this many levels below the root. Sizes stay exact —
    /// deeper levels are still walked, just not retained.
    pub max_depth: Option<usize>,
    /// Count a hard-linked file once, under the first path that reaches it.
    pub dedup_hardlinks: bool,
    /// Do not walk into network filesystems — NFS, SMB, sshfs, blobfuse and
    /// friends. Their contents are not on this disk, and stat-ing them one
    /// round trip at a time turns a one-second scan into a ten-minute one.
    ///
    /// A network mount named explicitly as the scan root is still scanned:
    /// asking for it is asking for it.
    pub skip_remote: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            one_file_system: false,
            max_depth: None,
            dedup_hardlinks: true,
            skip_remote: true,
        }
    }
}

/// A directory that has finished being counted, published while the rest of
/// the scan is still running.
///
/// Every figure here is final for that directory — a summary is only sent once
/// its whole subtree is counted. A UI showing these is showing incomplete
/// information, never wrong information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    pub path: PathBuf,
    pub allocated: u64,
    pub apparent: u64,
    pub items: u64,
}

/// Live counters a UI can poll while a scan runs on another thread.
#[derive(Debug, Default)]
pub struct Progress {
    entries: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
    finished: AtomicBool,
    current: Mutex<PathBuf>,
    /// Levels below the root that publish a summary as they complete. Zero
    /// means the caller is not watching, and nothing is queued.
    stream_depth: usize,
    completed: Mutex<Vec<Finished>>,
}

impl Progress {
    /// Publish directory summaries this many levels below the root as they
    /// finish, so a UI can show real numbers before the scan ends.
    pub fn streaming(stream_depth: usize) -> Self {
        Self {
            stream_depth,
            ..Default::default()
        }
    }

    /// Take everything published since the last call.
    pub fn drain_completed(&self) -> Vec<Finished> {
        match self.completed.lock() {
            Ok(mut queue) => std::mem::take(&mut *queue),
            Err(_) => Vec::new(),
        }
    }

    fn publish(&self, depth: usize, entry: &DiskEntry) {
        if depth == 0 || depth > self.stream_depth {
            return;
        }
        if let Ok(mut queue) = self.completed.lock() {
            queue.push(Finished {
                path: entry.path.clone(),
                allocated: entry.allocated_size,
                apparent: entry.apparent_size,
                items: entry.items,
            });
        }
    }

    pub fn entries(&self) -> u64 {
        self.entries.load(Ordering::Relaxed)
    }

    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// Directories that could not be read, almost always permissions.
    pub fn errors(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    pub fn current(&self) -> PathBuf {
        self.current.lock().map(|p| p.clone()).unwrap_or_default()
    }

    pub fn finish(&self) {
        self.finished.store(true, Ordering::Relaxed);
    }

    fn saw_entry(&self, bytes: u64) {
        self.entries.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    fn saw_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Best effort — a busy lock means another worker just published a path,
    /// which is just as good for a progress line.
    fn set_current(&self, path: &Path) {
        if let Ok(mut current) = self.current.try_lock() {
            current.clear();
            current.push(path);
        }
    }
}

/// A cancellation flag shared with a running scan. Cloning shares the flag.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

struct Ctx<'a> {
    options: &'a ScanOptions,
    progress: &'a Progress,
    cancel: &'a Cancel,
    root_device: u64,
    /// Mount points to stop at, resolved once before the walk starts.
    remote_mounts: Vec<PathBuf>,
    /// (device, inode) pairs already counted, for hard-link dedup.
    seen_links: Mutex<HashSet<(u64, u64)>>,
}

impl Ctx<'_> {
    fn is_remote_mount(&self, path: &Path) -> bool {
        self.remote_mounts.iter().any(|mount| mount == path)
    }

    /// True the first time this inode is seen, false for every later link.
    fn first_sighting(&self, meta: &Metadata) -> bool {
        match self.seen_links.lock() {
            Ok(mut seen) => seen.insert((device_of(meta), inode_of(meta))),
            // A poisoned lock means a worker panicked; counting the file again
            // is a better failure than aborting the whole scan.
            Err(_) => true,
        }
    }
}

/// Walk `root`, returning its subtree. Blocks until the walk finishes or
/// `cancel` is tripped, in which case the partial tree is returned.
pub fn scan(
    root: &Path,
    options: &ScanOptions,
    progress: &Progress,
    cancel: &Cancel,
) -> Result<DiskEntry> {
    let root = root
        .canonicalize()
        .with_context(|| format!("cannot read {}", root.display()))?;
    let meta =
        fs::symlink_metadata(&root).with_context(|| format!("cannot stat {}", root.display()))?;

    let remote_mounts = if options.skip_remote {
        crate::mounts::remote_mount_points()
            .into_iter()
            // Scanning a network mount on purpose is allowed; wandering into
            // one by accident is what this guards against.
            .filter(|mount| mount != &root)
            .collect()
    } else {
        Vec::new()
    };

    let ctx = Ctx {
        options,
        progress,
        cancel,
        root_device: device_of(&meta),
        remote_mounts,
        seen_links: Mutex::new(HashSet::new()),
    };

    let entry = if meta.is_dir() {
        scan_dir(&root, &meta, 0, &ctx)
    } else {
        leaf_entry(root, &meta, &ctx)
    };

    progress.finish();
    Ok(entry)
}

/// Same as [`scan`], run on its own thread. Returns the counters to poll, the
/// flag to stop it, and the handle to join for the tree.
pub fn scan_in_background(
    root: PathBuf,
    options: ScanOptions,
) -> (Arc<Progress>, Cancel, JoinHandle<Result<DiskEntry>>) {
    scan_in_background_streaming(root, options, 0)
}

/// As [`scan_in_background`], but publishing directory summaries up to
/// `stream_depth` levels down while the scan runs.
pub fn scan_in_background_streaming(
    root: PathBuf,
    options: ScanOptions,
    stream_depth: usize,
) -> (Arc<Progress>, Cancel, JoinHandle<Result<DiskEntry>>) {
    let progress = Arc::new(Progress::streaming(stream_depth));
    let cancel = Cancel::new();

    let handle = {
        let progress = Arc::clone(&progress);
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            let result = scan(&root, &options, &progress, &cancel);
            progress.finish();
            result
        })
    };

    (progress, cancel, handle)
}

fn scan_dir(path: &Path, meta: &Metadata, depth: usize, ctx: &Ctx) -> DiskEntry {
    let mut entry = DiskEntry::new(path.to_path_buf(), EntryType::Directory);
    // A directory's own inode occupies blocks, which is why `du` on an empty
    // tree is not zero. Its `st_size` is bookkeeping rather than content, so
    // it stays out of the apparent total — the same split `du` makes between
    // its default output and `--apparent-size`.
    entry.allocated_size = allocated_of(meta);
    entry.modified = modified_of(meta);
    ctx.progress.saw_entry(entry.allocated_size);

    if ctx.cancel.is_cancelled() {
        entry.scan_state = ScanState::Cancelled;
        return entry;
    }
    ctx.progress.set_current(path);

    let read_dir = match fs::read_dir(path) {
        Ok(read_dir) => read_dir,
        Err(_) => {
            ctx.progress.saw_error();
            entry.scan_state = ScanState::Denied;
            return entry;
        }
    };
    let dir_entries: Vec<_> = read_dir.filter_map(|result| result.ok()).collect();

    let children: Vec<DiskEntry> = dir_entries
        .par_iter()
        .filter_map(|dir_entry| {
            let child_path = join_exact(path, &dir_entry.file_name());
            let child_meta = match fs::symlink_metadata(&child_path) {
                Ok(meta) => meta,
                Err(_) => {
                    ctx.progress.saw_error();
                    return None;
                }
            };

            if !child_meta.is_dir() {
                return Some(leaf_entry(child_path, &child_meta, ctx));
            }

            let crosses_filesystem =
                ctx.options.one_file_system && device_of(&child_meta) != ctx.root_device;
            if crosses_filesystem || ctx.is_remote_mount(&child_path) {
                let mut skipped = DiskEntry::new(child_path, EntryType::Directory);
                skipped.modified = modified_of(&child_meta);
                skipped.scan_state = ScanState::Skipped;
                return Some(skipped);
            }

            Some(scan_dir(&child_path, &child_meta, depth + 1, ctx))
        })
        .collect();

    for child in &children {
        entry.absorb(child);
    }

    // Drop children past the depth limit as the recursion unwinds rather than
    // at the end, so a capped scan of `/` never holds the whole tree at once.
    let keep_children = ctx.options.max_depth.is_none_or(|max| depth < max);
    if keep_children {
        // Collecting in parallel leaves spare capacity on every one of these
        // vectors, and there is one per directory for the rest of the session.
        let mut children = children;
        children.shrink_to_fit();
        entry.children = children;
    }

    ctx.progress.publish(depth, &entry);
    entry
}

fn leaf_entry(path: PathBuf, meta: &Metadata, ctx: &Ctx) -> DiskEntry {
    let file_type = meta.file_type();
    let entry_type = if file_type.is_symlink() {
        EntryType::Symlink
    } else if file_type.is_file() {
        EntryType::File
    } else {
        EntryType::Other
    };

    let mut entry = DiskEntry::new(path, entry_type);
    entry.modified = modified_of(meta);

    // A hard link seen a second time contributes nothing: the blocks were
    // already counted under the path that reached it first.
    if ctx.options.dedup_hardlinks && link_count(meta) > 1 && !ctx.first_sighting(meta) {
        ctx.progress.saw_entry(0);
        return entry;
    }

    entry.apparent_size = meta.len();
    entry.allocated_size = allocated_of(meta);
    ctx.progress.saw_entry(entry.allocated_size);
    entry
}

/// `parent/name`, in a buffer sized to hold exactly that.
///
/// The obvious `DirEntry::path()` grows its buffer by doubling and leaves the
/// slack in place. Every entry in the tree keeps its path for the whole
/// session, so on a home directory with two million files in it that slack came
/// to 177 MB — paid for a length that was known before the first byte was
/// written.
fn join_exact(parent: &Path, name: &OsStr) -> PathBuf {
    let mut path =
        PathBuf::with_capacity(parent.as_os_str().len() + MAIN_SEPARATOR.len_utf8() + name.len());
    path.push(parent);
    path.push(name);
    path
}

/// Seconds since the Unix epoch, or 0 when the filesystem will not say.
fn modified_of(meta: &Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

#[cfg(unix)]
fn allocated_of(meta: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    // `blocks` is always in 512-byte units regardless of the filesystem's own
    // block size — that is what `stat(2)` specifies.
    meta.blocks() * 512
}

#[cfg(not(unix))]
fn allocated_of(meta: &Metadata) -> u64 {
    meta.len()
}

#[cfg(unix)]
fn device_of(meta: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.dev()
}

#[cfg(not(unix))]
fn device_of(_meta: &Metadata) -> u64 {
    0
}

#[cfg(unix)]
fn inode_of(meta: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino()
}

#[cfg(not(unix))]
fn inode_of(_meta: &Metadata) -> u64 {
    0
}

#[cfg(unix)]
fn link_count(meta: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.nlink()
}

#[cfg(not(unix))]
fn link_count(_meta: &Metadata) -> u64 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    /// A throwaway directory tree that cleans itself up.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!("disko-test-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, relative: &str, bytes: usize) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut file = File::create(&path).unwrap();
            file.write_all(&vec![b'x'; bytes]).unwrap();
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn scan_tree(root: &Path, options: ScanOptions) -> DiskEntry {
        let progress = Progress::default();
        scan(root, &options, &progress, &Cancel::new()).unwrap()
    }

    #[test]
    fn apparent_size_is_exactly_the_sum_of_file_lengths() {
        let tree = TempTree::new("sums");
        tree.file("a.txt", 1000);
        tree.file("nested/b.txt", 2000);
        tree.file("nested/deeper/c.txt", 3000);

        let root = scan_tree(&tree.0, ScanOptions::default());

        // Matches `du --apparent-size`: directory inodes are bookkeeping, not
        // content, so they do not inflate the total.
        assert_eq!(root.apparent_size, 6000);
        assert_eq!(root.scan_state, ScanState::Complete);
    }

    #[test]
    fn allocated_size_adds_each_directory_to_what_is_underneath() {
        let tree = TempTree::new("allocated");
        tree.file("nested/b.txt", 2000);

        let root = scan_tree(&tree.0, ScanOptions::default());
        let nested = root.child("nested").unwrap();
        let file = nested.child("b.txt").unwrap();

        // How much a directory inode itself occupies is filesystem-specific:
        // ext4 charges a block, APFS reports none. Either way the totals
        // accumulate upward and never lose what is below.
        assert_eq!(
            root.allocated_size,
            own_blocks(&tree.0) + nested.allocated_size
        );
        assert_eq!(
            nested.allocated_size,
            own_blocks(&tree.0.join("nested")) + file.allocated_size
        );
        // A 2000-byte file always occupies whole blocks.
        assert!(file.allocated_size >= 2000);
        assert_eq!(nested.apparent_size, 2000);
    }

    /// What the filesystem charges for one entry, ignoring its contents.
    fn own_blocks(path: &Path) -> u64 {
        allocated_of(&fs::symlink_metadata(path).unwrap())
    }

    #[test]
    fn max_depth_prunes_children_but_keeps_sizes_exact() {
        let tree = TempTree::new("depth");
        tree.file("nested/deeper/c.txt", 4000);

        let full = scan_tree(&tree.0, ScanOptions::default());
        let capped = scan_tree(
            &tree.0,
            ScanOptions {
                max_depth: Some(1),
                ..Default::default()
            },
        );

        assert_eq!(full.apparent_size, capped.apparent_size);
        assert_eq!(capped.children.len(), 1);
        assert!(capped.children[0].children.is_empty());
        assert!(!full.children[0].children.is_empty());
    }

    #[test]
    fn counts_every_entry_including_directories() {
        let tree = TempTree::new("items");
        tree.file("a.txt", 10);
        tree.file("nested/b.txt", 10);

        let root = scan_tree(&tree.0, ScanOptions::default());

        // root + a.txt + nested + nested/b.txt
        assert_eq!(root.items, 4);
    }

    #[test]
    fn cancelled_scan_returns_a_partial_tree() {
        let tree = TempTree::new("cancel");
        tree.file("a.txt", 10);

        let progress = Progress::default();
        let cancel = Cancel::new();
        cancel.cancel();
        let root = scan(&tree.0, &ScanOptions::default(), &progress, &cancel).unwrap();

        assert_eq!(root.scan_state, ScanState::Cancelled);
        assert!(root.children.is_empty());
    }

    #[test]
    fn completed_directories_are_published_while_the_scan_runs() {
        let tree = TempTree::new("stream");
        tree.file("alpha/one.txt", 1000);
        tree.file("beta/two.txt", 2000);
        tree.file("beta/deeper/three.txt", 3000);

        let progress = Progress::streaming(2);
        let root = scan(&tree.0, &ScanOptions::default(), &progress, &Cancel::new()).unwrap();

        let published = progress.drain_completed();
        let names: Vec<String> = published
            .iter()
            .map(|done| done.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();

        // Two levels down, and never the root itself.
        assert!(names.contains(&"alpha".to_string()), "{names:?}");
        assert!(names.contains(&"beta".to_string()), "{names:?}");
        assert!(names.contains(&"deeper".to_string()), "{names:?}");
        assert!(!names.contains(&root.name().to_string()), "{names:?}");

        // Every published figure is final, not a running partial.
        let beta = published
            .iter()
            .find(|done| done.path.ends_with("beta"))
            .unwrap();
        assert_eq!(beta.apparent, 5000);

        // Draining twice does not repeat anything.
        assert!(progress.drain_completed().is_empty());
    }

    #[test]
    fn nothing_is_published_when_nobody_is_watching() {
        let tree = TempTree::new("nostream");
        tree.file("alpha/one.txt", 1000);

        let progress = Progress::default();
        scan(&tree.0, &ScanOptions::default(), &progress, &Cancel::new()).unwrap();

        assert!(progress.drain_completed().is_empty());
    }

    #[test]
    fn streaming_stops_at_the_requested_depth() {
        let tree = TempTree::new("streamdepth");
        tree.file("a/b/c/deep.txt", 100);

        let progress = Progress::streaming(1);
        scan(&tree.0, &ScanOptions::default(), &progress, &Cancel::new()).unwrap();

        let published = progress.drain_completed();
        assert_eq!(published.len(), 1);
        assert!(published[0].path.ends_with("a"));
    }

    #[test]
    fn progress_tracks_entries_and_bytes() {
        let tree = TempTree::new("progress");
        tree.file("a.txt", 1000);
        tree.file("b.txt", 1000);

        let progress = Progress::default();
        let root = scan(&tree.0, &ScanOptions::default(), &progress, &Cancel::new()).unwrap();

        assert_eq!(progress.entries(), root.items);
        assert_eq!(progress.bytes(), root.allocated_size);
        assert!(progress.is_finished());
    }

    #[test]
    fn scanning_a_single_file_yields_one_entry() {
        let tree = TempTree::new("file");
        tree.file("solo.txt", 512);

        let root = scan_tree(&tree.0.join("solo.txt"), ScanOptions::default());

        assert_eq!(root.entry_type, EntryType::File);
        assert_eq!(root.apparent_size, 512);
        assert_eq!(root.items, 1);
    }

    #[test]
    fn joined_paths_are_correct_and_carry_no_slack() {
        let joined = join_exact(Path::new("/home/someone/code"), OsStr::new("disko"));

        assert_eq!(joined, PathBuf::from("/home/someone/code/disko"));
        // The whole point: no room left over. Every entry in a scan keeps its
        // path for the session, so a byte of slack here is a byte per file.
        assert!(
            joined.capacity() <= joined.as_os_str().len() + 1,
            "capacity {} for a path of {} bytes",
            joined.capacity(),
            joined.as_os_str().len()
        );
    }

    #[test]
    fn a_scanned_tree_holds_no_spare_path_capacity() {
        let tree = TempTree::new("capacity");
        tree.file("some/reasonably/deep/nesting/file.txt", 10);

        let root = scan_tree(&tree.0, ScanOptions::default());

        fn check(entry: &DiskEntry) {
            assert!(
                entry.path.capacity() <= entry.path.as_os_str().len() + 1,
                "{} has {} bytes of capacity for {} bytes of path",
                entry.path.display(),
                entry.path.capacity(),
                entry.path.as_os_str().len()
            );
            assert_eq!(
                entry.children.capacity(),
                entry.children.len(),
                "{} keeps a child list bigger than its child count",
                entry.path.display()
            );
            for child in &entry.children {
                check(child);
            }
        }
        check(&root);
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_are_counted_once() {
        let tree = TempTree::new("links");
        tree.file("original.txt", 4096);
        fs::hard_link(tree.0.join("original.txt"), tree.0.join("link.txt")).unwrap();

        let deduped = scan_tree(&tree.0, ScanOptions::default());
        let raw = scan_tree(
            &tree.0,
            ScanOptions {
                dedup_hardlinks: false,
                ..Default::default()
            },
        );

        assert_eq!(raw.apparent_size - deduped.apparent_size, 4096);
        assert_eq!(deduped.items, raw.items);
    }
}
