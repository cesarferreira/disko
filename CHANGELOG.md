# Changelog

All notable changes to this project will be documented in this file.

## [0.6.0] - 2026-08-07

### 🚀 Features

- Disk usage TUI with ranked overview and radial explorer
- What changed — disk usage over time, and developer-aware cleanup
- Delete from the TUI, and make "up" from the top return to the disk list
- Open the selection in the file manager, and colour the shortcut keys

### 🐛 Bug Fixes

- Gate the Linux-only volume-label lookup behind cfg(target_os)
- Deleting no longer freezes the interface
- Fixed makefile
- Scope release changelog hook
- Light up every wedge, and keep the explorer responsive
- Say when a wedge is too small to place, and highlight its run
- Light the whole selected slice, not just its innermost ring

### 💼 Other

- Added screenshot
- Long file name
- Optimize code
- Reworked makefile

### 📚 Documentation

- Show the radial explorer in the README

### ⚡ Performance

- Open instantly from the last snapshot, and stream a cold scan
- Store names, not paths, on every entry

### 🧪 Testing

- Make the allocated-size test filesystem-agnostic
