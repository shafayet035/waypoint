//! `wp mcp`: a Model Context Protocol server on stdin/stdout.
//!
//! Project tools (check, test, vehicle_test, screenshot, schema, docs) run this
//! same `wp` binary with `--json`, so their results match the CLI exactly.
//! `session_start` / `session_call` drive a live simulation in-process through
//! the control protocol. Screenshots are returned as images.

use crate::commands::{DriveArgs, make_session};
use crate::{CmdResult, Ctx};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::process::Command;

const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Waypoint is a driving-game engine operated entirely through text. \
Loop: edit project files (vehicles/*.vehicle.ron, scenes/*.scene.ron, tests/*.scenario.ron), call `check`, \
then `test`; on a failure call `screenshot` with the scenario and time to see it. Use `schema` for exact \
file formats and `docs` for errors. For live control, `session_start` then `session_call` with \
control-protocol methods (call method `rpc.methods` to list them). Physics runs at 480 Hz and is \
deterministic: same files and seed, same result.";

fn tools() -> Value {
    let drive = json!({
        "scenario": { "type": "string", "description": "Use this scenario's scene, vehicle, spawn and driver." },
        "scene": { "type": "string", "description": "Scene path (default: game.toml start_scene)." },
        "vehicle": { "type": "string" },
        "spawn": { "type": "string" },
        "throttle": { "type": "number", "minimum": 0, "maximum": 1 },
        "brake": { "type": "number", "minimum": 0, "maximum": 1 },
        "steer": { "type": "number", "minimum": -1, "maximum": 1, "description": "Positive steers left." },
        "seed": { "type": "integer", "minimum": 0 }
    });
    let mut shot_props = drive.as_object().unwrap().clone();
    shot_props.insert(
        "at".into(),
        json!({ "type": "number", "minimum": 0, "description": "Simulated seconds (default 0)." }),
    );
    shot_props.insert(
        "camera".into(),
        json!({ "type": "string", "enum": ["chase", "top", "side", "showcase"] }),
    );
    shot_props.insert(
        "width".into(),
        json!({ "type": "integer", "minimum": 16, "maximum": 2048 }),
    );
    shot_props.insert(
        "height".into(),
        json!({ "type": "integer", "minimum": 16, "maximum": 2048 }),
    );
    let empty = json!({ "type": "object", "properties": {} });
    json!([
        { "name": "check", "description": "Validate every project file (schemas and cross-references). Returns coded diagnostics with file, line and fix.", "inputSchema": empty },
        { "name": "test", "description": "Run scenario tests headless. Returns pass/fail per assertion with metrics.",
          "inputSchema": { "type": "object", "properties": { "filter": { "type": "string", "description": "Only scenarios whose name contains this." } } } },
        { "name": "vehicle_test", "description": "Standard manoeuvres for a vehicle: 0–100 km/h time, 100–0 km/h stopping distance, skidpad lateral g.",
          "inputSchema": { "type": "object", "properties": { "vehicle": { "type": "string" },
            "manoeuvre": { "type": "string", "enum": ["all", "accel_0_100", "brake_100_0", "skidpad"] } }, "required": ["vehicle"] } },
        { "name": "screenshot", "description": "Render one frame headless at a simulated time and return it as an image.",
          "inputSchema": { "type": "object", "properties": shot_props } },
        { "name": "schema", "description": "JSON Schema of a project file type, with field descriptions and units.",
          "inputSchema": { "type": "object", "properties": { "type": { "type": "string", "enum": ["game", "vehicle", "scene", "scenario"] } }, "required": ["type"] } },
        { "name": "docs", "description": "Offline docs. Topics: cli, scenario, vehicle, errors, errors/<CODE>.",
          "inputSchema": { "type": "object", "properties": { "topic": { "type": "string" } } } },
        { "name": "session_start", "description": "Start a live simulation (replacing any previous one) at t = 0 with the player spawned. Returns sim.status.",
          "inputSchema": { "type": "object", "properties": drive } },
        { "name": "session_call", "description": "Call a control-protocol method on the live session, e.g. sim.step, input.set, world.get, world.find, sim.snapshot, render.screenshot. Call rpc.methods for the list.",
          "inputSchema": { "type": "object", "properties": { "method": { "type": "string" }, "params": { "type": "object" } }, "required": ["method"] } }
    ])
}

const RESOURCES: &[(&str, &str, &str)] = &[
    ("waypoint://docs/cli", "CLI reference", "docs cli"),
    (
        "waypoint://docs/scenario",
        "Scenario format: drivers and assertions",
        "docs scenario",
    ),
    (
        "waypoint://docs/vehicle",
        "Vehicle model and manoeuvres",
        "docs vehicle",
    ),
    (
        "waypoint://docs/errors",
        "Every diagnostic code",
        "docs errors",
    ),
    ("waypoint://schema/game", "game.toml schema", "schema game"),
    (
        "waypoint://schema/vehicle",
        ".vehicle.ron schema",
        "schema vehicle",
    ),
    (
        "waypoint://schema/scene",
        ".scene.ron schema",
        "schema scene",
    ),
    (
        "waypoint://schema/scenario",
        ".scenario.ron schema",
        "schema scenario",
    ),
];

struct Server<'a> {
    ctx: &'a Ctx,
    session: Option<wp_control::Session>,
}

pub fn serve(ctx: &Ctx) -> CmdResult {
    let mut server = Server { ctx, session: None };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(req) => server.handle(&req),
            Err(e) => Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = response
            && writeln!(stdout, "{r}")
                .and_then(|_| stdout.flush())
                .is_err()
        {
            break;
        }
    }
    Ok(0)
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn text(t: impl Into<String>) -> Value {
    json!({ "type": "text", "text": t.into() })
}

fn tool_result(content: Vec<Value>, is_error: bool) -> Value {
    json!({ "content": content, "isError": is_error })
}

impl Server<'_> {
    fn handle(&mut self, req: &Value) -> Option<Value> {
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let id = req.get("id").cloned()?; // notifications need no reply
        let params = req.get("params").cloned().unwrap_or(json!({}));
        let result = match method {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = if PROTOCOL_VERSIONS.contains(&asked) {
                    asked
                } else {
                    PROTOCOL_VERSIONS[1]
                };
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false }, "resources": { "listChanged": false } },
                    "serverInfo": { "name": "waypoint", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({ "tools": tools() }),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                self.call_tool(name, &args)
            }
            "resources/list" => json!({
                "resources": self.resources().iter().map(|(uri, name, _)| json!({
                    "uri": uri, "name": name,
                    "mimeType": if uri.contains("/schema/") { "application/json" } else { "text/markdown" },
                })).collect::<Vec<_>>()
            }),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
                match self.read_resource(uri) {
                    Some(t) => {
                        json!({ "contents": [{ "uri": uri, "mimeType": "text/plain", "text": t }] })
                    }
                    None => {
                        return Some(rpc_error(id, -32002, &format!("resource not found: {uri}")));
                    }
                }
            }
            "" => return Some(rpc_error(id, -32600, "invalid request")),
            other => return Some(rpc_error(id, -32601, &format!("method not found: {other}"))),
        };
        Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    fn resources(&self) -> Vec<(String, String, String)> {
        let mut out: Vec<(String, String, String)> = RESOURCES
            .iter()
            .map(|(u, n, c)| (u.to_string(), n.to_string(), c.to_string()))
            .collect();
        if self.ctx.project.join("AGENTS.md").is_file() {
            out.push((
                "waypoint://project/AGENTS.md".into(),
                "This project's notes for agents".into(),
                String::new(),
            ));
        }
        out
    }

    fn read_resource(&self, uri: &str) -> Option<String> {
        if uri == "waypoint://project/AGENTS.md" {
            return std::fs::read_to_string(self.ctx.project.join("AGENTS.md")).ok();
        }
        let (_, _, cmd) = RESOURCES.iter().find(|(u, _, _)| *u == uri)?;
        let args: Vec<&str> = cmd.split(' ').collect();
        let (_, out) = self.wp(&args, false);
        Some(out)
    }

    /// Run this `wp` binary in the project. Returns (success, stdout).
    fn wp(&self, args: &[&str], json_output: bool) -> (bool, String) {
        let exe = std::env::current_exe().unwrap_or_else(|_| "wp".into());
        let mut cmd = Command::new(exe);
        cmd.arg("--project").arg(&self.ctx.project);
        if json_output {
            cmd.arg("--json");
        }
        match cmd.args(args).output() {
            Ok(o) => {
                let mut s = String::from_utf8_lossy(&o.stdout).to_string();
                if s.trim().is_empty() {
                    s = String::from_utf8_lossy(&o.stderr).to_string();
                }
                (o.status.success(), s)
            }
            Err(e) => (false, format!("cannot run wp: {e}")),
        }
    }

    fn call_tool(&mut self, name: &str, args: &Value) -> Value {
        let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |k: &str| args.get(k).and_then(Value::as_f64);
        match name {
            "check" => {
                let (ok, out) = self.wp(&["check"], true);
                tool_result(vec![text(out)], !ok)
            }
            "test" => {
                let mut a = vec!["test".to_string()];
                if let Some(f) = s("filter") {
                    a.push(f);
                }
                let refs: Vec<&str> = a.iter().map(String::as_str).collect();
                let (ok, out) = self.wp(&refs, true);
                tool_result(vec![text(out)], !ok)
            }
            "vehicle_test" => {
                let Some(v) = s("vehicle") else {
                    return tool_result(vec![text("`vehicle` is required")], true);
                };
                let m = s("manoeuvre").unwrap_or_else(|| "all".into());
                let (ok, out) = self.wp(&["vehicle", "test", &v, "--manoeuvre", &m], true);
                tool_result(vec![text(out)], !ok)
            }
            "schema" => {
                let t = s("type").unwrap_or_default();
                let (ok, out) = self.wp(&["schema", &t], false);
                tool_result(vec![text(out)], !ok)
            }
            "docs" => {
                let mut a = vec!["docs".to_string()];
                if let Some(t) = s("topic") {
                    a.push(t);
                }
                let refs: Vec<&str> = a.iter().map(String::as_str).collect();
                let (ok, out) = self.wp(&refs, false);
                tool_result(vec![text(out)], !ok)
            }
            "screenshot" => {
                let path =
                    std::env::temp_dir().join(format!("wp-mcp-shot-{}.png", std::process::id()));
                let mut a: Vec<String> =
                    vec!["shot".into(), "--out".into(), path.to_string_lossy().into()];
                for key in ["scenario", "scene", "vehicle", "spawn", "camera"] {
                    if let Some(v) = s(key) {
                        a.push(format!("--{key}"));
                        a.push(v);
                    }
                }
                for key in [
                    "at", "width", "height", "throttle", "brake", "steer", "seed",
                ] {
                    if let Some(v) = n(key) {
                        a.push(format!("--{key}"));
                        a.push(if matches!(key, "width" | "height" | "seed") {
                            format!("{}", v as u64)
                        } else {
                            v.to_string()
                        });
                    }
                }
                let refs: Vec<&str> = a.iter().map(String::as_str).collect();
                let (ok, out) = self.wp(&refs, true);
                if !ok {
                    return tool_result(vec![text(out)], true);
                }
                let png = std::fs::read(&path).unwrap_or_default();
                let _ = std::fs::remove_file(&path);
                tool_result(
                    vec![
                        json!({ "type": "image", "data": wp_control::base64(&png), "mimeType": "image/png" }),
                        text(out),
                    ],
                    false,
                )
            }
            "session_start" => {
                let drive = DriveArgs {
                    scenario: s("scenario"),
                    scene: s("scene"),
                    spawn: s("spawn"),
                    vehicle: s("vehicle"),
                    throttle: n("throttle").unwrap_or(0.0) as f32,
                    brake: n("brake").unwrap_or(0.0) as f32,
                    steer: n("steer").unwrap_or(0.0) as f32,
                    seed: n("seed").unwrap_or(0.0) as u64,
                };
                match make_session(self.ctx, &drive) {
                    Ok(session) => {
                        let status = session.status();
                        self.session = Some(session);
                        tool_result(vec![text(pretty(&status))], false)
                    }
                    Err(diags) => tool_result(
                        vec![text(pretty(&json!({ "ok": false, "diagnostics": diags })))],
                        true,
                    ),
                }
            }
            "session_call" => {
                let Some(session) = self.session.as_mut() else {
                    return tool_result(
                        vec![text("no live session; call session_start first")],
                        true,
                    );
                };
                let method = s("method").unwrap_or_default();
                let params = args.get("params").cloned().unwrap_or(json!({}));
                match session.call(&method, &params) {
                    Ok(mut v) => {
                        let mut content = Vec::new();
                        if let Some(png) = v
                            .get("png_base64")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                        {
                            content.push(
                                json!({ "type": "image", "data": png, "mimeType": "image/png" }),
                            );
                            v.as_object_mut().unwrap().remove("png_base64");
                        }
                        content.push(text(pretty(&v)));
                        tool_result(content, false)
                    }
                    Err(e) => tool_result(
                        vec![text(pretty(
                            &json!({ "code": e.code, "message": e.message, "data": e.data }),
                        ))],
                        true,
                    ),
                }
            }
            other => tool_result(vec![text(format!("unknown tool `{other}`"))], true),
        }
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap()
}
