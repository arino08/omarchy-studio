#!/bin/sh
set -e

# omarchy-studio quick installer
# Fetches the latest release binary from GitHub and installs it over the
# omarchy-studio already on PATH (so updates land where your shell actually
# looks), or to ~/.local/bin on a fresh machine.

REPO="arino08/omarchy-studio"
BIN_NAME="omarchy-studio"
ASSET_NAME="omarchy-studio-linux-x86_64"

# Install over the existing binary if one is on PATH — installing to a
# second location just gets shadowed and looks like the update "didn't take".
EXISTING=$(command -v "$BIN_NAME" 2>/dev/null || true)
if [ -n "$EXISTING" ]; then
    INSTALL_DIR=$(dirname "$EXISTING")
else
    INSTALL_DIR="$HOME/.local/bin"
fi

echo "=> Fetching latest release of $BIN_NAME..."

# Use GitHub API to get the latest release URLs (the binary and its .sha256)
RELEASE_JSON=$(curl -s "https://api.github.com/repos/$REPO/releases/latest" || true)
LATEST_URL=$(printf '%s\n' "$RELEASE_JSON" | grep "browser_download_url.*/$ASSET_NAME\"" | cut -d '"' -f 4 || true)
SUM_URL=$(printf '%s\n' "$RELEASE_JSON" | grep "browser_download_url.*/$ASSET_NAME\.sha256\"" | cut -d '"' -f 4 || true)

if [ -n "$LATEST_URL" ]; then
    echo "=> Downloading pre-built binary from $LATEST_URL..."
    mkdir -p "$INSTALL_DIR"
    TMP=$(mktemp "${TMPDIR:-/tmp}/$BIN_NAME.XXXXXX")
    trap 'rm -f "$TMP"' EXIT
    curl -fsSL "$LATEST_URL" -o "$TMP"
    # Never install something that isn't the binary (a rate-limited API
    # response or an error page must not end up on PATH).
    if [ "$(head -c 4 "$TMP")" != "$(printf '\177ELF')" ] || [ "$(wc -c <"$TMP")" -lt 1048576 ]; then
        echo "Error: download was not the release binary (rate-limited or network trouble?)." >&2
        echo "Try again in a minute, or download it yourself from:" >&2
        echo "  $LATEST_URL" >&2
        exit 1
    fi
    # Verify against the release's published SHA-256. Releases before the
    # checksum landed (<= v0.9.3) have none; everything after does, and a
    # mismatch is never installed.
    if [ -n "$SUM_URL" ]; then
        EXPECTED=$(curl -fsSL "$SUM_URL" | cut -d ' ' -f 1)
        ACTUAL=$(sha256sum "$TMP" | cut -d ' ' -f 1)
        if [ -z "$EXPECTED" ] || [ "$EXPECTED" != "$ACTUAL" ]; then
            echo "Error: checksum mismatch — the download is corrupt or was tampered with." >&2
            echo "  expected: ${EXPECTED:-<unreadable>}" >&2
            echo "  got:      $ACTUAL" >&2
            exit 1
        fi
        echo "=> Checksum verified ($ACTUAL)"
    else
        echo "=> Warning: this release publishes no checksum; skipping verification." >&2
    fi
    chmod +x "$TMP"
    mv "$TMP" "$INSTALL_DIR/$BIN_NAME"
    trap - EXIT
    echo "=> Success! Installed $("$INSTALL_DIR/$BIN_NAME" --version) to $INSTALL_DIR/$BIN_NAME"
else
    echo "=> No pre-built binary found for the latest release."
    if command -v cargo >/dev/null 2>&1; then
        echo "=> Rust toolchain detected. Building and updating from source..."
        cargo install --git https://github.com/$REPO.git $BIN_NAME
        INSTALL_DIR="$HOME/.cargo/bin"
        echo "=> Success! $BIN_NAME installed/updated via cargo."
    else
        echo "Error: No pre-built binary available, and Rust (cargo) is not installed."
        echo "Please install Rust from https://rustup.rs and try again."
        exit 1
    fi
fi

# Check if INSTALL_DIR is in PATH
if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
    echo ""
    echo "Note: $INSTALL_DIR is not in your PATH."
    echo "Add the following line to your ~/.bashrc or ~/.zshrc:"
    echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
fi

# A different copy earlier in PATH would shadow what we just installed.
RESOLVED=$(command -v "$BIN_NAME" 2>/dev/null || true)
if [ -n "$RESOLVED" ] && [ "$RESOLVED" != "$INSTALL_DIR/$BIN_NAME" ]; then
    echo ""
    echo "Warning: your shell resolves $BIN_NAME to $RESOLVED,"
    echo "which shadows the copy just installed. Remove or update that one:"
    echo "  rm \"$RESOLVED\"   # then re-run your shell"
fi

echo ""
echo "Run '$BIN_NAME' to start."
