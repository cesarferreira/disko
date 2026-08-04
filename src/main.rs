use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use disko_core::scan::{self, Cancel, Progress};
use disko_core::{Filesystem, Unit};

use disko::cli::{Cli, Command, Options};
use disko::commands;
use disko::model::{self, RowOptions};
use disko::output;
use disko::timefmt;
use disko::tui;
use disko::tui::app::{App, Settings, Watch};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        // `disko / | head` closes the pipe as soon as it has enough. That is
        // the reader being satisfied, not disko failing.
        Err(error) if is_broken_pipe(&error) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("disko: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let stdout = io::stdout();

    match cli.command {
        Some(Command::Diff { path, since }) => {
            let mut out = stdout.lock();
            commands::diff(&mut out, &cli.options, path, since)
        }
        Some(Command::Clean {
            path,
            safe_only,
            idle_for,
            delete,
            yes,
        }) => {
            let mut out = stdout.lock();
            commands::clean(
                &mut out,
                &cli.options,
                path,
                safe_only,
                idle_for,
                delete,
                yes,
            )
        }
        Some(Command::History { path, forget }) => {
            let mut out = stdout.lock();
            commands::history(&mut out, &cli.options, path, forget)
        }
        Some(Command::Watch { path, interval }) => watch(&cli.options, path, &interval),
        None => explore(&cli),
    }
}

fn explore(cli: &Cli) -> Result<()> {
    // A pipe or a file gets text, never escape codes, whether or not the user
    // remembered --plain.
    if io::stdout().is_terminal() && !cli.options.wants_text_output() {
        return run_tui(cli);
    }
    run_text(cli)
}

fn settings(options: &Options) -> Settings {
    Settings {
        size_kind: options.size_kind(),
        unit: options.unit(),
        top: options.top,
        scan_options: options.scan_options(),
        show_all_filesystems: options.all,
        record_snapshots: options.may_snapshot(),
    }
}

fn run_tui(cli: &Cli) -> Result<()> {
    let mut app = App::new(settings(&cli.options), cli.options.details);
    app.read_only = cli.options.read_only;
    let app = app;
    let app = match &cli.path {
        Some(path) => {
            ensure_readable(path)?;
            app.with_path(path)
        }
        None => app,
    };

    tui::run(app)
}

/// `disko watch` is the growth view with a heartbeat: scan once for a
/// baseline, then keep rescanning, so a build filling the disk is visible
/// while it happens rather than afterwards.
fn watch(options: &Options, path: Option<std::path::PathBuf>, interval: &str) -> Result<()> {
    let path = match path {
        Some(path) => path,
        None => std::env::current_dir().context("no current directory to watch")?,
    };
    ensure_readable(&path)?;

    let every = timefmt::parse_duration(interval)?.max(1);
    if !io::stdout().is_terminal() {
        return watch_plain(options, &path, every);
    }

    let app = App::new(settings(options), options.details)
        .watching(Watch::new(every))
        .with_path(&path);
    tui::run(app)
}

/// Piped `watch` prints a line per interval instead of taking over a terminal
/// that is not there.
fn watch_plain(options: &Options, path: &Path, every: u64) -> Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let kind = options.size_kind();
    let unit = options.unit();

    let progress = Progress::default();
    let baseline = scan::scan(path, &options.scan_options(), &progress, &Cancel::new())?;
    let start = baseline.size(kind);
    writeln!(
        out,
        "{}\t{}\tbaseline\t{}",
        start,
        disko_core::size::format(start, unit),
        path.display()
    )?;
    out.flush()?;

    loop {
        std::thread::sleep(std::time::Duration::from_secs(every));
        let progress = Progress::default();
        let current = scan::scan(path, &options.scan_options(), &progress, &Cancel::new())?;
        let delta = current.size(kind) as i64 - start as i64;
        writeln!(
            out,
            "{}\t{}\t{}\t{}",
            current.size(kind),
            disko_core::size::format(current.size(kind), unit),
            disko::report::signed(delta, unit),
            path.display()
        )?;
        out.flush()?;
    }
}

fn run_text(cli: &Cli) -> Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let unit = cli.options.unit();

    match &cli.path {
        Some(path) => {
            ensure_readable(path)?;
            report_scan(&mut out, &cli.options, path, unit)
        }
        None => report_filesystems(&mut out, &cli.options, unit),
    }
}

fn report_scan(out: &mut impl Write, options: &Options, path: &Path, unit: Unit) -> Result<()> {
    // --filesystems and --inodes are about the volume, not the tree, so they
    // skip the scan entirely.
    if options.inodes || options.filesystems {
        let filesystems: Vec<Filesystem> = disko_core::mounts::for_path(path).into_iter().collect();
        return if options.inodes {
            output::inodes_plain(out, &filesystems)
        } else {
            output::filesystems_plain(out, &filesystems, unit, true)
        };
    }

    let (tree, _) = commands::scan_and_record(path, options)?;
    let filesystem = disko_core::mounts::for_path(path);

    if options.json {
        return output::scan_json(out, &tree, filesystem.as_ref());
    }

    let row_options = RowOptions {
        size_kind: options.size_kind(),
        top: Some(options.top),
        sort: model::Sort::Size,
        filter: None,
    };
    output::scan_plain(out, &tree, &row_options, unit)?;

    if options.details {
        if let Some(fs) = &filesystem {
            writeln!(out)?;
            output::filesystem_details(out, fs, unit)?;
        }
    }
    Ok(())
}

fn report_filesystems(out: &mut impl Write, options: &Options, unit: Unit) -> Result<()> {
    // --filesystems is the "show me everything" switch, pseudo-mounts included.
    let filesystems = disko_core::mounts::list(options.all || options.filesystems);

    if options.json {
        return output::filesystems_json(out, &filesystems);
    }
    if options.inodes {
        return output::inodes_plain(out, &filesystems);
    }
    output::filesystems_plain(
        out,
        &filesystems,
        unit,
        options.details || options.filesystems,
    )
}

fn ensure_readable(path: &Path) -> Result<()> {
    std::fs::metadata(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(())
}

fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(io_error) = cause.downcast_ref::<io::Error>() {
            return io_error.kind() == io::ErrorKind::BrokenPipe;
        }
        // serde_json wraps the io error rather than exposing it in the chain.
        cause
            .downcast_ref::<serde_json::Error>()
            .and_then(serde_json::Error::io_error_kind)
            .is_some_and(|kind| kind == io::ErrorKind::BrokenPipe)
    })
}
