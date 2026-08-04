# Changelog

All notable changes to this project will be documented in this file.

## [0.3.0] - 2026-08-04

### 🚀 Features

- Disk usage TUI with ranked overview and radial explorer
- What changed — disk usage over time, and developer-aware cleanup
- Delete from the TUI, and make "up" from the top return to the disk list

### 🐛 Bug Fixes

- Gate the Linux-only volume-label lookup behind cfg(target_os)
- Deleting no longer freezes the interface
- Fixed makefile

### 💼 Other

- Added screenshot

### 📚 Documentation

- Show the radial explorer in the README

### ⚡ Performance

- Open instantly from the last snapshot, and stream a cold scan

### 🧪 Testing

- Make the allocated-size test filesystem-agnostic
