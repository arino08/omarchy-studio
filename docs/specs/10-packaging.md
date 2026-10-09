# Spec 10 — Packaging, Release, First-Run

## 1. Artifacts

- **AUR `omarchy-studio`** (build from tag) and **`omarchy-studio-bin`** (prebuilt from GitHub Releases). PKGBUILDs live in `pkg/aur/` here, pushed to AUR on release.
- Release binary: `cargo build --release`, stripped, glibc (Omarchy is Arch-only — no musl need), `< 15 MB` gate.
- `data/` (schemas, presets, integrations, elephant lua, templates) is embedded via `include_str!`/`include_bytes!` — the binary is the whole install except the hook/menu/provider files it writes at `install-integration` time.

## 2. Versioning & compatibility

- SemVer; `omarchy-studio --version` prints its own version + the tested-against Omarchy versions from `studio_core::TESTED_OMARCHY` (currently `3.8 or 4.0` — both config dialects).
- Capability probe (spec 02 §5) handles skew at runtime; releases bump the tested range only after the corpus job passes against that Omarchy tag.
- CHANGELOG.md, keep-a-changelog format; every entry states user-visible behavior, files touched, and migration notes if managed-block formats changed (blocks carry a `v=` in the marker; Studio migrates its own blocks forward automatically).

## 3. First-run wizard (FR in M10/M11)

Order: welcome → capability probe summary → dependency summary (guided installs, spec 06 §3.5) → snapshot 0 creation ("this is your pre-Studio state — restorable forever") → menu integration opt-in (FR10.1: menu block + elephant provider + hooks) → optional keybind `SUPER+CTRL+S` (conflict-checked by M3) → done, land on Themes.

Every step skippable; wizard re-runnable via `omarchy-studio install-integration`.

## 4. Uninstall (FR10.6)

`omarchy-studio uninstall`: read `installed.toml` manifest → remove menu block (file too if we created it and it's empty), hooks, elephant provider, keybind line → offer restore-all-from-snapshot-0 → print what was left behind (user configs — valid standalone — and the history repo path, with `rm -rf` one-liner if they want it gone). Then `pacman -R` instructions.

## 5. Docs to ship at v1.0

README quickstart · "What file did that touch?" reference table (generated from the schema `target`s — same no-drift trick as the deps matrix) · theme-crafting tutorial (wallhaven → wizard → export) · CONTRIBUTING.md (spec pointers, golden-test workflow, integration-registry PR guide).
