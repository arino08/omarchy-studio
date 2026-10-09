//! Golden-file round-trip tests (spec 09 §2): every parser must reproduce
//! real Omarchy config bytes exactly. As new dialects land (hyprlang in
//! 0.2.1, JSONC in 0.4.1) their fixtures join `tests/fixtures/` and this file.
//!
//! Fixtures are verbatim copies of Omarchy 3.8.2 theme files, so a change in
//! the parser that would mangle a real user's config fails here first.

use std::path::PathBuf;

use studio_core::modules::color::Color;
use studio_core::modules::themes::{Palette, PALETTE_KEYS};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/themes")
}

fn each_colors_toml(mut f: impl FnMut(&str, &str)) {
    let dir = fixtures();
    let mut any = false;
    for entry in std::fs::read_dir(&dir).expect("fixtures dir") {
        let path = entry.unwrap().path();
        if path.to_string_lossy().ends_with(".colors.toml") {
            any = true;
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let content = std::fs::read_to_string(&path).unwrap();
            f(&name, &content);
        }
    }
    assert!(any, "no colors.toml fixtures found in {}", dir.display());
}

#[test]
fn colors_toml_round_trips_byte_identical() {
    each_colors_toml(|name, content| {
        let p = Palette::parse(content).unwrap_or_else(|e| panic!("{name}: parse failed: {e:?}"));
        assert_eq!(p.to_string(), content, "{name} did not round-trip");
    });
}

#[test]
fn every_fixture_defines_the_core_palette_keys() {
    each_colors_toml(|name, content| {
        let p = Palette::parse(content).unwrap();
        // Real Omarchy themes must at least define accent/background/foreground
        // and the 16 ANSI colors; the selection/cursor pair is optional.
        for key in PALETTE_KEYS {
            let optional = matches!(
                key,
                "cursor" | "selection_foreground" | "selection_background"
            );
            if !optional {
                assert!(
                    p.get(key).is_some(),
                    "{name}: expected key `{key}` to be a valid colour"
                );
            }
        }
    });
}

#[test]
fn editing_a_fixture_preserves_all_other_bytes() {
    each_colors_toml(|name, content| {
        let mut p = Palette::parse(content).unwrap();
        let before = p.to_string();
        // Flip the accent to a known value and back; the rest must be untouched.
        let original = p.get("accent").expect("accent present");
        p.set("accent", &Color::parse("#abcdef").unwrap());
        assert!(
            p.to_string().contains("#abcdef"),
            "{name}: edit not applied"
        );
        p.set("accent", &original);
        assert_eq!(
            p.to_string(),
            before,
            "{name}: round-trip through an edit changed unrelated bytes"
        );
    });
}

mod hyprlang {
    use std::path::PathBuf;
    use studio_core::configfs::hyprlang::HyprDoc;

    fn hypr_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hypr")
    }

    #[test]
    fn real_hypr_configs_round_trip_byte_identical() {
        let mut any = false;
        for entry in std::fs::read_dir(hypr_dir()).expect("hypr fixtures") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) == Some("conf") {
                any = true;
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                let content = std::fs::read_to_string(&path).unwrap();
                assert_eq!(
                    HyprDoc::parse(&content).to_string(),
                    content,
                    "{name} did not round-trip byte-for-byte"
                );
            }
        }
        assert!(any, "no hypr fixtures found");
    }

    #[test]
    fn parses_real_binds_and_looknfeel_values() {
        let media = std::fs::read_to_string(hypr_dir().join("bindings-media.conf")).unwrap();
        let binds = HyprDoc::parse(&media).binds();
        assert!(binds.len() >= 8, "expected the media key binds");
        assert!(binds.iter().all(|b| b.key.starts_with("bind")));

        let lnf = std::fs::read_to_string(hypr_dir().join("looknfeel.conf")).unwrap();
        let doc = HyprDoc::parse(&lnf);
        assert_eq!(doc.get("general.gaps_in").as_deref(), Some("5"));
        assert_eq!(doc.get("general.border_size").as_deref(), Some("2"));
        // dotted key inside a category
        assert_eq!(
            doc.get("general.col.active_border").as_deref(),
            Some("$activeBorderColor")
        );
    }

    #[test]
    fn editing_looknfeel_preserves_every_other_byte() {
        let lnf = std::fs::read_to_string(hypr_dir().join("looknfeel.conf")).unwrap();
        let mut doc = HyprDoc::parse(&lnf);
        assert!(doc.set("general.gaps_in", "8"));
        let out = doc.to_string();
        // exactly one line differs
        let diffs = lnf.lines().zip(out.lines()).filter(|(a, b)| a != b).count()
            + (lnf.lines().count() as i64 - out.lines().count() as i64).unsigned_abs() as usize;
        assert_eq!(diffs, 1, "only the edited line should change");
        assert!(out.contains("gaps_in = 8"));
    }
}

mod jsonc {
    use std::path::PathBuf;
    use studio_core::configfs::jsonc::JsoncDoc;

    fn waybar_config() -> String {
        let p =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/waybar/config.jsonc");
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn real_waybar_config_round_trips_byte_identical() {
        let src = waybar_config();
        let doc = JsoncDoc::parse(&src).expect("real waybar config should parse");
        assert_eq!(doc.to_string(), src, "waybar config.jsonc must round-trip");
    }

    #[test]
    fn reads_and_reorders_real_module_lanes() {
        let src = waybar_config();
        let mut doc = JsoncDoc::parse(&src).unwrap();
        // the three lanes exist and are non-empty
        for lane in ["modules-left", "modules-center", "modules-right"] {
            let items = doc.array_items(lane).unwrap_or_default();
            assert!(!items.is_empty(), "{lane} should have modules");
        }
        // reverse modules-left and confirm order flips while the rest is intact
        let mut left = doc.array_items("modules-left").unwrap();
        left.reverse();
        doc.reorder("modules-left", &left).unwrap();
        let out = doc.to_string();
        let re = JsoncDoc::parse(&out).unwrap();
        assert_eq!(re.array_items("modules-left").unwrap(), left);
        // an untouched lane is unchanged
        assert_eq!(
            re.array_items("modules-center").unwrap(),
            JsoncDoc::parse(&src)
                .unwrap()
                .array_items("modules-center")
                .unwrap()
        );
    }
}

/// The Lua emitter (Omarchy 4) must produce source the real Lua compiler
/// accepts. String-equality unit tests only prove the output matches what we
/// expected to write; these prove it actually parses.
mod lua {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use studio_core::configfs::lua::{config_call, named_table_call, table_call, Value};

    /// `luac -p` (syntax-check only). Returns None when no Lua toolchain is
    /// installed, so the suite still runs on a machine without one.
    fn syntax_check(source: &str) -> Option<Result<(), String>> {
        for bin in ["luac", "luac5.4", "luac5.3"] {
            let spawned = Command::new(bin)
                .args(["-p", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let Ok(mut child) = spawned else { continue };
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(source.as_bytes())
                .expect("write source");
            let out = child.wait_with_output().expect("luac run");
            return Some(if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).to_string())
            });
        }
        None
    }

    fn assert_valid_lua(source: &str) {
        match syntax_check(source) {
            None => eprintln!("skipping: no luac on PATH"),
            Some(Ok(())) => {}
            Some(Err(e)) => panic!("emitted invalid Lua:\n{source}\n\nluac said:\n{e}"),
        }
    }

    #[test]
    fn emitted_config_blocks_are_valid_lua() {
        let entries: Vec<(String, Value)> = vec![
            ("general.gaps_in".into(), Value::Int(6)),
            ("general.layout".into(), Value::Str("dwindle".into())),
            ("general.resize_on_border".into(), Value::Bool(true)),
            ("decoration.rounding".into(), Value::Int(8)),
            ("decoration.blur.enabled".into(), Value::Bool(true)),
            ("decoration.blur.size".into(), Value::Int(8)),
            ("decoration.dim_strength".into(), Value::Float(0.15)),
            // A key that isn't a bare Lua identifier must still compile.
            ("input.kb-layout".into(), Value::Str("us,dk".into())),
            // As must a string carrying quotes and backslashes.
            ("misc.note".into(), Value::Str("a\"b\\c".into())),
        ];
        assert_valid_lua(&config_call(&entries));
    }

    #[test]
    fn emitted_monitor_and_curve_calls_are_valid_lua() {
        let monitor = table_call(
            "hl.monitor",
            &[
                ("output", Value::Str("eDP-1".into())),
                ("mode", Value::Str("1920x1080@120.21".into())),
                ("position", Value::Str("0x0".into())),
                ("scale", Value::Float(1.5)),
                ("transform", Value::Int(0)),
            ],
        );
        let curve = named_table_call(
            "hl.curve",
            "easeOutQuint",
            &[
                ("type", Value::Str("bezier".into())),
                (
                    "points",
                    Value::List(vec![
                        Value::List(vec![Value::Float(0.23), Value::Int(1)]),
                        Value::List(vec![Value::Float(0.32), Value::Int(1)]),
                    ]),
                ),
            ],
        );
        let animation = table_call(
            "hl.animation",
            &[
                ("leaf", Value::Str("windows".into())),
                ("enabled", Value::Bool(true)),
                ("speed", Value::Float(3.79)),
                ("bezier", Value::Str("easeOutQuint".into())),
                ("style", Value::Str("popin 87%".into())),
            ],
        );
        assert_valid_lua(&format!("{monitor}\n{curve}\n{animation}\n"));
    }
}
