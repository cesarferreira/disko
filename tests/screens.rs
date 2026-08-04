//! Renders every screen at real terminal sizes and checks what a person would
//! actually see. Layout bugs — a column that overflows, a section that eats
//! the list, a percentage that survives on a narrow window — only show up at
//! specific widths, so they are pinned here.

use std::path::PathBuf;

use disko::model::Sort;
use disko::tui::app::{App, Settings, View};
use disko::tui::{press, render_lines};
use disko_core::mounts::Filesystem;
use disko_core::{DiskEntry, EntryType, ScanOptions, SizeKind, Unit};
use ratatui::crossterm::event::KeyCode;

const GB: u64 = 1_000_000_000;

fn dir(path: &str, size: u64, children: Vec<DiskEntry>) -> DiskEntry {
    let mut entry = DiskEntry::new(PathBuf::from(path), EntryType::Directory);
    entry.allocated_size = size;
    entry.apparent_size = size;
    entry.items = 1 + children.iter().map(|c| c.items).sum::<u64>();
    entry.children = children;
    entry
}

/// A tree shaped like the machine in the design sketch.
fn macintosh() -> DiskEntry {
    dir(
        "/",
        404 * GB,
        vec![
            dir(
                "/Users",
                257 * GB,
                vec![dir(
                    "/Users/cesar",
                    255 * GB,
                    vec![
                        dir("/Users/cesar/Library", 71 * GB, vec![]),
                        dir("/Users/cesar/Downloads", 48 * GB, vec![]),
                        dir("/Users/cesar/code", 32 * GB, vec![]),
                        dir("/Users/cesar/Movies", 19 * GB, vec![]),
                    ],
                )],
            ),
            dir("/Library", 84 * GB, vec![]),
            dir("/Applications", 34 * GB, vec![]),
            dir("/System", 12 * GB, vec![]),
            dir("/private", 4 * GB, vec![]),
            dir("/opt", 3 * GB, vec![]),
        ],
    )
}

fn filesystem() -> Filesystem {
    Filesystem {
        name: "Macintosh HD".to_string(),
        mount_point: PathBuf::from("/"),
        device: "/dev/disk3s1s1".to_string(),
        fs_type: "apfs".to_string(),
        total: 494 * GB,
        used: 404 * GB,
        available: 90 * GB,
        read_only: false,
        removable: false,
        kind: "SSD".to_string(),
        pseudo: false,
        inodes: None,
    }
}

fn app() -> App {
    let mut app = App::new(
        Settings {
            size_kind: SizeKind::Allocated,
            unit: Unit::Decimal,
            top: 20,
            scan_options: ScanOptions::default(),
            show_all_filesystems: false,
            // Tests must never touch the user's real snapshot history.
            record_snapshots: false,
        },
        false,
    );
    app.root = PathBuf::from("/");
    app.cwd = PathBuf::from("/");
    app.tree = Some(macintosh());
    app.filesystem = Some(filesystem());
    app.view = View::Overview;
    app
}

fn joined(lines: &[String]) -> String {
    lines.join("\n")
}

#[test]
fn the_overview_answers_the_three_questions() {
    let lines = render_lines(&mut app(), 100, 30).unwrap();
    let screen = joined(&lines);

    // What is full?
    assert!(screen.contains("Macintosh HD"), "{screen}");
    assert!(screen.contains("404 GB used of 494 GB"), "{screen}");
    assert!(screen.contains("82%"), "{screen}");

    // What is using the space?
    assert!(screen.contains("257 GB"));
    assert!(screen.contains("Users"));
    assert!(screen.contains('█'), "expected bars\n{screen}");

    // Where should I look next?
    assert!(screen.contains("Largest items"), "{screen}");
    assert!(screen.contains("/Users/cesar"), "{screen}");

    // And the keys to go on with.
    assert!(screen.contains("Enter explore"));
    assert!(screen.contains("q quit"));
}

#[test]
fn disk_capacity_and_directory_usage_are_never_the_same_line() {
    let lines = render_lines(&mut app(), 100, 30).unwrap();

    let capacity = lines.iter().find(|l| l.contains("used of")).unwrap();
    let folder = lines.iter().find(|l| l.contains("Current folder")).unwrap();

    assert!(capacity.contains("Macintosh HD"));
    assert!(capacity.contains("494 GB"), "capacity is the whole volume");
    assert!(
        folder.contains("404 GB"),
        "the folder line is the scanned tree"
    );
    assert!(!folder.contains("494 GB"));
}

#[test]
fn filesystem_language_stays_out_of_the_default_view() {
    let screen = joined(&render_lines(&mut app(), 100, 30).unwrap());

    for jargon in ["apfs", "/dev/disk3s1s1", "inode", "block"] {
        assert!(
            !screen.to_lowercase().contains(jargon),
            "{jargon} should live behind d/--details, but the default view shows:\n{screen}"
        );
    }
}

#[test]
fn details_puts_the_filesystem_facts_one_key_away() {
    let mut app = app();
    press(&mut app, KeyCode::Char('d'));
    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());

    assert!(screen.contains("/dev/disk3s1s1"), "{screen}");
    assert!(screen.contains("apfs"), "{screen}");
    assert!(screen.contains("Read-only"), "{screen}");
}

#[test]
fn narrow_terminals_drop_the_percentage_before_the_bar() {
    let wide = joined(&render_lines(&mut app(), 100, 30).unwrap());
    let narrow = joined(&render_lines(&mut app(), 60, 30).unwrap());

    assert!(wide.contains("63.6%"), "{wide}");
    assert!(!narrow.contains("63.6%"), "{narrow}");
    assert!(narrow.contains('█'), "the bar survives\n{narrow}");
    assert!(narrow.contains("257 GB"), "so does the size\n{narrow}");
}

#[test]
fn nothing_overflows_the_terminal_width() {
    for width in [40u16, 60, 80, 100, 140] {
        for view in [View::Overview, View::Explorer, View::Picker] {
            let mut app = app();
            app.view = view;
            for line in render_lines(&mut app, width, 30).unwrap() {
                assert!(
                    line.chars().count() <= width as usize,
                    "{view:?} at {width} columns produced a {}-char line: {line}",
                    line.chars().count()
                );
            }
        }
    }
}

#[test]
fn a_short_terminal_keeps_the_ranked_list_and_drops_the_extras() {
    let short = joined(&render_lines(&mut app(), 100, 12).unwrap());

    assert!(short.contains("Users"), "the answer survives\n{short}");
    assert!(short.contains("Macintosh HD"), "{short}");
    assert!(
        !short.contains("Largest items"),
        "the nice-to-have goes first\n{short}"
    );
}

#[test]
fn the_explorer_draws_a_sunburst_with_a_legend() {
    let mut app = app();
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.view, View::Explorer);

    let lines = render_lines(&mut app, 100, 30).unwrap();
    let screen = joined(&lines);

    // Breadcrumb, chart, legend, keys.
    assert!(screen.starts_with(" Macintosh HD"), "{screen}");
    assert!(
        screen.contains('▀') || screen.contains('▄'),
        "no chart drawn\n{screen}"
    );
    assert!(screen.contains("■"), "no legend swatches\n{screen}");
    assert!(screen.contains("Enter open"), "{screen}");
    // The hole in the middle carries the directory total.
    assert!(screen.contains("404 GB"), "{screen}");
}

#[test]
fn drilling_down_moves_the_breadcrumb_and_the_chart() {
    let mut app = app();
    press(&mut app, KeyCode::Enter); // to the explorer
    press(&mut app, KeyCode::Enter); // into Users, the largest

    assert_eq!(app.cwd, PathBuf::from("/Users"));
    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(screen.contains("Macintosh HD › Users"), "{screen}");
    assert!(screen.contains("257 GB"), "{screen}");
}

#[test]
fn the_picker_lists_disks_when_no_path_was_given() {
    let mut app = app();
    app.view = View::Picker;
    app.filesystems = vec![filesystem()];

    let screen = joined(&render_lines(&mut app, 100, 20).unwrap());
    assert!(screen.contains("Disks"), "{screen}");
    assert!(screen.contains("Macintosh HD"), "{screen}");
    assert!(screen.contains("90 GB free of 494 GB"), "{screen}");
    assert!(screen.contains("Enter scan"), "{screen}");
}

#[test]
fn searching_narrows_the_list_live() {
    let mut app = app();
    press(&mut app, KeyCode::Char('/'));
    for ch in "app".chars() {
        press(&mut app, KeyCode::Char(ch));
    }

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(screen.contains("/app"), "the query is echoed\n{screen}");
    assert!(screen.contains("Applications"), "{screen}");
    assert!(!screen.contains("  Library "), "{screen}");
}

#[test]
fn sorting_is_visible_and_reversible() {
    let mut app = app();
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(app.sort, Sort::Name);
    assert!(joined(&render_lines(&mut app, 100, 30).unwrap()).contains("sorted by name"));

    press(&mut app, KeyCode::Char('s'));
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(app.sort, Sort::Size);
}

#[test]
fn marking_shows_how_much_would_be_freed() {
    let mut app = app();
    press(&mut app, KeyCode::Char(' '));

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(screen.contains("1 marked · 257 GB"), "{screen}");
}

#[test]
fn an_empty_directory_says_so_instead_of_showing_a_blank_panel() {
    let mut app = app();
    app.cwd = PathBuf::from("/opt");

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(screen.contains("this folder is empty"), "{screen}");
}

#[test]
fn a_scan_with_unreadable_folders_admits_it() {
    let mut app = app();
    if let Some(tree) = &mut app.tree {
        tree.scan_state = disko_core::ScanState::Partial;
    }

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(
        screen.contains("some folders skipped or unreadable"),
        "{screen}"
    );
}

/// The state of the same machine a week earlier, for the growth view.
fn macintosh_last_week() -> DiskEntry {
    dir(
        "/",
        322 * GB,
        vec![
            dir(
                "/Users",
                211 * GB,
                vec![dir(
                    "/Users/cesar",
                    209 * GB,
                    vec![
                        dir("/Users/cesar/Library", 25 * GB, vec![]),
                        dir("/Users/cesar/Downloads", 48 * GB, vec![]),
                        dir("/Users/cesar/code", 32 * GB, vec![]),
                        dir("/Users/cesar/Movies", 19 * GB, vec![]),
                    ],
                )],
            ),
            dir("/Library", 84 * GB, vec![]),
            dir("/System", 12 * GB, vec![]),
            dir("/opt", 3 * GB, vec![]),
        ],
    )
}

/// An app that knows what changed over the last week.
fn app_with_history() -> App {
    let mut app = app();
    let before = macintosh_last_week();
    let after = macintosh();
    app.diff = Some(disko_core::diff::diff(
        &before,
        &after,
        SizeKind::Allocated,
        1_800_000_000,
        1_800_604_800,
        0,
    ));
    app
}

#[test]
fn pressing_t_without_a_previous_scan_explains_itself() {
    let mut app = app();
    press(&mut app, KeyCode::Char('t'));

    assert!(!app.showing_growth());
    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(screen.contains("no earlier scan"), "{screen}");
}

#[test]
fn pressing_t_switches_the_whole_view_to_what_changed() {
    let mut app = app_with_history();
    press(&mut app, KeyCode::Char('t'));
    assert!(app.showing_growth());

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());

    // Signed changes, not absolute sizes, and the window they cover.
    assert!(screen.contains("+82 GB"), "{screen}");
    assert!(screen.contains("in 7 days"), "{screen}");
    assert!(
        screen.contains("+46 GB"),
        "growth of Users should show\n{screen}"
    );
    // The capacity header still describes the filesystem, unchanged.
    assert!(screen.contains("404 GB used of 494 GB"), "{screen}");
}

#[test]
fn growth_mode_shows_both_directions() {
    let mut app = app_with_history();
    app.cwd = PathBuf::from("/");
    press(&mut app, KeyCode::Char('t'));

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    // Applications is new this week; System did not move and is not listed.
    assert!(screen.contains("Applications"), "{screen}");
    assert!(
        !screen.contains("System"),
        "unchanged entries stay out\n{screen}"
    );
}

#[test]
fn growth_mode_survives_every_terminal_width() {
    for width in [40u16, 60, 80, 100, 140] {
        for view in [View::Overview, View::Explorer] {
            let mut app = app_with_history();
            app.view = view;
            press(&mut app, KeyCode::Char('t'));
            for line in render_lines(&mut app, width, 30).unwrap() {
                assert!(
                    line.chars().count() <= width as usize,
                    "growth {view:?} at {width} columns overflowed: {line}"
                );
            }
        }
    }
}

#[test]
fn the_explorer_charts_growth_when_growth_is_selected() {
    let mut app = app_with_history();
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.view, View::Explorer);

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(
        screen.contains('▀') || screen.contains('▄'),
        "no chart\n{screen}"
    );
    // The hole carries the total change rather than the total size.
    assert!(screen.contains("+82 GB"), "{screen}");
    assert!(screen.contains("■"), "{screen}");
}

#[test]
fn toggling_back_returns_to_sizes() {
    let mut app = app_with_history();
    press(&mut app, KeyCode::Char('t'));
    press(&mut app, KeyCode::Char('t'));

    assert!(!app.showing_growth());
    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(screen.contains("Largest items"), "{screen}");
    assert!(!screen.contains("+46 GB"), "{screen}");
}

#[test]
fn the_details_panel_reports_what_changed_and_what_made_it() {
    let mut app = app_with_history();
    press(&mut app, KeyCode::Char('d'));

    let screen = joined(&render_lines(&mut app, 110, 34).unwrap());
    assert!(screen.contains("Change (7 days)"), "{screen}");
    assert!(screen.contains("+46 GB"), "{screen}");
    assert!(screen.contains("Current size"), "{screen}");
}

#[test]
fn growth_is_advertised_in_the_footer() {
    let screen = joined(&render_lines(&mut app(), 100, 30).unwrap());
    assert!(screen.contains("t growth"), "{screen}");
}

/// Not an assertion — a way to eyeball the growth view during development.
#[test]
#[ignore = "visual check: cargo test -- --ignored --nocapture"]
fn preview_growth() {
    let mut app = app_with_history();
    press(&mut app, KeyCode::Char('t'));
    for line in render_lines(&mut app, 92, 22).unwrap() {
        println!("|{line}");
    }
    println!();
    press(&mut app, KeyCode::Enter);
    for line in render_lines(&mut app, 92, 24).unwrap() {
        println!("|{line}");
    }
}

// ---------------------------------------------------------------------------
// Deleting
// ---------------------------------------------------------------------------

/// A real tree on disk, so deletion tests exercise the real filesystem.
struct Sandbox(PathBuf);

impl Sandbox {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!("disko-screens-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("cache")).unwrap();
        std::fs::create_dir_all(path.join("keep")).unwrap();
        std::fs::write(path.join("cache/blob"), vec![b'x'; 4000]).unwrap();
        std::fs::write(path.join("keep/notes"), vec![b'x'; 100]).unwrap();
        Self(path)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An app over a real directory, scanned for real.
fn app_over(root: &std::path::Path) -> App {
    let tree = disko_core::scan::scan(
        root,
        &ScanOptions::default(),
        &disko_core::scan::Progress::default(),
        &disko_core::scan::Cancel::new(),
    )
    .unwrap();

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
    app.root = tree.path.clone();
    app.cwd = tree.path.clone();
    app.tree = Some(tree);
    app.view = View::Overview;
    app
}

fn type_word(app: &mut App, word: &str) {
    for ch in word.chars() {
        press(app, KeyCode::Char(ch));
    }
}

#[test]
fn deleting_asks_before_it_does_anything() {
    let sandbox = Sandbox::new("asks");
    let mut app = app_over(&sandbox.0);

    press(&mut app, KeyCode::Char('x'));

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(screen.contains("This cannot be undone"), "{screen}");
    assert!(screen.contains("Permanently delete"), "{screen}");
    assert!(screen.contains("Type delete to confirm"), "{screen}");
    // Still there, obviously.
    assert!(sandbox.0.join("cache/blob").exists());
}

#[test]
fn escape_calls_the_whole_thing_off() {
    let sandbox = Sandbox::new("cancel");
    let mut app = app_over(&sandbox.0);

    press(&mut app, KeyCode::Char('x'));
    press(&mut app, KeyCode::Esc);

    assert!(app.confirm.is_none());
    assert!(sandbox.0.join("cache/blob").exists());
    assert_eq!(app.status.as_deref(), Some("nothing was deleted"));
}

/// The guard that matters most: Enter on its own must not be enough.
#[test]
fn enter_alone_does_not_delete() {
    let sandbox = Sandbox::new("enter");
    let mut app = app_over(&sandbox.0);

    press(&mut app, KeyCode::Char('x'));
    press(&mut app, KeyCode::Enter);

    assert!(app.confirm.is_some(), "the prompt should still be up");
    assert!(sandbox.0.join("cache/blob").exists());
    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(screen.contains("type delete first"), "{screen}");
}

#[test]
fn a_half_typed_word_is_not_enough_either() {
    let sandbox = Sandbox::new("partial");
    let mut app = app_over(&sandbox.0);

    press(&mut app, KeyCode::Char('x'));
    type_word(&mut app, "del");
    press(&mut app, KeyCode::Enter);

    assert!(sandbox.0.join("cache/blob").exists());
    assert!(app.confirm.is_some());
}

#[test]
fn typing_the_word_and_pressing_enter_deletes_and_corrects_the_totals() {
    let sandbox = Sandbox::new("commit");
    let mut app = app_over(&sandbox.0);
    let before = app.current_entry().unwrap().allocated_size;

    // "cache" is the largest, so it is the selected row.
    press(&mut app, KeyCode::Char('x'));
    type_word(&mut app, "delete");
    press(&mut app, KeyCode::Enter);

    assert!(!sandbox.0.join("cache").exists(), "it should be gone");
    assert!(sandbox.0.join("keep/notes").exists(), "and nothing else");

    // The totals fix themselves without a rescan.
    let after = app.current_entry().unwrap().allocated_size;
    assert!(after < before);
    assert!(app.rows().iter().all(|row| row.name != "cache"));

    let status = app.status.clone().unwrap();
    assert!(status.starts_with("deleted 1"), "{status}");
    assert!(status.contains("freed"), "{status}");
}

#[test]
fn marked_entries_are_deleted_together() {
    let sandbox = Sandbox::new("marked");
    let mut app = app_over(&sandbox.0);

    press(&mut app, KeyCode::Char(' ')); // mark "cache"
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char(' ')); // mark "keep"
    press(&mut app, KeyCode::Char('x'));

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(screen.contains("Permanently delete 2 items"), "{screen}");

    type_word(&mut app, "delete");
    press(&mut app, KeyCode::Enter);

    assert!(!sandbox.0.join("cache").exists());
    assert!(!sandbox.0.join("keep").exists());
    assert!(
        app.marks.is_empty(),
        "marks should not outlive their targets"
    );
}

#[test]
fn navigation_keys_cannot_reach_the_list_behind_the_prompt() {
    let sandbox = Sandbox::new("modal");
    let mut app = app_over(&sandbox.0);
    let cwd = app.cwd.clone();

    press(&mut app, KeyCode::Char('x'));
    for key in [KeyCode::Down, KeyCode::Right, KeyCode::Left, KeyCode::Tab] {
        press(&mut app, key);
    }

    assert_eq!(app.cwd, cwd, "navigation must not happen behind the prompt");
    assert_eq!(app.selection, 0);
    assert!(app.confirm.is_some());
    assert!(sandbox.0.join("cache/blob").exists());
}

#[test]
fn read_only_mode_refuses_to_even_ask() {
    let sandbox = Sandbox::new("readonly");
    let mut app = app_over(&sandbox.0);
    app.read_only = true;

    press(&mut app, KeyCode::Char('x'));

    assert!(app.confirm.is_none());
    assert_eq!(app.status.as_deref(), Some("disko is running read-only"));
    assert!(sandbox.0.join("cache/blob").exists());
}

#[test]
fn the_scan_root_itself_can_never_be_deleted() {
    let sandbox = Sandbox::new("root");
    let mut app = app_over(&sandbox.0);
    // Force the root in as a target, the way no UI path would allow.
    app.confirm = Some(disko::tui::app::Confirm {
        targets: vec![disko::deletion::Target {
            path: sandbox.0.clone(),
            size: 4000,
            is_dir: true,
        }],
        typed: "delete".to_string(),
        nagged: false,
    });

    press(&mut app, KeyCode::Enter);

    assert!(sandbox.0.exists(), "the scanned folder must survive");
    let status = app.status.clone().unwrap();
    assert!(status.contains("nothing could be deleted"), "{status}");
}

#[test]
fn delete_is_advertised_in_the_footer() {
    let screen = joined(&render_lines(&mut app(), 100, 30).unwrap());
    assert!(screen.contains("x delete"), "{screen}");
}

#[test]
#[ignore = "visual check: cargo test -- --ignored --nocapture"]
fn preview_delete_prompt() {
    let sandbox = Sandbox::new("preview");
    let mut app = app_over(&sandbox.0);
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('x'));
    type_word(&mut app, "del");
    for line in render_lines(&mut app, 88, 20).unwrap() {
        println!("|{line}");
    }
}
