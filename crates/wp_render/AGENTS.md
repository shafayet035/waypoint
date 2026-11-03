# wp_render

`extract(world, CameraMode) -> FrameView` (read-only), then `Renderer::render(gpu, &frame, target)`.
One render pass: sky (fullscreen triangle) → ground (camera-following quad, grid, blob shadows) →
instanced meshes (cube, wheel). Shaders output linear colour into an sRGB target after ACES tonemapping.

- `src/extract/`: `mod.rs` (FrameView, `extract`), `camera.rs`, `vehicle.rs` (car/bus/truck-cab
  bodies), `trailer.rs`, `dock.rs`. Vehicles are drawn from their definitions until glTF meshes exist.
- Coordinates: Z up. Cameras: chase, top (north-up), side, showcase. Projection is glam's
  `camera::rh::proj::directx` (wgpu clip space: y up, depth 0..1).
- `mesh.rs` has a winding test; keep it passing or back-face culling hides faces.
- Visual check: `WP_RENDER_OUT=/tmp/out cargo test -p wp_render` writes PNGs from every camera.
