//! Handing a path over to the desktop's file manager.
//!
//! The rule everywhere: disko opens *folders*, never files. Launching a file
//! in whatever application claims it would be a surprising thing for a disk
//! tool to do — and the files disko surfaces are the enormous ones, which is
//! the worst possible case for opening something by accident.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What the file manager is called here, for status messages.
pub fn manager_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "Finder"
    } else if cfg!(target_os = "windows") {
        "Explorer"
    } else {
        "your file manager"
    }
}

/// The command that reveals `path`, and whether it is a directory being
/// opened or an item being selected inside its parent.
fn command_for(path: &Path, is_dir: bool) -> Option<Command> {
    if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        if is_dir {
            // Open the folder itself; revealing it would show the parent.
            command.arg(path);
        } else {
            // -R selects the item in its parent instead of launching it.
            command.arg("-R").arg(path);
        }
        return Some(command);
    }

    if cfg!(target_os = "windows") {
        let mut command = Command::new("explorer");
        if is_dir {
            command.arg(path);
        } else {
            command.arg(format!("/select,{}", path.display()));
        }
        return Some(command);
    }

    // Everywhere else: xdg-open on a *directory* only. Pointed at a file it
    // would launch the default application, which is exactly what must not
    // happen to a 40 GB video.
    let folder = if is_dir { path } else { path.parent()? };
    let mut command = Command::new("xdg-open");
    command.arg(folder);
    Some(command)
}

/// Open `path` in the desktop's file manager.
///
/// Returns as soon as the child is spawned — waiting on a GUI application
/// would freeze the interface for as long as it stayed open.
pub fn reveal(path: &Path, is_dir: bool) -> Result<(), String> {
    let Some(mut command) = command_for(path, is_dir) else {
        return Err(format!("no parent folder for {}", path.display()));
    };

    // The child must not write to the terminal: disko owns the alternate
    // screen, and a stray line from xdg-open would corrupt the display.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    match command.spawn() {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(format!(
            "could not find {} — is a desktop session running?",
            command.get_program().to_string_lossy()
        )),
        Err(error) => Err(error.to_string()),
    }
}

/// The folder that would be shown for `path`, which is the path itself when it
/// is a directory and its parent otherwise.
pub fn folder_for(path: &Path, is_dir: bool) -> Option<PathBuf> {
    if is_dir {
        Some(path.to_path_buf())
    } else {
        path.parent().map(Path::to_path_buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program_and_args(path: &Path, is_dir: bool) -> (String, Vec<String>) {
        let command = command_for(path, is_dir).unwrap();
        (
            command.get_program().to_string_lossy().to_string(),
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().to_string())
                .collect(),
        )
    }

    #[test]
    fn a_directory_is_opened_directly() {
        let (program, args) = program_and_args(Path::new("/tmp/folder"), true);

        if cfg!(target_os = "macos") {
            assert_eq!(program, "open");
            assert_eq!(args, ["/tmp/folder"]);
        } else if cfg!(target_os = "windows") {
            assert_eq!(program, "explorer");
        } else {
            assert_eq!(program, "xdg-open");
            assert_eq!(args, ["/tmp/folder"]);
        }
    }

    /// The important one: a file must never be handed to whatever application
    /// claims its extension.
    #[test]
    fn a_file_is_never_launched() {
        let (program, args) = program_and_args(Path::new("/tmp/folder/huge.mp4"), false);

        if cfg!(target_os = "macos") {
            // -R reveals rather than opens.
            assert_eq!(program, "open");
            assert_eq!(args, ["-R", "/tmp/folder/huge.mp4"]);
        } else if cfg!(target_os = "windows") {
            assert_eq!(args, ["/select,/tmp/folder/huge.mp4"]);
        } else {
            // The folder, never the file.
            assert_eq!(program, "xdg-open");
            assert_eq!(args, ["/tmp/folder"]);
            assert!(!args.iter().any(|arg| arg.contains("huge.mp4")));
        }
    }

    #[test]
    fn a_file_with_no_parent_has_nowhere_to_go() {
        assert!(folder_for(Path::new("/"), false).is_none());
        assert_eq!(
            folder_for(Path::new("/tmp/x"), false),
            Some(PathBuf::from("/tmp"))
        );
        assert_eq!(
            folder_for(Path::new("/tmp/x"), true),
            Some(PathBuf::from("/tmp/x"))
        );
    }

    #[test]
    fn the_manager_is_named_for_the_platform() {
        let name = manager_name();
        assert!(!name.is_empty());
        if cfg!(target_os = "macos") {
            assert_eq!(name, "Finder");
        }
    }
}
