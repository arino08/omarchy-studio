#!/usr/bin/env bash
# Capture every rail screen and tile them into one contact sheet (1.0.6).
#
# Runs against the same fixture Omarchy as tools/e2e.sh (scratch HOME +
# OMARCHY_PATH), so it needs no Omarchy, Hyprland or Wayland and is safe in CI.
#
# Two jobs in one pass:
#   1. a *check* — every screen must draw its own panel title. A screen that
#      panics, hangs or renders empty fails the run, which unit tests can't
#      catch and the e2e journeys only check for two screens.
#   2. an *artifact* — docs/assets/screens.png, all 16 screens at a glance, so
#      README media can be regenerated instead of quietly going stale.
#
#   tools/screenshot-grid.sh [path-to-binary] [out.png]
set -uo pipefail

cd "$(dirname "$0")/.."
BIN="${1:-./target/release/omarchy-studio}"
OUT="${2:-docs/assets/screens.png}"
[ -x "$BIN" ] || { echo "no binary at $BIN — cargo build --release first" >&2; exit 1; }
BIN=$(realpath "$BIN")
command -v tmux >/dev/null || { echo "tmux not installed" >&2; exit 1; }

ROOT=$(mktemp -d)
SESSION="studio-shots-$$"
FRAMES="$ROOT/frames"
mkdir -p "$FRAMES"
trap 'tmux kill-session -t "$SESSION" 2>/dev/null; rm -rf "$ROOT"' EXIT

# shellcheck source=tools/fixture.sh
source tools/fixture.sh
studio_fixture "$ROOT"

# The rail order from tui::Screen::ALL, by the panel title each screen draws.
# Keep in sync with that array — a mismatch here is exactly the drift this
# script exists to catch.
SCREENS=(
    "Themes" "Wallpaper" "Keybinds" "Look & Feel" "Animations" "Waybar"
    "Notifs / OSD" "Lock & Idle" "Snapshots" "Integrations" "Apps"
    "Monitors" "Tweaks" "Power" "Nice Launcher" "Niri Mode" "Doctor"
)

# termshot panics deep in the rasterizer on a missing font; say so up front.
for var in TERMSHOT_FONT TERMSHOT_FONT_BOLD; do
    path=${!var:-}
    if [ -n "$path" ] && [ ! -r "$path" ]; then
        echo "$var points at $path, which does not exist" >&2
        exit 1
    fi
done

echo "==> building termshot"
(cd tools/termshot && cargo build --release) >/dev/null || exit 1
SHOT=tools/termshot/target/release/termshot

echo "==> launching the TUI in a pty"
tmux new-session -d -s "$SESSION" -x 110 -y 32 \
    "env $(studio_fixture_env) '$BIN'"
sleep 3

# A fresh fixture opens on the first-run on-ramp, which owns input until
# dismissed — skip it so the rail screens are what gets captured.
tmux send-keys -t "$SESSION" d; sleep 1

fails=0
args=()
for i in "${!SCREENS[@]}"; do
    label="${SCREENS[$i]}"
    # Screens load lazily (theme lists, pacman probes, hyprctl queries); give
    # each one a beat to settle before the shutter.
    [ "$i" -gt 0 ] && { tmux send-keys -t "$SESSION" Down; sleep 1.2; }
    file="$FRAMES/$(printf '%02d' "$((i + 1))")-$(echo "$label" | tr -cd '[:alnum:]').txt"
    # `-e` keeps the colors termshot rasterizes; the plain twin is what gets
    # asserted, since escape sequences sit between the border and the title.
    tmux capture-pane -ep -t "$SESSION" > "$file"
    tmux capture-pane -p -t "$SESSION" > "$file.plain"

    # The panel title only reads `╮ <label>` once that screen actually drew.
    if grep -qF -- "╮ $label" "$file.plain"; then
        echo "  ok    $label"
    else
        echo "  FAIL  $label did not draw its panel"
        sed 's/^/  | /' "$file.plain"
        fails=$((fails + 1))
    fi
    args+=("$file")
done

# SCREENS is a hand-kept copy of tui::Screen::ALL. A *removed* screen fails
# above, but an added one would just never be visited. The rail wraps, so from
# the last screen one more Down must land back on the first — if the rail grew
# a screen we didn't list, we land on that instead.
tmux send-keys -t "$SESSION" Down; sleep 1.2
first="${SCREENS[0]}"
if tmux capture-pane -p -t "$SESSION" | grep -qF -- "╮ $first"; then
    echo "  ok    the rail wraps back to '$first' — no unlisted screens"
else
    echo "  FAIL  the rail has a screen past '${SCREENS[-1]}' — add it to SCREENS"
    fails=$((fails + 1))
fi

tmux send-keys -t "$SESSION" q; sleep 1
tmux kill-session -t "$SESSION" 2>/dev/null

if [ "$fails" -gt 0 ]; then
    echo
    echo "screenshot-grid: $fails screen(s) failed to draw"
    exit 1
fi

echo "==> rendering the grid"
mkdir -p "$(dirname "$OUT")"
# Fixture theme is nord, so match the margins to it.
"$SHOT" --cols 110 --rows 32 --px 14 --bg '#2e3440' --fg '#d8dee9' \
    --out "$FRAMES/png" --grid "$OUT" --grid-cols 4 "${args[@]}" >/dev/null || exit 1

echo "screenshot-grid: all ${#SCREENS[@]} screens drew → $OUT"
