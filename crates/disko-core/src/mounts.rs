//! Mounted filesystems, in human terms.
//!
//! The public shape here is deliberately friendly — a volume has a *name*, a
//! size and a mount point. Device nodes, filesystem types and inode counts are
//! still carried, but they are details a caller opts into rather than the
//! headline.

// Only the Linux mount table needs a lookup map for volume labels.
#[cfg(target_os = "linux")]
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Inodes {
    pub total: u64,
    pub used: u64,
    pub free: u64,
}

impl Inodes {
    pub fn used_fraction(&self) -> f64 {
        crate::size::fraction(self.used, self.total)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Filesystem {
    /// What a person would call it: "Macintosh HD", "Root", "backup".
    pub name: String,
    pub mount_point: PathBuf,
    /// The device node, e.g. `/dev/disk3s1s1`.
    pub device: String,
    /// APFS, ext4, xfs...
    pub fs_type: String,
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub read_only: bool,
    pub removable: bool,
    /// SSD / HDD / Removable / Unknown, as far as the OS will say.
    pub kind: String,
    /// Kernel bookkeeping rather than storage: tmpfs, proc, cgroup...
    pub pseudo: bool,
    pub inodes: Option<Inodes>,
}

impl Filesystem {
    pub fn used_fraction(&self) -> f64 {
        crate::size::fraction(self.used, self.total)
    }
}

/// Filesystem types that hold no durable data and only add noise to a list of
/// "what is full".
const PSEUDO_TYPES: &[&str] = &[
    "autofs",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "debugfs",
    "devfs",
    "devpts",
    "devtmpfs",
    "efivarfs",
    "fuse.gvfsd-fuse",
    "fuse.portal",
    "fusectl",
    "hugetlbfs",
    "mqueue",
    "nsfs",
    "overlay",
    "proc",
    "pstore",
    "ramfs",
    "rpc_pipefs",
    "securityfs",
    "selinuxfs",
    "squashfs",
    "sysfs",
    "tmpfs",
    "tracefs",
];

/// Filesystems whose contents do not live on this disk, or that answer a
/// `stat` over a network. Walking one is both useless for "what is filling my
/// disk" and ruinously slow: a single directory listing on a blob-storage
/// mount can take five seconds.
///
/// `fuseblk` is deliberately absent — that is how local NTFS and exFAT drives
/// are mounted, and those really are this machine's storage.
const REMOTE_TYPES: &[&str] = &[
    "9p",
    "afpfs",
    "afs",
    "beegfs",
    "ceph",
    "cifs",
    "curlftpfs",
    "davfs",
    "ftpfs",
    "fuse",
    "fuse.rclone",
    "fuse.s3fs",
    "fuse.sshfs",
    "gfs2",
    "glusterfs",
    "lustre",
    "ncpfs",
    "nfs",
    "nfs4",
    "smb2",
    "smb3",
    "smbfs",
    "sshfs",
    "webdav",
];

/// True for filesystems that are somewhere else pretending to be here.
pub fn is_remote(fs_type: &str) -> bool {
    REMOTE_TYPES.contains(&fs_type)
        // Most fuse drivers register as `fuse.<driver>`; blobfuse2, sshfs and
        // rclone all arrive this way.
        || fs_type.starts_with("fuse.")
}

/// Every mount point that is not really on this disk.
pub fn remote_mount_points() -> Vec<PathBuf> {
    list(true)
        .into_iter()
        .filter(|fs| is_remote(&fs.fs_type))
        .map(|fs| fs.mount_point)
        .collect()
}

/// Mount points that exist for the OS's benefit, not the user's.
const PSEUDO_PREFIXES: &[&str] = &["/proc", "/sys", "/dev", "/run", "/snap", "/var/snap"];

fn is_pseudo(fs_type: &str, mount_point: &Path) -> bool {
    if PSEUDO_TYPES.contains(&fs_type) {
        return true;
    }
    let mount = mount_point.to_string_lossy();
    // macOS synthesises a pile of mounts under /System/Volumes; only the Data
    // volume is somewhere a user actually stores things.
    if mount.starts_with("/System/Volumes/") && mount != "/System/Volumes/Data" {
        return true;
    }
    PSEUDO_PREFIXES
        .iter()
        .any(|prefix| mount == *prefix || mount.starts_with(&format!("{prefix}/")))
}

/// Every mounted filesystem, real ones first and largest first within that.
pub fn list(include_pseudo: bool) -> Vec<Filesystem> {
    let mut filesystems = collect();
    if !include_pseudo {
        filesystems.retain(|fs| !fs.pseudo && fs.total > 0);
    }
    filesystems.sort_by(|a, b| {
        a.pseudo
            .cmp(&b.pseudo)
            .then_with(|| b.total.cmp(&a.total))
            .then_with(|| a.mount_point.cmp(&b.mount_point))
    });
    filesystems
}

/// The filesystem `path` lives on: the mount point that is its longest prefix.
pub fn for_path(path: &Path) -> Option<Filesystem> {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    list(true)
        .into_iter()
        .filter(|fs| absolute.starts_with(&fs.mount_point))
        .max_by_key(|fs| fs.mount_point.as_os_str().len())
}

/// Pretty names, device kind and removability, keyed by mount point.
///
/// Linux builds the mount list from `/proc/mounts`, which knows nothing about
/// volume labels or whether a disk is removable, so sysinfo fills those in.
/// Elsewhere sysinfo *is* the mount list and this is not needed.
#[cfg(target_os = "linux")]
fn disk_hints() -> HashMap<PathBuf, (String, String, bool)> {
    use sysinfo::{DiskKind, Disks};

    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .map(|disk| {
            let kind = match disk.kind() {
                DiskKind::SSD => "SSD".to_string(),
                DiskKind::HDD => "HDD".to_string(),
                DiskKind::Unknown(_) => "Unknown".to_string(),
            };
            (
                disk.mount_point().to_path_buf(),
                (
                    disk.name().to_string_lossy().to_string(),
                    kind,
                    disk.is_removable(),
                ),
            )
        })
        .collect()
}

fn friendly_name(hint: Option<&str>, device: &str, mount_point: &Path) -> String {
    // A hint that is not a device path is a real volume label. Fuse mounts
    // report the same string for both ("blobfuse2"), which is a driver name
    // rather than something a person would call the volume.
    if let Some(hint) = hint {
        let hint = hint.trim();
        if !hint.is_empty() && !hint.starts_with('/') && hint != device {
            return hint.to_string();
        }
    }
    if mount_point == Path::new("/") {
        return "Root".to_string();
    }
    match mount_point.file_name() {
        Some(name) => name.to_string_lossy().to_string(),
        None => {
            if device.is_empty() {
                mount_point.to_string_lossy().to_string()
            } else {
                device.to_string()
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn collect() -> Vec<Filesystem> {
    let hints = disk_hints();
    let mut seen_devices: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut filesystems = Vec::new();

    for row in read_proc_mounts() {
        // Bind mounts, and the several mounts a container stacks onto one
        // device, would otherwise repeat the same numbers row after row.
        if row.device.starts_with("/dev/") && !seen_devices.insert(row.device.clone()) {
            continue;
        }

        let stat = os_stat(&row.mount_point);
        let hint = hints.get(&row.mount_point);
        let total = stat.as_ref().map(|s| s.total).unwrap_or(0);
        let available = stat.as_ref().map(|s| s.available).unwrap_or(0);
        let used = stat.as_ref().map(|s| s.used).unwrap_or(0);

        filesystems.push(Filesystem {
            name: friendly_name(hint.map(|h| h.0.as_str()), &row.device, &row.mount_point),
            device: row.device.clone(),
            pseudo: is_pseudo(&row.fs_type, &row.mount_point),
            fs_type: row.fs_type,
            total,
            used,
            available,
            read_only: stat.as_ref().map(|s| s.read_only).unwrap_or(false)
                || row.options.split(',').any(|opt| opt == "ro"),
            removable: hint.map(|h| h.2).unwrap_or(false),
            kind: hint
                .map(|h| h.1.clone())
                .unwrap_or_else(|| "Unknown".to_string()),
            inodes: stat.as_ref().and_then(|s| s.inodes.clone()),
            mount_point: row.mount_point,
        });
    }

    filesystems
}

#[cfg(not(target_os = "linux"))]
fn collect() -> Vec<Filesystem> {
    use sysinfo::{DiskKind, Disks};

    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .map(|disk| {
            let mount_point = disk.mount_point().to_path_buf();
            let fs_type = disk.file_system().to_string_lossy().to_string();
            let stat = os_stat(&mount_point);
            let raw_name = disk.name().to_string_lossy().to_string();

            // `statfs` knows the device node; sysinfo reports the volume label.
            let device = stat
                .as_ref()
                .and_then(|s| s.device.clone())
                .unwrap_or_else(|| raw_name.clone());

            let total = stat
                .as_ref()
                .map(|s| s.total)
                .unwrap_or_else(|| disk.total_space());
            let available = stat
                .as_ref()
                .map(|s| s.available)
                .unwrap_or_else(|| disk.available_space());
            let used = stat
                .as_ref()
                .map(|s| s.used)
                .unwrap_or_else(|| disk.total_space().saturating_sub(disk.available_space()));

            Filesystem {
                name: friendly_name(Some(&raw_name), &device, &mount_point),
                device,
                pseudo: is_pseudo(&fs_type, &mount_point),
                fs_type,
                total,
                used,
                available,
                read_only: stat
                    .as_ref()
                    .map(|s| s.read_only)
                    .unwrap_or_else(|| disk.is_read_only()),
                removable: disk.is_removable(),
                kind: match disk.kind() {
                    DiskKind::SSD => "SSD".to_string(),
                    DiskKind::HDD => "HDD".to_string(),
                    DiskKind::Unknown(_) => "Unknown".to_string(),
                },
                inodes: stat.as_ref().and_then(|s| s.inodes.clone()),
                mount_point,
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
struct MountRow {
    device: String,
    mount_point: PathBuf,
    fs_type: String,
    options: String,
}

#[cfg(target_os = "linux")]
fn read_proc_mounts() -> Vec<MountRow> {
    let Ok(contents) = std::fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };

    contents
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let device = unescape_mount_field(fields.next()?);
            let mount_point = unescape_mount_field(fields.next()?);
            let fs_type = unescape_mount_field(fields.next()?);
            let options = fields.next().unwrap_or("").to_string();
            Some(MountRow {
                device,
                mount_point: PathBuf::from(mount_point),
                fs_type,
                options,
            })
        })
        .collect()
}

/// `/proc/mounts` octal-escapes space, tab, newline and backslash.
#[cfg(target_os = "linux")]
fn unescape_mount_field(field: &str) -> String {
    if !field.contains('\\') {
        return field.to_string();
    }
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let digits: String = chars.clone().take(3).collect();
        match u8::from_str_radix(&digits, 8) {
            Ok(byte) if digits.len() == 3 => {
                out.push(byte as char);
                for _ in 0..3 {
                    chars.next();
                }
            }
            _ => out.push('\\'),
        }
    }
    out
}

struct FsStat {
    total: u64,
    used: u64,
    available: u64,
    read_only: bool,
    /// Only the BSD `statfs` path learns the device node here; Linux reads it
    /// from `/proc/mounts` instead.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    device: Option<String>,
    inodes: Option<Inodes>,
}

#[cfg(target_os = "linux")]
fn os_stat(mount_point: &Path) -> Option<FsStat> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(mount_point.as_os_str().as_bytes()).ok()?;
    let mut raw: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a valid NUL-terminated string and `raw` is a valid,
    // correctly sized statvfs the kernel fills in.
    if unsafe { libc::statvfs(path.as_ptr(), &mut raw) } != 0 {
        return None;
    }

    // f_frsize is the fragment size the block counts are expressed in; some
    // filesystems report 0 there, in which case f_bsize is the right unit.
    let unit = if raw.f_frsize > 0 {
        raw.f_frsize as u64
    } else {
        raw.f_bsize as u64
    };
    let total = raw.f_blocks as u64 * unit;
    let free = raw.f_bfree as u64 * unit;
    let available = raw.f_bavail as u64 * unit;

    let inodes_total = raw.f_files as u64;
    let inodes = (inodes_total > 0).then(|| Inodes {
        total: inodes_total,
        free: raw.f_ffree as u64,
        used: inodes_total.saturating_sub(raw.f_ffree as u64),
    });

    Some(FsStat {
        total,
        // `df` calls the reserved-for-root slice "used" too, which is why
        // used + available rarely equals total.
        used: total.saturating_sub(free),
        available,
        read_only: raw.f_flag as u64 & libc::ST_RDONLY != 0,
        device: None,
        inodes,
    })
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn os_stat(mount_point: &Path) -> Option<FsStat> {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(mount_point.as_os_str().as_bytes()).ok()?;
    let mut raw: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: as above — valid path, zeroed destination of the right size.
    if unsafe { libc::statfs(path.as_ptr(), &mut raw) } != 0 {
        return None;
    }

    let unit = raw.f_bsize as u64;
    let total = raw.f_blocks * unit;
    let free = raw.f_bfree * unit;
    let available = raw.f_bavail * unit;

    let inodes_total = raw.f_files;
    let inodes = (inodes_total > 0).then(|| Inodes {
        total: inodes_total,
        free: raw.f_ffree,
        used: inodes_total.saturating_sub(raw.f_ffree),
    });

    // SAFETY: f_mntfromname is a NUL-terminated C string inside the struct.
    let device = unsafe { CStr::from_ptr(raw.f_mntfromname.as_ptr()) }
        .to_string_lossy()
        .to_string();

    Some(FsStat {
        total,
        used: total.saturating_sub(free),
        available,
        read_only: raw.f_flags & libc::MNT_RDONLY as u32 != 0,
        device: (!device.is_empty()).then_some(device),
        inodes,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
fn os_stat(_mount_point: &Path) -> Option<FsStat> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_filesystems_are_recognised() {
        assert!(is_remote("nfs4"));
        assert!(is_remote("cifs"));
        assert!(is_remote("fuse"));
        assert!(is_remote("fuse.sshfs"));
        // A locally mounted NTFS drive is this machine's storage.
        assert!(!is_remote("fuseblk"));
        assert!(!is_remote("ext4"));
        assert!(!is_remote("apfs"));
        assert!(!is_remote("xfs"));
    }

    #[test]
    fn pseudo_filesystems_are_recognised() {
        assert!(is_pseudo("tmpfs", Path::new("/tmp")));
        assert!(is_pseudo("ext4", Path::new("/proc/whatever")));
        assert!(is_pseudo("apfs", Path::new("/System/Volumes/Preboot")));
        assert!(!is_pseudo("apfs", Path::new("/System/Volumes/Data")));
        assert!(!is_pseudo("ext4", Path::new("/")));
        assert!(!is_pseudo("ext4", Path::new("/home")));
        // A directory that merely starts with the same letters is not /dev.
        assert!(!is_pseudo("ext4", Path::new("/development")));
    }

    #[test]
    fn names_prefer_a_volume_label_over_a_device() {
        assert_eq!(
            friendly_name(Some("Macintosh HD"), "/dev/disk3s1s1", Path::new("/")),
            "Macintosh HD"
        );
        assert_eq!(
            friendly_name(Some("/dev/sda1"), "/dev/sda1", Path::new("/")),
            "Root"
        );
        assert_eq!(
            friendly_name(None, "/dev/sdb1", Path::new("/mnt/backup")),
            "backup"
        );
        // A fuse driver name is not a label.
        assert_eq!(
            friendly_name(
                Some("blobfuse2"),
                "blobfuse2",
                Path::new("/mnt/remote/session")
            ),
            "session"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mount_fields_are_unescaped() {
        assert_eq!(unescape_mount_field("/mnt/my\\040disk"), "/mnt/my disk");
        assert_eq!(unescape_mount_field("/mnt/plain"), "/mnt/plain");
        assert_eq!(
            unescape_mount_field("/mnt/back\\134slash"),
            "/mnt/back\\slash"
        );
    }

    #[test]
    fn the_root_filesystem_is_discoverable() {
        let root = for_path(Path::new("/")).expect("every system has a root filesystem");
        assert!(root.total > 0);
        assert!(root.used_fraction() >= 0.0 && root.used_fraction() <= 1.0);
    }

    #[test]
    fn listing_hides_pseudo_filesystems_by_default() {
        let visible = list(false);
        let everything = list(true);
        assert!(visible.len() <= everything.len());
        assert!(visible.iter().all(|fs| !fs.pseudo));
    }
}
