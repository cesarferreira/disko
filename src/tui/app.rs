//! Interactive state: what is on screen, what is selected, and what a key does.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::Result;
use disko_core::scan::{self, Cancel, Progress};
use disko_core::{DiskEntry, Filesystem, ScanOptions, SizeKind, Unit};
use disko_render::RadialNode;

use crate::model::{self, Row, RowOptions, Sort};

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
}

/// A scan running on another thread.
struct Session {
    progress: Arc<Progress>,
    cancel: Cancel,
    handle: Option<JoinHandle<Result<DiskEntry>>>,
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
    session: Option<Session>,
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
            session: None,
        }
    }

    /// Start straight in a scan instead of the disk picker.
    pub fn with_path(mut self, path: &Path) -> Self {
        self.start_scan(path);
        self
    }

    pub fn start_scan(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.filesystem = disko_core::mounts::for_path(&path);
        self.root = path.clone();
        self.cwd = path.clone();
        self.selection = 0;
        self.search = None;
        self.search_active = false;
        self.tree = None;
        self.status = None;
        self.view = View::Scanning;

        let (progress, cancel, handle) =
            scan::scan_in_background(path, self.settings.scan_options.clone());
        self.session = Some(Session {
            progress,
            cancel,
            handle: Some(handle),
        });
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
                self.tree = Some(tree);
                self.view = View::Overview;
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
        children,
    }
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
