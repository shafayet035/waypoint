//! JSON-RPC plumbing: the method table, error type and error responses.

#[allow(unused_imports)]
use crate::*;
use serde_json::{Value, json};
use wp_core::Diagnostic;

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
        "Speed, g-forces, gear, rpm, steering, position, heading, roll, pitch, ABS, air pressure, wheel loads.",
    ),
    (
        "events.poll",
        "{kind?: string}",
        "Vehicle events since the last poll (GearShift, AbsEngaged, WheelLift, LowAirPressure, RolledOver, ...), with tick and time.",
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
    pub(crate) fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
            data: None,
        }
    }
    pub(crate) fn engine(d: Diagnostic) -> Self {
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

pub(crate) type RpcResult = Result<Value, RpcError>;

pub(crate) fn error_response(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut err = json!({ "code": code, "message": message });
    if let Some(d) = data {
        err["data"] = d;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": err })
}
