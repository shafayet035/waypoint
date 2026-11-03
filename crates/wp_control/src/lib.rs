//! The control protocol (design: "The agent interface", layer 3).
//!
//! A [`Session`] owns a running [`App`] and answers JSON-RPC 2.0 requests:
//! query the world, change it, drive vehicles, step time, snapshot/restore and
//! take screenshots. Transports are thin: `wp serve` (stdin/stdout), the TCP
//! listener in [`tcp`] for a live window, and the MCP server in `wp mcp`.
//!
//! Call `rpc.methods` for the full list with parameters. Entities are strings
//! like `"3v0"`. Methods that take `entity` default to the player.

pub mod tcp;

use glam::Vec2;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use wp_app::App;
use wp_core::{Diagnostic, SimTime};
use wp_ecs::{Entity, WorldSnapshot};
use wp_physics::Pose;
use wp_render::CameraMode;
use wp_vehicle::{VehicleDef, VehicleInput};
use wp_world::{Name, Player};

/// Method name, parameters, summary. Served by `rpc.methods`.
pub const METHODS: &[(&str, &str, &str)] = &[
    (
        "rpc.methods",
        "{}",
        "List every method with its parameters.",
    ),
    (
        "sim.status",
        "{}",
        "Tick, simulated time, pause state, state hash and player telemetry.",
    ),
    (
        "sim.step",
        "{ticks?: int, seconds?: number}",
        "Advance exactly N 60 Hz ticks (default 1). Returns sim.status.",
    ),
    (
        "sim.pause",
        "{}",
        "Pause real-time ticking in a window. sim.step still works.",
    ),
    ("sim.resume", "{}", "Resume real-time ticking in a window."),
    (
        "sim.snapshot",
        "{}",
        "Save the full world state. Returns {id}.",
    ),
    (
        "sim.restore",
        "{id: int}",
        "Restore a snapshot; later steps replay bit-identically.",
    ),
    (
        "world.entities",
        "{with?: component}",
        "Entities with their names and component names.",
    ),
    (
        "world.get",
        "{entity?, component?}",
        "All components of an entity, or one component.",
    ),
    (
        "world.set",
        "{entity?, component, value, merge?: bool}",
        "Replace a component, or merge fields into it.",
    ),
    (
        "world.find",
        "{near?: [x, y], entity?, radius?: m, with?: component}",
        "Entities within radius (default 50 m), nearest first.",
    ),
    (
        "world.spawn",
        "{vehicle, position?: [x, y], heading_deg?, name?}",
        "Spawn a vehicle at rest. Returns {entity}.",
    ),
    ("world.despawn", "{entity}", "Remove an entity."),
    (
        "world.resources",
        "{}",
        "Reflected resources: SimTime, SimRng, Gravity, Ground.",
    ),
    (
        "input.set",
        "{entity?, throttle?, brake?, steer?}",
        "Set driver controls (unset fields keep their value).",
    ),
    (
        "vehicle.telemetry",
        "{entity?}",
        "Speed, g-forces, gear, rpm, steering, position, heading.",
    ),
    (
        "render.screenshot",
        "{camera?: chase|top|side|showcase, width?, height?, path?}",
        "Render the current frame. Writes a PNG to path, or returns png_base64.",
    ),
];

/// A JSON-RPC error. Engine errors use code -32000 with a diagnostic in `data`.
#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
            data: None,
        }
    }
    fn engine(d: Diagnostic) -> Self {
        Self {
            code: -32000,
            message: d.message.clone(),
            data: Some(serde_json::to_value(&d).unwrap()),
        }
    }
}

impl From<Diagnostic> for RpcError {
    fn from(d: Diagnostic) -> Self {
        RpcError::engine(d)
    }
}

type RpcResult = Result<Value, RpcError>;

/// A live simulation that can be driven over the control protocol.
pub struct Session {
    pub app: App,
    /// Default target for methods that take an entity.
    pub player: Option<Entity>,
    /// Set by `sim.pause`; windowed loops stop real-time ticking while true.
    pub paused: bool,
    /// Vehicles `world.spawn` can create, by name.
    pub vehicles: BTreeMap<String, VehicleDef>,
    snapshots: Vec<WorldSnapshot>,
    gpu: Option<wp_rhi::Gpu>,
}

/// Longest single `sim.step`: 10 simulated minutes.
const MAX_STEP_TICKS: u64 = 36_000;

impl Session {
    pub fn new(app: App, player: Option<Entity>, vehicles: BTreeMap<String, VehicleDef>) -> Self {
        Self {
            app,
            player,
            paused: false,
            vehicles,
            snapshots: Vec::new(),
            gpu: None,
        }
    }

    /// Handle one line of JSON. Returns the response line, or `None` for a notification.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let response = match serde_json::from_str::<Value>(line) {
            Ok(req) => self.handle(&req)?,
            Err(e) => error_response(Value::Null, -32700, &format!("parse error: {e}"), None),
        };
        Some(response.to_string())
    }

    /// Handle one parsed request. Returns `None` for a notification (no `id`).
    pub fn handle(&mut self, req: &Value) -> Option<Value> {
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(Value::as_str);
        let (Some(method), Some("2.0")) = (method, req.get("jsonrpc").and_then(Value::as_str))
        else {
            return Some(error_response(
                id.unwrap_or(Value::Null),
                -32600,
                "invalid request: need {\"jsonrpc\": \"2.0\", \"method\": ..., \"id\": ...}",
                None,
            ));
        };
        let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = self.call(method, &params);
        let id = id?;
        Some(match result {
            Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
            Err(e) => error_response(id, e.code, &e.message, e.data),
        })
    }

    /// Call a method directly (used by the MCP server).
    pub fn call(&mut self, method: &str, params: &Value) -> RpcResult {
        if !params.is_object() {
            return Err(RpcError::invalid_params("params must be an object"));
        }
        match method {
            "rpc.methods" => Ok(Value::Array(
                METHODS
                    .iter()
                    .map(|(n, p, s)| json!({ "name": n, "params": p, "summary": s }))
                    .collect(),
            )),
            "sim.status" => Ok(self.status()),
            "sim.step" => self.step(params),
            "sim.pause" => {
                self.paused = true;
                Ok(json!({ "paused": true }))
            }
            "sim.resume" => {
                self.paused = false;
                Ok(json!({ "paused": false }))
            }
            "sim.snapshot" => {
                self.snapshots.push(self.app.world.snapshot());
                Ok(json!({ "id": self.snapshots.len() - 1, "tick": self.app.time().tick }))
            }
            "sim.restore" => self.restore(params),
            "world.entities" => self.entities(params),
            "world.get" => self.get(params),
            "world.set" => self.set(params),
            "world.find" => self.find(params),
            "world.spawn" => self.spawn(params),
            "world.despawn" => {
                let e = self.entity(params)?;
                Ok(json!({ "despawned": self.app.world.despawn(e) }))
            }
            "world.resources" => {
                let w = &self.app.world;
                let map: Map<String, Value> = w
                    .reflected_resource_names()
                    .into_iter()
                    .filter_map(|n| w.get_resource_json(n).map(|v| (n.to_string(), v)))
                    .collect();
                Ok(Value::Object(map))
            }
            "input.set" => self.input_set(params),
            "vehicle.telemetry" => {
                let e = self.entity(params)?;
                let t = wp_vehicle::telemetry(&self.app.world, e)
                    .ok_or_else(|| k("WP-K007", format!("entity {e} is not a vehicle")))?;
                Ok(serde_json::to_value(t).unwrap())
            }
            "render.screenshot" => self.screenshot(params),
            other => Err(RpcError {
                code: -32601,
                message: format!("unknown method `{other}`; call rpc.methods for the list"),
                data: None,
            }),
        }
    }

    pub fn status(&self) -> Value {
        let t = self.app.time();
        let telemetry = self
            .player
            .and_then(|p| wp_vehicle::telemetry(&self.app.world, p));
        json!({
            "tick": t.tick,
            "seconds": t.seconds(),
            "tick_hz": t.tick_hz,
            "physics_hz": t.physics_hz(),
            "paused": self.paused,
            "entity_count": self.app.world.entity_count(),
            "state_hash": self.app.world.state_hash(),
            "player": self.player.map(|p| p.to_string()),
            "telemetry": telemetry,
        })
    }

    fn step(&mut self, params: &Value) -> RpcResult {
        let tick_hz = self.app.world.resource::<SimTime>().tick_hz;
        let ticks = match (params.get("ticks"), params.get("seconds")) {
            (Some(t), _) => t
                .as_u64()
                .ok_or_else(|| RpcError::invalid_params("ticks must be a non-negative integer"))?,
            (None, Some(s)) => {
                let s = s
                    .as_f64()
                    .filter(|s| *s >= 0.0)
                    .ok_or_else(|| RpcError::invalid_params("seconds must be ≥ 0"))?;
                (s * tick_hz as f64).round() as u64
            }
            (None, None) => 1,
        };
        if ticks > MAX_STEP_TICKS {
            return Err(k(
                "WP-K006",
                format!(
                    "cannot step {ticks} ticks at once; the limit is {MAX_STEP_TICKS} (10 minutes)"
                ),
            )
            .into());
        }
        self.app.run_ticks(ticks);
        Ok(self.status())
    }

    fn restore(&mut self, params: &Value) -> RpcResult {
        let id = params
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| RpcError::invalid_params("id is required"))?;
        let snap = self.snapshots.get(id as usize).ok_or_else(|| {
            k(
                "WP-K004",
                format!("no snapshot {id}; {} taken so far", self.snapshots.len()),
            )
        })?;
        self.app
            .world
            .restore(snap)
            .map_err(|e| k("WP-K004", format!("restore failed: {e}")))?;
        Ok(self.status())
    }

    /// `entity` from params, else the player.
    fn entity(&self, params: &Value) -> Result<Entity, RpcError> {
        let e = match params.get("entity") {
            Some(Value::String(s)) => s.parse::<Entity>().map_err(RpcError::invalid_params)?,
            Some(_) => {
                return Err(RpcError::invalid_params(
                    "entity must be a string like \"3v0\"",
                ));
            }
            None => self.player.ok_or_else(|| {
                k(
                    "WP-K001",
                    "no player in this session; pass `entity`".to_string(),
                )
            })?,
        };
        if !self.app.world.is_alive(e) {
            return Err(k(
                "WP-K001",
                format!("entity {e} does not exist; call world.entities"),
            )
            .into());
        }
        Ok(e)
    }

    fn name_of(&self, e: Entity) -> Option<String> {
        self.app.world.get::<Name>(e).map(|n| n.0.clone())
    }

    fn entities(&self, params: &Value) -> RpcResult {
        let with = params.get("with").and_then(Value::as_str);
        let w = &self.app.world;
        let list: Vec<Value> = w
            .entities()
            .into_iter()
            .filter(|e| with.is_none_or(|c| w.has_component_named(c, *e)))
            .map(|e| {
                let comps: Vec<String> = w
                    .entity_json(e)
                    .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect()))
                    .unwrap_or_default();
                json!({ "entity": e.to_string(), "name": self.name_of(e), "components": comps })
            })
            .collect();
        Ok(Value::Array(list))
    }

    fn get(&self, params: &Value) -> RpcResult {
        let e = self.entity(params)?;
        match params.get("component").and_then(Value::as_str) {
            None => Ok(self.app.world.entity_json(e).unwrap_or(Value::Null)),
            Some(c) => self.app.world.get_component_json(e, c).ok_or_else(|| {
                k("WP-K007", format!("entity {e} has no component `{c}`"))
                    .help(format!(
                        "components there: {}",
                        component_list(&self.app.world, e)
                    ))
                    .into()
            }),
        }
    }

    fn set(&mut self, params: &Value) -> RpcResult {
        let e = self.entity(params)?;
        let c = params
            .get("component")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("component is required"))?;
        let mut value = params
            .get("value")
            .cloned()
            .ok_or_else(|| RpcError::invalid_params("value is required"))?;
        if params
            .get("merge")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let mut current = self.app.world.get_component_json(e, c).ok_or_else(|| {
                RpcError::from(k(
                    "WP-K007",
                    format!("entity {e} has no component `{c}` to merge into"),
                ))
            })?;
            merge(&mut current, &value);
            value = current;
        }
        self.app
            .world
            .set_component_json(e, c, value)
            .map_err(|m| k("WP-K005", format!("cannot set {c} on {e}: {m}")).help(format!("read the current value with world.get {{\"entity\": \"{e}\", \"component\": \"{c}\"}}")))?;
        Ok(self
            .app
            .world
            .get_component_json(e, c)
            .unwrap_or(Value::Null))
    }

    fn find(&self, params: &Value) -> RpcResult {
        let w = &self.app.world;
        let center = match params.get("near") {
            Some(v) => point(v).ok_or_else(|| RpcError::invalid_params("near must be [x, y]"))?,
            None => {
                let e = self.entity(params)?;
                w.get::<Pose>(e)
                    .map(|p| p.position)
                    .ok_or_else(|| k("WP-K007", format!("entity {e} has no Pose")))?
            }
        };
        let radius = params.get("radius").and_then(Value::as_f64).unwrap_or(50.0) as f32;
        let with = params.get("with").and_then(Value::as_str);
        let mut hits: Vec<(f32, Entity, Vec2)> = Vec::new();
        w.query::<&Pose>().for_each(|e, p| {
            let d = p.position.distance(center);
            if d <= radius && with.is_none_or(|c| w.has_component_named(c, e)) {
                hits.push((d, e, p.position));
            }
        });
        hits.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        Ok(Value::Array(
            hits.into_iter()
                .map(|(d, e, p)| json!({ "entity": e.to_string(), "name": self.name_of(e), "distance_m": d, "position": [p.x, p.y] }))
                .collect(),
        ))
    }

    fn spawn(&mut self, params: &Value) -> RpcResult {
        let name = params
            .get("vehicle")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("vehicle is required"))?;
        let def = self.vehicles.get(name).cloned().ok_or_else(|| {
            k("WP-K003", format!("unknown vehicle `{name}`")).help(format!(
                "known: {}",
                self.vehicles.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
        })?;
        let pos = match params.get("position") {
            Some(v) => {
                point(v).ok_or_else(|| RpcError::invalid_params("position must be [x, y]"))?
            }
            None => Vec2::ZERO,
        };
        let heading = params
            .get("heading_deg")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as f32;
        let e = wp_vehicle::spawn_vehicle(&mut self.app.world, &def, pos, heading.to_radians());
        let label = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string();
        self.app.world.insert(e, Name(label));
        Ok(json!({ "entity": e.to_string() }))
    }

    fn input_set(&mut self, params: &Value) -> RpcResult {
        let e = self.entity(params)?;
        let mut input = *self.app.world.get::<VehicleInput>(e).ok_or_else(|| {
            k(
                "WP-K007",
                format!("entity {e} has no VehicleInput; it is not a vehicle"),
            )
        })?;
        let field = |key: &str, lo: f32, hi: f32| -> Result<Option<f32>, RpcError> {
            match params.get(key) {
                None => Ok(None),
                Some(v) => v
                    .as_f64()
                    .map(|x| x as f32)
                    .filter(|x| (lo..=hi).contains(x))
                    .map(Some)
                    .ok_or_else(|| {
                        RpcError::invalid_params(format!("{key} must be a number in {lo}..{hi}"))
                    }),
            }
        };
        if let Some(v) = field("throttle", 0.0, 1.0)? {
            input.throttle = v;
        }
        if let Some(v) = field("brake", 0.0, 1.0)? {
            input.brake = v;
        }
        if let Some(v) = field("steer", -1.0, 1.0)? {
            input.steer = v;
        }
        *self.app.world.get_mut::<VehicleInput>(e).unwrap() = input;
        Ok(serde_json::to_value(input).unwrap())
    }

    fn screenshot(&mut self, params: &Value) -> RpcResult {
        let camera_name = params
            .get("camera")
            .and_then(Value::as_str)
            .unwrap_or("chase");
        let camera = CameraMode::parse(camera_name).ok_or_else(|| {
            RpcError::invalid_params("camera must be chase, top, side or showcase")
        })?;
        let width = params.get("width").and_then(Value::as_u64).unwrap_or(960) as u32;
        let height = params.get("height").and_then(Value::as_u64).unwrap_or(540) as u32;
        if !(16..=4096).contains(&width) || !(16..=4096).contains(&height) {
            return Err(RpcError::invalid_params(
                "width and height must be 16..4096",
            ));
        }
        if self.gpu.is_none() {
            self.gpu = Some(wp_rhi::Gpu::headless()?);
        }
        let gpu = self.gpu.as_ref().unwrap();
        let frame = wp_render::extract(&self.app.world, camera);
        let rgba = wp_render::render_rgba(gpu, &frame, width, height)?;
        let meta = json!({ "width": width, "height": height, "camera": camera_name, "tick": self.app.time().tick, "mime": "image/png" });
        match params.get("path").and_then(Value::as_str) {
            Some(path) => {
                wp_rhi::write_png(std::path::Path::new(path), width, height, &rgba)?;
                let mut m = meta;
                m["path"] = json!(path);
                Ok(m)
            }
            None => {
                let png = wp_rhi::encode_png(width, height, &rgba)?;
                let mut m = meta;
                m["png_base64"] = json!(base64(&png));
                Ok(m)
            }
        }
    }
}

/// Mark an entity as the player (convenience for transports that spawn one).
pub fn set_player(app: &mut App, e: Entity) {
    app.world.insert(e, Player);
}

fn k(code: &str, msg: String) -> Diagnostic {
    Diagnostic::error(code, msg)
}

fn component_list(w: &wp_ecs::World, e: Entity) -> String {
    w.entity_json(e)
        .and_then(|v| {
            v.as_object()
                .map(|m| m.keys().cloned().collect::<Vec<_>>().join(", "))
        })
        .unwrap_or_default()
}

fn point(v: &Value) -> Option<Vec2> {
    let a = v.as_array()?;
    if a.len() != 2 {
        return None;
    }
    Some(Vec2::new(a[0].as_f64()? as f32, a[1].as_f64()? as f32))
}

/// Recursive merge: objects merge key by key, anything else replaces.
fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (key, v) in p {
                merge(t.entry(key.clone()).or_insert(Value::Null), v);
            }
        }
        (t, p) => *t = p.clone(),
    }
}

fn error_response(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut err = json!({ "code": code, "message": message });
    if let Some(d) = data {
        err["data"] = d;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": err })
}

/// Standard base64 (RFC 4648) with padding.
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut app = App::new();
        app.add_plugin(wp_physics::PhysicsPlugin)
            .add_plugin(wp_world::WorldPlugin)
            .add_plugin(wp_vehicle::VehiclePlugin);
        app.finish().unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sandbox");
        let def: VehicleDef = wp_data::load_ron(&root, "vehicles/hatchback.vehicle.ron")
            .unwrap()
            .value;
        let e = wp_vehicle::spawn_vehicle(&mut app.world, &def, Vec2::ZERO, 0.0);
        set_player(&mut app, e);
        Session::new(app, Some(e), BTreeMap::from([(def.name.clone(), def)]))
    }

    fn call(s: &mut Session, method: &str, params: Value) -> Value {
        let resp = s
            .handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
            .unwrap();
        assert!(resp.get("error").is_none(), "{method} failed: {resp}");
        resp["result"].clone()
    }

    #[test]
    fn base64_matches_rfc() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn merge_is_recursive() {
        let mut a = json!({ "position": [0, 0], "heading": 1.0, "x": { "y": 1, "z": 2 } });
        merge(&mut a, &json!({ "heading": 2.0, "x": { "z": 3 } }));
        assert_eq!(
            a,
            json!({ "position": [0, 0], "heading": 2.0, "x": { "y": 1, "z": 3 } })
        );
    }

    #[test]
    fn drive_snapshot_restore() {
        let mut s = session();
        call(&mut s, "input.set", json!({ "throttle": 1.0 }));
        let snap = call(&mut s, "sim.snapshot", json!({}));
        let a = call(&mut s, "sim.step", json!({ "seconds": 2.0 }));
        call(&mut s, "sim.restore", json!({ "id": snap["id"] }));
        let b = call(&mut s, "sim.step", json!({ "seconds": 2.0 }));
        assert_eq!(a["state_hash"], b["state_hash"]);
        assert!(a["telemetry"]["speed_kmh"].as_f64().unwrap() > 20.0);
    }

    #[test]
    fn errors_are_coded() {
        let mut s = session();
        let r = s.handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "world.get", "params": { "entity": "99v0" } })).unwrap();
        assert_eq!(r["error"]["code"], -32000);
        assert_eq!(r["error"]["data"]["code"], "WP-K001");
        let r = s
            .handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "nope" }))
            .unwrap();
        assert_eq!(r["error"]["code"], -32601);
        assert!(s.handle_line("{not json").unwrap().contains("-32700"));
        // Notifications get no response.
        assert!(
            s.handle(&json!({ "jsonrpc": "2.0", "method": "sim.step" }))
                .is_none()
        );
    }

    #[test]
    fn spawn_find_and_merge() {
        let mut s = session();
        let e = call(
            &mut s,
            "world.spawn",
            json!({ "vehicle": "hatchback", "position": [20.0, 0.0], "name": "other" }),
        );
        let near = call(
            &mut s,
            "world.find",
            json!({ "near": [18.0, 0.0], "radius": 5.0 }),
        );
        assert_eq!(near[0]["entity"], e["entity"]);
        assert_eq!(near[0]["name"], "other");
        let pose = call(
            &mut s,
            "world.set",
            json!({ "entity": e["entity"], "component": "Pose", "value": { "heading": 1.5 }, "merge": true }),
        );
        assert_eq!(pose["heading"], 1.5);
        assert_eq!(pose["position"], json!([20.0, 0.0]));
    }
}
