//! Renders every screen at real terminal sizes and checks what a person would
//! actually see. Layout bugs — a column that overflows, a section that eats
//! the list, a percentage that survives on a narrow window — only show up at
//! specific widths, so they are pinned here.

use std::path::PathBuf;

use disko::model::Sort;
use disko::tui::app::{App, Settings, View};
use disko::tui::{press, render_lines, settle};
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
            // A small entry, so the explorer's "too small to place" note is on
            // screen: it is the longest thing the breadcrumb line ever carries.
            app.selection = 5;
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

/// A downloads folder is a column of sixty-character release names, and the
/// legend can spare twenty. Whichever one is selected gets a full-width line
/// of its own under the chart, or its name is simply unreadable.
#[test]
fn the_selected_name_is_repeated_in_full_under_the_chart() {
    const LONG: &str = "Sonic.The.Hedgehog.2.2022.2160p.WEB-DL.DDP5.1.Atmos.HDR.HEVC-CMRG";

    let screen_with_selection = |selection: usize| {
        let mut app = app();
        app.tree = Some(dir(
            "/",
            10 * GB,
            vec![
                dir(&format!("/{LONG}"), 6 * GB, vec![]),
                dir("/Movies", 4 * GB, vec![]),
            ],
        ));
        app.view = View::Explorer;
        app.selection = selection;
        joined(&render_lines(&mut app, 100, 30).unwrap())
    };

    let selected = screen_with_selection(0);
    assert!(
        selected.contains(LONG),
        "the selected name is still cut off\n{selected}"
    );
    assert!(
        selected.contains("6 GB"),
        "and it says how big it is\n{selected}"
    );

    let elsewhere = screen_with_selection(1);
    assert!(
        !elsewhere.contains(LONG),
        "the line follows the cursor\n{elsewhere}"
    );
}

/// The reported bug: past the biggest few entries the chart stopped
/// responding, because every remaining wedge was thinner than a pixel and the
/// highlight had nothing to paint. Two different small rows must not draw the
/// same chart.
#[test]
fn selecting_a_small_entry_still_moves_the_highlight() {
    // Only the chart: the legend on the right always moves its own cursor, so
    // comparing whole screens would pass even with a dead chart.
    let chart_for = |selection: usize| {
        let mut app = app();
        app.view = View::Explorer;
        app.selection = selection;
        let row = app.selected_row().unwrap();
        // Small enough that the wedge is sub-pixel: this is the case that broke.
        assert!(row.fraction < 0.02, "{} is not a small row", row.name);

        let buffer = disko::tui::render(&mut app, 100, 30).unwrap();
        (0..30)
            .flat_map(|y| (0..60).map(move |x| (x, y)))
            .map(|(x, y)| format!("{:?}", buffer.cell((x, y)).unwrap()))
            .collect::<Vec<_>>()
    };

    assert_ne!(
        chart_for(4),
        chart_for(5),
        "the highlight did not move between two small entries"
    );
}

/// A real `~/Library` is a handful of huge entries and a long tail of tiny
/// ones. Every row has to change the chart — for the tail that means the
/// highlight band takes the colour of the row you are on, since the entries
/// themselves share a pixel of arc and cannot be told apart.
#[test]
fn every_row_of_a_long_tail_changes_the_chart() {
    const MB: u64 = 1_000_000;
    const KB: u64 = 1_000;

    let sizes = [
        58 * GB,
        49 * GB,
        14 * GB,
        12 * GB,
        10 * GB,
        8 * GB,
        4 * GB,
        736 * MB,
        551 * MB,
        157 * MB,
        122 * MB,
        100 * MB,
        47 * MB,
        10 * MB,
        3 * MB,
        696 * KB,
        213 * KB,
        57 * KB,
        25 * KB,
    ];
    let library = dir(
        "/Library",
        sizes.iter().sum(),
        sizes
            .iter()
            .enumerate()
            .map(|(index, size)| dir(&format!("/Library/entry{index:02}"), *size, vec![]))
            .collect(),
    );

    let chart_for = |selection: usize| {
        let mut app = app();
        app.tree = Some(library.clone());
        app.root = PathBuf::from("/Library");
        app.cwd = PathBuf::from("/Library");
        app.view = View::Explorer;
        app.selection = selection;

        let buffer = disko::tui::render(&mut app, 100, 30).unwrap();
        // The chart only: the legend moves its own cursor regardless.
        (0..30)
            .flat_map(|y| (0..60).map(move |x| (x, y)))
            .map(|(x, y)| format!("{:?}", buffer.cell((x, y)).unwrap()))
            .collect::<Vec<_>>()
    };

    for selection in 1..sizes.len() {
        assert_ne!(
            chart_for(selection - 1),
            chart_for(selection),
            "rows {} and {selection} draw the same chart",
            selection - 1
        );
    }
}

/// A highlight on a sub-pixel wedge can only honestly mean "somewhere in this
/// run", so the screen says so rather than let the cursor look stuck.
#[test]
fn the_breadcrumb_admits_when_a_wedge_is_too_small_to_place() {
    let screen_for = |selection: usize| {
        let mut app = app();
        app.view = View::Explorer;
        app.selection = selection;
        render_lines(&mut app, 100, 30).unwrap()
    };

    // /Users is 64% of the disk: its wedge is most of the ring.
    let big = screen_for(0);
    assert!(!joined(&big).contains("too small"), "{}", joined(&big));

    // /opt is 0.7%, which is a fraction of a pixel of arc.
    let tail = screen_for(5);
    assert!(
        tail[0].contains("0.7% of this folder — too small to place"),
        "{}",
        tail[0]
    );
    // The note lives up there precisely so the keys keep their room.
    assert!(
        tail.last().unwrap().contains("x delete"),
        "{:?}",
        tail.last()
    );
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
    app.root = tree.root_path().to_path_buf();
    app.cwd = tree.root_path().to_path_buf();
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
    settle(&mut app);

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
    settle(&mut app);

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
    settle(&mut app);

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

// ---------------------------------------------------------------------------
// Opening instantly
// ---------------------------------------------------------------------------

#[test]
fn last_known_numbers_are_labelled_as_last_known() {
    let mut app = app();
    // Two hours ago, while a fresh scan is on its way.
    app.provisional = Some(disko_core::history::now() - 7200);

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());

    assert!(screen.contains("as of 2 hours ago"), "{screen}");
    assert!(screen.contains("rescanning"), "{screen}");
    // The numbers themselves are still shown — they are real, just old.
    assert!(screen.contains("257 GB"), "{screen}");
}

#[test]
fn the_explorer_says_so_too() {
    let mut app = app();
    app.provisional = Some(disko_core::history::now() - 90);
    app.view = View::Explorer;

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(screen.contains("rescanning"), "{screen}");
}

#[test]
fn the_label_disappears_once_the_real_scan_lands() {
    let mut app = app();
    app.provisional = Some(disko_core::history::now() - 7200);
    assert!(joined(&render_lines(&mut app, 100, 30).unwrap()).contains("rescanning"));

    app.provisional = None;

    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(!screen.contains("rescanning"), "{screen}");
    assert!(!screen.contains("as of"), "{screen}");
}

#[test]
fn a_running_scan_shows_what_it_has_already_counted() {
    let mut app = app();
    app.view = View::Scanning;
    app.root = PathBuf::from("/");
    app.streamed = vec![
        disko_core::scan::Finished {
            path: PathBuf::from("/Users/cesar/Library"),
            allocated: 71 * GB,
            apparent: 71 * GB,
            items: 900,
        },
        disko_core::scan::Finished {
            path: PathBuf::from("/Users/cesar/Downloads"),
            allocated: 48 * GB,
            apparent: 48 * GB,
            items: 120,
        },
    ];

    let screen = joined(&render_lines(&mut app, 100, 24).unwrap());

    assert!(screen.contains("Biggest so far"), "{screen}");
    assert!(screen.contains("71 GB"), "{screen}");
    assert!(screen.contains("Library"), "{screen}");
    assert!(screen.contains("Scanning"), "{screen}");
}

#[test]
fn the_progress_screen_survives_a_short_terminal() {
    let mut app = app();
    app.view = View::Scanning;
    app.streamed = (0..12)
        .map(|index| disko_core::scan::Finished {
            path: PathBuf::from(format!("/some/deep/path/number-{index}")),
            allocated: (12 - index) * GB,
            apparent: 0,
            items: 1,
        })
        .collect();

    for (width, height) in [(40u16, 10u16), (60, 14), (100, 24), (140, 40)] {
        for line in render_lines(&mut app, width, height).unwrap() {
            assert!(
                line.chars().count() <= width as usize,
                "scanning at {width}x{height} overflowed: {line}"
            );
        }
    }
}

/// The reported bug: deleting 68 GB blocked the draw loop, so the whole
/// interface appeared to hang with no output and no way to interrupt it.
#[test]
fn a_running_deletion_keeps_the_interface_alive() {
    let sandbox = Sandbox::new("responsive");
    // Enough files that the delete does not finish instantly.
    for index in 0..400 {
        std::fs::write(sandbox.0.join(format!("cache/f{index}")), b"x").unwrap();
    }

    let mut app = app_over(&sandbox.0);
    press(&mut app, KeyCode::Char('x'));
    type_word(&mut app, "delete");
    press(&mut app, KeyCode::Enter);

    // Control comes straight back rather than blocking until it is done.
    assert!(app.deleting.is_some(), "the deletion should be in flight");

    // And the screen renders, showing progress rather than a frozen frame.
    let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
    assert!(screen.contains("Deleting"), "{screen}");
    assert!(screen.contains("Esc to stop"), "{screen}");

    settle(&mut app);
    assert!(!sandbox.0.join("cache").exists());
    assert!(app.status.unwrap().contains("freed"));
}

#[test]
fn escape_during_a_deletion_asks_it_to_stop_rather_than_doing_nothing() {
    let sandbox = Sandbox::new("stoppable");
    for index in 0..400 {
        std::fs::write(sandbox.0.join(format!("cache/f{index}")), b"x").unwrap();
    }

    let mut app = app_over(&sandbox.0);
    press(&mut app, KeyCode::Char('x'));
    type_word(&mut app, "delete");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Esc);

    if let Some(deleting) = &app.deleting {
        assert!(deleting.is_stopping());
        let screen = joined(&render_lines(&mut app, 100, 30).unwrap());
        assert!(screen.contains("Stopping"), "{screen}");
    }
    settle(&mut app);
    assert!(app.deleting.is_none());
}

#[test]
fn navigation_is_locked_out_while_files_are_being_removed() {
    let sandbox = Sandbox::new("locked");
    for index in 0..400 {
        std::fs::write(sandbox.0.join(format!("cache/f{index}")), b"x").unwrap();
    }

    let mut app = app_over(&sandbox.0);
    let cwd = app.cwd.clone();
    press(&mut app, KeyCode::Char('x'));
    type_word(&mut app, "delete");
    press(&mut app, KeyCode::Enter);

    for key in [KeyCode::Right, KeyCode::Down, KeyCode::Char('q')] {
        press(&mut app, key);
    }
    assert_eq!(app.cwd, cwd, "the tree must not move while it is changing");
    assert!(
        !app.quit,
        "quitting mid-delete would abandon a partial removal"
    );

    settle(&mut app);
}

#[test]
#[ignore = "visual check: cargo test -- --ignored --nocapture"]
fn preview_delete_progress() {
    let sandbox = Sandbox::new("preview-progress");
    for index in 0..6000 {
        let dir = sandbox.0.join(format!("cache/sub{}", index % 40));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("f{index}")), vec![b'x'; 400]).unwrap();
    }

    let mut app = app_over(&sandbox.0);
    press(&mut app, KeyCode::Char('x'));
    type_word(&mut app, "delete");
    press(&mut app, KeyCode::Enter);

    for frame in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(40));
        app.advance();
        println!("--- frame {frame} (deleting in the background) ---");
        for line in render_lines(&mut app, 84, 14).unwrap() {
            if !line.trim().is_empty() {
                println!("|{line}");
            }
        }
    }
    settle(&mut app);
    println!("--- done: {} ---", app.status.clone().unwrap());
}

// ---------------------------------------------------------------------------
// Footer and revealing
// ---------------------------------------------------------------------------

#[test]
fn shortcut_keys_are_coloured_apart_from_their_descriptions() {
    let mut app = app();
    let buffer = disko::tui::render(&mut app, 110, 30).unwrap();
    let last = 29;

    // Find the "q" of "q quit" and the "u" just after it in "quit".
    let row: String = (0..110)
        .map(|x| buffer.cell((x, last)).map(|c| c.symbol()).unwrap_or(" "))
        .collect::<Vec<_>>()
        .concat();
    let quit_at = row.find("q quit").expect("the quit hint should be there");

    let key_style = buffer.cell((quit_at as u16, last)).unwrap().style();
    let label_style = buffer.cell((quit_at as u16 + 2, last)).unwrap().style();

    assert_ne!(
        key_style.fg, label_style.fg,
        "the key and its description should not be the same colour"
    );
}

#[test]
fn the_footer_advertises_revealing_in_the_file_manager() {
    let screen = joined(&render_lines(&mut app(), 130, 30).unwrap());
    assert!(screen.contains("o reveal"), "{screen}");
}

#[test]
fn the_way_out_survives_even_the_narrowest_terminal() {
    for width in [24u16, 32, 40, 56, 80, 120, 200] {
        for view in [View::Overview, View::Explorer, View::Picker] {
            let mut app = app();
            app.view = view;
            let lines = render_lines(&mut app, width, 30).unwrap();
            let footer = lines.last().unwrap();

            let escape = match view {
                View::Explorer => "Esc",
                _ => "q",
            };
            assert!(
                footer.contains(escape),
                "{view:?} at {width} columns lost its way out: {footer:?}"
            );
            assert!(
                footer.chars().count() <= width as usize,
                "{view:?} at {width} overflowed: {footer:?}"
            );
        }
    }
}

#[test]
fn hints_are_dropped_from_the_least_important_end() {
    let mut app = app();
    let wide = render_lines(&mut app, 140, 30)
        .unwrap()
        .last()
        .unwrap()
        .clone();
    let narrow = render_lines(&mut app, 46, 30)
        .unwrap()
        .last()
        .unwrap()
        .clone();

    // The most useful hint stays, the tail goes.
    assert!(wide.contains("Enter explore"), "{wide}");
    assert!(narrow.contains("Enter explore"), "{narrow}");
    assert!(wide.contains("d details"), "{wide}");
    assert!(!narrow.contains("d details"), "{narrow}");
}

#[test]
fn copying_reports_the_path_it_put_on_the_clipboard() {
    let mut app = app();
    app.selection = 1;
    let expected = app.selected_row().unwrap().path.unwrap();

    press(&mut app, KeyCode::Char('y'));

    let status = app.status.clone().unwrap();
    assert!(status.starts_with("copied"), "{status}");
    assert!(
        status.ends_with(&expected.display().to_string()),
        "{status}"
    );
}

/// A group row stands for several paths, so there is no single one to copy —
/// the directory being looked at is the useful answer instead of a complaint.
#[test]
fn copying_a_group_row_falls_back_to_the_directory() {
    let mut app = app();
    app.settings.top = 1;
    app.selection = 1;
    assert!(app.selected_row().unwrap().is_other());

    press(&mut app, KeyCode::Char('c'));

    let status = app.status.clone().unwrap();
    assert_eq!(status, "copied /", "{status}");
}

#[test]
fn revealing_the_other_row_explains_itself_rather_than_opening_nothing() {
    let mut app = app();
    // Force the synthetic "Other" row to be the selection.
    app.settings.top = 1;
    app.selection = 1;
    assert!(app.selected_row().unwrap().is_other());

    press(&mut app, KeyCode::Char('o'));

    let status = app.status.clone().unwrap();
    assert!(status.contains("group"), "{status}");
}

#[test]
#[ignore = "visual check: cargo test -- --ignored --nocapture"]
fn preview_footer() {
    for width in [130u16, 100, 76, 56, 40] {
        let mut app = app();
        let footer = render_lines(&mut app, width, 30)
            .unwrap()
            .last()
            .unwrap()
            .clone();
        println!("{width:>4} |{footer}");
    }
    println!();
    let mut app = app();
    app.view = View::Explorer;
    let footer = render_lines(&mut app, 100, 30)
        .unwrap()
        .last()
        .unwrap()
        .clone();
    println!("expl |{footer}");
}
