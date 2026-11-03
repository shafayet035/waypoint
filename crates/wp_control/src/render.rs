//! `render.screenshot`.

#[allow(unused_imports)]
use crate::*;
use serde_json::{Value, json};
use wp_render::CameraMode;

impl Session {
    pub(crate) fn screenshot(&mut self, params: &Value) -> RpcResult {
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
