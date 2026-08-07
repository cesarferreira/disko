.PHONY: all build build-release install install-release clean test check fmt lint run demo release ensure-git-cliff

LEVEL ?= minor
GIT_CLIFF_VERSION ?= 2.13.1

# Default target
all: check build test

# Build debug version
build:
	cargo build

# Build release version
build-release:
	cargo build --release

# Install debug binary to ~/.cargo/bin
install:
	CARGO_INCREMENTAL=0 cargo install --path . --locked --bins --debug --force

# Install release binary to ~/.cargo/bin
install-release:
	CARGO_INCREMENTAL=0 cargo install --path . --locked --bins --force

# Clean build artifacts
clean:
	cargo clean

# Run tests
#
# nextest runs the whole workspace in one parallel pool instead of one test
# binary after another, and names the slow ones on the way past. It does not
# run doctests at all, so those still go through cargo — the two together cover
# what `cargo test --workspace` covers on its own.
#
# It is not part of a default toolchain, so fall back rather than failing on a
# machine that has not got it. Installing it takes minutes; `make test` is not
# the place to spring that on anyone.
test:
	@if command -v cargo-nextest >/dev/null 2>&1; then \
		cargo nextest run --workspace && cargo test --workspace --doc; \
	else \
		echo "cargo-nextest not found; using cargo test. Install it with:"; \
		echo "    cargo install cargo-nextest --locked"; \
		cargo test --workspace; \
	fi

# Type-check and lint
#
# Clippy only: it runs the same front end `cargo check` does and adds its lints
# on top, so checking first compiles everything twice to learn the same thing.
check:
	cargo clippy --workspace --all-targets -- -D warnings

# Format code
fmt:
	cargo fmt --all

# Lint (check formatting)
lint:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings

# Run with arguments (usage: make run ARGS="~/ --top 10")
run:
	cargo run -- $(ARGS)

# Quick demo
demo: install
	@echo "=== disko demo ==="
	disko --help

# Bump version, regenerate CHANGELOG.md, tag, publish workspace crates, and push
# (requires cargo-release). `--workspace` publishes disko-core and disko-render
# before disko-cli, whose published manifest depends on both.
release: ensure-git-cliff
	cargo release $(LEVEL) --workspace --execute --no-confirm

# Ensure git-cliff (changelog generator, used by cargo-release's pre-release hook)
# is available via cargo, so releasing does not depend on an OS-level install.
ensure-git-cliff:
	@command -v git-cliff >/dev/null 2>&1 || { \
		echo "git-cliff not found; installing v$(GIT_CLIFF_VERSION) via cargo..."; \
		cargo install git-cliff --version $(GIT_CLIFF_VERSION) --locked; \
	}
