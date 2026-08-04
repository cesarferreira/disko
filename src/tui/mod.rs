//! Terminal setup, the event loop, and the key map.

pub mod app;
mod confirm;
mod details;
mod explorer;
mod footer;
mod overview;
mod picker;
mod scanning;
mod text;
mod theme;

use std::io::{Stdout, stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::ListState;
use ratatui::{Frame, Terminal};

use app::{App, View};

/// How long a frame waits for input before redrawing anyway. Fast enough that
/// the scan progress looks live, slow enough to stay idle when nothing moves.
const FRAME: Duration = Duration::from_millis(120);

/// Widget state that belongs to the screen rather than the app.
#[derive(Default)]
struct Ui {
    list: ListState,
    picker: ListState,
}

pub fn run(mut app: App) -> Result<()> {
    let mut terminal = enter()?;
    let result = event_loop(&mut terminal, &mut app);
    leave(&mut terminal)?;
    result
}

fn enter() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    // Without this a panic leaves the user in raw mode on the alternate
    // screen, with no echo and no prompt.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        previous(info);
    }));

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn leave(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    let mut ui = Ui::default();
    let mut tick = 0usize;

    loop {
        app.poll_scan();
        app.poll_delete();
        app.tick_watch();
        app.advance();
        terminal.draw(|frame| draw(frame, app, &mut ui, tick))?;

        if event::poll(FRAME)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => handle_key(app, key),
                _ => {}
            }
        }

        tick = tick.wrapping_add(1);
        if app.quit {
            app.cancel_scan();
            return Ok(());
        }
    }
}

fn draw(frame: &mut Frame, app: &mut App, ui: &mut Ui, tick: usize) {
    let area = frame.area();
    match app.view {
        View::Picker => picker::draw(frame, area, app, &mut ui.picker),
        View::Scanning => scanning::draw(frame, area, app, tick),
        View::Overview => overview::draw(frame, area, app, &mut ui.list),
        View::Explorer => explorer::draw(frame, area, app),
    }
    if app.details {
        details::draw(frame, area, app);
    }
    // The confirmation sits above everything, including the details panel.
    if let Some(pending) = app.confirm.clone() {
        confirm::draw(frame, area, app, &pending);
    }
    if let Some(deleting) = &app.deleting {
        confirm::draw_progress(frame, area, app, deleting);
    }
}

/// Draw one frame into an off-screen buffer of the given size.
///
/// Every screen is laid out by hand against the terminal width, so the layout
/// is worth testing at real sizes rather than only at whatever the developer's
/// window happens to be.
pub fn render(app: &mut App, width: u16, height: u16) -> Result<ratatui::buffer::Buffer> {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, height))?;
    let mut ui = Ui::default();
    terminal.draw(|frame| draw(frame, app, &mut ui, 0))?;
    Ok(terminal.backend().buffer().clone())
}

/// The rendered frame as plain lines, trailing spaces trimmed.
pub fn render_lines(app: &mut App, width: u16, height: u16) -> Result<Vec<String>> {
    let buffer = render(app, width, height)?;
    Ok((0..height)
        .map(|y| {
            let line: String = (0..width)
                .map(|x| {
                    buffer
                        .cell((x, y))
                        .map(|cell| cell.symbol())
                        .unwrap_or(" ")
                        .to_string()
                })
                .collect();
            line.trim_end().to_string()
        })
        .collect())
}

/// A key press applied to an app, for tests and scripted walkthroughs.
pub fn press(app: &mut App, code: KeyCode) {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

/// Block until any background deletion has finished and been folded back in.
///
/// The event loop polls for this between frames; tests and scripted runs need
/// somewhere to wait instead.
pub fn settle(app: &mut App) {
    while app.deleting.is_some() {
        app.poll_delete();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        app.quit = true;
        return;
    }

    // A deletion in flight takes only "stop"; nothing else should reach the
    // tree while it is being changed underneath.
    if app.deleting.is_some() {
        if matches!(key.code, KeyCode::Esc) {
            app.stop_delete();
        }
        return;
    }

    // A pending deletion owns the keyboard until it is resolved one way or
    // the other — no navigation key should reach the list behind it.
    if app.confirm.is_some() {
        handle_confirm(app, key);
        return;
    }

    if app.search_active {
        handle_search(app, key);
        return;
    }

    // Any deliberate keystroke means the last message has been read.
    app.status = None;
    app.outcomes.clear();

    match app.view {
        View::Picker => handle_picker(app, key),
        View::Scanning => handle_scanning(app, key),
        View::Overview | View::Explorer => handle_browsing(app, key),
    }
}

fn handle_confirm(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.cancel_delete(),
        KeyCode::Enter => app.commit_delete(),
        KeyCode::Backspace => app.untype_confirmation(),
        KeyCode::Char(ch) => app.type_confirmation(ch),
        _ => {}
    }
}

fn handle_search(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.clear_search(),
        // Enter keeps the filter but hands the keyboard back to navigation.
        KeyCode::Enter => app.search_active = false,
        KeyCode::Backspace => app.pop_search(),
        KeyCode::Char(ch) => app.push_search(ch),
        _ => {}
    }
}

fn handle_picker(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
        KeyCode::Down | KeyCode::Char('j') => app.move_picker(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_picker(-1),
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => app.pick_selected_filesystem(),
        _ => {}
    }
}

fn handle_scanning(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Esc => {
            app.cancel_scan();
            app.status = Some("stopping — showing what was counted so far".into());
        }
        _ => {}
    }
}

fn handle_browsing(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') => app.quit = true,

        KeyCode::Esc => {
            if app.details {
                app.details = false;
            } else if app.search.is_some() {
                app.clear_search();
            } else if app.view == View::Explorer {
                app.view = View::Overview;
            } else if app.from_picker {
                // Nothing left to close: step back out to the disk list.
                app.return_to_picker();
            }
        }

        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
        KeyCode::PageDown => app.move_selection(10),
        KeyCode::PageUp => app.move_selection(-10),
        KeyCode::Home | KeyCode::Char('g') => app.select_first(),
        KeyCode::End | KeyCode::Char('G') => app.select_last(),

        // From the overview, Enter switches to the radial explorer; once
        // there, Enter drills into whatever is selected.
        KeyCode::Enter => match app.view {
            View::Overview => app.view = View::Explorer,
            _ => app.open_selected(),
        },
        KeyCode::Tab => {
            app.view = if app.view == View::Overview {
                View::Explorer
            } else {
                View::Overview
            }
        }
        KeyCode::Right | KeyCode::Char('l') => app.open_selected(),
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => app.go_up(),

        KeyCode::Char('/') => app.begin_search(),
        KeyCode::Char('s') => app.cycle_sort(),
        KeyCode::Char('d') => app.details = !app.details,
        KeyCode::Char('a') => app.toggle_size_kind(),
        KeyCode::Char('t') => app.toggle_metric(),
        KeyCode::Char(' ') => app.toggle_mark(),
        KeyCode::Char('x') | KeyCode::Delete => app.request_delete(),
        KeyCode::Char('r') => app.rescan(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Settings;
    use disko_core::{DiskEntry, EntryType, ScanOptions, SizeKind, Unit};
    use std::path::PathBuf;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn test_app() -> App {
        let mut root = DiskEntry::new(PathBuf::from("/root"), EntryType::Directory);
        root.allocated_size = 300;
        for (name, size) in [("big", 200u64), ("small", 100u64)] {
            let mut child =
                DiskEntry::new(PathBuf::from(format!("/root/{name}")), EntryType::Directory);
            child.allocated_size = size;
            root.children.push(child);
        }

        let mut app = App::new(
            Settings {
                size_kind: SizeKind::Allocated,
                unit: Unit::Decimal,
                top: 20,
                scan_options: ScanOptions::default(),
                show_all_filesystems: false,
                record_snapshots: false,
            },
            false,
        );
        app.root = PathBuf::from("/root");
        app.cwd = PathBuf::from("/root");
        app.tree = Some(root);
        app.view = View::Overview;
        app
    }

    #[test]
    fn enter_opens_the_explorer_then_drills_down() {
        let mut app = test_app();
        handle_key(&mut app, press(KeyCode::Enter));
        assert_eq!(app.view, View::Explorer);

        handle_key(&mut app, press(KeyCode::Enter));
        assert_eq!(app.cwd, PathBuf::from("/root/big"));
    }

    #[test]
    fn escape_unwinds_one_layer_at_a_time() {
        let mut app = test_app();
        app.view = View::Explorer;
        app.details = true;
        app.begin_search();
        app.search_active = false;

        handle_key(&mut app, press(KeyCode::Esc));
        assert!(!app.details);
        handle_key(&mut app, press(KeyCode::Esc));
        assert!(app.search.is_none());
        handle_key(&mut app, press(KeyCode::Esc));
        assert_eq!(app.view, View::Overview);
        // Escape at the top does not quit by accident.
        handle_key(&mut app, press(KeyCode::Esc));
        assert!(!app.quit);
    }

    #[test]
    fn typing_a_search_does_not_trigger_navigation_shortcuts() {
        let mut app = test_app();
        handle_key(&mut app, press(KeyCode::Char('/')));
        for ch in "sad".chars() {
            handle_key(&mut app, press(KeyCode::Char(ch)));
        }

        assert_eq!(app.search.as_deref(), Some("sad"));
        assert!(
            !app.details,
            "'d' should have been typed, not toggled details"
        );
        assert_eq!(
            app.sort,
            crate::model::Sort::Size,
            "'s' should not have re-sorted"
        );
    }

    #[test]
    fn enter_leaves_the_search_filter_in_place() {
        let mut app = test_app();
        handle_key(&mut app, press(KeyCode::Char('/')));
        handle_key(&mut app, press(KeyCode::Char('s')));
        handle_key(&mut app, press(KeyCode::Enter));

        assert!(!app.search_active);
        assert_eq!(app.search.as_deref(), Some("s"));
        assert_eq!(app.rows().len(), 1);
    }

    #[test]
    fn ctrl_c_always_quits() {
        let mut app = test_app();
        app.search_active = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.quit);
    }

    #[test]
    fn navigation_keys_move_the_selection_and_the_directory() {
        let mut app = test_app();
        handle_key(&mut app, press(KeyCode::Down));
        assert_eq!(app.selection, 1);
        handle_key(&mut app, press(KeyCode::Right));
        assert_eq!(app.cwd, PathBuf::from("/root/small"));
        handle_key(&mut app, press(KeyCode::Backspace));
        assert_eq!(app.cwd, PathBuf::from("/root"));
    }

    /// The reported bug: after picking a disk, "up" from the top of the tree
    /// left you stranded with no way back to the list you came from.
    #[test]
    fn going_up_from_the_top_returns_to_the_disk_list() {
        let mut app = test_app();
        app.from_picker = true;
        app.cwd = PathBuf::from("/root/big");

        // First press climbs out of the subdirectory...
        handle_key(&mut app, press(KeyCode::Left));
        assert_eq!(app.cwd, PathBuf::from("/root"));
        assert_eq!(app.view, View::Overview);

        // ...the second leaves the scan entirely.
        handle_key(&mut app, press(KeyCode::Left));
        assert_eq!(app.view, View::Picker);
        assert!(app.tree.is_none(), "the finished scan should be released");
        assert!(!app.quit);
    }

    #[test]
    fn backspace_and_escape_leave_the_scan_the_same_way() {
        for key in [KeyCode::Backspace, KeyCode::Esc, KeyCode::Char('h')] {
            let mut app = test_app();
            app.from_picker = true;
            handle_key(&mut app, press(key));
            assert_eq!(app.view, View::Picker, "{key:?} should step back out");
        }
    }

    /// A path named on the command line has no list behind it, so leaving
    /// would mean landing nowhere.
    #[test]
    fn a_scan_started_from_the_command_line_stays_put() {
        let mut app = test_app();
        app.from_picker = false;

        handle_key(&mut app, press(KeyCode::Left));

        assert_eq!(app.view, View::Overview);
        assert!(app.tree.is_some());
        let status = app.status.unwrap();
        assert!(status.contains("top of this scan"), "{status}");
    }

    #[test]
    fn leaving_a_scan_forgets_what_belonged_to_it() {
        let mut app = test_app();
        app.from_picker = true;
        app.toggle_mark();
        app.begin_search();
        app.push_search('b');
        assert!(!app.marks.is_empty());

        app.return_to_picker();

        assert!(app.marks.is_empty(), "marks point into the abandoned scan");
        assert!(app.search.is_none());
        assert_eq!(app.selection, 0);
        assert!(app.root.as_os_str().is_empty());
    }

    #[test]
    fn escape_during_a_scan_stops_it_rather_than_quitting() {
        let mut app = test_app();
        app.view = View::Scanning;
        handle_key(&mut app, press(KeyCode::Esc));
        assert!(!app.quit);
        assert!(app.status.is_some());
    }
}
