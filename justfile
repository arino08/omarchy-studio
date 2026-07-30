# omarchy-studio — one-stop Omarchy configuration TUI/CLI

# Show available recipes
default:
    @just --list --unsorted

# Run the app (pass args after --, e.g. `just run doctor`)
run *ARGS:
    cargo run -- {{ARGS}}

# Build debug
build:
    cargo build

# Build release binary
build-release:
    cargo build --release

# Fast type-check
check:
    cargo check

# Lint with clippy (warnings are errors)
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Format code
fmt:
    cargo fmt

# Verify formatting without writing
fmt-check:
    cargo fmt -- --check

# Clippy + format check
lint: clippy fmt-check

# Run tests
test:
    cargo test

# Run tests with stdout captured output
test-verbose:
    cargo test -- --nocapture

# Build and open API docs
doc:
    cargo doc --no-deps --open

# Create .crate in target/package for inspection (no upload)
package:
    cargo package

# Release quality gate (fmt + clippy + test)
release-check:
    cargo fmt -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test

# Everything CI runs
ci: release-check build

# Preview what a release would do without changing anything
release-dry-run LEVEL:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! "{{LEVEL}}" =~ ^(patch|minor|major)$ ]]; then
        echo "Usage: just release-dry-run patch|minor|major"; exit 1
    fi
    CURRENT=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
    echo "Current version: $CURRENT"
    echo "Bump level: {{LEVEL}}"
    just release-check
    echo ""
    echo "All checks passed. Run: just release {{LEVEL}}"

# Bump version, create release branch + PR (requires: cargo-set-version, gh)
release LEVEL: release-check
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! "{{LEVEL}}" =~ ^(patch|minor|major)$ ]]; then
        echo "Usage: just release patch|minor|major"; exit 1
    fi
    if [[ -n "$(git status --porcelain)" ]]; then
        echo "Error: dirty working tree"; exit 1
    fi
    BRANCH=$(git rev-parse --abbrev-ref HEAD)
    if [[ "$BRANCH" != "main" ]]; then
        echo "Error: must be on main (currently on $BRANCH)"; exit 1
    fi
    git pull --ff-only origin main
    cargo set-version --bump {{LEVEL}} --workspace
    cargo check --quiet
    VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
    git checkout -b "release/v${VERSION}"
    git add Cargo.toml Cargo.lock
    git commit -m "release: v${VERSION}"
    git push -u origin "release/v${VERSION}"
    gh pr create \
        --title "release: v${VERSION}" \
        --body "Bump to v${VERSION} ({{LEVEL}} release)" \
        --base main
    echo ""
    echo "PR created. Next steps:"
    echo "  gh pr checks           # watch CI"
    echo "  gh pr merge --squash --delete-branch"
    echo "  gh run watch           # watch release workflow"
