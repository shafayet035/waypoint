//! End-to-end tests of the `wp` binary, the way an agent uses it.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn wp(dir: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_wp"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run wp");
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

fn json(dir: &Path, args: &[&str]) -> (i32, Value) {
    let mut all = args.to_vec();
    all.push("--json");
    let out = Command::new(env!("CARGO_BIN_EXE_wp"))
        .current_dir(dir)
        .args(&all)
        .output()
        .expect("run wp");
    let v = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)));
    (out.status.code().unwrap_or(-1), v)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wp-cli-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn new_check_test_loop() {
    let dir = temp_dir("loop");
    let (code, out) = wp(&dir, &["new", "my_bus_game"]);
    assert_eq!(code, 0, "{out}");
    let game = dir.join("my_bus_game");
    assert!(
        std::fs::read_to_string(game.join("game.toml"))
            .unwrap()
            .contains("name = \"my_bus_game\"")
    );

    let (code, v) = json(&game, &["check"]);
    assert_eq!(code, 0);
    assert_eq!(v["ok"], true);
    assert_eq!(v["errors"], 0);

    let (code, v) = json(&game, &["test"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["passed"], true);
    assert_eq!(v["total"], 2);

    // Creating it again must fail with a coded error.
    let (code, v) = json(&dir, &["new", "my_bus_game"]);
    assert_eq!(code, 1);
    assert_eq!(v["diagnostics"][0]["code"], "WP-C004");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_reports_coded_errors_with_locations() {
    let dir = temp_dir("broken");
    wp(&dir, &["new", "g"]);
    let game = dir.join("g");
    let vehicle = game.join("vehicles/hatchback.vehicle.ron");
    let src = std::fs::read_to_string(&vehicle)
        .unwrap()
        .replace("mass_kg: 1250.0", "mass_kg: -5.0");
    std::fs::write(&vehicle, src).unwrap();

    let (code, v) = json(&game, &["check"]);
    assert_eq!(code, 1);
    let d = &v["diagnostics"][0];
    assert_eq!(d["code"], "WP-V001");
    assert_eq!(d["location"]["file"], "vehicles/hatchback.vehicle.ron");
    assert!(d["location"]["line"].as_u64().unwrap() > 1);

    // Tests refuse to run on a broken project.
    let (code, _) = json(&game, &["test"]);
    assert_eq!(code, 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn headless_run_is_deterministic() {
    let sandbox = repo().join("examples/sandbox");
    let args = [
        "run",
        "--headless",
        "--seconds",
        "3",
        "--throttle",
        "0.8",
        "--steer",
        "0.2",
        "--seed",
        "4",
    ];
    let (c1, a) = json(&sandbox, &args);
    let (c2, b) = json(&sandbox, &args);
    assert_eq!((c1, c2), (0, 0));
    assert_eq!(a["state_hash"], b["state_hash"]);
    assert_eq!(a["ticks"], 180);
    assert!(a["telemetry"]["speed_kmh"].as_f64().unwrap() > 10.0);
}

#[test]
fn windowed_run_explains_itself() {
    let (code, v) = json(&repo().join("examples/sandbox"), &["run"]);
    assert_eq!(code, 1);
    assert_eq!(v["diagnostics"][0]["code"], "WP-C002");
}

#[test]
fn every_error_code_in_the_source_is_documented() {
    let (_, docs) = json(&repo(), &["docs", "errors"]);
    let documented: Vec<String> = docs
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_str().unwrap().to_string())
        .collect();
    let mut missing = Vec::new();
    for file in rust_files(&repo().join("crates")) {
        let text = std::fs::read_to_string(&file).unwrap();
        for (i, _) in text.match_indices("\"WP-") {
            let code = &text[i + 1..i + 8];
            let valid = code.len() == 7
                && code[3..4].chars().all(|c| c.is_ascii_uppercase())
                && code[4..].chars().all(|c| c.is_ascii_digit());
            if valid
                && !documented.iter().any(|d| d == code)
                && !missing.contains(&code.to_string())
            {
                missing.push(code.to_string());
            }
        }
    }
    assert!(
        missing.is_empty(),
        "codes used but not in wp_cli/src/catalog.rs: {missing:?}"
    );
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(rust_files(&p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    out
}

#[test]
fn schemas_are_valid_json() {
    for kind in ["game", "vehicle", "scene", "scenario"] {
        let (code, text) = wp(&repo(), &["schema", kind]);
        assert_eq!(code, 0);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(
            v["properties"].is_object() || v["oneOf"].is_array(),
            "{kind}: {v}"
        );
    }
}
