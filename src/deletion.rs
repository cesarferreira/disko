//! Deleting things, carefully.
//!
//! This is the one part of disko that cannot be undone, so every path is
//! checked against the scan it came from immediately before it is removed —
//! not when the list was drawn, which may have been minutes ago.

use std::fmt;
use std::path::{Path, PathBuf};

use disko_core::DiskEntry;

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
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Refusal::OutsideScan => "outside the current scan",
            Refusal::ScanRoot => "this is the folder being scanned",
            Refusal::MountPoint => "this is a mount point",
            Refusal::Missing => "no longer exists",
            Refusal::Filesystem => "this is a filesystem root",
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

/// Remove a checked path. Symlinks are unlinked, never followed.
pub fn delete(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        std::fs::remove_file(path)
    } else {
        std::fs::remove_dir_all(path)
    }
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

/// Delete every target that passes its check, correcting `tree` as it goes so
/// the display matches reality without a rescan.
pub fn delete_all(targets: &[Target], root: &Path, tree: &mut DiskEntry) -> Vec<Outcome> {
    targets
        .iter()
        .map(|target| match check(&target.path, root) {
            Err(reason) => Outcome::Refused {
                path: target.path.clone(),
                reason,
            },
            Ok(()) => match delete(&target.path) {
                Ok(()) => {
                    // Trust the tree's own figure over the caller's: it is what
                    // the totals on screen were built from.
                    let size = tree
                        .remove(&target.path)
                        .map(|removed| removed.allocated_size)
                        .unwrap_or(target.size);
                    Outcome::Deleted {
                        path: target.path.clone(),
                        size,
                    }
                }
                Err(error) => Outcome::Failed {
                    path: target.path.clone(),
                    error: error.to_string(),
                },
            },
        })
        .collect()
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
    use disko_core::EntryType;
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

    fn entry(path: &Path, size: u64, children: Vec<DiskEntry>) -> DiskEntry {
        let mut entry = DiskEntry::new(path.to_path_buf(), EntryType::Directory);
        entry.allocated_size = size;
        entry.apparent_size = size;
        entry.children = children;
        entry
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
    fn deleting_corrects_the_tree_and_reports_what_was_freed() {
        let temp = TempTree::new("outcomes");
        let victim = temp.dir("victim");
        temp.file("victim/blob", 1000);
        let missing = temp.0.join("already-gone");

        let mut tree = entry(&temp.0, 5000, vec![entry(&victim, 3000, vec![])]);
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

        let outcomes = delete_all(&targets, &temp.0, &mut tree);

        assert!(matches!(outcomes[0], Outcome::Deleted { size: 3000, .. }));
        assert!(matches!(
            outcomes[1],
            Outcome::Refused {
                reason: Refusal::Missing,
                ..
            }
        ));
        assert_eq!(freed(&outcomes), 3000);
        // The totals on screen correct themselves without a rescan.
        assert_eq!(tree.allocated_size, 2000);
        assert!(tree.children.is_empty());
        assert!(!victim.exists());
    }

    #[test]
    fn a_refused_target_is_never_touched() {
        let temp = TempTree::new("refused");
        let outsider = TempTree::new("refused-other");
        let precious = outsider.dir("precious");

        let mut tree = entry(&temp.0, 100, vec![]);
        let targets = vec![Target {
            path: precious.clone(),
            size: 10,
            is_dir: true,
        }];

        let outcomes = delete_all(&targets, &temp.0, &mut tree);

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
