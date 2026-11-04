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
    assert_eq!(v["total"], 14);

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
fn shot_writes_a_png_of_a_scenario() {
    let dir = temp_dir("shot");
    let out = dir.join("frame.png");
    let sandbox = repo().join("examples/sandbox");
    let (code, v) = json(
        &sandbox,
        &[
            "shot",
            "--scenario",
            "skidpad",
            "--at",
            "5s",
            "--camera",
            "showcase",
            "--width",
            "320",
            "--height",
            "180",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    if code == 1 && v["diagnostics"][0]["code"] == "WP-R001" {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    }
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["at_s"], 5.0);
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[1..4], b"PNG");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Send JSON lines to a long-running `wp` subcommand and collect the responses.
fn converse(dir: &Path, args: &[&str], requests: &[Value]) -> Vec<Value> {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_wp"))
        .current_dir(dir)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for r in requests {
            writeln!(stdin, "{r}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn serve_speaks_json_rpc_and_replays_snapshots() {
    let req = |id: u64, method: &str, params: Value| serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    let r = converse(
        &repo().join("examples/sandbox"),
        &["serve"],
        &[
            req(
                1,
                "input.set",
                serde_json::json!({ "throttle": 1.0, "steer": 0.2 }),
            ),
            req(2, "sim.snapshot", serde_json::json!({})),
            req(3, "sim.step", serde_json::json!({ "seconds": 2 })),
            req(4, "sim.restore", serde_json::json!({ "id": 0 })),
            req(5, "sim.step", serde_json::json!({ "seconds": 2 })),
            serde_json::json!({ "jsonrpc": "2.0", "method": "sim.step" }),
            req(6, "rpc.methods", serde_json::json!({})),
            req(7, "world.get", serde_json::json!({ "entity": "42v0" })),
        ],
    );
    assert_eq!(r.len(), 7, "the notification must not get a reply");
    assert_eq!(r[2]["result"]["state_hash"], r[4]["result"]["state_hash"]);
    assert_eq!(r[2]["result"]["tick"], 120);
    assert!(r[5]["result"].as_array().unwrap().len() >= 15);
    assert_eq!(r[6]["error"]["data"]["code"], "WP-K001");
}

#[test]
fn mcp_lists_tools_and_runs_check() {
    let r = converse(
        &repo().join("examples/sandbox"),
        &["mcp"],
        &[
            serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } } }),
            serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "check", "arguments": {} } }),
            serde_json::json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "session_start", "arguments": { "throttle": 1.0 } } }),
            serde_json::json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "session_call", "arguments": { "method": "sim.step", "params": { "ticks": 60 } } } }),
        ],
    );
    assert_eq!(r.len(), 5);
    assert_eq!(r[0]["result"]["protocolVersion"], "2025-06-18");
    let tools: Vec<&str> = r[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(tools.contains(&"screenshot") && tools.contains(&"session_call"));
    assert_eq!(r[2]["result"]["isError"], false);
    let status: Value =
        serde_json::from_str(r[4]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(status["tick"], 60);
}

#[test]
fn bad_camera_is_a_coded_error() {
    let (code, v) = json(
        &repo().join("examples/sandbox"),
        &["shot", "--camera", "drone"],
    );
    assert_eq!(code, 1);
    assert_eq!(v["diagnostics"][0]["code"], "WP-C001");
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

/// A 3 × 3 grid of two-way streets about 110 m apart, as OSM XML, with one
/// node outside the bounds (clipped) and a footway (ignored).
fn grid_osm() -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?>\n<osm version=\"0.6\">\n\
         <bounds minlat=\"23.7995\" minlon=\"90.3995\" maxlat=\"23.8025\" maxlon=\"90.4025\"/>\n",
    );
    for r in 0..3 {
        for c in 0..3 {
            xml += &format!(
                "<node id=\"{}\" lat=\"{}\" lon=\"{}\"/>\n",
                10 + r * 3 + c,
                23.800 + r as f64 * 0.001,
                90.400 + c as f64 * 0.001
            );
        }
    }
    xml += "<node id=\"99\" lat=\"23.800\" lon=\"90.410\"/>\n";
    for r in 0..3 {
        let ids: Vec<i32> = (0..3).map(|c| 10 + r * 3 + c).collect();
        let extra = if r == 0 { "<nd ref=\"99\"/>" } else { "" };
        xml += &format!(
            "<way id=\"{}\"><nd ref=\"{}\"/><nd ref=\"{}\"/><nd ref=\"{}\"/>{extra}\
             <tag k=\"highway\" v=\"residential\"/><tag k=\"name\" v=\"Row {r}\"/></way>\n",
            100 + r,
            ids[0],
            ids[1],
            ids[2]
        );
    }
    for c in 0..3 {
        xml += &format!(
            "<way id=\"{}\"><nd ref=\"{}\"/><nd ref=\"{}\"/><nd ref=\"{}\"/>\
             <tag k=\"highway\" v=\"residential\"/></way>\n",
            200 + c,
            10 + c,
            13 + c,
            16 + c
        );
    }
    xml += "<way id=\"300\"><nd ref=\"10\"/><nd ref=\"18\"/><tag k=\"highway\" v=\"footway\"/></way>\n</osm>\n";
    xml
}

#[test]
fn import_osm_then_drive_a_route() {
    let dir = temp_dir("osm");
    wp(&dir, &["new", "g"]);
    let game = dir.join("g");
    std::fs::write(dir.join("grid.osm"), grid_osm()).unwrap();
    let osm = dir.join("grid.osm");
    let osm = osm.to_str().unwrap();

    let (code, v) = json(
        &game,
        &[
            "import",
            "osm",
            osm,
            "--name",
            "grid",
            "--drive-side",
            "left",
        ],
    );
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["drivable_ways"], 6, "the footway is skipped: {v}");
    assert_eq!(v["clipped_to_bounds"], true);
    assert_eq!(
        v["roads"], 12,
        "each street splits at the two crossings inside it"
    );
    assert_eq!(
        v["reachable_roads"], 12,
        "a grid of two-way streets is fully connected"
    );
    let roads = std::fs::read_to_string(game.join("roads/grid.road.ron")).unwrap();
    assert!(
        roads.contains("© OpenStreetMap contributors"),
        "ODbL attribution"
    );
    assert!(roads.contains("drive_side: Left"));

    // Importing again needs --force.
    let (code, v) = json(&game, &["import", "osm", osm, "--name", "grid"]);
    assert_eq!(code, 1);
    assert_eq!(v["diagnostics"][0]["code"], "WP-C001");

    // The imported map is a valid scene that a route driver can cross.
    std::fs::write(
        game.join("tests/grid_route.scenario.ron"),
        r#"(
  scene: "scenes/grid.scene.ron",
  seed: 1,
  player: (vehicle: "hatchback", spawn: "start", driver: FollowRoute(to_road: "w102_1", kmh: 40.0)),
  duration_s: 120.0,
  assert: [RouteFollowed(max_offset_m: 0.6), NoNaN, Deterministic],
)"#,
    )
    .unwrap();
    let (code, v) = json(&game, &["test", "grid_route"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["passed"], true);

    // Not OSM at all: a coded import error.
    std::fs::write(dir.join("empty.osm"), "<osm></osm>").unwrap();
    let empty = dir.join("empty.osm");
    let (code, v) = json(
        &game,
        &["import", "osm", empty.to_str().unwrap(), "--name", "x"],
    );
    assert_eq!(code, 1);
    assert_eq!(v["diagnostics"][0]["code"], "WP-I001");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn import_opendrive_then_drive_a_route() {
    let dir = temp_dir("xodr");
    wp(&dir, &["new", "g"]);
    let game = dir.join("g");
    let xodr = repo().join("crates/wp_road/tests/data/crossing.xodr");
    let xodr = xodr.to_str().unwrap();
    let (code, v) = json(&game, &["import", "opendrive", xodr, "--name", "cross"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["roads_in_file"], 4);
    assert_eq!(
        v["connecting_roads"], 1,
        "junction roads are rebuilt by the engine"
    );
    assert_eq!(v["roads"], 4, "road 2 splits where it gains a lane");
    assert_eq!(v["drive_side"], "Right");
    // Every road can be reached from the spawn (its own road counts).
    assert_eq!(v["reachable_roads"], 4, "{v}");
    let spawn_lane = v["spawn"]["lane"].as_str().unwrap().to_string();
    let target = ["r1", "r2_1", "r3"]
        .into_iter()
        .find(|r| !spawn_lane.starts_with(&format!("{r}:")))
        .unwrap();
    std::fs::write(
        game.join("tests/cross_route.scenario.ron"),
        format!(
            r#"(scene: "scenes/cross.scene.ron", seed: 1,
  player: (vehicle: "hatchback", spawn: "start", driver: FollowRoute(to_road: "{target}", kmh: 30.0)),
  duration_s: 120.0, assert: [RouteFollowed(max_offset_m: 0.8), NoNaN])"#
        ),
    )
    .unwrap();
    let (code, v) = json(&game, &["test", "cross_route"]);
    assert_eq!(code, 0, "{v}");

    // Not OpenDRIVE: a coded import error.
    let osm = dir.join("x.osm");
    std::fs::write(&osm, "<osm></osm>").unwrap();
    let (code, v) = json(
        &game,
        &["import", "opendrive", osm.to_str().unwrap(), "--name", "x"],
    );
    assert_eq!(code, 1);
    assert_eq!(v["diagnostics"][0]["code"], "WP-I001");
    std::fs::remove_dir_all(&dir).unwrap();
}
