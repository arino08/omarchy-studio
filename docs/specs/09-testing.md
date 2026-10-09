# Spec 09 — Testing & Quality

The product's entire value proposition rests on *trust* (edits never lose user content; applies never brick the desktop). Test spend follows that.

## 1. Unit & property tests (`studio-core`, pure — no Omarchy needed)

- Parsers: hyprlang CST, INI, managed-block editors — table-driven cases + **round-trip invariant** `to_string(parse(s)) == s` property-tested with generated mutations (blank lines, weird spacing, unicode comments).
- Conflict detection, modmask rendering, palette math (WCAG, HSL transforms), preset → keyword translation: pure-function tables.
- Extraction: golden palettes for a fixed image corpus (`tests/fixtures/images/`), compared against Aether's outputs for the same inputs (tolerance ±2 per channel) — proves the port.

## 2. Golden-file corpus (the big one)

`tests/fixtures/omarchy-3.8.2/` — a **vendored snapshot** of the reference machine's relevant files (sanitized): all `default/hypr/*.conf`, stock user hypr confs, `waybar/config.jsonc` + `style.css`, built-in `themed/*.tpl`, three theme dirs (dark, light, overlay).

Lua emission (Omarchy 4) is covered differently: rather than vendoring a fixture, every emitted block is compiled by the real `luac -p` in the golden suite, and the reader is asserted to be the emitter's exact inverse. Call forms and field names (`hl.monitor`'s `disabled`, `hl.dsp.layout`, `require` in place of `source =`) were established by probing the running compositor, which validates its tables and names the offending field. Tests:

- Every parser round-trips every fixture byte-identically.
- Every write operation on fixtures produces snapshot-reviewed output (`insta` crate): "add bind", "set blur.size", "reorder waybar lane", "materialize mako tpl" each have expected-output files a human reviewed once.
- **Corpus refresh job** (CI, weekly): clone omarchy master, re-run parser round-trips against *its* defaults — early warning when upstream changes file shapes (PRD risk #1). Failures open an issue, not a red main build.

## 3. Integration tests (need a live-ish environment)

`tests/it/` behind `--features it`, run in the Arch container CI job (omarchy bins vendored as stubs recording invocations):

- Apply pipeline end-to-end against a fake `$HOME`: snapshot → write → (stub) reload → verify → rollback path (inject configerrors output).
- Menu-block install idempotence: run install 3×, byte-identical result; foreign-content preservation; uninstall exactness.
- Doctor scenarios: `.bak.<epoch>` clobber detection, drift commits, dirty-`$OMARCHY_PATH` flag.

## 4. TUI e2e (dogfood scripts, PRD §14)

tmux-driven: spawn `omarchy-studio` in tmux, send keys, assert on captured pane text (`tmux capture-pane`). One script per persona journey:
1. fork theme → edit accent → apply → undo;
2. rebind SUPER+B with conflict → override → verify effective via stubbed `hyprctl binds -j`;
3. animation preset try→apply→undo.
Plus the screenshot grid: render the shell in all 19 stock palettes → PNG artifacts for eyeball review on PRs that touch styling.

## 5. CI (GitHub Actions, archlinux container)

| Job | When |
|---|---|
| fmt + clippy (deny warnings) + `cargo check` all features | every push |
| unit + golden + insta | every push |
| integration (`--features it`) | every push |
| tmux e2e + screenshot grid | PRs to main |
| corpus refresh vs omarchy master | weekly cron |
| release build (musl static? evaluate — glibc fine for AUR) + size gate < 15 MB | tags |

## 6. Manual test protocol (pre-release, on the reference machine)

Checklist doc generated per release: theme apply across all 19 themes, keybind override survives `omarchy-refresh-hyprland` recovery flow, waybar crash-rollback (inject bad module), mako tpl survives `omarchy-theme-set` + `omarchy-update`, uninstall leaves a working desktop. Run on real Omarchy before every AUR release — no exceptions.
