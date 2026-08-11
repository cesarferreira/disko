//! Deleting things, carefully.
//!
//! This is the one part of disko that cannot be undone, so every path is
//! checked against the scan it came from immediately before it is removed —
//! not when the list was drawn, which may have been minutes ago.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use disko_core::scan::Cancel;

/// Something the user has asked to remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
}

/// Why disko will not touch a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Outside the tree on screen. Deleting something you are not looking at
    /// is never what was meant.
    OutsideScan,
    /// The directory the scan started from.
    ScanRoot,
    /// A mount point: removing it would not free space here, and might free a
    /// great deal somewhere else.
    MountPoint,
    /// Vanished between the scan and now.
    Missing,
    /// A path with no parent — `/` and friends.
    Filesystem,
    /// The user stopped the deletion before reaching this one.
    Stopped,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Refusal::OutsideScan => "outside the current scan",
            Refusal::ScanRoot => "this is the folder being scanned",
            Refusal::MountPoint => "this is a mount point",
            Refusal::Missing => "no longer exists",
            Refusal::Filesystem => "this is a filesystem root",
            Refusal::Stopped => "stopped before reaching it",
        };
        f.write_str(reason)
    }
}

/// Decide whether `path` may be removed as part of a scan rooted at `root`.
pub fn check(path: &Path, root: &Path) -> Result<(), Refusal> {
    if path.parent().is_none() {
        return Err(Refusal::Filesystem);
    }
    if path == root {
        return Err(Refusal::ScanRoot);
    }
    if !path.starts_with(root) {
        return Err(Refusal::OutsideScan);
    }
    // symlink_metadata, so a symlink is judged as itself rather than as
    // whatever it points at.
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Err(Refusal::Missing);
    };
    if meta.is_dir() && is_mount_point(path) {
        return Err(Refusal::MountPoint);
    }
    Ok(())
}

/// Live counters for a deletion running on another thread.
///
/// Removing 68 GB of build caches takes long enough that a UI without these
/// looks like it has hung.
#[derive(Debug, Default)]
pub struct DeleteProgress {
    items_done: AtomicUsize,
    items_total: AtomicUsize,
    files_removed: AtomicU64,
    freed: AtomicU64,
    finished: AtomicBool,
    current: Mutex<PathBuf>,
}

impl DeleteProgress {
    pub fn new(total: usize) -> Self {
        let progress = Self::default();
        progress.items_total.store(total, Ordering::Relaxed);
        progress
    }

    /// Which of the requested items is being worked on, 1-based.
    pub fn items_done(&self) -> usize {
        self.items_done.load(Ordering::Relaxed)
    }

    pub fn items_total(&self) -> usize {
        self.items_total.load(Ordering::Relaxed)
    }

    /// Individual files and directories unlinked so far. The number that moves
    /// while one enormous directory is being cleared.
    pub fn files_removed(&self) -> u64 {
        self.files_removed.load(Ordering::Relaxed)
    }

    pub fn freed(&self) -> u64 {
        self.freed.load(Ordering::Relaxed)
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    pub fn current(&self) -> PathBuf {
        self.current
            .lock()
            .map(|path| path.clone())
            .unwrap_or_default()
    }

    fn start_item(&self, path: &Path) {
        if let Ok(mut current) = self.current.lock() {
            current.clear();
            current.push(path);
        }
    }

    fn finish_item(&self) {
        self.items_done.fetch_add(1, Ordering::Relaxed);
    }

    /// Bytes are credited as each file is unlinked rather than when a whole
    /// item finishes: clearing one 9 GB directory would otherwise show
    /// "freed 0 B" for the entire time it was working.
    fn file_removed(&self, bytes: u64) {
        self.files_removed.fetch_add(1, Ordering::Relaxed);
        self.freed.fetch_add(bytes, Ordering::Relaxed);
    }

    fn finish(&self) {
        self.finished.store(true, Ordering::Relaxed);
    }
}

/// Remove a checked path. Symlinks are unlinked, never followed.
pub fn delete(path: &Path) -> std::io::Result<()> {
    delete_watched(path, &DeleteProgress::default(), &Cancel::new())
}

/// Remove a path, counting entries as they go and stopping if asked.
///
/// This walks and unlinks by hand rather than calling `remove_dir_all`, which
/// is a single opaque call: on a directory with a hundred thousand files that
/// is the difference between a progress counter and a frozen screen.
fn delete_watched(path: &Path, progress: &DeleteProgress, cancel: &Cancel) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;

    // A symlink is unlinked as itself; following one would delete whatever it
    // happens to point at.
    if meta.file_type().is_symlink() || !meta.is_dir() {
        let bytes = allocated_of(&meta);
        std::fs::remove_file(path)?;
        progress.file_removed(bytes);
        return Ok(());
    }
    let own_bytes = allocated_of(&meta);

    for entry in std::fs::read_dir(path)? {
        if cancel.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "stopped",
            ));
        }
        delete_watched(&entry?.path(), progress, cancel)?;
    }

    std::fs::remove_dir(path)?;
    progress.file_removed(own_bytes);
    Ok(())
}

/// What the entry actually occupies, matching how disko counts sizes
/// everywhere else.
#[cfg(unix)]
fn allocated_of(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks() * 512
}

#[cfg(not(unix))]
fn allocated_of(meta: &std::fs::Metadata) -> u64 {
    meta.len()
}

/// A directory whose device differs from its parent's is a mount point.
#[cfg(unix)]
fn is_mount_point(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Some(parent) = path.parent() else {
        return true;
    };
    match (
        std::fs::symlink_metadata(path),
        std::fs::symlink_metadata(parent),
    ) {
        (Ok(here), Ok(above)) => here.dev() != above.dev(),
        // If it cannot be established, assume the more cautious answer.
        _ => true,
    }
}

#[cfg(not(unix))]
fn is_mount_point(_path: &Path) -> bool {
    false
}

/// What happened to one target.
#[derive(Clone, Debug)]
pub enum Outcome {
    Deleted { path: PathBuf, size: u64 },
    Refused { path: PathBuf, reason: Refusal },
    Failed { path: PathBuf, error: String },
}

impl Outcome {
    pub fn path(&self) -> &Path {
        match self {
            Outcome::Deleted { path, .. }
            | Outcome::Refused { path, .. }
            | Outcome::Failed { path, .. } => path,
        }
    }

    pub fn succeeded(&self) -> bool {
        matches!(self, Outcome::Deleted { .. })
    }

    /// Short explanation for the status bar or the details panel.
    pub fn detail(&self) -> String {
        match self {
            Outcome::Deleted { .. } => "deleted".into(),
            Outcome::Refused { reason, .. } => reason.to_string(),
            Outcome::Failed { error, .. } => error.clone(),
        }
    }
}

/// Delete every target that passes its check.
///
/// Each path is re-checked immediately before it is removed, not when the list
/// was drawn.
pub fn delete_all(
    targets: &[Target],
    root: &Path,
    progress: &DeleteProgress,
    cancel: &Cancel,
) -> Vec<Outcome> {
    let mut outcomes = Vec::with_capacity(targets.len());

    for target in targets {
        if cancel.is_cancelled() {
            outcomes.push(Outcome::Refused {
                path: target.path.clone(),
                reason: Refusal::Stopped,
            });
            continue;
        }

        progress.start_item(&target.path);
        let outcome = match check(&target.path, root) {
            Err(reason) => Outcome::Refused {
                path: target.path.clone(),
                reason,
            },
            Ok(()) => match delete_watched(&target.path, progress, cancel) {
                Ok(()) => {
                    progress.finish_item();
                    Outcome::Deleted {
                        path: target.path.clone(),
                        size: target.size,
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => Outcome::Failed {
                    path: target.path.clone(),
                    error: "stopped part-way through".to_string(),
                },
                Err(error) => Outcome::Failed {
                    path: target.path.clone(),
                    error: error.to_string(),
                },
            },
        };
        outcomes.push(outcome);
    }

    progress.finish();
    outcomes
}

/// Run a deletion on its own thread so the interface stays alive.
pub fn delete_in_background(
    targets: Vec<Target>,
    root: PathBuf,
) -> (Arc<DeleteProgress>, Cancel, JoinHandle<Vec<Outcome>>) {
    let progress = Arc::new(DeleteProgress::new(targets.len()));
    let cancel = Cancel::new();

    let handle = {
        let progress = Arc::clone(&progress);
        let cancel = cancel.clone();
        std::thread::spawn(move || delete_all(&targets, &root, &progress, &cancel))
    };

    (progress, cancel, handle)
}

/// Total actually freed.
pub fn freed(outcomes: &[Outcome]) -> u64 {
    outcomes
        .iter()
        .map(|outcome| match outcome {
            Outcome::Deleted { size, .. } => *size,
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!("disko-delete-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn dir(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(&path).unwrap();
            path
        }

        fn file(&self, name: &str, bytes: usize) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, vec![b'x'; bytes]).unwrap();
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn run(targets: &[Target], root: &Path) -> Vec<Outcome> {
        delete_all(
            targets,
            root,
            &DeleteProgress::new(targets.len()),
            &Cancel::new(),
        )
    }

    #[test]
    fn a_path_inside_the_scan_may_be_removed() {
        let tree = TempTree::new("ok");
        let victim = tree.dir("victim");
        assert_eq!(check(&victim, &tree.0), Ok(()));
    }

    #[test]
    fn the_scan_root_is_never_a_target() {
        let tree = TempTree::new("root");
        assert_eq!(check(&tree.0, &tree.0), Err(Refusal::ScanRoot));
    }

    #[test]
    fn paths_outside_the_scan_are_refused() {
        let tree = TempTree::new("outside");
        let elsewhere = std::env::temp_dir().join("somewhere-else-entirely");
        assert_eq!(check(&elsewhere, &tree.0), Err(Refusal::OutsideScan));
    }

    #[test]
    fn a_filesystem_root_is_refused_outright() {
        assert_eq!(
            check(Path::new("/"), Path::new("/")),
            Err(Refusal::Filesystem)
        );
    }

    #[test]
    fn something_deleted_behind_our_back_is_refused_not_retried() {
        let tree = TempTree::new("gone");
        let ghost = tree.0.join("never-existed");
        assert_eq!(check(&ghost, &tree.0), Err(Refusal::Missing));
    }

    #[test]
    fn deleting_a_directory_takes_its_contents() {
        let tree = TempTree::new("recursive");
        let victim = tree.dir("victim");
        tree.file("victim/inner.txt", 100);

        delete(&victim).unwrap();
        assert!(!victim.exists());
    }

    #[test]
    fn a_symlink_is_unlinked_rather_than_followed() {
        let tree = TempTree::new("symlink");
        let precious = tree.dir("precious");
        tree.file("precious/keep.txt", 10);
        let link = tree.0.join("link");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&precious, &link).unwrap();

        delete(&link).unwrap();

        assert!(!link.exists(), "the link should be gone");
        assert!(precious.join("keep.txt").exists(), "its target must not be");
    }

    #[test]
    fn deleting_reports_what_went_and_what_did_not() {
        let temp = TempTree::new("outcomes");
        let victim = temp.dir("victim");
        temp.file("victim/blob", 1000);
        let missing = temp.0.join("already-gone");

        let targets = vec![
            Target {
                path: victim.clone(),
                size: 3000,
                is_dir: true,
            },
            Target {
                path: missing.clone(),
                size: 99,
                is_dir: true,
            },
        ];

        let outcomes = run(&targets, &temp.0);

        assert!(matches!(outcomes[0], Outcome::Deleted { size: 3000, .. }));
        assert!(matches!(
            outcomes[1],
            Outcome::Refused {
                reason: Refusal::Missing,
                ..
            }
        ));
        assert_eq!(freed(&outcomes), 3000);
        assert!(!victim.exists());
    }

    #[test]
    fn progress_moves_while_a_big_directory_is_being_cleared() {
        let temp = TempTree::new("progress");
        let victim = temp.dir("victim");
        for index in 0..25 {
            temp.file(&format!("victim/file-{index}"), 10);
        }

        let targets = vec![Target {
            path: victim.clone(),
            size: 250,
            is_dir: true,
        }];
        let progress = DeleteProgress::new(1);
        delete_all(&targets, &temp.0, &progress, &Cancel::new());

        // 25 files plus the directory itself.
        assert_eq!(progress.files_removed(), 26);
        assert_eq!(progress.items_done(), 1);
        // Bytes are credited per file as they go, so the counter moves during
        // the work rather than jumping at the end.
        assert!(progress.freed() > 0);
        assert!(progress.is_finished());
    }

    #[test]
    fn a_cancelled_deletion_stops_and_says_which_ones_it_never_reached() {
        let temp = TempTree::new("cancelled");
        let first = temp.dir("first");
        let second = temp.dir("second");

        let targets = vec![
            Target {
                path: first,
                size: 10,
                is_dir: true,
            },
            Target {
                path: second.clone(),
                size: 10,
                is_dir: true,
            },
        ];

        let cancel = Cancel::new();
        cancel.cancel();
        let outcomes = delete_all(&targets, &temp.0, &DeleteProgress::new(2), &cancel);

        assert!(outcomes.iter().all(|outcome| matches!(
            outcome,
            Outcome::Refused {
                reason: Refusal::Stopped,
                ..
            }
        )));
        assert!(second.exists(), "a stopped deletion leaves the rest alone");
        assert_eq!(freed(&outcomes), 0);
    }

    #[test]
    fn a_background_deletion_can_be_joined_for_its_outcomes() {
        let temp = TempTree::new("background");
        let victim = temp.dir("victim");
        temp.file("victim/blob", 500);

        let (progress, _cancel, handle) = delete_in_background(
            vec![Target {
                path: victim.clone(),
                size: 500,
                is_dir: true,
            }],
            temp.0.clone(),
        );
        let outcomes = handle.join().unwrap();

        assert!(progress.is_finished());
        assert_eq!(freed(&outcomes), 500);
        assert!(!victim.exists());
    }

    #[test]
    fn a_refused_target_is_never_touched() {
        let temp = TempTree::new("refused");
        let outsider = TempTree::new("refused-other");
        let precious = outsider.dir("precious");

        let targets = vec![Target {
            path: precious.clone(),
            size: 10,
            is_dir: true,
        }];

        let outcomes = run(&targets, &temp.0);

        assert!(matches!(
            outcomes[0],
            Outcome::Refused {
                reason: Refusal::OutsideScan,
                ..
            }
        ));
        assert!(precious.exists(), "a refused path must survive");
        assert_eq!(freed(&outcomes), 0);
    }
}
