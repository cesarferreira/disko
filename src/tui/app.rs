//! Interactive state: what is on screen, what is selected, and what a key does.

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use disko_core::scan::{self, Cancel, Finished, Progress};
use disko_core::{Diff, DiskEntry, Filesystem, ScanOptions, SizeKind, Unit};
use disko_render::{RadialNode, palette};

use crate::deletion::{self, DeleteProgress, Outcome, Target};
use crate::model::{self, Metric, Row, RowOptions, Sort};

/// The word that has to be typed before anything is removed.
pub const CONFIRM_WORD: &str = "delete";

/// How far down a running scan publishes finished directories. Two levels is
/// enough that a scan of `/` shows `~/Library` long before `/` itself is done.
const STREAM_DEPTH: usize = 2;

/// How many finished directories to keep on the progress screen.
const STREAM_KEEP: usize = 12;

/// How much of the previous tree a rescan keeps on screen while it runs. The
/// sunburst draws three rings and the list draws one level, so five is already
/// more than anything on screen can reach.
const REFRESH_KEEP_DEPTH: usize = 5;

/// A pending deletion, waiting to be confirmed or called off.
#[derive(Clone, Debug, Default)]
pub struct Confirm {
    pub targets: Vec<Target>,
    /// What the user has typed toward [`CONFIRM_WORD`].
    pub typed: String,
    /// Set when Enter was pressed before the word was complete.
    pub nagged: bool,
}

impl Confirm {
    pub fn total(&self) -> u64 {
        self.targets.iter().map(|target| target.size).sum()
    }

    pub fn is_armed(&self) -> bool {
        self.typed == CONFIRM_WORD
    }
}

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

/// A deletion running on another thread, so the interface stays alive while
/// tens of thousands of files are being unlinked.
pub struct Deleting {
    pub progress: Arc<DeleteProgress>,
    cancel: Cancel,
    handle: Option<JoinHandle<Vec<Outcome>>>,
    stopping: bool,
}

impl Deleting {
    pub fn is_stopping(&self) -> bool {
        self.stopping
    }
}

/// The sunburst for one directory: the tree the layout runs over, and the
/// table taking a path back to the wedge id standing for it.
pub struct RadialCache {
    key: RadialKey,
    pub root: RadialNode,
    ids: HashMap<PathBuf, usize>,
}

impl RadialCache {
    /// The wedge drawn for `path`, if it is one of the ones drawn at all.
    pub fn id_of(&self, path: &Path) -> Option<usize> {
        self.ids.get(path).copied()
    }
}

/// Everything the sunburst is built out of. While none of it has moved, the
/// cached sunburst is still the right one.
#[derive(Clone, PartialEq, Eq)]
struct RadialKey {
    cwd: PathBuf,
    rings: usize,
    size_kind: SizeKind,
    metric: Metric,
    /// Bumped whenever the scanned tree itself changes under the view.
    revision: u64,
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
    /// True when the disk picker is where this scan came from, so "up" from
    /// the top of the tree has somewhere to go back to.
    pub from_picker: bool,
    /// A deletion waiting on confirmation.
    pub confirm: Option<Confirm>,
    /// What the last deletion did, shown until the next keystroke.
    pub outcomes: Vec<Outcome>,
    /// When set, disko will not delete anything at all.
    pub read_only: bool,
    /// When the numbers on screen came from a stored snapshot rather than the
    /// scan that is still running: the moment that snapshot was taken.
    pub provisional: Option<u64>,
    /// Directories the running scan has finished counting, largest first.
    pub streamed: Vec<Finished>,
    /// A deletion in flight.
    pub deleting: Option<Deleting>,
    tick: usize,
    session: Option<Session>,
    mode: ScanMode,
    /// How many times the tree behind the view has been replaced or edited.
    revision: u64,
    /// The last sunburst built, kept for as long as it stands.
    radial: RefCell<Option<RadialCache>>,
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
            from_picker: false,
            confirm: None,
            outcomes: Vec::new(),
            read_only: false,
            provisional: None,
            streamed: Vec::new(),
            deleting: None,
            tick: 0,
            session: None,
            mode: ScanMode::Fresh,
            revision: 0,
            radial: RefCell::new(None),
        }
    }

    /// Rescan on a timer and report growth since the moment watching started.
    pub fn watching(mut self, watch: Watch) -> Self {
        self.watch = Some(watch);
        self.metric = Metric::Growth;
        self
    }

    /// Start straight in a scan instead of the disk picker. There is no list
    /// to go back to in this case — the user named the place themselves.
    pub fn with_path(mut self, path: &Path) -> Self {
        self.from_picker = false;
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
        self.touch();

        if mode == ScanMode::Fresh {
            self.filesystem = disko_core::mounts::for_path(&path);
            self.root = path.clone();
            self.cwd = path.clone();
            self.selection = 0;
            self.search = None;
            self.search_active = false;
            self.tree = None;
            self.status = None;
            self.streamed.clear();
            self.provisional = None;
            self.view = View::Scanning;

            // Paint what disko knew last time straight away, clearly labelled,
            // and let the scan correct it. Waiting on a spinner for numbers we
            // already have is a second nobody needs to spend.
            if self.settings.record_snapshots
                && let Some(snapshot) = crate::commands::last_snapshot(&path)
            {
                self.tree = Some(snapshot.tree);
                self.provisional = Some(snapshot.taken_at);
                self.view = View::Overview;
            }
        } else if let Some(previous) = self.tree.take() {
            // A rescan builds a whole second tree before it can replace this
            // one, so holding on to it intact means every entry is in memory
            // twice for the length of the scan — a gigabyte, on a home
            // directory with two million things under it. Keep the part the
            // screen can actually reach and let go of the rest; `absorb_scan`
            // puts the full tree back in a moment.
            self.tree = Some(visible_stub(&previous, &self.cwd, REFRESH_KEEP_DEPTH));
        }

        let (progress, cancel, handle) = scan::scan_in_background_streaming(
            path,
            self.settings.scan_options.clone(),
            STREAM_DEPTH,
        );
        self.session = Some(Session {
            progress,
            cancel,
            handle: Some(handle),
            started: Instant::now(),
        });
    }

    /// Advance the animation clock. Called once per frame.
    pub fn advance(&mut self) {
        self.tick = self.tick.wrapping_add(1);
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
        self.collect_streamed();

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

    /// Absorb whatever the running scan has finished counting since the last
    /// frame, keeping the largest first.
    fn collect_streamed(&mut self) {
        let Some(session) = &self.session else { return };
        let fresh = session.progress.drain_completed();
        if fresh.is_empty() {
            return;
        }

        self.streamed.extend(fresh);
        // A parent supersedes children it already contains, so the list stays
        // a set of non-overlapping totals rather than double-counting.
        self.streamed
            .sort_by_key(|done| std::cmp::Reverse(done.allocated));
        let mut kept: Vec<Finished> = Vec::with_capacity(self.streamed.len());
        for done in std::mem::take(&mut self.streamed) {
            if !kept.iter().any(|other| done.path.starts_with(&other.path)) {
                kept.push(done);
            }
        }
        kept.truncate(STREAM_KEEP);
        self.streamed = kept;
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
        self.touch();
        // Whatever is on screen is now the real thing.
        self.provisional = None;
        self.streamed.clear();
        if self.mode == ScanMode::Fresh {
            self.view = View::Overview;
        }
        // A snapshot's tree is pruned, so the cursor may have been sitting on
        // a row that the full scan renders differently.
        let rows = self.rows().len();
        self.selection = self.selection.min(rows.saturating_sub(1));
    }

    /// Frame counter, so overlays can animate without threading it through
    /// every draw call.
    pub fn tick(&self) -> usize {
        self.tick
    }

    /// True while the screen is showing last-known numbers.
    pub fn is_provisional(&self) -> bool {
        self.provisional.is_some() && self.session.is_some()
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

    /// Back up one level. At the top of the tree that means back to the disk
    /// list, when that is where this scan came from.
    pub fn go_up(&mut self) {
        if self.cwd == self.root {
            if self.from_picker {
                self.return_to_picker();
            } else {
                self.status = Some(format!(
                    "top of this scan — run disko on {} to go higher",
                    self.root
                        .parent()
                        .map(model::display_path)
                        .unwrap_or_else(|| "the parent".into())
                ));
            }
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

    /// Ask to delete: everything marked, or whatever is selected if nothing
    /// is marked.
    pub fn request_delete(&mut self) {
        if self.read_only {
            self.status = Some("disko is running read-only".into());
            return;
        }
        let Some(tree) = &self.tree else { return };

        let paths: Vec<PathBuf> = if self.marks.is_empty() {
            self.selected_row()
                .and_then(|row| row.path)
                .into_iter()
                .collect()
        } else {
            let mut marked: Vec<PathBuf> = self.marks.iter().cloned().collect();
            marked.sort();
            marked
        };

        let targets: Vec<Target> = paths
            .into_iter()
            .filter_map(|path| {
                let entry = tree.resolve(&path)?;
                Some(Target {
                    size: entry.size(self.settings.size_kind),
                    is_dir: entry.is_dir(),
                    path,
                })
            })
            .collect();

        if targets.is_empty() {
            self.status = Some("nothing selected to delete".into());
            return;
        }

        self.confirm = Some(Confirm {
            targets,
            ..Default::default()
        });
    }

    pub fn cancel_delete(&mut self) {
        self.confirm = None;
        self.status = Some("nothing was deleted".into());
    }

    pub fn type_confirmation(&mut self, ch: char) {
        if let Some(confirm) = &mut self.confirm {
            confirm.typed.push(ch);
            confirm.nagged = false;
        }
    }

    pub fn untype_confirmation(&mut self) {
        if let Some(confirm) = &mut self.confirm {
            confirm.typed.pop();
            confirm.nagged = false;
        }
    }

    /// Go through with it, if the word has been typed in full.
    ///
    /// The work happens on another thread: unlinking a few hundred thousand
    /// files takes long enough that doing it here would freeze the screen with
    /// no way to tell whether anything was happening.
    pub fn commit_delete(&mut self) {
        let Some(confirm) = &mut self.confirm else {
            return;
        };
        if !confirm.is_armed() {
            confirm.nagged = true;
            return;
        }

        let targets = std::mem::take(&mut confirm.targets);
        self.confirm = None;
        self.outcomes.clear();

        let (progress, cancel, handle) = deletion::delete_in_background(targets, self.root.clone());
        self.deleting = Some(Deleting {
            progress,
            cancel,
            handle: Some(handle),
            stopping: false,
        });
    }

    /// Ask the running deletion to stop once it finishes the file it is on.
    pub fn stop_delete(&mut self) {
        if let Some(deleting) = &mut self.deleting {
            deleting.cancel.cancel();
            deleting.stopping = true;
        }
    }

    /// Collect a finished deletion and fold the result back into the tree.
    pub fn poll_delete(&mut self) {
        let Some(deleting) = &mut self.deleting else {
            return;
        };
        let done = deleting
            .handle
            .as_ref()
            .is_some_and(|handle| handle.is_finished());
        if !done {
            return;
        }

        let Some(handle) = deleting.handle.take() else {
            return;
        };
        let outcomes = handle.join().unwrap_or_default();
        self.deleting = None;
        self.apply_outcomes(outcomes);
    }

    fn apply_outcomes(&mut self, outcomes: Vec<Outcome>) {
        // Correct the totals in place rather than paying for a rescan.
        if let Some(tree) = &mut self.tree {
            for outcome in &outcomes {
                if let Outcome::Deleted { path, .. } = outcome {
                    tree.remove(path);
                }
            }
            self.touch();
        }

        let freed = deletion::freed(&outcomes);
        let deleted = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Outcome::Deleted { .. }))
            .count();
        let refused = outcomes.len() - deleted;

        for outcome in &outcomes {
            if let Outcome::Deleted { path, .. } = outcome {
                self.marks.remove(path);
            }
        }
        self.outcomes = outcomes;

        self.status = Some(match (deleted, refused) {
            (0, _) => "nothing could be deleted — press d for why".to_string(),
            (n, 0) => format!(
                "deleted {n} · freed {}",
                disko_core::size::format(freed, self.settings.unit)
            ),
            (n, skipped) => format!(
                "deleted {n} · freed {} · {skipped} skipped",
                disko_core::size::format(freed, self.settings.unit)
            ),
        });

        // The row under the cursor may have just been removed.
        let rows = self.rows().len();
        self.selection = self.selection.min(rows.saturating_sub(1));
    }

    /// Drop everything belonging to the current scan and show the disks again.
    pub fn return_to_picker(&mut self) {
        self.cancel_scan();
        self.session = None;
        self.view = View::Picker;
        self.tree = None;
        self.diff = None;
        self.touch();
        self.metric = Metric::Size;
        self.root = PathBuf::new();
        self.cwd = PathBuf::new();
        self.selection = 0;
        self.search = None;
        self.search_active = false;
        // Marks are paths inside the scan that is being left behind.
        self.marks.clear();
        self.status = None;
        // Capacities move while you are busy looking at one disk.
        self.filesystems = disko_core::mounts::list(self.settings.show_all_filesystems);
    }

    /// Show the selected entry in the desktop's file manager.
    pub fn reveal_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let Some(path) = row.path else {
            self.status = Some("\"Other\" is a group, not a folder".into());
            return;
        };

        match crate::reveal::reveal(&path, row.is_dir) {
            Ok(()) => {
                let shown = crate::reveal::folder_for(&path, row.is_dir).unwrap_or(path);
                self.status = Some(format!(
                    "opened {} in {}",
                    model::display_path(&shown),
                    crate::reveal::manager_name()
                ));
            }
            Err(error) => self.status = Some(error),
        }
    }

    /// Put the path under the cursor on the clipboard — the way out of disko
    /// and into whatever you actually meant to do with the folder you found.
    pub fn copy_selected_path(&mut self) {
        // A group row stands for no single path, and an empty directory has no
        // row at all; the directory being looked at is the honest answer to
        // both.
        let path = self
            .selected_row()
            .and_then(|row| row.path)
            .unwrap_or_else(|| self.cwd.clone());

        // The clipboard gets the real path, the message gets the short one.
        self.status = Some(match crate::clipboard::copy(&path.to_string_lossy()) {
            Ok(()) => format!("copied {}", model::display_path(&path)),
            Err(error) => error,
        });
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

    /// The sunburst for the current directory, plus the table taking a path
    /// back to the wedge that stands for it.
    ///
    /// Building it walks `rings` levels of the tree and clones a path per node,
    /// which is far too much to redo for every keystroke — so the last one is
    /// kept until something it was built from moves.
    pub fn radial_tree(&self, rings: usize) -> Ref<'_, RadialCache> {
        let key = RadialKey {
            cwd: self.cwd.clone(),
            rings,
            size_kind: self.settings.size_kind,
            metric: self.metric,
            revision: self.revision,
        };

        let stale = match &*self.radial.borrow() {
            Some(cache) => cache.key != key,
            None => true,
        };
        if stale {
            *self.radial.borrow_mut() = Some(self.build_radial(key));
        }

        Ref::map(self.radial.borrow(), |cache| {
            cache.as_ref().expect("the cache was just filled")
        })
    }

    fn build_radial(&self, key: RadialKey) -> RadialCache {
        let mut ids = Vec::new();

        let root = if self.showing_growth() {
            match self.current_change() {
                Some(change) => {
                    let scale = biggest_change(change);
                    build_growth_radial(change, key.rings, scale, &mut ids)
                }
                None => RadialNode::leaf(0, "", 0),
            }
        } else {
            match self.current_entry() {
                Some(entry) => build_radial(entry, key.size_kind, key.rings, &mut ids),
                None => RadialNode::leaf(0, "", 0),
            }
        };

        RadialCache {
            key,
            root,
            ids: ids
                .into_iter()
                .enumerate()
                .map(|(id, path)| (path, id))
                .collect(),
        }
    }

    /// Note that the tree behind the view has changed, so anything derived
    /// from it has to be built again.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn pick_selected_filesystem(&mut self) {
        let Some(fs) = self.filesystems.get(self.picker_index) else {
            return;
        };
        let mount = fs.mount_point.clone();
        self.from_picker = true;
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

/// The part of `entry` the screen can still reach with the cursor at `cwd`:
/// `below` levels under the cursor, and enough of the way back up that leaving
/// the directory lists what it listed before.
///
/// Everything else is dropped. The numbers that survive are the same numbers
/// the last scan produced — this trades depth the view is not showing for the
/// memory a second full tree would need.
fn visible_stub(entry: &DiskEntry, cwd: &Path, below: usize) -> DiskEntry {
    let mut stub = entry.without_children();

    if entry.path.starts_with(cwd) {
        // At or under the cursor: a few levels is everything that is drawn.
        if below > 0 {
            stub.children = entry
                .children
                .iter()
                .map(|child| visible_stub(child, cwd, below - 1))
                .collect();
        }
    } else if cwd.starts_with(&entry.path) {
        // Above the cursor: keep every child, so going back up still lists the
        // same rows, but only follow the one that leads back down to it.
        stub.children = entry
            .children
            .iter()
            .map(|child| {
                if cwd.starts_with(&child.path) {
                    visible_stub(child, cwd, below)
                } else {
                    child.without_children()
                }
            })
            .collect();
    }

    stub
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

    /// `/root` -> `a` -> `deep` -> `deeper`, with a sibling at every level, so
    /// a stub can be checked for both what it keeps and what it lets go.
    fn deep_tree() -> DiskEntry {
        entry(
            "/root",
            1000,
            vec![
                entry(
                    "/root/a",
                    700,
                    vec![
                        entry(
                            "/root/a/deep",
                            600,
                            vec![entry(
                                "/root/a/deep/deeper",
                                500,
                                vec![entry("/root/a/deep/deeper/deepest", 400, vec![])],
                            )],
                        ),
                        entry("/root/a/sibling", 100, vec![entry("/root/a/sibling/x", 1, vec![])]),
                    ],
                ),
                entry("/root/b", 300, vec![entry("/root/b/hidden", 50, vec![])]),
            ],
        )
    }

    #[test]
    fn a_stub_keeps_the_levels_under_the_cursor() {
        let stub = visible_stub(&deep_tree(), Path::new("/root/a"), 2);
        let a = stub.child("a").unwrap();

        // Two levels under the cursor survive with their sizes intact...
        assert_eq!(a.allocated_size, 700);
        let deeper = a.child("deep").unwrap().child("deeper").unwrap();
        assert_eq!(deeper.allocated_size, 500);
        // ...and the third does not.
        assert!(deeper.children.is_empty(), "the stub stops at the depth asked for");
    }

    #[test]
    fn a_stub_keeps_enough_to_go_back_up() {
        let stub = visible_stub(&deep_tree(), Path::new("/root/a"), 2);

        // Leaving `/root/a` must still list the same rows it listed before,
        // with the same numbers, even though nothing under them is kept.
        let b = stub.child("b").unwrap();
        assert_eq!(b.allocated_size, 300);
        assert!(b.children.is_empty(), "a branch off the path is not followed");
    }

    #[test]
    fn a_stub_at_the_root_is_bounded_by_depth() {
        let stub = visible_stub(&deep_tree(), Path::new("/root"), 1);

        assert_eq!(stub.allocated_size, 1000);
        assert_eq!(stub.children.len(), 2);
        // One level down, and no further.
        assert!(stub.children.iter().all(|child| child.children.is_empty()));
    }

    #[test]
    fn rescanning_drops_the_old_tree_down_to_the_stub() {
        let mut app = app_with_tree();
        app.cwd = PathBuf::from("/root");
        app.begin(&PathBuf::from("/root"), ScanMode::Refresh);

        // The rescan is under way and the view still has its numbers, but the
        // full tree is no longer being held alongside the one being built.
        let tree = app.tree.as_ref().expect("the view keeps something to draw");
        assert_eq!(tree.allocated_size, 1000);
        assert_eq!(app.rows().len(), 2);
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
        let cache = app.radial_tree(2);

        assert_eq!(cache.root.size, 1000);
        // Largest first, depth first.
        for (id, path) in [
            (0, "/root"),
            (1, "/root/big"),
            (2, "/root/big/inner"),
            (3, "/root/small"),
        ] {
            assert_eq!(cache.id_of(Path::new(path)), Some(id), "{path}");
        }
        assert_eq!(cache.root.children[0].children[0].id, 2);
    }

    #[test]
    fn the_radial_tree_stops_at_the_ring_limit() {
        let app = app_with_tree();
        let cache = app.radial_tree(1);
        assert!(cache.root.children[0].children.is_empty());
    }

    /// Rebuilding the sunburst for every keystroke is what made holding an
    /// arrow key down drag, so the same directory has to hand back the same
    /// build — and a changed one must not.
    #[test]
    fn the_radial_tree_is_only_rebuilt_when_something_moves() {
        let mut app = app_with_tree();
        let first = app.radial_tree(2).root.size;
        assert_eq!(first, 1000);
        assert_eq!(app.revision, 0, "reading it should change nothing");

        // Moving the cursor leaves the chart alone...
        app.move_selection(1);
        assert_eq!(app.revision, 0);

        // ...but editing the tree does not.
        app.apply_outcomes(vec![Outcome::Deleted {
            path: PathBuf::from("/root/small"),
            size: 300,
        }]);
        assert_eq!(app.revision, 1);
        assert_eq!(app.radial_tree(2).id_of(Path::new("/root/small")), None);
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
