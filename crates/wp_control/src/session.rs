//! The session: ticking, request dispatch, and the `sim.*` methods.

#[allow(unused_imports)]
use crate::*;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use wp_app::App;
use wp_core::SimTime;
use wp_ecs::{Entity, WorldSnapshot};
use wp_vehicle::VehicleDef;
use wp_world::Name;

/// A live simulation that can be driven over the control protocol.
pub struct Session {
    pub app: App,
    /// Default target for methods that take an entity.
    pub player: Option<Entity>,
    /// Set by `sim.pause`; windowed loops stop real-time ticking while true.
    pub paused: bool,
    /// Vehicles `world.spawn` can create, by name.
    pub vehicles: BTreeMap<String, VehicleDef>,
    pub(crate) snapshots: Vec<WorldSnapshot>,
    /// Events collected after each tick, until `events.poll` takes them.
    pub(crate) events: std::collections::VecDeque<Value>,
    /// Events dropped because nobody polled (the buffer keeps the newest).
    pub(crate) events_dropped: u64,
    pub(crate) gpu: Option<wp_rhi::Gpu>,
}

/// Most events kept between polls.
const EVENT_BUFFER: usize = 10_000;

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
            events: std::collections::VecDeque::new(),
            events_dropped: 0,
            gpu: None,
        }
    }

    /// Advance one tick and collect its events. Every front end should tick
    /// through this so `events.poll` sees everything.
    pub fn tick(&mut self) {
        self.app.tick();
        let time = self.app.time();
        if let Some(evs) = self
            .app
            .world
            .get_resource::<wp_ecs::Events<wp_vehicle::VehicleEvent>>()
        {
            for e in evs.iter() {
                let mut v = serde_json::to_value(e).unwrap();
                v["tick"] = json!(time.tick);
                v["t"] = json!(time.seconds());
                self.events.push_back(v);
            }
        }
        while self.events.len() > EVENT_BUFFER {
            self.events.pop_front();
            self.events_dropped += 1;
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
            "events.poll" => {
                let kind = params.get("kind").and_then(Value::as_str);
                let all: Vec<Value> = self.events.drain(..).collect();
                let events: Vec<Value> = all
                    .into_iter()
                    .filter(|e| kind.is_none_or(|k| e["kind"] == k))
                    .collect();
                let dropped = std::mem::take(&mut self.events_dropped);
                Ok(json!({ "events": events, "dropped": dropped }))
            }
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

    pub(crate) fn step(&mut self, params: &Value) -> RpcResult {
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
        for _ in 0..ticks {
            self.tick();
        }
        Ok(self.status())
    }

    pub(crate) fn restore(&mut self, params: &Value) -> RpcResult {
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
    pub(crate) fn entity(&self, params: &Value) -> Result<Entity, RpcError> {
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

    pub(crate) fn name_of(&self, e: Entity) -> Option<String> {
        self.app.world.get::<Name>(e).map(|n| n.0.clone())
    }
}
