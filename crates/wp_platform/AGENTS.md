# wp_platform

`run_windowed(app, player, options)`: winit window + wgpu surface + fixed 60 Hz ticks with an
accumulator (max 8 ticks per frame, then time is dropped). Keys are smoothed into analogue inputs
in `update_input`. R restores the spawn snapshot. `exit_after_s` closes the window for smoke tests.
The window title shows speed, gear, rpm, lateral g and the camera.
