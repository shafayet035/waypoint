//! `world.*`, `input.set`: reading and changing the simulation.

#[allow(unused_imports)]
use crate::*;
use glam::Vec2;
use serde_json::{Value, json};
use wp_ecs::Entity;
use wp_physics::Pose;
use wp_vehicle::VehicleInput;
use wp_world::Name;

impl Session {
    pub(crate) fn entities(&self, params: &Value) -> RpcResult {
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

    pub(crate) fn get(&self, params: &Value) -> RpcResult {
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

    pub(crate) fn set(&mut self, params: &Value) -> RpcResult {
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

    pub(crate) fn find(&self, params: &Value) -> RpcResult {
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

    pub(crate) fn spawn(&mut self, params: &Value) -> RpcResult {
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

    pub(crate) fn input_set(&mut self, params: &Value) -> RpcResult {
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
}
