//! disko's command surface, view model and TUI.
//!
//! The reusable engine lives in `disko-core` (scanning, sizes, filesystems)
//! and `disko-render` (bars, canvases, the sunburst). This crate is the part
//! that decides what a person sees.

pub mod cli;
pub mod clipboard;
pub mod commands;
pub mod deletion;
pub mod model;
pub mod output;
pub mod report;
pub mod reveal;
pub mod timefmt;
pub mod tui;
