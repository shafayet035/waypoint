# wp_platform

`run_windowed(session, options)`: winit window + wgpu surface + fixed 60 Hz ticks with an
accumulator (max 8 ticks per frame, then time is dropped). The window owns a `wp_control::Session`;
control requests from `options.control` are answered at the start of each frame. Keys are smoothed
into analogue inputs and only written while the keyboard is in use, so a control client can drive.
R restores the spawn state, P (or sim.pause) pauses real-time ticking. `exit_after_s` closes the
window for smoke tests. The title shows speed, gear, rpm, lateral g and the camera.
