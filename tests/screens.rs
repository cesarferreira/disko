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
    assert!(screen.contains("some folders unreadable"), "{screen}");
}
