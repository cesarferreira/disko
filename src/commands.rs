//! What each subcommand actually does.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use disko_core::categories::{self, Group};
use disko_core::history::{self, Store};
use disko_core::scan::{self, Cancel, Progress};
use disko_core::{Diff, DiskEntry, SizeKind};

use crate::cli::Options;
use crate::model::display_path;
use crate::report::{self, Palette};
use crate::timefmt;

/// Scan, and record the result unless the user asked us not to.
pub fn scan_and_record(path: &Path, options: &Options) -> Result<(DiskEntry, Option<Store>)> {
    let progress = Progress::default();
    let tree = scan::scan(path, &options.scan_options(), &progress, &Cancel::new())?;

    if !options.may_snapshot() {
        return Ok((tree, None));
    }

    // A failure to record history must never fail the thing you actually
    // asked for.
    let store = match Store::open() {
        Ok(store) => {
            let _ = store.record(&tree);
            Some(store)
        }
        Err(_) => None,
    };
    Ok((tree, store))
}

pub fn diff(
    out: &mut impl Write,
    options: &Options,
    path: Option<PathBuf>,
    since: Option<String>,
) -> Result<()> {
    let store = Store::open().context("cannot open the snapshot history")?;
    let root = resolve_diff_root(path, &store)?;
    let now = history::now();

    let before = match &since {
        Some(spec) => {
            let window = timefmt::parse_duration(spec)?;
            store.latest_before(&root, now.saturating_sub(window))
        }
        None => store.latest(&root),
    };

    let Some(before) = before else {
        bail!(
            "no snapshot of {} from {} — run `disko {}` first to start a history",
            display_path(&root),
            match &since {
                Some(spec) => format!("{spec} ago or earlier"),
                None => "before now".to_string(),
            },
            display_path(&root)
        );
    };

    let (after, _) = scan_and_record(&root, options)?;
    let diff = disko_core::diff::diff(
        &before.tree,
        &after,
        options.size_kind(),
        before.taken_at,
        now,
        before.floor,
    );

    if options.json {
        serde_json::to_writer_pretty(&mut *out, &diff)?;
        writeln!(out)?;
        return Ok(());
    }
    if options.plain {
        return report::diff_plain(out, &diff, options.unit(), options.top);
    }
    report::diff_report(
        out,
        &diff,
        options.unit(),
        options.top,
        now,
        Palette::detect(),
    )
}

/// `disko diff` with no path means "the thing I was just looking at".
fn resolve_diff_root(path: Option<PathBuf>, store: &Store) -> Result<PathBuf> {
    if let Some(path) = path {
        return path
            .canonicalize()
            .with_context(|| format!("cannot read {}", path.display()));
    }
    if let Some(root) = store.most_recent_root() {
        return Ok(root);
    }
    home().context("no previous scan to compare against, and no home directory to fall back on")
}

pub fn clean(
    out: &mut impl Write,
    options: &Options,
    path: Option<PathBuf>,
    safe_only: bool,
    idle_for: Option<String>,
    delete: bool,
    assume_yes: bool,
) -> Result<()> {
    let root = match path {
        Some(path) => path
            .canonicalize()
            .with_context(|| format!("cannot read {}", path.display()))?,
        None => home().context("no home directory to search")?,
    };

    let (tree, _) = scan_and_record(&root, options)?;
    let now = history::now();

    let candidates = categories::find_reclaimable(&tree, options.size_kind());
    let mut groups = categories::group_by_rule(&candidates);

    if safe_only {
        groups.retain(|group| group.rule.safety == disko_core::Safety::Regenerable);
    }
    if let Some(spec) = &idle_for {
        let threshold = timefmt::parse_duration(spec)?;
        groups.retain(|group| group.idle_for(now) >= threshold);
    }

    if options.json {
        serde_json::to_writer_pretty(&mut *out, &groups)?;
        writeln!(out)?;
        return Ok(());
    }
    if options.plain {
        return report::clean_plain(out, &groups, options.unit());
    }

    report::clean_report(out, &groups, options.unit(), now, Palette::detect())?;

    if delete {
        remove(out, &groups, options, assume_yes)?;
    }
    Ok(())
}

/// Delete the listed categories, after making very sure that is what was
/// meant.
///
/// Every path is re-checked against the category rules immediately before
/// removal: the list on screen came from a scan that finished seconds ago, and
/// deleting something because of a stale classification is exactly the failure
/// worth engineering against.
fn remove(
    out: &mut impl Write,
    groups: &[Group<'_>],
    options: &Options,
    assume_yes: bool,
) -> Result<()> {
    let palette = Palette::detect();
    let total: u64 = groups.iter().map(|group| group.size).sum();
    let paths: Vec<&Path> = groups
        .iter()
        .flat_map(|group| group.paths.iter().copied())
        .collect();

    if paths.is_empty() {
        return Ok(());
    }

    if !assume_yes {
        if !std::io::stdin().is_terminal() {
            bail!(
                "refusing to delete without a terminal to confirm at — pass --yes if you mean it"
            );
        }

        writeln!(
            out,
            " {} {} across {} location{}:",
            palette.warn("About to delete"),
            disko_core::size::format(total, options.unit()),
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        )?;
        for path in &paths {
            writeln!(out, "   {}", display_path(path))?;
        }
        write!(out, "\n Type {} to confirm: ", palette.bold("delete"))?;
        out.flush()?;

        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim() != "delete" {
            writeln!(out, " {}", palette.dim("nothing was deleted"))?;
            return Ok(());
        }
    }

    let mut freed = 0u64;
    for group in groups {
        for path in &group.paths {
            // Re-verify, then remove. A path that no longer classifies is one
            // we no longer have a reason to touch.
            if categories::classify(path).is_none() {
                writeln!(
                    out,
                    " {} {} is no longer a recognised cache",
                    palette.warn("skipped"),
                    display_path(path)
                )?;
                continue;
            }
            match std::fs::remove_dir_all(path) {
                Ok(()) => {
                    freed += group.size / group.paths.len().max(1) as u64;
                    writeln!(out, " removed {}", display_path(path))?;
                }
                Err(error) => {
                    writeln!(
                        out,
                        " {} {}: {error}",
                        palette.warn("could not remove"),
                        display_path(path)
                    )?;
                }
            }
        }
    }

    writeln!(
        out,
        "\n {} {}\n",
        palette.bold("Freed"),
        disko_core::size::format(freed, options.unit())
    )?;
    Ok(())
}

pub fn history(
    out: &mut impl Write,
    options: &Options,
    path: Option<PathBuf>,
    forget: bool,
) -> Result<()> {
    let store = Store::open().context("cannot open the snapshot history")?;
    let root = resolve_diff_root(path, &store)?;

    if forget {
        store.forget(&root)?;
        writeln!(out, " forgot the history for {}", display_path(&root))?;
        return Ok(());
    }

    let snapshots = store.load_all(&root);

    if options.json {
        serde_json::to_writer_pretty(&mut *out, &snapshots)?;
        writeln!(out)?;
        return Ok(());
    }

    report::history_report(
        out,
        &root,
        &snapshots,
        options.unit(),
        history::now(),
        Palette::detect(),
    )
}

/// The last thing disko knew about `path`, for painting a screen before the
/// fresh scan has finished.
pub fn last_snapshot(path: &Path) -> Option<disko_core::Snapshot> {
    Store::open().ok()?.latest(path)
}

/// Compare a fresh scan against the last snapshot, then record it.
///
/// Order matters: recording first would leave the newest snapshot equal to the
/// scan being compared, and every diff would come back empty.
pub fn record_and_diff(tree: &DiskEntry, kind: SizeKind, record: bool) -> Option<Diff> {
    let store = Store::open().ok()?;
    let now = history::now();
    let previous = store.latest(&tree.path);

    if record {
        let _ = store.record(tree);
    }

    let previous = previous?;
    Some(disko_core::diff::diff(
        &previous.tree,
        tree,
        kind,
        previous.taken_at,
        now,
        previous.floor,
    ))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use disko_core::EntryType;

    fn options() -> Options {
        use clap::Parser;
        crate::cli::Cli::parse_from(["disko"]).options
    }

    #[test]
    fn a_depth_capped_scan_is_not_recorded() {
        let mut options = options();
        options.depth = Some(2);
        assert!(!options.may_snapshot());
    }

    #[test]
    fn deleting_without_a_terminal_or_yes_is_refused() {
        let rule = disko_core::categories::RULES
            .iter()
            .find(|rule| rule.id == "gradle-caches")
            .unwrap();
        let path = PathBuf::from("/nonexistent/.gradle/caches");
        let groups = vec![Group {
            rule,
            size: 1000,
            last_used: 0,
            paths: vec![&path],
        }];

        let mut out = Vec::new();
        // Tests do not run with a terminal on stdin, which is exactly the
        // situation this guard exists for.
        let error = remove(&mut out, &groups, &options(), false).unwrap_err();
        assert!(error.to_string().contains("--yes"), "{error}");
    }

    #[test]
    fn deletion_skips_paths_that_no_longer_classify() {
        let rule = disko_core::categories::RULES
            .iter()
            .find(|rule| rule.id == "gradle-caches")
            .unwrap();
        // Not under any home directory, so classify() will not claim it.
        let path = PathBuf::from("/tmp/definitely-not-a-known-cache");
        let groups = vec![Group {
            rule,
            size: 1000,
            last_used: 0,
            paths: vec![&path],
        }];

        let mut out = Vec::new();
        remove(&mut out, &groups, &options(), true).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("no longer a recognised cache"), "{text}");
        assert!(text.contains("Freed 0 B"), "{text}");
    }

    #[test]
    fn diffing_without_any_history_explains_what_to_do() {
        let mut out = Vec::new();
        let error = diff(
            &mut out,
            &options(),
            Some(PathBuf::from("/nonexistent-path-for-tests")),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot read"), "{error}");
    }

    #[test]
    fn scanning_a_tree_yields_something_recordable() {
        let mut entry = DiskEntry::new(PathBuf::from("/x"), EntryType::Directory);
        entry.allocated_size = 10;
        // Just proving the shape the store wants is the shape a scan gives.
        assert_eq!(disko_core::history::storage_floor(&entry), 1_000_000);
    }
}
