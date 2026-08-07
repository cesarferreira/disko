//! The view model both the TUI and the text output read from.
//!
//! Turning a scanned tree into rows — sorting, filtering, capping to the top N
//! and folding the rest into "Other" — happens once, here, so `--plain` and
//! the interactive view can never disagree about what the biggest thing is.

use std::path::{Path, PathBuf};

use disko_core::diff::ChangeNode;
use disko_core::{DiskEntry, SizeKind};

/// What the rows and the sunburst are measuring.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Metric {
    /// How much space something takes.
    #[default]
    Size,
    /// How much it changed since the last scan.
    Growth,
}

impl Metric {
    pub fn label(self) -> &'static str {
        match self {
            Metric::Size => "size",
            Metric::Growth => "growth",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Size,
    Name,
    Items,
}

impl Sort {
    pub fn next(self) -> Self {
        match self {
            Sort::Size => Sort::Name,
            Sort::Name => Sort::Items,
            Sort::Items => Sort::Size,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sort::Size => "size",
            Sort::Name => "name",
            Sort::Items => "items",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    /// `None` for the synthetic "Other" row, which stands for several entries.
    pub path: Option<PathBuf>,
    pub name: String,
    pub size: u64,
    /// How much this entry changed, when a comparison is loaded.
    pub delta: Option<i64>,
    pub items: u64,
    /// Share of the parent directory.
    pub fraction: f64,
    /// Rank by size, so a row keeps its colour when the sort changes.
    pub color_index: usize,
    pub is_dir: bool,
}

impl Row {
    pub fn is_other(&self) -> bool {
        self.path.is_none()
    }

    /// The number this row is being ranked and drawn by.
    pub fn magnitude(&self, metric: Metric) -> u64 {
        match metric {
            Metric::Size => self.size,
            Metric::Growth => self.delta.unwrap_or(0).unsigned_abs(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RowOptions {
    pub size_kind: SizeKind,
    /// Cap the list, folding everything past it into "Other".
    pub top: Option<usize>,
    pub sort: Sort,
    /// Case-insensitive substring filter on the entry name.
    pub filter: Option<String>,
}

impl Default for RowOptions {
    fn default() -> Self {
        Self {
            size_kind: SizeKind::Allocated,
            top: None,
            sort: Sort::Size,
            filter: None,
        }
    }
}

/// `parent_path` is where `parent` sits: entries below the root of a scan hold
/// only their own name, so a row's full path is built here from the two.
pub fn rows(parent_path: &Path, parent: &DiskEntry, options: &RowOptions) -> Vec<Row> {
    let total = parent.size(options.size_kind);

    // Rank by size first and keep that index on every row: colours in the
    // legend, the bars and the sunburst then agree no matter how the list is
    // ordered on screen.
    let mut by_size: Vec<&DiskEntry> = parent.children.iter().collect();
    by_size.sort_by(|a, b| {
        b.size(options.size_kind)
            .cmp(&a.size(options.size_kind))
            .then_with(|| a.name().cmp(&b.name()))
    });

    let mut ranked: Vec<(usize, &DiskEntry)> = by_size.into_iter().enumerate().collect();

    let filtering = options.filter.as_ref().is_some_and(|f| !f.is_empty());
    if let Some(filter) = &options.filter {
        let needle = filter.to_lowercase();
        ranked.retain(|(_, entry)| entry.name().to_lowercase().contains(&needle));
    }

    // "Other" only makes sense over a complete list; while filtering, the user
    // asked to see a subset and hiding part of it again would be perverse.
    let cap = options.top.filter(|_| !filtering);
    let (visible, hidden) = match cap {
        Some(top) if ranked.len() > top => ranked.split_at(top),
        _ => (ranked.as_slice(), [].as_slice()),
    };

    let mut rows: Vec<Row> = visible
        .iter()
        .map(|(rank, entry)| Row {
            path: Some(parent_path.join(entry.name_os())),
            name: entry.name().to_string(),
            size: entry.size(options.size_kind),
            delta: None,
            items: entry.items,
            fraction: disko_core::size::fraction(entry.size(options.size_kind), total),
            color_index: *rank,
            is_dir: entry.is_dir(),
        })
        .collect();

    if !hidden.is_empty() {
        let size: u64 = hidden.iter().map(|(_, e)| e.size(options.size_kind)).sum();
        let items: u64 = hidden.iter().map(|(_, e)| e.items).sum();
        rows.push(Row {
            path: None,
            name: format!("Other ({} entries)", hidden.len()),
            size,
            delta: None,
            items,
            fraction: disko_core::size::fraction(size, total),
            color_index: usize::MAX,
            is_dir: false,
        });
    }

    sort_rows(&mut rows, options.sort);
    rows
}

fn sort_rows(rows: &mut [Row], sort: Sort) {
    match sort {
        // "Other" stays pinned last: it is a summary of the tail, not a row
        // that competes with the rest.
        Sort::Size => rows.sort_by(|a, b| {
            a.is_other()
                .cmp(&b.is_other())
                .then_with(|| b.size.cmp(&a.size))
                .then_with(|| a.name.cmp(&b.name))
        }),
        Sort::Name => rows.sort_by(|a, b| {
            a.is_other()
                .cmp(&b.is_other())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
        Sort::Items => rows.sort_by(|a, b| {
            a.is_other()
                .cmp(&b.is_other())
                .then_with(|| b.items.cmp(&a.items))
                .then_with(|| a.name.cmp(&b.name))
        }),
    }
}

/// Rows built from a comparison rather than from a scan.
///
/// These come from the change tree, not the current one, so a directory that
/// was deleted still gets a row — "50 GB freed here" is exactly as much of an
/// answer as "50 GB appeared there".
pub fn growth_rows(node: &ChangeNode, options: &RowOptions) -> Vec<Row> {
    let mut rows: Vec<Row> = node
        .children
        .iter()
        .filter(|child| child.change.delta != 0)
        .map(|child| Row {
            path: Some(child.change.path.clone()),
            name: child
                .change
                .path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| child.change.path.display().to_string()),
            size: child.change.after,
            delta: Some(child.change.delta),
            items: 0,
            fraction: 0.0,
            color_index: 0,
            is_dir: true,
        })
        .collect();

    if let Some(filter) = &options.filter {
        let needle = filter.to_lowercase();
        rows.retain(|row| row.name.to_lowercase().contains(&needle));
    }

    // Biggest movement first, in whichever direction.
    rows.sort_by_key(|row| std::cmp::Reverse(row.delta.unwrap_or(0).abs()));

    // Bars are drawn relative to the biggest mover in view rather than to the
    // parent's total: a directory can grow by more than its parent did when
    // something else shrank.
    let scale = rows
        .first()
        .map(|row| row.delta.unwrap_or(0).unsigned_abs())
        .unwrap_or(0);
    for (index, row) in rows.iter_mut().enumerate() {
        row.color_index = index;
        row.fraction = disko_core::size::fraction(row.delta.unwrap_or(0).unsigned_abs(), scale);
    }

    if let Some(top) = options.top {
        rows.truncate(top);
    }
    rows
}

/// The biggest things two levels down — the "where should I look next" list.
///
/// Direct children answer *what* is big ("Users"), grandchildren answer where
/// to actually go ("~/Downloads").
pub fn largest_items<'a>(
    parent_path: &Path,
    parent: &'a DiskEntry,
    count: usize,
    size_kind: SizeKind,
) -> Vec<(PathBuf, &'a DiskEntry)> {
    parent
        .largest_at_depth(parent_path, 2, count, size_kind)
        .into_iter()
        .filter(|(_, entry)| entry.size(size_kind) > 0)
        .collect()
}

/// `/home/cesar/code` -> `~/code`, so paths stay readable in a narrow column.
pub fn display_path(path: &Path) -> String {
    display_path_from(path, home_dir().as_deref())
}

/// The same, with the home directory passed in — so tests do not have to
/// mutate the process environment out from under everything else running in
/// parallel.
pub fn display_path_from(path: &Path, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.display().to_string();
    };
    match path.strip_prefix(home) {
        Ok(relative) if relative.as_os_str().is_empty() => "~".to_string(),
        Ok(relative) => format!("~/{}", relative.display()),
        Err(_) => path.display().to_string(),
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use disko_core::EntryType;

    fn entry(name: &str, size: u64, items: u64) -> DiskEntry {
        let mut entry =
            DiskEntry::new(PathBuf::from(format!("/root/{name}")), EntryType::Directory);
        entry.allocated_size = size;
        entry.apparent_size = size;
        entry.items = items;
        entry
    }

    fn parent(children: Vec<DiskEntry>) -> DiskEntry {
        let mut parent = DiskEntry::new(PathBuf::from("/root"), EntryType::Directory);
        for child in &children {
            parent.allocated_size += child.size(SizeKind::Allocated);
            parent.apparent_size += child.apparent_size;
        }
        parent.children = children;
        parent
    }

    fn sample() -> DiskEntry {
        parent(vec![
            entry("alpha", 100, 5),
            entry("beta", 300, 1),
            entry("gamma", 600, 20),
        ])
    }

    #[test]
    fn rows_are_largest_first_with_shares_of_the_parent() {
        let tree = sample();
        let rows = rows(Path::new("/root"), &tree, &RowOptions::default());

        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["gamma", "beta", "alpha"]
        );
        assert!((rows[0].fraction - 0.6).abs() < 1e-9);
    }

    #[test]
    fn the_tail_folds_into_one_other_row() {
        let tree = sample();
        let rows = rows(
            Path::new("/root"),
            &tree,
            &RowOptions {
                top: Some(1),
                ..Default::default()
            },
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "gamma");
        assert!(rows[1].is_other());
        assert_eq!(rows[1].size, 400);
        assert_eq!(rows[1].items, 6);
        assert!(rows[1].name.contains("2 entries"));
    }

    #[test]
    fn colours_stay_put_when_the_sort_changes() {
        let tree = sample();
        let by_size = rows(Path::new("/root"), &tree, &RowOptions::default());
        let by_name = rows(
            Path::new("/root"),
            &tree,
            &RowOptions {
                sort: Sort::Name,
                ..Default::default()
            },
        );

        let colour_of =
            |rows: &[Row], name: &str| rows.iter().find(|r| r.name == name).unwrap().color_index;
        assert_eq!(colour_of(&by_size, "gamma"), colour_of(&by_name, "gamma"));
        assert_eq!(colour_of(&by_name, "gamma"), 0);
        assert_eq!(
            by_name.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["alpha", "beta", "gamma"]
        );
    }

    #[test]
    fn filtering_shows_matches_and_drops_the_other_row() {
        let tree = sample();
        let rows = rows(
            Path::new("/root"),
            &tree,
            &RowOptions {
                top: Some(1),
                filter: Some("a".to_string()),
                ..Default::default()
            },
        );

        // alpha, beta and gamma all contain "a", and none are folded away.
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| !r.is_other()));
    }

    #[test]
    fn filtering_is_case_insensitive() {
        let tree = sample();
        let rows = rows(
            Path::new("/root"),
            &tree,
            &RowOptions {
                filter: Some("GAM".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "gamma");
    }

    #[test]
    fn sorting_by_items_still_pins_other_last() {
        let tree = sample();
        let rows = rows(
            Path::new("/root"),
            &tree,
            &RowOptions {
                top: Some(1),
                sort: Sort::Items,
                ..Default::default()
            },
        );
        assert_eq!(rows[0].name, "gamma");
        assert!(rows.last().unwrap().is_other());
    }

    #[test]
    fn an_empty_directory_produces_no_rows() {
        let tree = parent(vec![]);
        assert!(rows(Path::new("/root"), &tree, &RowOptions::default()).is_empty());
    }

    #[test]
    fn paths_under_home_are_collapsed() {
        let home = Some(Path::new("/home/tester"));
        assert_eq!(
            display_path_from(Path::new("/home/tester/code"), home),
            "~/code"
        );
        assert_eq!(display_path_from(Path::new("/home/tester"), home), "~");
        assert_eq!(display_path_from(Path::new("/var/log"), home), "/var/log");
        // With no home to compare against, a path is just a path.
        assert_eq!(display_path_from(Path::new("/var/log"), None), "/var/log");
    }
}
