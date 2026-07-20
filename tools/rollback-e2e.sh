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

echo "==> a rejected first-ever apply leaves no file behind"
# The nastiest case: the file does not exist yet, so the pre-snapshot has
# nothing to restore. Rolling back has to *delete* it, or a rejected config
# survives while we report a successful rollback.
HYPRCTL_ERRORS="error: invalid value at looknfeel.conf:1" \
    "$BIN" looknfeel set general.gaps_in 6 >/dev/null 2>&1
if [ ! -f "$CONF" ]; then
    ok "the created file was removed by the rollback"
else
    fail "a rejected first apply left the file behind"
    sed 's/^/  | /' "$CONF"
fi

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
# Deliberately before the rollback case: a rollback records its own snapshots,
# so running undo afterwards would target those instead of this apply.
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

echo "==> a config Hyprland rejects is rolled back automatically"
# looknfeel applies through the pipeline (E1), so a failing `configerrors`
# reverts the write with no user action. The value must be schema-valid (5 is
# in range) or the CLI rejects it *before* the pipeline and this passes without
# ever applying — exactly how the first draft of this script fooled itself.
before=$(gaps)
HYPRCTL_ERRORS="error: invalid value at looknfeel.conf:1" \
    "$BIN" looknfeel set general.gaps_in 5 >/dev/null 2>&1
if [ "$?" -ne 0 ]; then
    ok "the rejected apply exits nonzero"
else
    fail "a rejected config still exited 0"
fi
if [ "$(gaps)" = "$before" ]; then
    ok "the file was rolled back to '$before'"
else
    fail "the rejected value survived (now '$(gaps)', was '$before')"
fi

echo "==> keybinds roll back too (E1)"
BINDS="$HOME_DIR/.config/hypr/bindings.conf"
"$BIN" keybind add "SUPER+SHIFT+T" exec kitty >/dev/null 2>&1
if grep -q "SUPER SHIFT, T" "$BINDS" 2>/dev/null; then
    ok "the bind is on disk"
else
    fail "the bind was not written"
fi
HYPRCTL_ERRORS="error: invalid bind at bindings.conf:1" \
    "$BIN" keybind add "SUPER+SHIFT+Y" exec firefox >/dev/null 2>&1
if grep -q "SUPER SHIFT, Y" "$BINDS" 2>/dev/null; then
    fail "the rejected bind survived"
    sed 's/^/  | /' "$BINDS"
else
    ok "the rejected bind was rolled back"
fi
if grep -q "SUPER SHIFT, T" "$BINDS" 2>/dev/null; then
    ok "the earlier bind survived the rollback"
else
    fail "the rollback lost the earlier bind"
fi

echo "==> tweaks roll back too (E1)"
INPUT="$HOME_DIR/.config/hypr/input.conf"
"$BIN" tweak caps-escape on >/dev/null 2>&1
if grep -q "caps:escape" "$INPUT" 2>/dev/null; then
    ok "the tweak is on disk"
else
    fail "the tweak was not written"
fi
# A rejected tweak must come back off, leaving no half-applied block.
HYPRCTL_ERRORS="error: invalid input at input.conf:1" \
    "$BIN" tweak inactive-transparency on >/dev/null 2>&1
if grep -q "tweak-transparency" "$HOME_DIR/.config/hypr/looknfeel.conf" 2>/dev/null; then
    fail "the rejected tweak survived"
else
    ok "the rejected tweak was rolled back"
fi
if grep -q "caps:escape" "$INPUT" 2>/dev/null; then
    ok "the earlier tweak survived the rollback"
else
    fail "the rollback lost the earlier tweak"
fi

echo
if [ "$fails" -gt 0 ]; then
    echo "rollback-e2e: $fails check(s) failed"
    exit 1
fi
echo "rollback-e2e: all checks passed"
