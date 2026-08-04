//! Rendering for the time-travel commands: `diff`, `clean` and `history`.
//!
//! These print and exit rather than taking over the terminal — the question
//! "what happened?" usually wants an answer you can scroll back to, not a
//! session to quit out of.

use std::io::{IsTerminal, Write};

use anyhow::Result;
use disko_core::categories::Group;
use disko_core::diff::{Change, ChangeKind, Diff};
use disko_core::size::format;
use disko_core::{Snapshot, Unit};

use crate::model::display_path;
use crate::timefmt;

/// Minimal ANSI, applied only when a person is looking.
#[derive(Copy, Clone, Debug)]
pub struct Palette {
    enabled: bool,
}

impl Palette {
    pub fn detect() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        Self {
            enabled: std::io::stdout().is_terminal() && !no_color,
        }
    }

    pub fn plain() -> Self {
        Self { enabled: false }
    }

    fn wrap(&self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn grew(&self, text: &str) -> String {
        self.wrap("32", text)
    }

    pub fn freed(&self, text: &str) -> String {
        self.wrap("36", text)
    }

    pub fn bold(&self, text: &str) -> String {
        self.wrap("1", text)
    }

    pub fn dim(&self, text: &str) -> String {
        self.wrap("2", text)
    }

    pub fn warn(&self, text: &str) -> String {
        self.wrap("33", text)
    }
}

/// `+46 GB` / `-12 GB`, always signed so the direction is unmistakable.
pub fn signed(delta: i64, unit: Unit) -> String {
    let magnitude = format(delta.unsigned_abs(), unit);
    if delta < 0 {
        format!("-{magnitude}")
    } else {
        format!("+{magnitude}")
    }
}

pub fn diff_report(
    out: &mut impl Write,
    diff: &Diff,
    unit: Unit,
    top: usize,
    now: u64,
    palette: Palette,
) -> Result<()> {
    let grew = diff.growth(top);
    let freed = diff.shrinkage(top);

    let when = timefmt::moment(diff.before_at, now);
    // Under a minute the window is noise; "since today, 01:39" already said it.
    let elapsed = if diff.elapsed() >= 60 {
        format!(" ({})", timefmt::humanize(diff.elapsed()))
    } else {
        String::new()
    };

    let headline = if diff.total_delta() >= 0 {
        format!("{} added", signed(diff.total_delta(), unit))
    } else {
        format!("{} freed", format(diff.total_delta().unsigned_abs(), unit))
    };
    writeln!(
        out,
        "\n {} — {} since {when}{elapsed}\n",
        palette.bold("disko"),
        palette.bold(&headline),
    )?;

    if grew.is_empty() && freed.is_empty() {
        writeln!(
            out,
            " {}\n",
            palette.dim("nothing moved enough to be worth reporting")
        )?;
        return Ok(());
    }

    if !grew.is_empty() {
        write_changes(out, &grew, unit, palette, true)?;
    }

    if !freed.is_empty() {
        writeln!(out, "\n {}", palette.dim("Freed"))?;
        write_changes(out, &freed, unit, palette, false)?;
    }

    writeln!(
        out,
        "\n {}\n",
        palette.dim(&format!(
            "{} → {}",
            format(diff.before_total, unit),
            format(diff.after_total, unit)
        ))
    )?;
    Ok(())
}

fn write_changes(
    out: &mut impl Write,
    changes: &[&Change],
    unit: Unit,
    palette: Palette,
    growing: bool,
) -> Result<()> {
    let width = changes
        .iter()
        .map(|change| signed(change.delta, unit).chars().count())
        .max()
        .unwrap_or(8);

    for change in changes {
        let amount = format!("{:>width$}", signed(change.delta, unit));
        let amount = if growing {
            palette.grew(&amount)
        } else {
            palette.freed(&amount)
        };

        // "new" earns its place: it is the difference between a cache that
        // grew and a directory that appeared out of nowhere.
        let note = match change.kind {
            ChangeKind::Added if !change.before_is_bound => "  (new)",
            ChangeKind::Removed => "  (gone)",
            _ => "",
        };

        writeln!(
            out,
            "  {}  {}{}",
            amount,
            display_path(&change.path),
            palette.dim(note)
        )?;
    }
    Ok(())
}

/// Tab-separated `delta_bytes  delta_human  kind  path`.
pub fn diff_plain(out: &mut impl Write, diff: &Diff, unit: Unit, top: usize) -> Result<()> {
    for change in diff.growth(top).into_iter().chain(diff.shrinkage(top)) {
        writeln!(
            out,
            "{}\t{}\t{}\t{}",
            change.delta,
            signed(change.delta, unit),
            kind_label(change.kind),
            change.path.display()
        )?;
    }
    Ok(())
}

fn kind_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "added",
        ChangeKind::Grown => "grown",
        ChangeKind::Shrunk => "shrunk",
        ChangeKind::Removed => "removed",
        ChangeKind::Unchanged => "unchanged",
    }
}

pub fn clean_report(
    out: &mut impl Write,
    groups: &[Group<'_>],
    unit: Unit,
    now: u64,
    palette: Palette,
) -> Result<()> {
    if groups.is_empty() {
        writeln!(
            out,
            "\n {}\n",
            palette.dim("no reclaimable developer storage found")
        )?;
        return Ok(());
    }

    let total: u64 = groups.iter().map(|group| group.size).sum();
    writeln!(
        out,
        "\n {}{:>width$}\n",
        palette.bold("Reclaimable developer storage"),
        palette.bold(&format(total, unit)),
        // 48 columns of label, then the total right-aligned after it.
        width = 22
    )?;

    let size_width = groups
        .iter()
        .map(|group| format(group.size, unit).chars().count())
        .max()
        .unwrap_or(8);
    let label_width = groups
        .iter()
        .map(|group| group.rule.label.chars().count())
        .max()
        .unwrap_or(20)
        .min(28);

    for group in groups {
        let safety = match group.rule.safety {
            disko_core::Safety::Regenerable => palette.dim(group.rule.safety.label()),
            disko_core::Safety::ReviewFirst => palette.warn(group.rule.safety.label()),
        };
        writeln!(
            out,
            "  {:>size_width$}  {:<label_width$}  {}",
            format(group.size, unit),
            group.rule.label,
            safety
        )?;

        let mut notes = Vec::new();
        if let Some(command) = group.rule.regenerate {
            // No backticks: some entries are commands ("cargo build") and
            // some are actions ("recreate the AVD"), and dressing the latter
            // up as shell reads badly.
            notes.push(format!("regenerate: {command}"));
        }
        // Four months of dust is the strongest argument for deleting anything.
        let idle = group.idle_for(now);
        if idle > 30 * 24 * 3600 {
            notes.push(format!("unused for {}", timefmt::humanize(idle)));
        } else {
            notes.push(format!("last used {}", timefmt::ago(idle)));
        }
        if group.paths.len() > 1 {
            notes.push(format!("{} locations", group.paths.len()));
        }

        writeln!(
            out,
            "  {:>size_width$}  {}",
            "",
            palette.dim(&notes.join(" · "))
        )?;
    }

    writeln!(
        out,
        "\n {}\n",
        palette.dim("disko clean --delete removes these, after confirmation")
    )?;
    Ok(())
}

/// Tab-separated `bytes  human  safety  last_used_unix  regenerate  path`.
pub fn clean_plain(out: &mut impl Write, groups: &[Group<'_>], unit: Unit) -> Result<()> {
    for group in groups {
        for path in &group.paths {
            writeln!(
                out,
                "{}\t{}\t{}\t{}\t{}\t{}",
                group.size,
                format(group.size, unit),
                group.rule.id,
                group.last_used,
                group.rule.regenerate.unwrap_or(""),
                path.display()
            )?;
        }
    }
    Ok(())
}

pub fn history_report(
    out: &mut impl Write,
    root: &std::path::Path,
    snapshots: &[Snapshot],
    unit: Unit,
    now: u64,
    palette: Palette,
) -> Result<()> {
    if snapshots.is_empty() {
        writeln!(
            out,
            "\n {}\n",
            palette.dim(&format!("no snapshots yet for {}", display_path(root)))
        )?;
        return Ok(());
    }

    writeln!(
        out,
        "\n {} {}\n",
        palette.bold("History for"),
        display_path(root)
    )?;

    let mut previous: Option<&Snapshot> = None;
    for snapshot in snapshots {
        let change = match previous {
            Some(earlier) => {
                let delta = snapshot.total_allocated as i64 - earlier.total_allocated as i64;
                if delta == 0 {
                    palette.dim("no change")
                } else if delta > 0 {
                    palette.grew(&signed(delta, unit))
                } else {
                    palette.freed(&signed(delta, unit))
                }
            }
            None => palette.dim("first scan"),
        };

        writeln!(
            out,
            "  {:<22}  {:>10}  {}",
            timefmt::moment(snapshot.taken_at, now),
            format(snapshot.total_allocated, unit),
            change
        )?;
        previous = Some(snapshot);
    }

    writeln!(out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use disko_core::{DiskEntry, EntryType, SizeKind};
    use std::path::PathBuf;

    const GB: u64 = 1_000_000_000;

    fn dir(path: &str, size: u64, children: Vec<DiskEntry>) -> DiskEntry {
        let mut entry = DiskEntry::new(PathBuf::from(path), EntryType::Directory);
        entry.allocated_size = size;
        entry.apparent_size = size;
        entry.children = children;
        entry
    }

    fn sample_diff() -> Diff {
        let before = dir(
            "/home",
            100 * GB,
            vec![dir("/home/Library", 10 * GB, vec![])],
        );
        let after = dir(
            "/home",
            182 * GB,
            vec![
                dir("/home/Library", 56 * GB, vec![]),
                dir("/home/.gradle", 18 * GB, vec![]),
                dir("/home/Downloads", 11 * GB, vec![]),
            ],
        );
        disko_core::diff::diff(
            &before,
            &after,
            SizeKind::Allocated,
            1_800_000_000,
            1_800_259_200,
            0,
        )
    }

    fn rendered(f: impl Fn(&mut Vec<u8>) -> Result<()>) -> String {
        let mut buffer = Vec::new();
        f(&mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    #[test]
    fn the_diff_headline_says_how_much_and_since_when() {
        let diff = sample_diff();
        let text = rendered(|out| {
            diff_report(
                out,
                &diff,
                Unit::Decimal,
                10,
                1_800_259_200,
                Palette::plain(),
            )
        });

        assert!(text.contains("+82 GB added since"), "{text}");
        assert!(text.contains("3 days"), "{text}");
        assert!(text.contains("+46 GB"), "{text}");
        assert!(text.contains("/home/Library"), "{text}");
        assert!(text.contains("100 GB → 182 GB"), "{text}");
    }

    #[test]
    fn new_directories_are_marked_in_the_report() {
        let diff = sample_diff();
        let text = rendered(|out| {
            diff_report(
                out,
                &diff,
                Unit::Decimal,
                10,
                1_800_259_200,
                Palette::plain(),
            )
        });
        // .gradle did not exist before.
        let gradle = text.lines().find(|line| line.contains(".gradle")).unwrap();
        assert!(gradle.contains("(new)"), "{gradle}");
        // Library did, so it merely grew.
        let library = text.lines().find(|line| line.contains("Library")).unwrap();
        assert!(!library.contains("(new)"), "{library}");
    }

    #[test]
    fn a_quiet_period_says_so_rather_than_printing_an_empty_list() {
        let tree = dir("/home", 100 * GB, vec![]);
        let diff = disko_core::diff::diff(&tree, &tree, SizeKind::Allocated, 0, 100, 0);

        let text =
            rendered(|out| diff_report(out, &diff, Unit::Decimal, 10, 100, Palette::plain()));
        assert!(text.contains("nothing moved"), "{text}");
    }

    #[test]
    fn plain_diff_output_leads_with_signed_bytes() {
        let diff = sample_diff();
        let text = rendered(|out| diff_plain(out, &diff, Unit::Decimal, 10));

        let first = text.lines().next().unwrap();
        let fields: Vec<&str> = first.split('\t').collect();
        assert_eq!(fields[0], "46000000000");
        assert_eq!(fields[1], "+46 GB");
        assert_eq!(fields[2], "grown");
        assert_eq!(fields[3], "/home/Library");
    }

    #[test]
    fn freed_space_is_reported_as_a_negative() {
        let before = dir("/home", 60 * GB, vec![dir("/home/gone", 50 * GB, vec![])]);
        let after = dir("/home", 10 * GB, vec![]);
        let diff = disko_core::diff::diff(&before, &after, SizeKind::Allocated, 0, 86400, 0);

        let text =
            rendered(|out| diff_report(out, &diff, Unit::Decimal, 10, 86400, Palette::plain()));

        assert!(text.contains("50 GB freed"), "{text}");
        assert!(text.contains("Freed"), "{text}");
        assert!(text.contains("-50 GB"), "{text}");
        assert!(text.contains("(gone)"), "{text}");
    }

    #[test]
    fn signed_sizes_always_carry_their_sign() {
        assert_eq!(signed(1_000_000_000, Unit::Decimal), "+1 GB");
        assert_eq!(signed(-1_000_000_000, Unit::Decimal), "-1 GB");
        assert_eq!(signed(0, Unit::Decimal), "+0 B");
    }

    #[test]
    fn colour_is_off_when_nobody_is_looking() {
        let plain = Palette::plain();
        assert_eq!(plain.grew("+1 GB"), "+1 GB");
        assert!(!plain.bold("x").contains('\x1b'));
    }
}
