<div align="center">
  <h1>disko</h1>

  <p><strong>Not just where your space went, but when and why</strong></p>

  <p>
    <img alt="License" src="https://img.shields.io/badge/license-MIT-green">
    <img alt="Rust" src="https://img.shields.io/badge/rust-1.85%2B-orange">
    <img alt="Edition" src="https://img.shields.io/badge/edition-2024-blue">
    <a href="https://crates.io/crates/disko-cli"><img alt="crates.io" src="https://img.shields.io/crates/v/disko-cli.svg"></a>
  </p>

  <p>
    <a href="#install">Install</a>
    &nbsp;·&nbsp;
    <a href="#what-changed">What changed</a>
    &nbsp;·&nbsp;
    <a href="#what-can-i-delete">What can I delete</a>
    &nbsp;·&nbsp;
    <a href="#exploring">Exploring</a>
    &nbsp;·&nbsp;
    <a href="#library">Library</a>
  </p>
</div>

---

`du` and `dust` tell you what is large **now**. `df` tells you a disk is 82% full.
Neither answers the question you actually have at 2am:

> My disk was fine two days ago. **What happened?**

disko records a snapshot every time it scans, so it can tell you.

```console
$ disko diff --since 7d

 disko — +82 GB added since Monday, 14:32 (7 days)

  +46 GB  ~/Library/Developer/Xcode/DerivedData
  +18 GB  ~/.gradle/caches
  +11 GB  ~/Downloads
   +7 GB  ~/Library/Containers/com.docker.docker/Data  (new)

 322 GB → 404 GB
```

Not "your home directory grew 82 GB", which you already knew. disko follows the
growth down to the directory that actually owns it, and stops when it reaches
something worth naming.

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

## What changed

Every full scan writes a snapshot to `~/.local/share/disko` (a few KB each, 64
kept per directory). Nothing to set up: by the time you think to ask what
happened, the evidence already exists.

```bash
disko diff                # since the previous scan
disko diff --since 7d     # since a week ago
disko diff ~/ --since 3mo # a specific directory, a specific window
disko history             # every snapshot, and what moved between them
```

Inside the TUI, press **`t`** and the same data becomes the view: growth glows,
shrinking directories cool to blue, everything that did not move recedes into
the background.

```text
 Macintosh HD                                          404 GB used of 494 GB
 ████████████████████████████████████████████░░░░░░░░  82%

 Current folder: /    404 GB    +82 GB in 7 days

   +46 GB  Users         ████████████████████████████████     257 GB
   +34 GB  Applications  ███████████████████████▋              34 GB
    +4 GB  private       ██▊                                    4 GB
```

Select anything and press `d` to see when it grew and what produced it.

### Watching it happen

```bash
disko watch                    # the current directory, every 3s
disko watch ~/ --interval 30s
```

Growth since the moment you started watching, updated live — for when a build,
a download or an agent is filling the disk right now and you want to see which
directory is doing it.

### It opens instantly

disko paints the last snapshot the moment you open it, labelled for what it is,
and corrects itself as the fresh scan lands a fraction of a second later:

```text
 Current folder: ~/code    78 GB    as of 2 hours ago · rescanning…

   41 GB  monorepo   ████████████████▉   52.7%
   18 GB  .cache     ███████▋            23.6%
   18 GB  models     ███████▌            23.6%
```

On a first scan there is nothing to paint from, so the progress screen shows
what has actually been counted so far instead of a bare spinner:

```text
  ⠙  113689 items · 62 GB · 2s
     ~/code/.cache/bazel/63bf4…/site-packages/urllib3

  Biggest so far
     40 GB   ~/code/monorepo/.git
     56 MB   ~/code/monorepo/services
```

Those are finished directories, not running estimates — each number is final
for that folder. The list is incomplete, never wrong.

disko deliberately does *not* cache scans to make them faster. It cannot be
done correctly: appending 10 MB to a file changes no directory's mtime, so any
cache keyed on directory timestamps silently reports stale sizes, and validating
one properly means stat-ing every file — which is the scan. The kernel's dentry
cache already does this job well, which is why a repeat scan of 700k entries
takes 0.6 seconds.

## What can I delete

```console
$ disko clean

 Reclaimable developer storage                 73 GB

  28 GB  Xcode DerivedData        safe to regenerate
         regenerate: rebuild in Xcode · last used 2 days ago
  19 GB  Gradle caches            safe to regenerate
         regenerate: ./gradlew build · last used 6 hours ago · 3 locations
  14 GB  Docker data              review first
         regenerate: docker system prune · last used 3 weeks ago
   7 GB  Android emulator images  review first
         regenerate: recreate the AVD · unused for 4 months
   5 GB  Rust target directories  safe to regenerate
         regenerate: cargo build · last used 1 hour ago · 12 locations
```

Not "this folder is large" but **what produced it**, **whether removing it is
safe**, **what would bring it back**, and **when anything last touched it** —
that last one from real modification times gathered during the scan, not a
guess.

```bash
disko clean --safe-only          # only things that regenerate themselves
disko clean --idle-for 3mo       # only what nobody has touched in a season
disko clean --delete             # after typing "delete" to confirm
```

Deletion is opt-in, prompts for a typed confirmation, and re-checks every path
against the category rules immediately before removing it. A `target` directory
only counts as build output when a `Cargo.toml` sits beside it.

## Exploring

```bash
disko              # pick a disk, then explore it
disko ~            # scan your home directory right away
disko / --top 20   # the 20 largest things on the root filesystem
```

The default screen answers three questions and nothing else — what is full,
what is using the space, where to look next:

```text
 Macintosh HD                                      404 GB used of 494 GB
 ████████████████████████████████████████░░░░░░░░  82%

 Current folder: /    404 GB

   257 GB  Users         ████████████████████▍             63.6%
    84 GB  Library       ██████▋                           20.8%
    34 GB  Applications  ██▊                                8.4%

 Largest items
   71 GB   ~/Library
   48 GB   ~/Downloads
   32 GB   ~/code
```

Press `Enter` for a DaisyDisk-style sunburst you can walk into.

| Key | Does |
| --- | --- |
| `↑` `↓` | move the selection |
| `Enter` | open the radial explorer (and drill in, once there) |
| `→` `←` | open the selected folder / go back up |
| **`t`** | **switch between size and growth** |
| `/` | filter the list as you type |
| `s` | cycle sort: size, name, item count |
| `d` | details — device, type, inodes, change, category |
| `a` | apparent sizes instead of blocks used |
| `Space` | mark an entry; the footer totals what you marked |
| **`x`** | **delete what is marked** (or the selection), after confirmation |
| `r` | rescan |
| `q` | quit |

### Deleting

Mark things with `Space`, press **`x`**, and disko shows exactly what will go:

```text
 ┌ This cannot be undone ───────────────────────────────────────────┐
 │Permanently delete 2 items, 16 GB                                 │
 │                                                                  │
 │  • ~/Library/Developer/Xcode/DerivedData/    12 GB                │
 │  • ~/.gradle/caches/                          4 GB                │
 │                                                                  │
 │ Type delete to confirm: del▏  Esc to cancel                      │
 └──────────────────────────────────────────────────────────────────┘
```

Nothing happens until the word is typed in full — `Enter` on its own is not
enough, and navigation keys cannot reach the list behind the prompt. Every path
is re-checked against the filesystem immediately before removal, so disko will
refuse the scan root, anything outside the current scan, a mount point, or
something that has vanished since the scan. Symlinks are unlinked, never
followed into. Totals correct themselves instantly, without a rescan.

`disko --read-only` removes the ability entirely.

For caches specifically, `disko clean --delete` is usually the better tool: it
knows what regenerates them.

### Disk capacity and directory usage never share a line

The header always describes the **filesystem** (`Macintosh HD — 404 GB used of
494 GB`). The body always describes the **directory you are in** (`Current
folder: /Users/cesar — 257 GB`). Conflating those two numbers is the single
most confusing thing a disk tool can do.

### Details are one key away, not in your face

Filesystem types, device identifiers, inode counts and pseudo-filesystems are
real information — they are just not the answer to "what is filling my disk".

```bash
disko --details        # device, type, read-only, inodes
disko --filesystems    # every mount, pseudo-filesystems included
disko --inodes         # inode usage instead of bytes
```

## Scripting

Piping switches to text automatically — no flag needed.

```bash
disko ~ --plain            # bytes, human size, share, path (tab separated)
disko ~ --json             # the whole tree, structured
disko diff --plain         # signed bytes, human, kind, path
disko clean --json         # categories, safety, regenerate command, last used
disko watch --plain        # one line per interval, forever
```

Raw bytes come first so `cut -f1` and `sort -n` work.

### Network filesystems are skipped by default

NFS, SMB, sshfs and blobfuse mounts are not on your disk, and stat-ing them one
round trip at a time is brutal — a single directory listing on a blob-storage
mount can take five seconds. disko stops at the mount point and says so rather
than silently reporting a low number:

```console
$ disko /mnt --plain          # a blobfuse2 mount lives under here
16384   16 KB   57.1%   /mnt/lost+found
4096    4.1 KB  14.3%   /mnt/remote
                                        # 0.009s, vs ~10 minutes walking Azure
```

Pass `--remote` to walk them anyway. Naming a network mount as the scan root
still scans it — asking for it is asking for it.

Note that `--depth` limits what is *kept*, not what is *walked*: sizes stay
exact, so it does not make a slow mount fast.

### Options

| Flag | Does |
| --- | --- |
| `-t, --top <N>` | show the N largest entries, grouping the rest as "Other" |
| `--depth <N>` | keep only N levels of tree; sizes stay exact either way |
| `--apparent` | count file lengths (`ls`) instead of blocks used (`du`) |
| `--binary` | GiB/MiB instead of GB/MB |
| `-x, --one-file-system` | do not cross into other mounts |
| `--count-hardlinks` | count a hard-linked file once per link |
| `-a, --all` | include pseudo-filesystems in the disk list |
| `--remote` | walk network filesystems too (skipped by default) |
| `--read-only` | disable deleting entirely |
| `--no-snapshot` | do not record this scan in the history |

Sizes match `du` byte for byte: `--apparent` agrees with `du -sb`, and the
default agrees with `du -s --block-size=1`.

## Library

The engine is split out so the TUI, `--json` and anything you build sit on the
same implementation.

```text
disko-core      scanning · size aggregation · filesystem metadata · tree model
                snapshots · diffing and growth attribution · category rules
disko-render    radial layout · half-block canvas · braille canvas · bars
disko-cli       commands · TUI · output modes
```

`disko-core` returns neutral data and never formats anything for a display:

```rust
use std::path::Path;
use disko_core::{scan, ScanOptions, SizeKind, Store};

let tree = scan::scan(
    Path::new("/Users/cesar"),
    &ScanOptions::default(),
    &scan::Progress::default(),
    &scan::Cancel::new(),
)?;

// Compare against last time, then remember this time.
let store = Store::open()?;
if let Some(before) = store.latest(&tree.path) {
    let diff = disko_core::diff::diff(
        &before.tree, &tree, SizeKind::Allocated,
        before.taken_at, disko_core::history::now(), before.floor,
    );
    for change in diff.growth(10) {
        println!("{:>14}  {}", change.delta, change.path.display());
    }
}
store.record(&tree)?;
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

Every screen is rendered into an off-screen buffer and asserted at real
terminal widths, so layout regressions — a column that overflows at 60 columns,
a section that eats the list at 12 rows — fail the build rather than the user.

## Platforms

macOS and Linux. Windows compiles but is untested and ships no release binary:
allocated sizes, hard-link dedup and the mount table all fall back to
approximations there.

## License

MIT © Cesar Ferreira
