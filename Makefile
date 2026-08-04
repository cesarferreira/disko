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
test:
	cargo test --workspace

# Run clippy and check
check:
	cargo check --workspace --all-targets
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
