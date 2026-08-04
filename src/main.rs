use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use disko_core::scan::{self, Cancel, Progress};
use disko_core::{Filesystem, Unit};

use disko::cli::Cli;
use disko::model::{self, RowOptions};
use disko::output;
use disko::tui;
use disko::tui::app::{App, Settings};

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

    // A pipe or a file gets text, never escape codes, whether or not the user
    // remembered --plain.
    let interactive = io::stdout().is_terminal() && !cli.wants_text_output();

    if interactive {
        return run_tui(&cli);
    }
    run_text(&cli)
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

fn run_tui(cli: &Cli) -> Result<()> {
    let settings = Settings {
        size_kind: cli.size_kind(),
        unit: cli.unit(),
        top: cli.top,
        scan_options: cli.scan_options(),
        show_all_filesystems: cli.all,
    };

    let app = App::new(settings, cli.details);
    let app = match &cli.path {
        Some(path) => {
            ensure_readable(path)?;
            app.with_path(path)
        }
        None => app,
    };

    tui::run(app)
}

fn run_text(cli: &Cli) -> Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let unit = cli.unit();

    match &cli.path {
        Some(path) => {
            ensure_readable(path)?;
            report_scan(&mut out, cli, path, unit)
        }
        None => report_filesystems(&mut out, cli, unit),
    }
}

fn report_scan(out: &mut impl Write, cli: &Cli, path: &Path, unit: Unit) -> Result<()> {
    // --filesystems and --inodes are about the volume, not the tree, so they
    // skip the scan entirely.
    if cli.inodes || cli.filesystems {
        let filesystems: Vec<Filesystem> = disko_core::mounts::for_path(path).into_iter().collect();
        return if cli.inodes {
            output::inodes_plain(out, &filesystems)
        } else {
            output::filesystems_plain(out, &filesystems, unit, true)
        };
    }

    let progress = Progress::default();
    let tree = scan::scan(path, &cli.scan_options(), &progress, &Cancel::new())?;
    let filesystem = disko_core::mounts::for_path(path);

    if cli.json {
        return output::scan_json(out, &tree, filesystem.as_ref());
    }

    let options = RowOptions {
        size_kind: cli.size_kind(),
        top: Some(cli.top),
        sort: model::Sort::Size,
        filter: None,
    };
    output::scan_plain(out, &tree, &options, unit)?;

    if cli.details {
        if let Some(fs) = &filesystem {
            writeln!(out)?;
            output::filesystem_details(out, fs, unit)?;
        }
    }
    Ok(())
}

fn report_filesystems(out: &mut impl Write, cli: &Cli, unit: Unit) -> Result<()> {
    // --filesystems is the "show me everything" switch, pseudo-mounts included.
    let filesystems = disko_core::mounts::list(cli.all || cli.filesystems);

    if cli.json {
        return output::filesystems_json(out, &filesystems);
    }
    if cli.inodes {
        return output::inodes_plain(out, &filesystems);
    }
    output::filesystems_plain(out, &filesystems, unit, cli.details || cli.filesystems)
}

fn ensure_readable(path: &Path) -> Result<()> {
    std::fs::metadata(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(())
}
