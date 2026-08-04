<div align="center">
  <h1>disko</h1>

  <p><strong>Disk usage TUI that shows what is full and what is using it</strong></p>

  <p>
    <img alt="License" src="https://img.shields.io/badge/license-MIT-green">
    <img alt="Rust" src="https://img.shields.io/badge/rust-1.85%2B-orange">
    <img alt="Edition" src="https://img.shields.io/badge/edition-2024-blue">
    <a href="https://crates.io/crates/disko-cli"><img alt="crates.io" src="https://img.shields.io/crates/v/disko-cli.svg"></a>
  </p>

  <p>
    <a href="#install">Install</a>
    &nbsp;·&nbsp;
    <a href="#quickstart">Quickstart</a>
    &nbsp;·&nbsp;
    <a href="#usage">Usage</a>
    &nbsp;·&nbsp;
    <a href="#library">Library</a>
    &nbsp;·&nbsp;
    <a href="#development">Development</a>
  </p>
</div>

---

`df` tells you a disk is 82% full. `du` tells you which directory is big. Neither
tells you *where to go next*, and both answer in filesystem language when the
question was about your laptop.

disko answers three questions on one screen — **what is full, what is using the
space, and where should I look next** — and keeps device nodes, inode counts and
filesystem types behind a key press.

```
 Macintosh HD                                      404 GB used of 494 GB
 ████████████████████████████████████████░░░░░░░░  82%

 Current folder: /    404 GB

   257 GB  Users         ████████████████████▍             63.6%
    84 GB  Library       ██████▋                           20.8%
    34 GB  Applications  ██▊                                8.4%
    12 GB  System        █                                    3%
     4 GB  private       ▍                                    1%
     3 GB  opt           ▎                                  0.7%

 Largest items
   71 GB   ~/Library
   48 GB   ~/Downloads
   32 GB   ~/code
   19 GB   ~/Movies

 Enter explore   → open   ← up   / search   s sort   d details   q quit
```

Press `Enter` and the same data becomes a DaisyDisk-style sunburst you can walk
into, with the legend keeping every wedge tied to a name and a size.

## Install

Requires [Rust](https://rustup.rs) **1.85+** and `~/.cargo/bin` on your `PATH`.

```bash
cargo install disko-cli
```

Verify:

```bash
disko --help
```

<details>
<summary><strong>Build from source</strong> — for development or unreleased changes</summary>

```bash
git clone https://github.com/cesarferreira/disko
cd disko
make install-release
```

</details>

## Quickstart

```bash
disko              # pick a disk, then explore it
disko ~            # scan your home directory right away
disko / --top 20   # the 20 largest things on the root filesystem
```

Inside the TUI:

| Key | Does |
| --- | --- |
| `↑` `↓` | move the selection |
| `Enter` | open the radial explorer (and drill in, once there) |
| `→` `←` | open the selected folder / go back up |
| `/` | filter the list as you type |
| `s` | cycle sort: size, name, item count |
| `d` | filesystem details — device, type, inodes, read-only |
| `a` | switch between space used on disk and apparent file sizes |
| `Space` | mark an entry; the footer totals what you marked |
| `r` | rescan |
| `q` | quit |

## Usage

### Disk capacity and directory usage never share a line

The header always describes the **filesystem**:

```
Macintosh HD    404 GB used of 494 GB    82%
```

The body always describes the **directory you are in**:

```
Current folder: /Users/cesar    257 GB
```

Conflating those two numbers is the single most confusing thing a disk tool can
do, so disko keeps them apart on purpose.

### Details are one key away, not in your face

Filesystem types, device identifiers, inode counts, mount options and
pseudo-filesystems are all real information — they are just not the answer to
"what is filling my disk". They live behind `d` or a flag:

```bash
disko --details        # device, type, read-only, inodes
disko --filesystems    # every mount, pseudo-filesystems included
disko --inodes         # inode usage instead of bytes
```

```
Volume       Macintosh HD
Mount        /
Device       /dev/disk3s1s1
Filesystem   apfs
Read-only    no
Capacity     404 GB used of 494 GB (82%)
Inodes       1,452,254 used of 16,646,144 (9%)
```

### Scripting

Piping switches to text automatically — no flag needed — but they are there when
you want to be explicit.

```bash
disko ~ --plain          # bytes, human size, share, path (tab separated)
disko ~ --json           # the whole tree, structured
```

```console
$ disko ~/code --plain --top 3
8822644736	8.8 GB	81.1%	/home/cesar/code/stax
1193025536	1.2 GB	11%	/home/cesar/code/disko
847593472	848 MB	7.8%	/home/cesar/code/changed
13811712	14 MB	0.1%	/home/cesar/code (other)
```

Raw bytes come first so `cut -f1` and `sort -n` work; the human column is there
for when a person reads it too.

### Options

| Flag | Does |
| --- | --- |
| `-t, --top <N>` | show the N largest entries, grouping the rest as "Other" (default 20) |
| `--depth <N>` | keep only N levels of tree; sizes stay exact either way |
| `--apparent` | count file lengths (`ls`) instead of blocks used (`du`) |
| `--binary` | GiB/MiB instead of GB/MB |
| `-x, --one-file-system` | do not cross into other mounts |
| `--count-hardlinks` | count a hard-linked file once per link instead of once per inode |
| `-a, --all` | include pseudo-filesystems in the disk list |

Sizes match `du` byte for byte: `--apparent` agrees with `du -sb`, and the
default agrees with `du -s --block-size=1`.

## Library

The engine is split out so the TUI, `--json` and anything you build sit on the
same scanning implementation.

```text
disko-core      scanning · size aggregation · filesystem metadata
                tree model · cancellation and progress
disko-render    radial layout · half-block canvas · braille canvas · bars
disko-cli       commands · TUI · output modes
```

`disko-core` returns neutral data and never formats anything for a particular
display:

```rust
use std::path::Path;
use disko_core::{scan, ScanOptions, SizeKind};

let tree = scan::scan(
    Path::new("/Users/cesar"),
    &ScanOptions::default(),
    &scan::Progress::default(),
    &scan::Cancel::new(),
)?;

for child in &tree.children {
    println!("{:>12}  {}", child.size(SizeKind::Allocated), child.name());
}
```

Scans run on rayon and can be driven from a UI thread:

```rust
let (progress, cancel, handle) = scan::scan_in_background(path, ScanOptions::default());
println!("{} items so far", progress.entries());
cancel.cancel();               // returns whatever was counted
let tree = handle.join().unwrap()?;
```

## Development

```bash
make            # check, build, test
make test       # cargo test --workspace
make lint       # rustfmt + clippy -D warnings
make install    # debug build into ~/.cargo/bin
make release    # bump, changelog, tag, publish (LEVEL=patch|minor|major)
```

Every screen is rendered into an off-screen buffer and asserted at real terminal
widths, so layout regressions — a column that overflows at 60 columns, a section
that eats the list at 12 rows — fail the build rather than the user.

## Platforms

macOS and Linux. Windows compiles but is untested and ships no release binary:
allocated sizes, hard-link dedup and the mount table all fall back to
approximations there.

## License

MIT © Cesar Ferreira
