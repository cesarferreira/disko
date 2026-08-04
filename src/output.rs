//! Non-interactive output: the `--plain`, `--json`, `--filesystems`,
//! `--inodes` and `--details` modes.

use std::io::Write;

use anyhow::Result;
use disko_core::size::{format, format_percent, format_percent_whole};
use disko_core::{DiskEntry, Filesystem, Unit};
use serde::Serialize;

use crate::model::{self, RowOptions};

#[derive(Serialize)]
struct ScanReport<'a> {
    root: &'a DiskEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    filesystem: Option<&'a Filesystem>,
}

pub fn scan_json(
    out: &mut impl Write,
    root: &DiskEntry,
    filesystem: Option<&Filesystem>,
) -> Result<()> {
    let report = ScanReport { root, filesystem };
    serde_json::to_writer_pretty(&mut *out, &report)?;
    writeln!(out)?;
    Ok(())
}

pub fn filesystems_json(out: &mut impl Write, filesystems: &[Filesystem]) -> Result<()> {
    serde_json::to_writer_pretty(&mut *out, filesystems)?;
    writeln!(out)?;
    Ok(())
}

/// Tab-separated `bytes  human  share  path`, one line per entry.
///
/// Bytes come first and raw so `cut -f1` and `sort -n` work; the human column
/// is there for the times a person reads it too.
pub fn scan_plain(
    out: &mut impl Write,
    root: &DiskEntry,
    options: &RowOptions,
    unit: Unit,
) -> Result<()> {
    let rows = model::rows(root, options);
    for row in &rows {
        let path = match &row.path {
            Some(path) => path.display().to_string(),
            None => format!("{} (other)", root.path.display()),
        };
        writeln!(
            out,
            "{}\t{}\t{}\t{}",
            row.size,
            format(row.size, unit),
            format_percent(row.fraction),
            path
        )?;
    }
    Ok(())
}

/// The default `--plain` filesystem table: names and sizes, nothing else.
pub fn filesystems_plain(
    out: &mut impl Write,
    filesystems: &[Filesystem],
    unit: Unit,
    details: bool,
) -> Result<()> {
    if filesystems.is_empty() {
        writeln!(out, "no filesystems found")?;
        return Ok(());
    }

    let mut columns = vec![
        ("NAME", Align::Left),
        ("SIZE", Align::Right),
        ("USED", Align::Right),
        ("FREE", Align::Right),
        ("USE%", Align::Right),
        ("MOUNTED ON", Align::Left),
    ];
    if details {
        columns.extend([
            ("DEVICE", Align::Left),
            ("TYPE", Align::Left),
            ("RO", Align::Left),
        ]);
    }
    let mut table = Table::new(columns);

    for fs in filesystems {
        let mut row = vec![
            fs.name.clone(),
            format(fs.total, unit),
            format(fs.used, unit),
            format(fs.available, unit),
            format_percent_whole(fs.used_fraction()),
            fs.mount_point.display().to_string(),
        ];
        if details {
            row.push(fs.device.clone());
            row.push(fs.fs_type.clone());
            row.push(if fs.read_only { "yes" } else { "no" }.to_string());
        }
        table.push(row);
    }

    table.write(out)
}

pub fn inodes_plain(out: &mut impl Write, filesystems: &[Filesystem]) -> Result<()> {
    let mut table = Table::new(vec![
        ("NAME", Align::Left),
        ("INODES", Align::Right),
        ("USED", Align::Right),
        ("FREE", Align::Right),
        ("USE%", Align::Right),
        ("MOUNTED ON", Align::Left),
    ]);
    let mut reported = 0;

    for fs in filesystems {
        let Some(inodes) = &fs.inodes else { continue };
        reported += 1;
        table.push(vec![
            fs.name.clone(),
            count(inodes.total),
            count(inodes.used),
            count(inodes.free),
            format_percent_whole(inodes.used_fraction()),
            fs.mount_point.display().to_string(),
        ]);
    }

    if reported == 0 {
        writeln!(out, "no filesystem reported inode counts")?;
        return Ok(());
    }
    table.write(out)
}

/// The detail panel `--details` promises when a path was scanned.
pub fn filesystem_details(out: &mut impl Write, fs: &Filesystem, unit: Unit) -> Result<()> {
    let mut lines = vec![
        ("Volume", fs.name.clone()),
        ("Mount", fs.mount_point.display().to_string()),
        ("Device", fs.device.clone()),
        ("Filesystem", fs.fs_type.clone()),
        (
            "Read-only",
            if fs.read_only { "yes" } else { "no" }.to_string(),
        ),
        (
            "Removable",
            if fs.removable { "yes" } else { "no" }.to_string(),
        ),
        ("Kind", fs.kind.clone()),
        (
            "Capacity",
            format!(
                "{} used of {} ({})",
                format(fs.used, unit),
                format(fs.total, unit),
                format_percent_whole(fs.used_fraction())
            ),
        ),
        ("Available", format(fs.available, unit)),
    ];
    if let Some(inodes) = &fs.inodes {
        lines.push((
            "Inodes",
            format!(
                "{} used of {} ({})",
                count(inodes.used),
                count(inodes.total),
                format_percent_whole(inodes.used_fraction())
            ),
        ));
    }

    for (label, value) in lines {
        writeln!(out, "{label:<12} {value}")?;
    }
    Ok(())
}

/// `1448344` -> `1,448,344`. Inode counts are unreadable without separators.
fn count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Align {
    Left,
    Right,
}

/// A minimal column-aligned table. Alignment is declared per column rather
/// than inferred from position: a mount path in the middle of a row still
/// needs to hang left.
struct Table {
    columns: Vec<(String, Align)>,
    rows: Vec<Vec<String>>,
}

impl Table {
    fn new(columns: Vec<(&str, Align)>) -> Self {
        Self {
            columns: columns
                .into_iter()
                .map(|(name, align)| (name.to_string(), align))
                .collect(),
            rows: Vec::new(),
        }
    }

    fn push(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    fn write(&self, out: &mut impl Write) -> Result<()> {
        let widths: Vec<usize> = self
            .columns
            .iter()
            .enumerate()
            .map(|(column, (header, _))| {
                self.rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| cell.chars().count())
                    .chain(std::iter::once(header.chars().count()))
                    .max()
                    .unwrap_or(0)
            })
            .collect();

        let render = |cells: &[String]| -> String {
            let mut line = String::new();
            for (column, cell) in cells.iter().enumerate() {
                if column > 0 {
                    line.push_str("  ");
                }
                let width = widths[column];
                match self.columns[column].1 {
                    Align::Right => line.push_str(&format!("{cell:>width$}")),
                    Align::Left => line.push_str(&format!("{cell:<width$}")),
                }
            }
            line.trim_end().to_string()
        };

        let headers: Vec<String> = self.columns.iter().map(|(name, _)| name.clone()).collect();
        writeln!(out, "{}", render(&headers))?;
        for row in &self.rows {
            writeln!(out, "{}", render(row))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use disko_core::EntryType;
    use std::path::PathBuf;

    fn tree() -> DiskEntry {
        let mut root = DiskEntry::new(PathBuf::from("/root"), EntryType::Directory);
        let mut big = DiskEntry::new(PathBuf::from("/root/big"), EntryType::Directory);
        big.allocated_size = 750;
        big.apparent_size = 750;
        let mut small = DiskEntry::new(PathBuf::from("/root/small"), EntryType::File);
        small.allocated_size = 250;
        small.apparent_size = 250;
        root.allocated_size = 1000;
        root.apparent_size = 1000;
        root.items = 3;
        root.children = vec![big, small];
        root
    }

    fn rendered(f: impl Fn(&mut Vec<u8>) -> Result<()>) -> String {
        let mut buffer = Vec::new();
        f(&mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    #[test]
    fn plain_output_leads_with_raw_bytes_for_scripts() {
        let tree = tree();
        let text = rendered(|out| scan_plain(out, &tree, &RowOptions::default(), Unit::Decimal));

        let first = text.lines().next().unwrap();
        let fields: Vec<&str> = first.split('\t').collect();
        assert_eq!(fields[0], "750");
        assert_eq!(fields[1], "750 B");
        assert_eq!(fields[2], "75%");
        assert_eq!(fields[3], "/root/big");
    }

    #[test]
    fn json_output_round_trips() {
        let tree = tree();
        let text = rendered(|out| scan_json(out, &tree, None));
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();

        assert_eq!(parsed["root"]["allocated_size"], 1000);
        assert_eq!(parsed["root"]["children"][0]["path"], "/root/big");
        assert_eq!(parsed["root"]["entry_type"], "directory");
        assert!(parsed.get("filesystem").is_none());
    }

    #[test]
    fn thousands_separators_land_in_the_right_places() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_000), "1,000");
        assert_eq!(count(1_448_344), "1,448,344");
    }

    #[test]
    fn tables_align_their_columns() {
        let mut table = Table::new(vec![
            ("NAME", Align::Left),
            ("SIZE", Align::Right),
            ("MOUNTED ON", Align::Left),
        ]);
        table.push(vec!["Root".into(), "132 GB".into(), "/".into()]);
        table.push(vec!["backup".into(), "1 GB".into(), "/mnt/backup".into()]);
        let text = rendered(|out| table.write(out));

        // Sizes right-align, and so does their header.
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "NAME      SIZE  MOUNTED ON");
        assert_eq!(lines[1], "Root    132 GB  /");
        assert_eq!(lines[2], "backup    1 GB  /mnt/backup");
    }

    #[test]
    fn an_empty_filesystem_list_says_so_rather_than_printing_a_bare_header() {
        let text = rendered(|out| filesystems_plain(out, &[], Unit::Decimal, false));
        assert_eq!(text.trim(), "no filesystems found");
    }
}
