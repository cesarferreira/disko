//! The scanning engine behind [disko](https://github.com/cesarferreira/disko).
//!
//! `disko-core` returns neutral data — sizes in bytes, paths, filesystem
//! facts — and never formats anything for a particular display. The TUI,
//! `--plain`, `--json` and any other consumer all sit on top of this.
//!
//! ```no_run
//! use std::path::Path;
//! use disko_core::{scan, ScanOptions, SizeKind};
//!
//! let tree = scan::scan(
//!     Path::new("."),
//!     &ScanOptions::default(),
//!     &scan::Progress::default(),
//!     &scan::Cancel::new(),
//! )?;
//!
//! for child in &tree.children {
//!     println!("{:>12}  {}", child.size(SizeKind::Allocated), child.name());
//! }
//! # Ok::<(), anyhow::Error>(())
//! ```

pub mod mounts;
pub mod scan;
pub mod size;
pub mod tree;

pub use mounts::{Filesystem, Inodes};
pub use scan::{Cancel, Progress, ScanOptions};
pub use size::{SizeKind, Unit};
pub use tree::{DiskEntry, EntryType, ScanState};
