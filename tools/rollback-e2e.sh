#!/usr/bin/env bash
# Drive the recovery path with the real binary (spec 09 §3, roadmap 0.2.8).
#
# The apply pipeline's verify→rollback is unit-tested against a StubRunner, but
# that proves the *engine* rolls back. This drives the shipped binary against a
# fixture Omarchy with real file writes and a real snapshot store.
#
# IMPORTANT — what this does and does not cover. Writing this test is how we
# found that `Pipeline` (drift → pre → write → reload → verify → rollback) is
# used by exactly one feature, `rice import`. Every everyday path — looknfeel,
# keybinds, waybar, mako, tweaks, monitors — saves and reloads directly: it
# snapshots before and after, but never runs `hyprctl configerrors` and never
# auto-reverts. So on those paths the real safety net is the snapshot store
# plus `snapshot undo`, and that is what this script asserts. Automatic
# verify→rollback everywhere is tracked separately on the roadmap; when a path
# gains it, add the injected-configerrors case here.
#
# A fake `hyprctl` stands in for Hyprland, which CI does not have.
#
#   tools/rollback-e2e.sh [path-to-binary]
set -uo pipefail

cd "$(dirname "$0")/.."
BIN="${1:-./target/release/omarchy-studio}"
[ -x "$BIN" ] || { echo "no binary at $BIN — cargo build --release first" >&2; exit 1; }
BIN=$(realpath "$BIN")

ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

# shellcheck source=tools/fixture.sh
source tools/fixture.sh
studio_fixture "$ROOT"

# ── the fake Hyprland. Only the calls this path makes need to work.
FAKEBIN="$ROOT/bin"
mkdir -p "$FAKEBIN"
cat > "$FAKEBIN/hyprctl" <<'SH'
#!/usr/bin/env bash
case "$1" in
    configerrors) if [ -n "${HYPRCTL_ERRORS:-}" ]; then echo "$HYPRCTL_ERRORS"; else echo "no errors"; fi ;;
    reload)  ;;                       # succeed silently
    version) echo "Hyprland 0.55.0" ;;
    binds)   echo "[]" ;;
    *)       ;;
esac
exit 0
SH
chmod +x "$FAKEBIN/hyprctl"

export PATH="$FAKEBIN:$PATH"
eval "export $(studio_fixture_env)"

CONF="$HOME_DIR/.config/hypr/looknfeel.conf"
fails=0
ok()   { echo "  ok    $1"; }
fail() { echo "  FAIL  $1"; fails=$((fails + 1)); }
gaps() { grep -o 'gaps_in = [0-9]*' "$CONF" 2>/dev/null | head -1; }

echo "==> an apply writes the value"
# Values must be schema-valid or the CLI rejects them before anything is
# written — an out-of-range value would make this pass without ever applying,
# which is exactly how the first version of this script fooled itself.
"$BIN" looknfeel set general.gaps_in 7 >/dev/null 2>&1
if [ "$(gaps)" = "gaps_in = 7" ]; then
    ok "the applied value is on disk"
else
    fail "the apply did not write the value (got '$(gaps)')"
fi

echo "==> a second apply replaces it, and is snapshotted"
"$BIN" looknfeel set general.gaps_in 9 >/dev/null 2>&1
if [ "$(gaps)" = "gaps_in = 9" ]; then
    ok "the second value is on disk"
else
    fail "the second apply did not take (got '$(gaps)')"
fi
if "$BIN" snapshot log 2>/dev/null | grep -qi "looknfeel"; then
    ok "the change is in the snapshot log"
else
    fail "nothing was recorded in the snapshot log"
    "$BIN" snapshot log 2>&1 | sed 's/^/  | /' | head -5
fi

echo "==> undo restores the previous value"
# This is the recovery users actually have on this path, so it is the thing
# worth proving end-to-end rather than in a stub.
if "$BIN" snapshot undo >/dev/null 2>&1; then
    ok "undo reports success"
else
    fail "undo failed"
fi
if [ "$(gaps)" = "gaps_in = 7" ]; then
    ok "the file is back to the previous value"
else
    fail "undo did not restore the file (got '$(gaps)')"
    sed 's/^/  | /' "$CONF" 2>/dev/null
fi

echo
if [ "$fails" -gt 0 ]; then
    echo "rollback-e2e: $fails check(s) failed"
    exit 1
fi
echo "rollback-e2e: all checks passed"
