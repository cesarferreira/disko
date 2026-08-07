//! Recognising reclaimable developer storage.
//!
//! "This folder is 28 GB" is a fact. "This is Xcode's build cache, Xcode will
//! rebuild it, and nothing has touched it since March" is a decision you can
//! actually make. Every rule here carries four things: what produced it,
//! whether removing it is safe, what would regenerate it, and — from the
//! scan's modification times — when it was last used.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::size::SizeKind;
use crate::tree::DiskEntry;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Safety {
    /// Deleting costs time, not data: the tool rebuilds it on demand.
    Regenerable,
    /// Recoverable in principle, but the cost is high enough to look first —
    /// a slow re-download, a re-provisioned emulator, a running container.
    ReviewFirst,
}

impl Safety {
    pub fn label(self) -> &'static str {
        match self {
            Safety::Regenerable => "safe to regenerate",
            Safety::ReviewFirst => "review first",
        }
    }
}

/// How a path is recognised as belonging to a category.
#[derive(Copy, Clone, Debug)]
enum Matcher {
    /// A fixed path below the user's home directory.
    Home(&'static str),
    /// A fixed absolute path.
    Absolute(&'static str),
    /// A directory of this name anywhere, but only when a sibling file proves
    /// what produced it — otherwise every directory called `target` matches.
    Named {
        name: &'static str,
        sibling: Option<&'static str>,
    },
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct Rule {
    pub id: &'static str,
    /// What a person would call it: "Xcode DerivedData".
    pub label: &'static str,
    pub safety: Safety,
    /// The command that brings it back, when there is one.
    pub regenerate: Option<&'static str>,
    #[serde(skip)]
    matcher: Matcher,
}

const fn home(
    id: &'static str,
    label: &'static str,
    path: &'static str,
    safety: Safety,
    regenerate: Option<&'static str>,
) -> Rule {
    Rule {
        id,
        label,
        safety,
        regenerate,
        matcher: Matcher::Home(path),
    }
}

const fn named(
    id: &'static str,
    label: &'static str,
    name: &'static str,
    sibling: Option<&'static str>,
    safety: Safety,
    regenerate: Option<&'static str>,
) -> Rule {
    Rule {
        id,
        label,
        safety,
        regenerate,
        matcher: Matcher::Named { name, sibling },
    }
}

use Safety::{Regenerable, ReviewFirst};

/// Ordered most specific first: `~/.gradle/caches` should win over a bare
/// `caches` rule, and the first match wins.
pub const RULES: &[Rule] = &[
    // Apple / Xcode
    home(
        "xcode-derived-data",
        "Xcode DerivedData",
        "Library/Developer/Xcode/DerivedData",
        Regenerable,
        Some("rebuild in Xcode"),
    ),
    home(
        "xcode-device-support",
        "iOS DeviceSupport",
        "Library/Developer/Xcode/iOS DeviceSupport",
        Regenerable,
        Some("reconnect the device"),
    ),
    home(
        "xcode-archives",
        "Xcode archives",
        "Library/Developer/Xcode/Archives",
        ReviewFirst,
        None,
    ),
    home(
        "coresimulator-caches",
        "Simulator caches",
        "Library/Developer/CoreSimulator/Caches",
        Regenerable,
        Some("relaunch the simulator"),
    ),
    home(
        "coresimulator-devices",
        "Simulator devices",
        "Library/Developer/CoreSimulator/Devices",
        ReviewFirst,
        Some("xcrun simctl delete unavailable"),
    ),
    home(
        "swiftpm-cache",
        "SwiftPM cache",
        "Library/Caches/org.swift.swiftpm",
        Regenerable,
        Some("swift package resolve"),
    ),
    home(
        "cocoapods-cache",
        "CocoaPods cache",
        "Library/Caches/CocoaPods",
        Regenerable,
        Some("pod install"),
    ),
    // JVM
    home(
        "gradle-caches",
        "Gradle caches",
        ".gradle/caches",
        Regenerable,
        Some("./gradlew build"),
    ),
    home(
        "gradle-wrapper",
        "Gradle distributions",
        ".gradle/wrapper/dists",
        Regenerable,
        Some("./gradlew"),
    ),
    home(
        "maven-repository",
        "Maven repository",
        ".m2/repository",
        Regenerable,
        Some("mvn dependency:go-offline"),
    ),
    // Android
    home(
        "android-system-images",
        "Android system images",
        "Library/Android/sdk/system-images",
        ReviewFirst,
        Some("sdkmanager"),
    ),
    home(
        "android-system-images-linux",
        "Android system images",
        "Android/Sdk/system-images",
        ReviewFirst,
        Some("sdkmanager"),
    ),
    home(
        "android-avd",
        "Android emulator images",
        ".android/avd",
        ReviewFirst,
        Some("recreate the AVD"),
    ),
    // JavaScript
    home(
        "npm-cache",
        "npm cache",
        ".npm/_cacache",
        Regenerable,
        Some("npm install"),
    ),
    home(
        "yarn-cache",
        "Yarn cache",
        ".cache/yarn",
        Regenerable,
        Some("yarn install"),
    ),
    home(
        "yarn-cache-macos",
        "Yarn cache",
        "Library/Caches/Yarn",
        Regenerable,
        Some("yarn install"),
    ),
    home(
        "pnpm-store",
        "pnpm store",
        "Library/pnpm/store",
        Regenerable,
        Some("pnpm install"),
    ),
    home(
        "pnpm-store-linux",
        "pnpm store",
        ".local/share/pnpm/store",
        Regenerable,
        Some("pnpm install"),
    ),
    // Rust
    home(
        "cargo-registry-cache",
        "Cargo registry cache",
        ".cargo/registry/cache",
        Regenerable,
        Some("cargo fetch"),
    ),
    home(
        "cargo-git",
        "Cargo git checkouts",
        ".cargo/git",
        Regenerable,
        Some("cargo fetch"),
    ),
    // Go
    home(
        "go-module-cache",
        "Go module cache",
        "go/pkg/mod",
        Regenerable,
        Some("go mod download"),
    ),
    home(
        "go-build-cache",
        "Go build cache",
        ".cache/go-build",
        Regenerable,
        Some("go build"),
    ),
    // Python
    home(
        "pip-cache",
        "pip cache",
        ".cache/pip",
        Regenerable,
        Some("pip install"),
    ),
    home(
        "pip-cache-macos",
        "pip cache",
        "Library/Caches/pip",
        Regenerable,
        Some("pip install"),
    ),
    // Containers
    home(
        "docker-desktop",
        "Docker Desktop data",
        "Library/Containers/com.docker.docker/Data",
        ReviewFirst,
        Some("docker system prune"),
    ),
    home(
        "docker-linux",
        "Docker data",
        ".local/share/docker",
        ReviewFirst,
        Some("docker system prune"),
    ),
    Rule {
        id: "docker-var-lib",
        label: "Docker data",
        safety: ReviewFirst,
        regenerate: Some("docker system prune"),
        matcher: Matcher::Absolute("/var/lib/docker"),
    },
    // Compiler and package caches
    home("ccache", "ccache", ".ccache", Regenerable, Some("rebuild")),
    home(
        "homebrew-cache",
        "Homebrew downloads",
        "Library/Caches/Homebrew",
        Regenerable,
        Some("brew cleanup"),
    ),
    // Trash
    home(
        "trash-linux",
        "Trash",
        ".local/share/Trash",
        ReviewFirst,
        None,
    ),
    home("trash-macos", "Trash", ".Trash", ReviewFirst, None),
    // Per-project build output. These need a sibling manifest as proof, or
    // every directory called `target` or `build` would match.
    named(
        "rust-target",
        "Rust target directories",
        "target",
        Some("Cargo.toml"),
        Regenerable,
        Some("cargo build"),
    ),
    named(
        "node-modules",
        "node_modules",
        "node_modules",
        Some("package.json"),
        Regenerable,
        Some("npm install"),
    ),
    named(
        "python-venv",
        "Python virtualenvs",
        ".venv",
        None,
        Regenerable,
        Some("python -m venv .venv"),
    ),
    named(
        "pycache",
        "Python bytecode caches",
        "__pycache__",
        None,
        Regenerable,
        Some("re-run the code"),
    ),
];

/// The rule that claims `path`, if any.
pub fn classify(path: &Path) -> Option<&'static Rule> {
    let home = home_dir();
    RULES
        .iter()
        .find(|rule| matches(rule, path, home.as_deref()))
}

fn matches(rule: &Rule, path: &Path, home: Option<&Path>) -> bool {
    match rule.matcher {
        Matcher::Home(relative) => home.is_some_and(|home| path == home.join(relative)),
        Matcher::Absolute(absolute) => path == Path::new(absolute),
        Matcher::Named { name, sibling } => {
            if path.file_name().is_none_or(|actual| actual != name) {
                return false;
            }
            match sibling {
                // The proof has to sit next to the directory: a `target` beside
                // a Cargo.toml is build output, a `target` anywhere else is
                // somebody's data.
                Some(sibling) => path
                    .parent()
                    .is_some_and(|parent| parent.join(sibling).exists()),
                None => true,
            }
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub path: PathBuf,
    pub size: u64,
    pub rule: &'static Rule,
    /// Newest modification anywhere inside, as seconds since the Unix epoch.
    pub last_used: u64,
}

impl Candidate {
    /// Seconds since anything in here was touched.
    pub fn idle_for(&self, now: u64) -> u64 {
        now.saturating_sub(self.last_used)
    }
}

/// Every reclaimable thing in `tree`, largest first.
///
/// Once a directory matches, its children are not searched: a `node_modules`
/// inside a `target` is already counted by the `target`.
pub fn find_reclaimable(tree: &DiskEntry, kind: SizeKind) -> Vec<Candidate> {
    find_reclaimable_from(tree, kind, home_dir().as_deref())
}

pub fn find_reclaimable_from(
    tree: &DiskEntry,
    kind: SizeKind,
    home: Option<&Path>,
) -> Vec<Candidate> {
    let mut found = Vec::new();
    walk(tree.root_path(), tree, kind, home, &mut found);
    found.sort_by_key(|candidate| std::cmp::Reverse(candidate.size));
    found
}

/// `path` is where the walk has got to; entries below the root do not carry
/// one, so it is built on the way down.
fn walk(
    path: &Path,
    entry: &DiskEntry,
    kind: SizeKind,
    home: Option<&Path>,
    out: &mut Vec<Candidate>,
) {
    if !entry.is_dir() {
        return;
    }

    if let Some(rule) = RULES.iter().find(|rule| matches(rule, path, home)) {
        // An empty cache is not worth mentioning.
        if entry.size(kind) > 0 {
            out.push(Candidate {
                path: path.to_path_buf(),
                size: entry.size(kind),
                rule,
                last_used: entry.modified,
            });
        }
        return;
    }

    for child in &entry.children {
        walk(&path.join(child.name_os()), child, kind, home, out);
    }
}

/// Group candidates by rule, so `clean` reports "Rust target directories
/// 5 GB" once rather than listing forty projects.
pub fn group_by_rule(candidates: &[Candidate]) -> Vec<Group<'_>> {
    let mut groups: Vec<Group> = Vec::new();
    for candidate in candidates {
        match groups
            .iter_mut()
            .find(|group| group.rule.id == candidate.rule.id)
        {
            Some(group) => {
                group.size += candidate.size;
                group.last_used = group.last_used.max(candidate.last_used);
                group.paths.push(&candidate.path);
            }
            None => groups.push(Group {
                rule: candidate.rule,
                size: candidate.size,
                last_used: candidate.last_used,
                paths: vec![&candidate.path],
            }),
        }
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.size));
    groups
}

#[derive(Clone, Debug, Serialize)]
pub struct Group<'a> {
    pub rule: &'static Rule,
    pub size: u64,
    pub last_used: u64,
    pub paths: Vec<&'a Path>,
}

impl Group<'_> {
    pub fn idle_for(&self, now: u64) -> u64 {
        now.saturating_sub(self.last_used)
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::EntryType;
    use std::fs;

    fn rule_for(path: &str, home: &str) -> Option<&'static Rule> {
        RULES
            .iter()
            .find(|rule| matches(rule, Path::new(path), Some(Path::new(home))))
    }

    #[test]
    fn known_developer_caches_are_recognised() {
        let home = "/home/cesar";
        assert_eq!(
            rule_for("/home/cesar/Library/Developer/Xcode/DerivedData", home)
                .unwrap()
                .id,
            "xcode-derived-data"
        );
        assert_eq!(
            rule_for("/home/cesar/.gradle/caches", home).unwrap().id,
            "gradle-caches"
        );
        assert_eq!(
            rule_for("/home/cesar/go/pkg/mod", home).unwrap().id,
            "go-module-cache"
        );
        assert_eq!(
            rule_for("/var/lib/docker", home).unwrap().id,
            "docker-var-lib"
        );
    }

    #[test]
    fn unknown_directories_are_left_alone() {
        assert!(rule_for("/home/cesar/Documents", "/home/cesar").is_none());
        assert!(rule_for("/home/cesar/.gradle", "/home/cesar").is_none());
        // Another user's cache is not this user's cache.
        assert!(rule_for("/home/someone-else/.gradle/caches", "/home/cesar").is_none());
    }

    #[test]
    fn every_rule_says_whether_it_is_safe_and_has_a_unique_id() {
        let mut ids: Vec<&str> = RULES.iter().map(|rule| rule.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate rule id");

        for rule in RULES {
            assert!(!rule.label.is_empty(), "{} has no label", rule.id);
            // Anything called safe to regenerate must say what regenerates it.
            if rule.safety == Regenerable {
                assert!(
                    rule.regenerate.is_some(),
                    "{} claims to be regenerable but says nothing about how",
                    rule.id
                );
            }
        }
    }

    /// A `target` directory only counts as build output when a Cargo.toml sits
    /// beside it — otherwise a photographer's `target` folder gets swept up.
    #[test]
    fn project_output_needs_a_sibling_manifest_as_proof() {
        let mut base = std::env::temp_dir();
        base.push(format!("disko-categories-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        let real = base.join("project");
        let decoy = base.join("archery");
        fs::create_dir_all(real.join("target")).unwrap();
        fs::create_dir_all(decoy.join("target")).unwrap();
        fs::write(real.join("Cargo.toml"), b"[package]").unwrap();

        assert_eq!(
            classify(&real.join("target")).map(|rule| rule.id),
            Some("rust-target")
        );
        assert!(classify(&decoy.join("target")).is_none());

        let _ = fs::remove_dir_all(&base);
    }

    fn dir(path: &str, size: u64, modified: u64, children: Vec<DiskEntry>) -> DiskEntry {
        let mut entry = DiskEntry::new(PathBuf::from(path), EntryType::Directory);
        entry.allocated_size = size;
        entry.apparent_size = size;
        entry.modified = modified;
        entry.children = children;
        entry
    }

    #[test]
    fn reclaimable_scan_finds_caches_and_stops_descending_into_them() {
        let tree = dir(
            "/home/tester",
            100,
            500,
            vec![
                dir(
                    "/home/tester/.gradle",
                    40,
                    400,
                    vec![dir("/home/tester/.gradle/caches", 40, 400, vec![])],
                ),
                dir("/home/tester/Documents", 60, 500, vec![]),
            ],
        );

        let found =
            find_reclaimable_from(&tree, SizeKind::Allocated, Some(Path::new("/home/tester")));

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule.id, "gradle-caches");
        assert_eq!(found[0].size, 40);
        assert_eq!(found[0].last_used, 400);
        assert_eq!(found[0].idle_for(1000), 600);
    }

    #[test]
    fn empty_caches_are_not_offered() {
        let tree = dir(
            "/home/tester",
            0,
            0,
            vec![dir("/home/tester/.ccache", 0, 0, vec![])],
        );
        assert!(
            find_reclaimable_from(&tree, SizeKind::Allocated, Some(Path::new("/home/tester")))
                .is_empty()
        );
    }

    #[test]
    fn grouping_totals_the_same_category_across_projects() {
        let rust = RULES.iter().find(|rule| rule.id == "rust-target").unwrap();
        let candidates = vec![
            Candidate {
                path: PathBuf::from("/a/target"),
                size: 300,
                rule: rust,
                last_used: 100,
            },
            Candidate {
                path: PathBuf::from("/b/target"),
                size: 200,
                rule: rust,
                last_used: 900,
            },
        ];

        let groups = group_by_rule(&candidates);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].size, 500);
        assert_eq!(groups[0].paths.len(), 2);
        // The category is as recently used as its most recently used member.
        assert_eq!(groups[0].last_used, 900);
    }
}
