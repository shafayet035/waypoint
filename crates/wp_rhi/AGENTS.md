# wp_rhi

Thin layer over wgpu. `Gpu::headless()` for screenshots/tests, `Gpu::for_surface` for windows.
Device limits are `downlevel_defaults` so everything we use also runs on phones.
`OffscreenTarget::read_rgba` handles the 256-byte row alignment; `write_png` saves frames.
Errors: WP-R001 (no adapter), R002 (device), R003 (readback), R004 (surface), R010 (PNG write).
wgpu 30 notes: `queue.present(tex)`, `get_current_texture` returns `CurrentSurfaceTexture`,
depth state fields are `Option`s, pipelines take `multiview_mask` and `immediate_size`.
