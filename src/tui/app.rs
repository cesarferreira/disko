//! Interactive state: what is on screen, what is selected, and what a key does.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use disko_core::scan::{self, Cancel, Progress};
use disko_core::{Diff, DiskEntry, Filesystem, ScanOptions, SizeKind, Unit};
use disko_render::{RadialNode, palette};

use crate::model::{self, Metric, Row, RowOptions, Sort};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum View {
    /// No path was given: choose a disk first.
    Picker,
    /// A scan is running.
    Scanning,
    /// The ranked overview — the default answer to "what is using the space".
    Overview,
    /// The radial explorer.
    Explorer,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub size_kind: SizeKind,
    pub unit: Unit,
    pub top: usize,
    pub scan_options: ScanOptions,
    pub show_all_filesystems: bool,
    /// Whether finished scans join the snapshot history.
    pub record_snapshots: bool,
}

/// Live watching: rescan every `every` seconds and report growth against the
/// state of the world when watching started.
#[derive(Clone, Debug)]
pub struct Watch {
    pub every: u64,
    /// The pruned tree the deltas are measured from.
    baseline: Option<DiskEntry>,
    started_at: u64,
    last_scan: Instant,
    pub rounds: u64,
}

impl Watch {
    pub fn new(every: u64) -> Self {
        Self {
            every,
            baseline: None,
            started_at: disko_core::history::now(),
            last_scan: Instant::now(),
            rounds: 0,
        }
    }

    pub fn is_due(&self) -> bool {
        self.last_scan.elapsed() >= Duration::from_secs(self.every)
    }

    pub fn elapsed(&self) -> u64 {
        disko_core::history::now().saturating_sub(self.started_at)
    }
}

/// How a scan should treat the state around it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ScanMode {
    /// A new place: reset the cursor and show progress.
    Fresh,
    /// The same place again: keep where the user was standing.
    Refresh,
}

/// A scan running on another thread.
struct Session {
    progress: Arc<Progress>,
    cancel: Cancel,
    handle: Option<JoinHandle<Result<DiskEntry>>>,
    started: Instant,
}

pub struct App {
    pub settings: Settings,
    pub view: View,
    pub filesystems: Vec<Filesystem>,
    pub picker_index: usize,
    pub tree: Option<DiskEntry>,
    pub root: PathBuf,
    pub cwd: PathBuf,
    pub selection: usize,
    pub marks: HashSet<PathBuf>,
    pub search: Option<String>,
    pub search_active: bool,
    pub sort: Sort,
    pub details: bool,
    pub filesystem: Option<Filesystem>,
    pub status: Option<String>,
    pub quit: bool,
    /// Measuring size, or measuring what changed.
    pub metric: Metric,
    /// The comparison behind growth mode, when there is one.
    pub diff: Option<Diff>,
    pub watch: Option<Watch>,
    session: Option<Session>,
    mode: ScanMode,
}

impl App {
    pub fn new(settings: Settings, details: bool) -> Self {
        let filesystems = disko_core::mounts::list(settings.show_all_filesystems);
        Self {
            settings,
            view: View::Picker,
            filesystems,
            picker_index: 0,
            tree: None,
            root: PathBuf::new(),
            cwd: PathBuf::new(),
            selection: 0,
            marks: HashSet::new(),
            search: None,
            search_active: false,
            sort: Sort::Size,
            details,
            filesystem: None,
            status: None,
            quit: false,
            metric: Metric::Size,
            diff: None,
            watch: None,
            session: None,
            mode: ScanMode::Fresh,
        }
    }

    /// Rescan on a timer and report growth since the moment watching started.
    pub fn watching(mut self, watch: Watch) -> Self {
        self.watch = Some(watch);
        self.metric = Metric::Growth;
        self
    }

    /// Start straight in a scan instead of the disk picker.
    pub fn with_path(mut self, path: &Path) -> Self {
        self.start_scan(path);
        self
    }

    pub fn start_scan(&mut self, path: &Path) {
        self.begin(path, ScanMode::Fresh);
    }

    /// Scan the same place again without disturbing the view — what watch mode
    /// does on every tick, and what `r` does on demand.
    pub fn refresh(&mut self) {
        if self.root.as_os_str().is_empty() || self.session.is_some() {
            return;
        }
        let root = self.root.clone();
        self.begin(&root, ScanMode::Refresh);
    }

    fn begin(&mut self, path: &Path, mode: ScanMode) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.mode = mode;

        if mode == ScanMode::Fresh {
            self.filesystem = disko_core::mounts::for_path(&path);
            self.root = path.clone();
            self.cwd = path.clone();
            self.selection = 0;
            self.search = None;
            self.search_active = false;
            self.tree = None;
            self.status = None;
            self.view = View::Scanning;
        }

        let (progress, cancel, handle) =
            scan::scan_in_background(path, self.settings.scan_options.clone());
        self.session = Some(Session {
            progress,
            cancel,
            handle: Some(handle),
            started: Instant::now(),
        });
    }

    /// Kick off a rescan when the watch interval is up.
    pub fn tick_watch(&mut self) {
        let due = self
            .watch
            .as_ref()
            .is_some_and(|watch| watch.is_due() && self.tree.is_some());
        if due && self.session.is_none() {
            self.refresh();
        }
    }

    /// How long the running scan has been going. A scan that has stopped
    /// counting but keeps ticking is usually stuck on a slow mount, and the
    /// number is the first clue.
    pub fn scan_elapsed(&self) -> Option<Duration> {
        self.session
            .as_ref()
            .map(|session| session.started.elapsed())
    }

    pub fn progress(&self) -> Option<&Progress> {
        self.session
            .as_ref()
            .map(|session| session.progress.as_ref())
    }

    /// Collect a finished scan. Called once per frame; cheap while running.
    pub fn poll_scan(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        let finished = session
            .handle
            .as_ref()
            .is_some_and(|handle| handle.is_finished());
        if !finished {
            return;
        }

        let Some(handle) = session.handle.take() else {
            return;
        };
        match handle.join() {
            Ok(Ok(tree)) => {
                self.absorb_scan(tree);
            }
            Ok(Err(error)) => {
                self.status = Some(format!("{error}"));
                self.view = View::Picker;
            }
            // A panicked worker must not take the UI down with it.
            Err(_) => {
                self.status = Some("scan thread panicked".to_string());
                self.view = View::Picker;
            }
        }
        self.session = None;
    }

    /// Take a finished scan: work out what changed, then remember it.
    ///
    /// The comparison has to happen before the snapshot is recorded, or this
    /// scan would be diffed against itself and every week would look quiet.
    fn absorb_scan(&mut self, tree: DiskEntry) {
        let kind = self.settings.size_kind;

        match &mut self.watch {
            Some(watch) => {
                watch.last_scan = Instant::now();
                watch.rounds += 1;
                match &watch.baseline {
                    Some(baseline) => {
                        self.diff = Some(disko_core::diff::diff(
                            baseline,
                            &tree,
                            kind,
                            watch.started_at,
                            disko_core::history::now(),
                            disko_core::history::storage_floor(baseline),
                        ));
                    }
                    None => {
                        // The first pass is the baseline; there is nothing to
                        // compare it against yet.
                        watch.baseline = Some(disko_core::history::prune_for_storage(&tree));
                    }
                }
            }
            None => {
                self.diff =
                    crate::commands::record_and_diff(&tree, kind, self.settings.record_snapshots);
            }
        }

        self.tree = Some(tree);
        if self.mode == ScanMode::Fresh {
            self.view = View::Overview;
        }
    }

    /// Switch between "what is big" and "what changed".
    pub fn toggle_metric(&mut self) {
        if self.diff.is_none() {
            self.status = Some(match self.watch.is_some() {
                true => "waiting for the first rescan".into(),
                false => "no earlier scan to compare against — this one is now the baseline".into(),
            });
            return;
        }
        self.metric = match self.metric {
            Metric::Size => Metric::Growth,
            Metric::Growth => Metric::Size,
        };
        self.selection = 0;
        self.status = Some(format!("showing {}", self.metric.label()));
    }

    /// The change tree node for wherever the user is standing.
    pub fn current_change(&self) -> Option<&disko_core::diff::ChangeNode> {
        self.diff.as_ref()?.tree.find(&self.cwd)
    }

    pub fn showing_growth(&self) -> bool {
        self.metric == Metric::Growth && self.diff.is_some()
    }

    pub fn cancel_scan(&mut self) {
        if let Some(session) = &self.session {
            session.cancel.cancel();
        }
    }

    pub fn current_entry(&self) -> Option<&DiskEntry> {
        self.tree.as_ref()?.resolve(&self.cwd)
    }

    pub fn row_options(&self, cap: bool) -> RowOptions {
        RowOptions {
            size_kind: self.settings.size_kind,
            top: cap.then_some(self.settings.top),
            sort: self.sort,
            filter: self.search.clone(),
        }
    }

    /// Rows for the current directory. Owned, so callers can hold them while
    /// mutating the app.
    pub fn rows(&self) -> Vec<Row> {
        // The explorer draws every wedge, so it must not fold the tail into
        // an "Other" row that has no path to open.
        let cap = self.view != View::Explorer;
        if self.showing_growth() {
            return match self.current_change() {
                Some(node) => model::growth_rows(node, &self.row_options(cap)),
                None => Vec::new(),
            };
        }
        match self.current_entry() {
            Some(entry) => model::rows(entry, &self.row_options(cap)),
            None => Vec::new(),
        }
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.selection)
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = self.rows().len();
        if count == 0 {
            self.selection = 0;
            return;
        }
        let next = self.selection as isize + delta;
        self.selection = next.clamp(0, count as isize - 1) as usize;
    }

    pub fn select_first(&mut self) {
        self.selection = 0;
    }

    pub fn select_last(&mut self) {
        self.selection = self.rows().len().saturating_sub(1);
    }

    /// Descend into the selected directory.
    pub fn open_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let Some(path) = row.path else {
            self.status = Some("\"Other\" groups several entries — raise --top to see them".into());
            return;
        };
        if !row.is_dir {
            self.status = Some(format!("{} is a file", model::display_path(&path)));
            return;
        }
        self.cwd = path;
        self.selection = 0;
        self.search = None;
        self.search_active = false;
    }

    /// Back up one level, stopping at the directory the scan started from.
    pub fn go_up(&mut self) {
        if self.cwd == self.root {
            self.status = Some("already at the top of this scan".into());
            return;
        }
        let Some(parent) = self.cwd.parent().map(Path::to_path_buf) else {
            return;
        };
        // Remember where we came from so going up lands on the child we left.
        let leaving = self.cwd.clone();
        self.cwd = parent;
        self.search = None;
        self.search_active = false;
        self.selection = self
            .rows()
            .iter()
            .position(|row| row.path.as_ref() == Some(&leaving))
            .unwrap_or(0);
    }

    pub fn toggle_mark(&mut self) {
        let Some(path) = self.selected_row().and_then(|row| row.path) else {
            return;
        };
        if !self.marks.remove(&path) {
            self.marks.insert(path);
        }
    }

    pub fn marked_total(&self) -> u64 {
        let Some(tree) = &self.tree else { return 0 };
        self.marks
            .iter()
            .filter_map(|path| tree.resolve(path))
            .map(|entry| entry.size(self.settings.size_kind))
            .sum()
    }

    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        self.status = Some(format!("sorted by {}", self.sort.label()));
    }

    pub fn toggle_size_kind(&mut self) {
        self.settings.size_kind = match self.settings.size_kind {
            SizeKind::Allocated => SizeKind::Apparent,
            SizeKind::Apparent => SizeKind::Allocated,
        };
        self.status = Some(match self.settings.size_kind {
            SizeKind::Allocated => "showing space used on disk".into(),
            SizeKind::Apparent => "showing apparent file sizes".into(),
        });
    }

    pub fn begin_search(&mut self) {
        self.search_active = true;
        self.search.get_or_insert_with(String::new);
    }

    pub fn push_search(&mut self, ch: char) {
        if let Some(search) = &mut self.search {
            search.push(ch);
            self.selection = 0;
        }
    }

    pub fn pop_search(&mut self) {
        if let Some(search) = &mut self.search {
            search.pop();
            self.selection = 0;
        }
    }

    pub fn clear_search(&mut self) {
        self.search = None;
        self.search_active = false;
        self.selection = 0;
    }

    pub fn rescan(&mut self) {
        let root = self.root.clone();
        if !root.as_os_str().is_empty() {
            self.start_scan(&root);
        }
    }

    /// Breadcrumb for the explorer: `Macintosh HD › Users › cesar`.
    pub fn breadcrumb(&self) -> String {
        let volume = self
            .filesystem
            .as_ref()
            .map(|fs| fs.name.clone())
            .unwrap_or_else(|| model::display_path(&self.root));

        let relative = self.cwd.strip_prefix(&self.root).ok();
        let mut parts = vec![volume];
        if let Some(relative) = relative {
            parts.extend(
                relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().to_string()),
            );
        }
        parts.join(" › ")
    }

    /// The sunburst for the current directory, plus the table mapping each
    /// wedge id back to the path it came from.
    pub fn radial_tree(&self, rings: usize) -> (RadialNode, Vec<PathBuf>) {
        let mut ids = Vec::new();

        if self.showing_growth() {
            let node = match self.current_change() {
                Some(change) => {
                    let scale = biggest_change(change);
                    build_growth_radial(change, rings, scale, &mut ids)
                }
                None => RadialNode::leaf(0, "", 0),
            };
            return (node, ids);
        }

        let node = match self.current_entry() {
            Some(entry) => build_radial(entry, self.settings.size_kind, rings, &mut ids),
            None => RadialNode::leaf(0, "", 0),
        };
        (node, ids)
    }

    pub fn pick_selected_filesystem(&mut self) {
        let Some(fs) = self.filesystems.get(self.picker_index) else {
            return;
        };
        let mount = fs.mount_point.clone();
        self.start_scan(&mount);
    }

    pub fn move_picker(&mut self, delta: isize) {
        if self.filesystems.is_empty() {
            return;
        }
        let next = self.picker_index as isize + delta;
        self.picker_index = next.clamp(0, self.filesystems.len() as isize - 1) as usize;
    }
}

fn build_radial(
    entry: &DiskEntry,
    kind: SizeKind,
    rings: usize,
    ids: &mut Vec<PathBuf>,
) -> RadialNode {
    let id = ids.len();
    ids.push(entry.path.clone());

    let mut children = Vec::new();
    if rings > 0 {
        let mut sorted: Vec<&DiskEntry> = entry.children.iter().collect();
        sorted.sort_by(|a, b| {
            b.size(kind)
                .cmp(&a.size(kind))
                .then_with(|| a.name().cmp(&b.name()))
        });
        children = sorted
            .into_iter()
            .map(|child| build_radial(child, kind, rings - 1, ids))
            .collect();
    }

    RadialNode {
        id,
        label: entry.name().to_string(),
        size: entry.size(kind),
        color: None,
        children,
    }
}

/// The same sunburst, but wedges are sized by how much moved and coloured by
/// which way: growth glows, shrinkage cools, and anything that stayed put
/// recedes into the background.
fn build_growth_radial(
    node: &disko_core::diff::ChangeNode,
    rings: usize,
    scale: i64,
    ids: &mut Vec<PathBuf>,
) -> RadialNode {
    let id = ids.len();
    ids.push(node.change.path.clone());

    let mut children = Vec::new();
    if rings > 0 {
        let mut sorted: Vec<&disko_core::diff::ChangeNode> = node
            .children
            .iter()
            .filter(|child| child.change.delta != 0)
            .collect();
        sorted.sort_by_key(|child| std::cmp::Reverse(child.change.delta.abs()));
        children = sorted
            .into_iter()
            .map(|child| build_growth_radial(child, rings - 1, scale, ids))
            .collect();
    }

    RadialNode {
        id,
        label: node
            .change
            .path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        size: node.change.delta.unsigned_abs(),
        color: Some(palette::growth_color(node.change.delta, scale)),
        children,
    }
}

fn biggest_change(node: &disko_core::diff::ChangeNode) -> i64 {
    node.children
        .iter()
        .map(|child| child.change.delta.abs())
        .max()
        .unwrap_or(0)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use disko_core::EntryType;

    fn settings() -> Settings {
        Settings {
            size_kind: SizeKind::Allocated,
            unit: Unit::Decimal,
            top: 20,
            scan_options: ScanOptions::default(),
            show_all_filesystems: false,
            // Tests must never touch the user's real snapshot history.
            record_snapshots: false,
        }
    }

    fn entry(path: &str, size: u64, children: Vec<DiskEntry>) -> DiskEntry {
        let mut entry = DiskEntry::new(PathBuf::from(path), EntryType::Directory);
        entry.allocated_size = size;
        entry.apparent_size = size;
        entry.children = children;
        entry
    }

    /// An app with a canned tree, skipping the real scan.
    fn app_with_tree() -> App {
        let tree = entry(
            "/root",
            1000,
            vec![
                entry(
                    "/root/big",
                    700,
                    vec![entry("/root/big/inner", 500, vec![])],
                ),
                entry("/root/small", 300, vec![]),
            ],
        );
        let mut app = App::new(settings(), false);
        app.root = PathBuf::from("/root");
        app.cwd = PathBuf::from("/root");
        app.tree = Some(tree);
        app.view = View::Overview;
        app
    }

    #[test]
    fn opening_a_directory_descends_and_resets_the_selection() {
        let mut app = app_with_tree();
        app.selection = 1;
        app.selection = 0; // "big" is largest, so it is first
        app.open_selected();

        assert_eq!(app.cwd, PathBuf::from("/root/big"));
        assert_eq!(app.selection, 0);
        assert_eq!(app.rows().len(), 1);
    }

    #[test]
    fn going_up_lands_back_on_the_directory_just_left() {
        let mut app = app_with_tree();
        app.cwd = PathBuf::from("/root/small");
        app.go_up();

        assert_eq!(app.cwd, PathBuf::from("/root"));
        assert_eq!(app.selected_row().unwrap().name, "small");
    }

    #[test]
    fn the_scan_root_is_the_floor() {
        let mut app = app_with_tree();
        app.go_up();
        assert_eq!(app.cwd, PathBuf::from("/root"));
        assert!(app.status.is_some());
    }

    #[test]
    fn selection_stays_inside_the_list() {
        let mut app = app_with_tree();
        app.move_selection(-5);
        assert_eq!(app.selection, 0);
        app.move_selection(99);
        assert_eq!(app.selection, 1);
    }

    #[test]
    fn marks_toggle_and_total_up() {
        let mut app = app_with_tree();
        app.toggle_mark();
        assert_eq!(app.marked_total(), 700);
        app.toggle_mark();
        assert_eq!(app.marked_total(), 0);
    }

    #[test]
    fn searching_filters_the_rows() {
        let mut app = app_with_tree();
        app.begin_search();
        for ch in "sma".chars() {
            app.push_search(ch);
        }
        assert_eq!(app.rows().len(), 1);
        assert_eq!(app.rows()[0].name, "small");

        app.clear_search();
        assert_eq!(app.rows().len(), 2);
    }

    #[test]
    fn the_breadcrumb_follows_the_current_directory() {
        let mut app = app_with_tree();
        assert_eq!(app.breadcrumb(), "/root");
        app.cwd = PathBuf::from("/root/big/inner");
        assert_eq!(app.breadcrumb(), "/root › big › inner");
    }

    #[test]
    fn the_radial_tree_ids_map_back_to_paths() {
        let app = app_with_tree();
        let (node, ids) = app.radial_tree(2);

        assert_eq!(node.size, 1000);
        assert_eq!(ids[0], PathBuf::from("/root"));
        // Largest first, depth first.
        assert_eq!(ids[1], PathBuf::from("/root/big"));
        assert_eq!(ids[2], PathBuf::from("/root/big/inner"));
        assert_eq!(ids[3], PathBuf::from("/root/small"));
        assert_eq!(node.children[0].children[0].id, 2);
    }

    #[test]
    fn the_radial_tree_stops_at_the_ring_limit() {
        let app = app_with_tree();
        let (node, _) = app.radial_tree(1);
        assert!(node.children[0].children.is_empty());
    }

    #[test]
    fn opening_a_file_explains_itself_instead_of_navigating() {
        let mut app = app_with_tree();
        if let Some(tree) = &mut app.tree {
            tree.children[1].entry_type = EntryType::File;
        }
        app.selection = 1;
        app.open_selected();

        assert_eq!(app.cwd, PathBuf::from("/root"));
        assert!(app.status.unwrap().contains("is a file"));
    }

    #[test]
    fn switching_size_kind_changes_what_rows_report() {
        let mut app = app_with_tree();
        if let Some(tree) = &mut app.tree {
            tree.children[0].apparent_size = 10;
        }
        assert_eq!(app.rows()[0].size, 700);
        app.toggle_size_kind();
        assert_eq!(app.settings.size_kind, SizeKind::Apparent);
        // Now "small" is the biggest by apparent size.
        assert_eq!(app.rows()[0].name, "small");
    }
}
