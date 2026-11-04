//! The control protocol (design: "The agent interface", layer 3).
//!
//! A [`Session`] owns a running [`App`] and answers JSON-RPC 2.0 requests:
//! query the world, change it, drive vehicles, step time, snapshot/restore and
//! take screenshots. Transports are thin: `wp serve` (stdin/stdout), the TCP
//! listener in [`tcp`] for a live window, and the MCP server in `wp mcp`.
//!
//! Call `rpc.methods` for the full list with parameters. Entities are strings
//! like `"3v0"`. Methods that take `entity` default to the player.

mod json;
mod render;
mod rpc;
mod session;
pub mod tcp;
mod world;

pub(crate) use json::*;
pub use json::{base64, set_player};
pub(crate) use rpc::*;
pub use rpc::{METHODS, RpcError};
pub use session::Session;

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use wp_app::App;
    use wp_vehicle::VehicleDef;

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
    fn subscriptions_push_matching_events() {
        let mut s = session();
        let (tx, rx) = std::sync::mpsc::channel();
        let line = |m: &str, p: Value| {
            json!({ "jsonrpc": "2.0", "id": 1, "method": m, "params": p }).to_string()
        };
        let r = s.handle_line_with(
            &line("events.subscribe", json!({ "kinds": ["GearShift"] })),
            &tx,
        );
        assert!(r.unwrap().contains("\"subscription\":1"));
        s.handle_line(&line("input.set", json!({ "throttle": 1.0 })));
        s.handle_line(&line("sim.step", json!({ "seconds": 6.0 })));
        let pushed: Vec<Value> = rx
            .try_iter()
            .map(|l| serde_json::from_str(&l).unwrap())
            .collect();
        assert!(!pushed.is_empty(), "no gear shifts pushed");
        assert!(
            pushed
                .iter()
                .all(|p| p["method"] == "events.event"
                    && p["params"]["event"]["kind"] == "GearShift")
        );
        // Without a push channel, subscribing is a coded error.
        let r: Value =
            serde_json::from_str(&s.handle_line(&line("events.subscribe", json!({}))).unwrap())
                .unwrap();
        assert!(r["error"].is_object(), "{r}");
        s.handle_line(&line("events.unsubscribe", json!({ "subscription": 1 })));
        s.handle_line(&line("sim.step", json!({ "seconds": 6.0 })));
        assert_eq!(rx.try_iter().count(), 0);
    }

    #[test]
    fn replay_start_and_stop_record_inputs_and_hashes() {
        let mut s = session();
        call(&mut s, "replay.start", json!({ "hash_every": 30 }));
        call(&mut s, "input.set", json!({ "throttle": 1.0 }));
        call(&mut s, "sim.step", json!({ "ticks": 60 }));
        call(
            &mut s,
            "input.set",
            json!({ "throttle": 0.0, "brake": 1.0 }),
        );
        call(&mut s, "sim.step", json!({ "ticks": 30 }));
        let r = call(&mut s, "replay.stop", json!({}));
        let replay = &r["replay"];
        assert_eq!(replay["ticks"], 90);
        assert_eq!(replay["hashes"].as_array().unwrap().len(), 3);
        assert_eq!(replay["inputs"].as_array().unwrap().len(), 2, "{replay}");
        let again = s.handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "replay.stop" }));
        assert!(again.unwrap()["error"].is_object());
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
