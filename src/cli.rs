use std::path::PathBuf;

use clap::Parser;
use disko_core::{ScanOptions, SizeKind, Unit};

#[derive(Parser, Debug)]
#[command(
    name = "disko",
    version,
    about = "Disk usage TUI that shows what is full and what is using it",
    long_about = "disko answers three questions: what is full, what is using the space, \
                  and where to look next.\n\n\
                  Run it with no arguments to pick a disk, or point it at a directory to \
                  scan straight away. Filesystem types, inode counts and device names live \
                  behind --details rather than cluttering the default view."
)]
pub struct Cli {
    /// Directory to scan. Omit to choose from your mounted disks.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Show only the N largest entries, grouping the rest as "Other".
    #[arg(short = 't', long, value_name = "N", default_value_t = 20)]
    pub top: usize,

    /// Keep only N levels of the tree. Sizes stay exact either way.
    #[arg(long, value_name = "N")]
    pub depth: Option<usize>,

    /// Show device names, filesystem types and mount details.
    #[arg(long)]
    pub details: bool,

    /// List every mounted filesystem, pseudo-filesystems included.
    #[arg(long)]
    pub filesystems: bool,

    /// Report inode usage instead of bytes.
    #[arg(long)]
    pub inodes: bool,

    /// Script-friendly output, no TUI.
    #[arg(long)]
    pub plain: bool,

    /// Structured output for integrations.
    #[arg(long)]
    pub json: bool,

    /// Count file lengths (ls) instead of blocks actually used (du).
    #[arg(long)]
    pub apparent: bool,

    /// Use GiB/MiB instead of GB/MB.
    #[arg(long)]
    pub binary: bool,

    /// Do not cross into other mounted filesystems.
    #[arg(short = 'x', long)]
    pub one_file_system: bool,

    /// Count hard-linked files once per link instead of once per inode.
    #[arg(long)]
    pub count_hardlinks: bool,

    /// Include pseudo-filesystems in the disk list.
    #[arg(short = 'a', long)]
    pub all: bool,
}

impl Cli {
    pub fn size_kind(&self) -> SizeKind {
        if self.apparent {
            SizeKind::Apparent
        } else {
            SizeKind::Allocated
        }
    }

    pub fn unit(&self) -> Unit {
        if self.binary {
            Unit::Binary
        } else {
            Unit::Decimal
        }
    }

    pub fn scan_options(&self) -> ScanOptions {
        ScanOptions {
            one_file_system: self.one_file_system,
            max_depth: self.depth,
            dedup_hardlinks: !self.count_hardlinks,
        }
    }

    /// True when the user asked for text rather than the interactive view.
    pub fn wants_text_output(&self) -> bool {
        self.plain || self.json || self.filesystems || self.inodes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_bare_invocation_scans_nothing_and_shows_everything_by_default() {
        let cli = Cli::parse_from(["disko"]);
        assert!(cli.path.is_none());
        assert_eq!(cli.top, 20);
        assert!(!cli.wants_text_output());
        assert_eq!(cli.size_kind(), SizeKind::Allocated);
        assert_eq!(cli.unit(), Unit::Decimal);
    }

    #[test]
    fn flags_map_onto_scan_options() {
        let cli = Cli::parse_from(["disko", "/", "--depth", "3", "-x", "--count-hardlinks"]);
        let options = cli.scan_options();
        assert_eq!(cli.path, Some(PathBuf::from("/")));
        assert_eq!(options.max_depth, Some(3));
        assert!(options.one_file_system);
        assert!(!options.dedup_hardlinks);
    }

    #[test]
    fn detail_modes_imply_text_output() {
        assert!(Cli::parse_from(["disko", "--filesystems"]).wants_text_output());
        assert!(Cli::parse_from(["disko", "--inodes"]).wants_text_output());
        assert!(Cli::parse_from(["disko", "--json"]).wants_text_output());
        // --details on its own opens the panel in the TUI.
        assert!(!Cli::parse_from(["disko", "--details"]).wants_text_output());
    }

    #[test]
    fn apparent_and_binary_change_the_units() {
        let cli = Cli::parse_from(["disko", "--apparent", "--binary"]);
        assert_eq!(cli.size_kind(), SizeKind::Apparent);
        assert_eq!(cli.unit(), Unit::Binary);
    }
}
