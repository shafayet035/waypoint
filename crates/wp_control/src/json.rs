//! Small helpers: JSON points and merges, base64, diagnostics.

#[allow(unused_imports)]
use crate::*;
use glam::Vec2;
use serde_json::Value;
use wp_app::App;
use wp_core::Diagnostic;
use wp_ecs::Entity;
use wp_world::Player;

/// Mark an entity as the player (convenience for transports that spawn one).
pub fn set_player(app: &mut App, e: Entity) {
    app.world.insert(e, Player);
}

pub(crate) fn k(code: &str, msg: String) -> Diagnostic {
    Diagnostic::error(code, msg)
}

pub(crate) fn component_list(w: &wp_ecs::World, e: Entity) -> String {
    w.entity_json(e)
        .and_then(|v| {
            v.as_object()
                .map(|m| m.keys().cloned().collect::<Vec<_>>().join(", "))
        })
        .unwrap_or_default()
}

pub(crate) fn point(v: &Value) -> Option<Vec2> {
    let a = v.as_array()?;
    if a.len() != 2 {
        return None;
    }
    Some(Vec2::new(a[0].as_f64()? as f32, a[1].as_f64()? as f32))
}

/// Recursive merge: objects merge key by key, anything else replaces.
pub(crate) fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (key, v) in p {
                merge(t.entry(key.clone()).or_insert(Value::Null), v);
            }
        }
        (t, p) => *t = p.clone(),
    }
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
