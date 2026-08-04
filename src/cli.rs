use std::path::PathBuf;

use clap::{Parser, Subcommand};
use disko_core::{ScanOptions, SizeKind, Unit};

#[derive(Parser, Debug)]
#[command(
    name = "disko",
    version,
    about = "Not just where your space went, but when and why",
    long_about = "disko answers three questions: what is full, what is using the space, and \
                  where to look next — and then a fourth that most disk tools cannot: what \
                  changed.\n\n\
                  Every scan records a snapshot, so `disko diff` can tell you what ate 80 GB \
                  since Monday. `disko clean` names reclaimable developer caches, whether they \
                  are safe to remove, and what would regenerate them.\n\n\
                  Filesystem types, inode counts and device names live behind --details rather \
                  than cluttering the default view."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Directory to scan. Omit to choose from your mounted disks.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    #[command(flatten)]
    pub options: Options,
}

/// Flags that mean the same thing wherever they appear, so `disko diff --json`
/// works as readily as `disko --json diff`.
#[derive(Parser, Debug, Clone)]
pub struct Options {
    /// Show only the N largest entries, grouping the rest as "Other".
    #[arg(
        short = 't',
        long,
        value_name = "N",
        default_value_t = 20,
        global = true
    )]
    pub top: usize,

    /// Keep only N levels of the tree. Sizes stay exact either way.
    #[arg(long, value_name = "N", global = true)]
    pub depth: Option<usize>,

    /// Show device names, filesystem types and mount details.
    #[arg(long, global = true)]
    pub details: bool,

    /// List every mounted filesystem, pseudo-filesystems included.
    #[arg(long, global = true)]
    pub filesystems: bool,

    /// Report inode usage instead of bytes.
    #[arg(long, global = true)]
    pub inodes: bool,

    /// Script-friendly output, no TUI.
    #[arg(long, global = true)]
    pub plain: bool,

    /// Structured output for integrations.
    #[arg(long, global = true)]
    pub json: bool,

    /// Count file lengths (ls) instead of blocks actually used (du).
    #[arg(long, global = true)]
    pub apparent: bool,

    /// Use GiB/MiB instead of GB/MB.
    #[arg(long, global = true)]
    pub binary: bool,

    /// Do not cross into other mounted filesystems.
    #[arg(short = 'x', long, global = true)]
    pub one_file_system: bool,

    /// Count hard-linked files once per link instead of once per inode.
    #[arg(long, global = true)]
    pub count_hardlinks: bool,

    /// Include pseudo-filesystems in the disk list.
    #[arg(short = 'a', long, global = true)]
    pub all: bool,

    /// Walk into network filesystems (NFS, SMB, sshfs, blobfuse) too. They are
    /// skipped by default: their contents are not on this disk, and stat-ing
    /// them over the wire can take minutes.
    #[arg(long, global = true)]
    pub remote: bool,

    /// Do not record this scan in the snapshot history.
    #[arg(long, global = true)]
    pub no_snapshot: bool,

    /// Disable deleting entirely, for when disko is only ever meant to look.
    #[arg(long, global = true)]
    pub read_only: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// What changed since the last scan.
    #[command(visible_alias = "changed")]
    Diff {
        /// Directory to compare. Defaults to the last thing you scanned.
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,

        /// How far back to look: 30m, 12h, 7d, 2w, 3mo. Defaults to the
        /// previous scan, whenever that was.
        #[arg(long, value_name = "WHEN")]
        since: Option<String>,
    },

    /// Reclaimable developer storage, with what regenerates it.
    Clean {
        /// Directory to search. Defaults to your home directory.
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,

        /// Only show categories that are safe to regenerate.
        #[arg(long)]
        safe_only: bool,

        /// Only show categories nothing has touched in this long.
        #[arg(long, value_name = "WHEN")]
        idle_for: Option<String>,

        /// Delete the listed categories. Asks for confirmation first.
        #[arg(long)]
        delete: bool,

        /// Skip the confirmation prompt. Only meaningful with --delete.
        #[arg(long, requires = "delete")]
        yes: bool,
    },

    /// Watch a directory grow while something is running.
    Watch {
        /// Directory to watch. Defaults to the current directory.
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,

        /// How often to rescan: 2s, 30s, 1m.
        #[arg(long, value_name = "EVERY", default_value = "3s")]
        interval: String,
    },

    /// Inspect the snapshot history disko keeps.
    History {
        /// Directory whose history to show. Defaults to the last one scanned.
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,

        /// Delete the stored history for this directory.
        #[arg(long)]
        forget: bool,
    },
}

impl Options {
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
            skip_remote: !self.remote,
        }
    }

    /// True when the user asked for text rather than the interactive view.
    pub fn wants_text_output(&self) -> bool {
        self.plain || self.json || self.filesystems || self.inodes
    }

    /// A depth-capped scan sees exact totals but a truncated tree, which would
    /// make a misleading snapshot: later diffs would read the missing depth as
    /// things having disappeared.
    pub fn may_snapshot(&self) -> bool {
        !self.no_snapshot && self.depth.is_none_or(|depth| depth >= 8)
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
    fn a_bare_invocation_explores_with_sensible_defaults() {
        let cli = Cli::parse_from(["disko"]);
        assert!(cli.command.is_none());
        assert!(cli.path.is_none());
        assert_eq!(cli.options.top, 20);
        assert!(!cli.options.wants_text_output());
        assert!(cli.options.may_snapshot());
    }

    #[test]
    fn global_flags_work_on_either_side_of_a_subcommand() {
        // The way people actually type it...
        let after = Cli::parse_from(["disko", "diff", "--json"]);
        assert!(after.options.json);
        assert!(matches!(after.command, Some(Command::Diff { .. })));

        // ...and the way clap would traditionally demand.
        let before = Cli::parse_from(["disko", "--json", "diff"]);
        assert!(before.options.json);
    }

    #[test]
    fn diff_takes_a_path_and_a_window() {
        let cli = Cli::parse_from(["disko", "diff", "/home/cesar", "--since", "7d"]);
        match cli.command {
            Some(Command::Diff { path, since }) => {
                assert_eq!(path, Some(PathBuf::from("/home/cesar")));
                assert_eq!(since.as_deref(), Some("7d"));
            }
            other => panic!("expected diff, got {other:?}"),
        }
    }

    #[test]
    fn clean_defaults_to_listing_rather_than_deleting() {
        let cli = Cli::parse_from(["disko", "clean"]);
        match cli.command {
            Some(Command::Clean { delete, yes, .. }) => {
                assert!(!delete, "clean must never delete unless asked");
                assert!(!yes);
            }
            other => panic!("expected clean, got {other:?}"),
        }
    }

    /// `--yes` on its own would be a foot-gun waiting for a typo.
    #[test]
    fn skipping_the_confirmation_requires_asking_to_delete() {
        assert!(Cli::try_parse_from(["disko", "clean", "--yes"]).is_err());
        assert!(Cli::try_parse_from(["disko", "clean", "--delete", "--yes"]).is_ok());
    }

    #[test]
    fn network_filesystems_are_skipped_unless_asked_for() {
        assert!(
            Cli::parse_from(["disko", "/mnt"])
                .options
                .scan_options()
                .skip_remote
        );
        assert!(
            !Cli::parse_from(["disko", "/mnt", "--remote"])
                .options
                .scan_options()
                .skip_remote
        );
    }

    #[test]
    fn a_shallow_scan_does_not_poison_the_snapshot_history() {
        assert!(
            !Cli::parse_from(["disko", "--depth", "2"])
                .options
                .may_snapshot()
        );
        assert!(
            Cli::parse_from(["disko", "--depth", "12"])
                .options
                .may_snapshot()
        );
        assert!(
            !Cli::parse_from(["disko", "--no-snapshot"])
                .options
                .may_snapshot()
        );
    }

    #[test]
    fn detail_modes_imply_text_output() {
        assert!(
            Cli::parse_from(["disko", "--filesystems"])
                .options
                .wants_text_output()
        );
        assert!(
            Cli::parse_from(["disko", "--inodes"])
                .options
                .wants_text_output()
        );
        // --details on its own opens the panel in the TUI.
        assert!(
            !Cli::parse_from(["disko", "--details"])
                .options
                .wants_text_output()
        );
    }

    #[test]
    fn changed_is_an_alias_for_diff() {
        let cli = Cli::parse_from(["disko", "changed"]);
        assert!(matches!(cli.command, Some(Command::Diff { .. })));
    }

    #[test]
    fn watch_has_a_default_interval() {
        let cli = Cli::parse_from(["disko", "watch"]);
        match cli.command {
            Some(Command::Watch { interval, .. }) => assert_eq!(interval, "3s"),
            other => panic!("expected watch, got {other:?}"),
        }
    }
}
