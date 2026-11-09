//! Interactive play: a window, keyboard input and a fixed-timestep loop.
//!
//! The simulation always advances in whole 60 Hz ticks (so it stays
//! deterministic); rendering runs at display rate and only reads the world.
//! Controls: W/↑ throttle, S/↓ brake (hold at a standstill to reverse), A/← and D/→ steer, C camera,
//! Q/E or a mouse drag to look around in the cab,
//! R reset, P pause, Esc quit. On touch screens: left thumb drags to steer,
//! right thumb high for throttle, low for brake, tap the top edge for camera.
//! With tilt steering (`WindowOptions::steering`) the phone is the wheel: left
//! thumb brakes, right thumb accelerates. Tapping the top right corner switches
//! between the two (and remembers it in settings.toml). A control client
//! (`wp run --control <port>`) can query and drive the same session while it runs.

pub mod audio;
pub mod motion;
pub mod settings;
pub mod thermal;
use serde::Serialize;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};
use wp_control::{Session, tcp::Pending};
use wp_core::Diagnostic;
use wp_ecs::Entity;
use wp_physics::Pose;
use wp_render::{CameraMode, ChaseSmoothing, Renderer, ViewOptions, extract_view};
use wp_rhi::{Gpu, GpuInfo, wgpu};
use wp_vehicle::{Controls, DriverAids, TiltSteering, Vehicle, VehicleInput, VehicleLibrary};

pub struct WindowOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub camera: CameraMode,
    /// Close automatically after this many real seconds (for smoke tests).
    pub exit_after_s: Option<f32>,
    /// Quality tier: internal resolution, shadow map and road streaming
    /// distance. `None` renders at full size with default shadows and the
    /// whole map resident.
    pub quality: Option<wp_world::QualityTier>,
    /// Play the player vehicle's engine, road and skid sound.
    pub audio: bool,
    /// Keep frames on time by lowering resolution, then effects, then frame
    /// rate when frames run late or the device gets hot (needs `quality`).
    pub governor: bool,
    /// Pretend the device is this hot (testing the governor); `None` asks the OS.
    pub thermal: Option<thermal::Thermal>,
    /// Touch-screen steering: thumb or tilt. Tilt falls back to touch on
    /// devices without a motion sensor (unless `fake_tilt_deg` is set).
    pub steering: settings::Steering,
    /// Pretend the phone is held straight for a second, then turned this far,
    /// degrees, left positive (testing tilt on desktops).
    pub fake_tilt_deg: Option<f32>,
    /// Where to remember the player's steering choice (settings.toml).
    pub settings: Option<std::path::PathBuf>,
    /// Control-protocol requests to answer between frames (see `wp_control::tcp`).
    pub control: Option<Receiver<Pending>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WindowSummary {
    pub frames: u64,
    pub ticks: u64,
    pub control_requests: u64,
    pub real_seconds: f64,
    pub sim_seconds: f64,
    pub gpu: Option<GpuInfo>,
    /// What the last frame drew (draw calls, triangles, road tiles).
    pub render: Option<wp_render::RenderStats>,
    /// Where the governor ended up, if it ran.
    pub governor: Option<GovernorSummary>,
    /// How the player was steering at the end.
    pub steering: settings::Steering,
    /// The session's recording, if it was recording when the window closed.
    #[serde(skip)]
    pub replay: Option<wp_vehicle::replay::Replay>,
}

/// A line in the platform's log: logcat (tag `waypoint`) on Android, else stderr.
/// Parts of the frame to leave out while profiling (comma-separated).
fn debug_skip() -> String {
    #[cfg(target_os = "android")]
    {
        unsafe extern "C" {
            fn __system_property_get(
                name: *const std::ffi::c_char,
                value: *mut std::ffi::c_char,
            ) -> i32;
        }
        let mut buf = [0 as std::ffi::c_char; 92];
        let n = unsafe { __system_property_get(c"debug.waypoint.skip".as_ptr(), buf.as_mut_ptr()) };
        if n > 0 {
            let bytes: Vec<u8> = buf[..n as usize].iter().map(|c| *c as u8).collect();
            return String::from_utf8_lossy(&bytes).into_owned();
        }
        String::new()
    }
    #[cfg(not(target_os = "android"))]
    {
        std::env::var("WP_SKIP").unwrap_or_default()
    }
}

fn log_line(msg: &str) {
    #[cfg(target_os = "android")]
    {
        #[link(name = "log")]
        unsafe extern "C" {
            fn __android_log_write(
                prio: i32,
                tag: *const std::ffi::c_char,
                text: *const std::ffi::c_char,
            ) -> i32;
        }
        if let Ok(text) = std::ffi::CString::new(msg) {
            // SAFETY: both pointers are valid NUL-terminated strings for the call.
            unsafe { __android_log_write(4, c"waypoint".as_ptr(), text.as_ptr()) };
        }
    }
    #[cfg(not(target_os = "android"))]
    eprintln!("{msg}");
}

/// Run a session in a window until the user closes it. The session's player
/// receives keyboard input; control requests are answered between frames.
pub fn run_windowed(session: Session, opts: WindowOptions) -> Result<WindowSummary, Diagnostic> {
    let event_loop = EventLoop::new().map_err(|e| {
        Diagnostic::error("WP-P001", format!("cannot open a window: {e}"))
            .help("use --headless on machines without a display")
    })?;
    run_with_event_loop(event_loop, session, opts)
}

/// Android entry: the event loop must be built from the `AndroidApp` that
/// `android_main` receives.
#[cfg(target_os = "android")]
pub fn run_windowed_android(
    app: winit::platform::android::activity::AndroidApp,
    session: Session,
    opts: WindowOptions,
) -> Result<WindowSummary, Diagnostic> {
    use winit::platform::android::EventLoopBuilderExtAndroid;
    let event_loop = EventLoop::builder()
        .with_android_app(app)
        .build()
        .map_err(|e| {
            Diagnostic::error(
                "WP-P001",
                format!("cannot start the Android event loop: {e}"),
            )
        })?;
    run_with_event_loop(event_loop, session, opts)
}

fn run_with_event_loop(
    event_loop: EventLoop<()>,
    session: Session,
    mut opts: WindowOptions,
) -> Result<WindowSummary, Diagnostic> {
    let reset = session.player.and_then(|p| snapshot(&session, p));
    let mut handler = Handler {
        control: opts.control.take(),
        steering: opts.steering,
        camera: opts.camera,
        session,
        requests: 0,
        reset,
        opts,
        gfx: None,
        paused_by_suspend: false,
        error: None,
        keys: Keys::default(),
        touch: Touches::default(),
        motion: None,
        tilt: TiltSteering::default(),
        input: VehicleInput::default(),
        aids: DriverAids::default(),
        chase: ChaseSmoothing::default(),
        started: Instant::now(),
        last: Instant::now(),
        accumulator: 0.0,
        frames: 0,
        ticks: 0,
        last_title: Instant::now(),
        profile: ([0.0; 5], 0, Instant::now()),
        audio: None,
        look: (0.0, 0.0),
        drag: None,
        mouse_down: false,
        governor: None,
        thermal_probe: thermal::ThermalProbe::new(),
        heat: thermal::Thermal::Nominal,
        heat_read: None,
    };
    if handler.opts.governor
        && let Some(tier) = handler.opts.quality
    {
        let fps = if tier == wp_world::QualityTier::Low {
            30.0
        } else {
            60.0
        };
        handler.governor = Some(wp_render::governor::Governor::new(fps));
    }
    if handler.opts.audio {
        handler.audio = Some(audio::EngineAudio::start());
    }
    event_loop
        .run_app(&mut handler)
        .map_err(|e| Diagnostic::error("WP-P002", format!("window loop failed: {e}")))?;
    if let Some(e) = handler.error {
        return Err(e);
    }
    Ok(WindowSummary {
        frames: handler.frames,
        ticks: handler.ticks,
        control_requests: handler.requests,
        real_seconds: handler.started.elapsed().as_secs_f64(),
        sim_seconds: handler.session.app.time().seconds(),
        gpu: handler.gfx.as_ref().map(|g| g.gpu.info()),
        render: handler.gfx.as_ref().map(|g| g.renderer.stats()),
        governor: handler.governor.as_ref().map(|g| GovernorSummary {
            step: g.level(),
            changes: g.changes,
            thermal: g.thermal(),
            frame_ms: g.smoothed_ms(),
            target_fps: g.target_fps,
        }),
        steering: handler.steering,
        replay: handler.session.stop_recording(),
    })
}

/// The governor at the end of a window's run.
#[derive(Clone, Debug, Serialize)]
pub struct GovernorSummary {
    /// 0 (full quality) to 5 (lowest resolution, effects off, 30 fps cap).
    pub step: usize,
    pub changes: u32,
    pub thermal: thermal::Thermal,
    /// Smoothed frame time, ms, and the frame rate aimed for.
    pub frame_ms: f32,
    pub target_fps: f32,
}

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    renderer: Renderer,
}

/// On phones the display hardware scales the picture up to the screen for
/// free, so the engine draws straight into a smaller surface (the tier's
/// height, times the governor's scale) instead of upscaling on the GPU,
/// which cost a budget phone a third of its frame.
const HARDWARE_SCALING: bool = cfg!(any(target_os = "android", target_os = "ios"));

impl Gfx {
    /// Size the surface for the window: on phones capped to `tier`'s height
    /// and scaled by `scale`; elsewhere the window's size.
    fn fit(&mut self, tier: Option<wp_world::QualityTier>, scale: f32) {
        let size = self.window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let (w, h) = if HARDWARE_SCALING {
            let cap = tier.map_or(h, |t| t.max_height()).min(h);
            let k = cap as f32 / h as f32 * scale.clamp(0.25, 1.0);
            (
                ((w as f32 * k).round() as u32).max(1),
                ((h as f32 * k).round() as u32).max(1),
            )
        } else {
            (w, h)
        };
        if (w, h) != (self.config.width, self.config.height) {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.gpu.device, &self.config);
        }
        self.renderer.resize(&self.gpu, w, h);
    }
}

#[derive(Default)]
struct Keys {
    throttle: bool,
    brake: bool,
    left: bool,
    right: bool,
    /// Q / E: glance left / right in the cab.
    look_left: bool,
    look_right: bool,
}

impl Keys {
    fn any(&self) -> bool {
        self.throttle || self.brake || self.left || self.right
    }
}

/// (start, current) positions of one touch, in pixels.
type TouchPoint = ((f64, f64), (f64, f64));

/// Two-thumb touch layout for phones and tablets:
/// - left half: drag sideways from where the thumb landed to steer;
/// - right half: upper part is throttle, lower part is brake;
/// - a tap in the top strip cycles the camera (top right corner: switch
///   between thumb and tilt steering).
///
/// With tilt steering the left half brakes and the right half accelerates.
#[derive(Default)]
struct Touches {
    /// Touch id → where the thumb landed and where it is now.
    active: std::collections::BTreeMap<u64, TouchPoint>,
    size: (f64, f64),
}

impl Touches {
    fn any(&self) -> bool {
        !self.active.is_empty()
    }

    /// (throttle, brake, steer) targets from the current touches.
    fn targets(&self, tilt: bool) -> (f32, f32, Option<f32>) {
        let (w, h) = (self.size.0.max(1.0), self.size.1.max(1.0));
        let (mut throttle, mut brake, mut steer) = (0.0, 0.0, None);
        for (start, pos) in self.active.values() {
            if tilt {
                if start.0 < w / 2.0 {
                    brake = 1.0;
                } else {
                    throttle = 1.0;
                }
            } else if start.0 < w / 2.0 {
                let dx = (pos.0 - start.0) / (w * 0.12);
                steer = Some((-dx as f32).clamp(-1.0, 1.0));
            } else if pos.1 < h * 0.6 {
                throttle = 1.0;
            } else {
                brake = 1.0;
            }
        }
        (throttle, brake, steer)
    }
}

/// Pose and part state to restore on R.
type Snapshot = wp_vehicle::replay::StartState;

fn snapshot(session: &Session, e: Entity) -> Option<Snapshot> {
    wp_vehicle::replay::StartState::capture(&session.app.world, e)
}

struct Handler {
    session: Session,
    control: Option<Receiver<Pending>>,
    requests: u64,
    reset: Option<Snapshot>,
    opts: WindowOptions,
    gfx: Option<Gfx>,
    /// Set when the OS took the surface away (app in background on mobile).
    paused_by_suspend: bool,
    error: Option<Diagnostic>,
    keys: Keys,
    touch: Touches,
    steering: settings::Steering,
    /// The motion sensor, while tilt steering is on and the app is in front.
    motion: Option<motion::MotionSensor>,
    tilt: TiltSteering,
    input: VehicleInput,
    /// Progressive brakes, speed-sensitive steering, counter-steer.
    aids: DriverAids,
    camera: CameraMode,
    chase: ChaseSmoothing,
    started: Instant,
    last: Instant,
    accumulator: f64,
    frames: u64,
    ticks: u64,
    last_title: Instant,
    /// Where frame time goes (ms summed since the last report): simulation,
    /// extraction, waiting for a surface texture (the GPU is behind),
    /// rendering (encode + submit), presenting; and the frames counted.
    profile: ([f64; 5], u32, Instant),
    /// Engine sound, if a sound device opened.
    audio: Option<audio::EngineAudio>,
    /// Head turn in the cab (yaw, pitch), rad, and the mouse drag driving it.
    look: (f32, f32),
    drag: Option<(f64, f64)>,
    mouse_down: bool,
    /// Frame-time and heat control, when the window has a quality tier.
    governor: Option<wp_render::governor::Governor>,
    thermal_probe: thermal::ThermalProbe,
    heat: thermal::Thermal,
    heat_read: Option<Instant>,
}

impl Handler {
    fn init_graphics(&mut self, event_loop: &ActiveEventLoop) -> Result<Gfx, Diagnostic> {
        let attrs = Window::default_attributes()
            .with_title(&self.opts.title)
            .with_inner_size(LogicalSize::new(self.opts.width, self.opts.height));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| Diagnostic::error("WP-P001", format!("cannot open a window: {e}")))?,
        );
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())),
        );
        let surface = instance.create_surface(window.clone()).map_err(|e| {
            Diagnostic::error("WP-R004", format!("cannot render to the window: {e}"))
        })?;
        let gpu = Gpu::for_surface(instance, &surface)?;
        let info = gpu.info();
        log_line(&format!(
            "gpu: {} ({}, {})",
            info.name, info.backend, info.device_type
        ));
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| Diagnostic::error("WP-R004", "the GPU cannot present to this window"))?;
        let caps = surface.get_capabilities(&gpu.adapter);
        if let Some(srgb) = caps.formats.iter().find(|f| f.is_srgb()) {
            config.format = *srgb;
        }
        surface.configure(&gpu.device, &config);
        let samples = self
            .opts
            .quality
            .map_or(wp_render::MSAA_SAMPLES, |q| q.msaa_samples());
        let cheap = self.opts.quality == Some(wp_world::QualityTier::Low);
        let mut renderer = Renderer::with_samples(
            &gpu,
            config.format,
            config.width,
            config.height,
            samples,
            cheap,
        );
        // GPU time per stage goes into the frame report (logged every 5 s).
        renderer.set_gpu_timing(&gpu, true);
        if let Some(q) = self.opts.quality {
            renderer.set_quality(&gpu, q);
            renderer.set_streaming(Some(q.view_distance_m()));
        }
        let mut gfx = Gfx {
            window,
            surface,
            config,
            gpu,
            renderer,
        };
        if HARDWARE_SCALING {
            // The surface is the size to draw at: no GPU upscaling.
            gfx.renderer.set_max_height(&gfx.gpu, None);
            gfx.fit(self.opts.quality, 1.0);
            log_line(&format!(
                "drawing at {}×{} (the display scales it to the screen)",
                gfx.config.width, gfx.config.height
            ));
        }
        Ok(gfx)
    }

    fn key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) {
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        let down = event.state == ElementState::Pressed;
        match code {
            KeyCode::KeyW | KeyCode::ArrowUp => self.keys.throttle = down,
            KeyCode::KeyS | KeyCode::ArrowDown => self.keys.brake = down,
            KeyCode::KeyA | KeyCode::ArrowLeft => self.keys.left = down,
            KeyCode::KeyD | KeyCode::ArrowRight => self.keys.right = down,
            KeyCode::KeyQ => self.keys.look_left = down,
            KeyCode::KeyE => self.keys.look_right = down,
            KeyCode::KeyC if down && !event.repeat => self.camera = self.camera.next(),
            KeyCode::KeyP if down && !event.repeat => self.session.paused = !self.session.paused,
            KeyCode::KeyR if down && !event.repeat => self.reset_player(),
            KeyCode::Escape if down => event_loop.exit(),
            _ => {}
        }
    }

    fn reset_player(&mut self) {
        let (Some(e), Some(start)) = (self.session.player, self.reset.clone()) else {
            return;
        };
        start.apply(&mut self.session.app.world, e);
        self.session.note_reset();
        self.input = VehicleInput::default();
        self.aids = DriverAids::default();
        self.tilt.recentre();
        self.chase.snap();
    }

    /// Start (or stop) the motion sensor to match the steering mode; the way
    /// the phone is held when tilt starts is straight ahead.
    fn sync_motion(&mut self) {
        if self.steering != settings::Steering::Tilt {
            self.motion = None;
            return;
        }
        if self.motion.is_none() {
            self.motion = match self.opts.fake_tilt_deg {
                Some(deg) => Some(motion::MotionSensor::fake(deg, 1.0)),
                None => motion::MotionSensor::open(),
            };
            self.tilt = TiltSteering::default();
            if self.motion.is_none() {
                log_line("no motion sensor: steering by touch");
                self.steering = settings::Steering::Touch;
            }
        }
    }

    /// Switch between thumb and tilt steering, and remember the choice.
    fn toggle_steering(&mut self) {
        self.steering = self.steering.other();
        self.motion = None;
        self.sync_motion();
        log_line(&format!("steering: {:?}", self.steering).to_lowercase());
        if let Some(path) = &self.opts.settings
            && let Err(e) = settings::save_steering(path, self.steering)
        {
            log_line(&format!("steering not saved: {e}"));
        }
    }

    /// Smooth digital keys into analogue controls, like a pad trigger.
    fn update_input(&mut self, dt: f32) {
        let k = &self.keys;
        let tilting = self.motion.is_some();
        let (touch_throttle, touch_brake, mut touch_steer) = self.touch.targets(tilting);
        if let Some(up) = self.motion.as_ref().and_then(|m| m.up()) {
            touch_steer = Some(self.tilt.update(up, dt));
        }
        let key_steer = match (k.left, k.right) {
            (true, false) => Some(1.0),
            (false, true) => Some(-1.0),
            _ => None,
        };
        let controls = Controls {
            accelerate: if k.throttle { 1.0 } else { touch_throttle },
            brake: if k.brake { 1.0 } else { touch_brake },
            steer: key_steer.or(touch_steer).unwrap_or(0.0),
            // A thumb gives an analogue position: follow it quickly.
            analog_steer: key_steer.is_none() && touch_steer.is_some(),
        };
        let w = &self.session.app.world;
        let Some(e) = self.session.player else { return };
        let def = w.get::<Vehicle>(e).and_then(|v| {
            w.get_resource::<VehicleLibrary>()
                .and_then(|l| l.0.get(&v.model).cloned())
        });
        if let (Some(def), Some(motion)) = (def, wp_vehicle::telemetry(w, e)) {
            self.input = self.aids.update(controls, &motion, &def, dt);
        }
    }

    /// Answer every waiting control request before this frame's ticks.
    fn serve_control(&mut self) {
        let Some(rx) = &self.control else { return };
        while let Ok(p) = rx.try_recv() {
            self.requests += 1;
            let _ = p
                .reply
                .send(self.session.handle_line_with(&p.line, &p.push));
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let real_dt = (now - self.last).as_secs_f64().min(0.25);
        self.last = now;
        self.serve_control();
        let tick_dt = self.session.app.time().dt() as f64;
        let mut lap = Instant::now();
        let mut split = |slot: usize, prof: &mut [f64; 5]| {
            let t = Instant::now();
            prof[slot] += (t - lap).as_secs_f64() * 1000.0;
            lap = t;
        };
        let mut prof = [0.0f64; 5];
        if !self.session.paused {
            self.accumulator += real_dt;
            let mut steps = 0;
            while self.accumulator >= tick_dt && steps < 8 {
                self.update_input(tick_dt as f32);
                // The keyboard drives only while it is in use, so a control client can drive too.
                let keyboard_active = self.keys.any()
                    || self.touch.any()
                    || self.motion.is_some()
                    || self.input != VehicleInput::default();
                if keyboard_active
                    && let Some(e) = self.session.player
                    && let Some(mut i) = self.session.app.world.get_mut::<VehicleInput>(e)
                {
                    *i = self.input;
                }
                self.session.tick();
                self.ticks += 1;
                self.accumulator -= tick_dt;
                steps += 1;
            }
            if steps == 8 {
                self.accumulator = 0.0; // fell behind; drop time rather than spiral
            }
        }

        if let Some(a) = &self.audio {
            let sound = match self.session.player {
                Some(p) if !self.session.paused && !self.paused_by_suspend => {
                    audio::sound_sources(&self.session.app.world, p)
                }
                _ => Vec::new(),
            };
            a.set(sound);
        }
        let heading = self
            .session
            .player
            .and_then(|e| self.session.app.world.get::<Pose>(e))
            .map(|p| self.chase.update(p.heading, real_dt as f32));
        // Glance with Q / E, or drag with the mouse; let go and the view
        // swings back to the road.
        let dt = real_dt as f32;
        let glance = match (self.keys.look_left, self.keys.look_right) {
            (true, false) => Some(1.4),
            (false, true) => Some(-1.4),
            _ => None,
        };
        let ease = |v: f32, to: f32, rate: f32| v + (to - v) * (1.0 - (-rate * dt).exp());
        match glance {
            Some(to) => self.look = (ease(self.look.0, to, 8.0), ease(self.look.1, 0.0, 8.0)),
            None if !self.mouse_down => {
                self.look = (ease(self.look.0, 0.0, 4.0), ease(self.look.1, 0.0, 4.0))
            }
            None => {}
        }
        self.look = (self.look.0.clamp(-2.4, 2.4), self.look.1.clamp(-0.6, 0.6));
        split(0, &mut prof);
        let frame = extract_view(
            &self.session.app.world,
            self.camera,
            &ViewOptions {
                chase_heading: heading,
                look: self.look,
            },
        );
        split(1, &mut prof);
        let Some(gfx) = self.gfx.as_mut() else { return };
        if let (Some(gov), Some(tier)) = (self.governor.as_mut(), self.opts.quality) {
            // The OS's heat state, every two seconds (or the test override).
            if self
                .heat_read
                .is_none_or(|t| t.elapsed().as_secs_f32() > 2.0)
            {
                self.heat_read = Some(Instant::now());
                self.heat = self
                    .opts
                    .thermal
                    .or_else(|| self.thermal_probe.read())
                    .unwrap_or_default();
            }
            if let Some(level) = gov.update(real_dt as f32 * 1000.0, self.heat) {
                let step = gov.step();
                if HARDWARE_SCALING {
                    // Smaller surface, not a GPU upscale.
                    gfx.renderer.apply_step(
                        &gfx.gpu,
                        tier,
                        wp_render::governor::Step {
                            render_scale: 1.0,
                            ..step
                        },
                    );
                    gfx.fit(Some(tier), step.render_scale);
                } else {
                    gfx.renderer.apply_step(&gfx.gpu, tier, step);
                }
                log_line(&format!(
                    "render step {level} ({:?} heat, {:.1} ms frames): {:?}",
                    self.heat,
                    gov.smoothed_ms(),
                    gov.step()
                ));
            }
        }
        split(1, &mut prof);
        match gfx.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex)
            | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                split(2, &mut prof);
                let view = tex
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                gfx.renderer.render(&gfx.gpu, &frame, &view);
                split(3, &mut prof);
                gfx.gpu.queue.present(tex);
                split(4, &mut prof);
                self.frames += 1;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gfx.surface.configure(&gfx.gpu.device, &gfx.config);
            }
            _ => {}
        }

        // Profiling: parts of the frame to leave out (`adb shell setprop
        // debug.waypoint.skip grass,post` on Android, WP_SKIP elsewhere).
        if self.profile.1.is_multiple_of(60) {
            let skip = debug_skip();
            let parts: Vec<&str> = skip
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .collect();
            gfx.renderer.set_skip(&parts);
        }
        // Every 5 s: where the time went, per frame.
        let (sum, n, since) = &mut self.profile;
        for (s, p) in sum.iter_mut().zip(prof) {
            *s += p;
        }
        *n += 1;
        if since.elapsed().as_secs_f32() > 5.0 {
            let k = (*n).max(1) as f64;
            let stats = gfx.renderer.stats();
            let gpu_ms: String = gfx
                .renderer
                .gpu_times()
                .iter()
                .map(|(l, ms)| format!(" {l} {ms:.1}"))
                .collect();
            log_line(&format!(
                "frame {:.1} ms: sim {:.1}, extract {:.1}, wait gpu {:.1}, render {:.1}, present {:.1} ({} draws, {}k tris, {}×{}{}); gpu ms:{}",
                since.elapsed().as_secs_f64() * 1000.0 / k,
                sum[0] / k,
                sum[1] / k,
                sum[2] / k,
                sum[3] / k,
                sum[4] / k,
                stats.draw_calls,
                stats.triangles / 1000,
                gfx.renderer.internal_size().0,
                gfx.renderer.internal_size().1,
                {
                    let s = debug_skip();
                    if s.is_empty() {
                        String::new()
                    } else {
                        format!(", skipping {s}")
                    }
                },
                if gpu_ms.is_empty() {
                    " n/a".to_string()
                } else {
                    gpu_ms
                }
            ));
            *sum = [0.0; 5];
            *n = 0;
            *since = Instant::now();
        }
        if self.last_title.elapsed().as_secs_f32() > 0.2 {
            self.last_title = Instant::now();
            let w = &self.session.app.world;
            let player = self.session.player;
            let status = player
                .and_then(|e| wp_vehicle::telemetry(w, e))
                .map(|t| {
                    format!(
                        "{:.0} km/h · gear {} · {:.0} rpm · lat {:+.2} g",
                        t.speed_kmh, t.gear, t.engine_rpm, t.lateral_g
                    )
                })
                .unwrap_or_default();
            let model = player
                .and_then(|e| w.get::<Vehicle>(e).map(|v| v.model.clone()))
                .unwrap_or_default();
            let paused = if self.session.paused {
                " · PAUSED"
            } else {
                ""
            };
            let mission = w
                .get_resource::<wp_mission::MissionStatus>()
                .map(|m| {
                    format!(
                        " · {} · {} on board · score {}",
                        m.objective, m.on_board, m.score.total
                    )
                })
                .unwrap_or_default();
            gfx.window.set_title(&format!(
                "{} · {model} · {status}{mission} · camera {:?}{paused}   [W/S drive (hold S when stopped to reverse), A/D steer, C camera, R reset, P pause]",
                self.opts.title, self.camera
            ));
        }
    }
}

impl ApplicationHandler for Handler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        match self.init_graphics(event_loop) {
            Ok(g) => {
                g.window.request_redraw();
                self.gfx = Some(g);
                self.last = Instant::now();
                self.sync_motion();
                if self.paused_by_suspend {
                    self.session.paused = false;
                    self.paused_by_suspend = false;
                }
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    /// Mobile apps lose their surface in the background: drop all graphics
    /// (rebuilt in `resumed`) and stop the clock so no time is skipped.
    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.gfx = None;
        // No sensor in the background (battery); it restarts on resume.
        self.motion = None;
        if !self.session.paused {
            self.session.paused = true;
            self.paused_by_suspend = true;
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gfx.as_mut()
                    && size.width > 0
                    && size.height > 0
                {
                    if HARDWARE_SCALING {
                        let scale = self
                            .governor
                            .as_ref()
                            .map_or(1.0, |gov| gov.step().render_scale);
                        g.fit(self.opts.quality, scale);
                    } else {
                        g.config.width = size.width;
                        g.config.height = size.height;
                        g.surface.configure(&g.gpu.device, &g.config);
                        g.renderer.resize(&g.gpu, size.width, size.height);
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(&event, event_loop),
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                self.mouse_down = state == winit::event::ElementState::Pressed;
                if !self.mouse_down {
                    self.drag = None;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.mouse_down && self.camera == CameraMode::Cab {
                    if let Some((x, y)) = self.drag {
                        self.look.0 -= ((position.x - x) * 0.005) as f32;
                        self.look.1 -= ((position.y - y) * 0.004) as f32;
                    }
                    self.drag = Some((position.x, position.y));
                }
            }
            WindowEvent::Touch(t) => {
                use winit::event::TouchPhase;
                if let Some(g) = &self.gfx {
                    let size = g.window.inner_size();
                    self.touch.size = (size.width as f64, size.height as f64);
                }
                let p = (t.location.x, t.location.y);
                match t.phase {
                    TouchPhase::Started if p.1 < self.touch.size.1 * 0.12 => {
                        if p.0 > self.touch.size.0 * 0.85 {
                            self.toggle_steering();
                        } else {
                            self.camera = self.camera.next()
                        }
                    }
                    TouchPhase::Started => {
                        self.touch.active.insert(t.id, (p, p));
                    }
                    TouchPhase::Moved => {
                        if let Some(e) = self.touch.active.get_mut(&t.id) {
                            e.1 = p;
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        self.touch.active.remove(&t.id);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.frame();
                if let Some(limit) = self.opts.exit_after_s
                    && self.started.elapsed().as_secs_f32() >= limit
                {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        use winit::event_loop::ControlFlow;
        let Some(g) = &self.gfx else { return };
        // A frame-rate cap from the governor: sleep until the next frame is due.
        let cap = self.governor.as_ref().and_then(|gov| gov.step().fps_cap);
        match cap {
            Some(fps) => {
                let next = self.last + std::time::Duration::from_secs_f32(1.0 / fps);
                if Instant::now() >= next {
                    g.window.request_redraw();
                } else {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(next));
                }
            }
            None => {
                event_loop.set_control_flow(ControlFlow::Wait);
                g.window.request_redraw();
            }
        }
    }
}
