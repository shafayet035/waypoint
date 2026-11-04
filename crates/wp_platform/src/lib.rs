//! Interactive play: a window, keyboard input and a fixed-timestep loop.
//!
//! The simulation always advances in whole 60 Hz ticks (so it stays
//! deterministic); rendering runs at display rate and only reads the world.
//! Controls: W/↑ throttle, S/↓ brake (hold at a standstill to reverse), A/← and D/→ steer, C camera,
//! R reset, P pause, Esc quit. On touch screens: left thumb drags to steer,
//! right thumb high for throttle, low for brake, tap the top edge for camera. A control client (`wp run --control <port>`)
//! can query and drive the same session while it runs.

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
use wp_physics::{Pose, Velocity};
use wp_render::{CameraMode, ChaseSmoothing, Renderer, extract_with};
use wp_rhi::{Gpu, GpuInfo, wgpu};
use wp_vehicle::{Controls, DriverAids, Vehicle, VehicleInput, VehicleLibrary};

pub struct WindowOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub camera: CameraMode,
    /// Close automatically after this many real seconds (for smoke tests).
    pub exit_after_s: Option<f32>,
    /// Render at most this tall and scale up (quality tier); `None`: full size.
    pub max_height: Option<u32>,
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
        input: VehicleInput::default(),
        aids: DriverAids::default(),
        chase: ChaseSmoothing::default(),
        started: Instant::now(),
        last: Instant::now(),
        accumulator: 0.0,
        frames: 0,
        ticks: 0,
        last_title: Instant::now(),
    };
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
    })
}

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    renderer: Renderer,
}

#[derive(Default)]
struct Keys {
    throttle: bool,
    brake: bool,
    left: bool,
    right: bool,
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
/// - a tap in the top strip cycles the camera.
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
    fn targets(&self) -> (f32, f32, Option<f32>) {
        let (w, h) = (self.size.0.max(1.0), self.size.1.max(1.0));
        let (mut throttle, mut brake, mut steer) = (0.0, 0.0, None);
        for (start, pos) in self.active.values() {
            if start.0 < w / 2.0 {
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
type Snapshot = (Pose, Vehicle);

fn snapshot(session: &Session, e: Entity) -> Option<Snapshot> {
    let w = &session.app.world;
    Some((*w.get::<Pose>(e)?, w.get::<Vehicle>(e)?.clone()))
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
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| Diagnostic::error("WP-R004", "the GPU cannot present to this window"))?;
        let caps = surface.get_capabilities(&gpu.adapter);
        if let Some(srgb) = caps.formats.iter().find(|f| f.is_srgb()) {
            config.format = *srgb;
        }
        surface.configure(&gpu.device, &config);
        let mut renderer = Renderer::new(&gpu, config.format, config.width, config.height);
        renderer.set_max_height(&gpu, self.opts.max_height);
        Ok(Gfx {
            window,
            surface,
            config,
            gpu,
            renderer,
        })
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
            KeyCode::KeyC if down && !event.repeat => self.camera = self.camera.next(),
            KeyCode::KeyP if down && !event.repeat => self.session.paused = !self.session.paused,
            KeyCode::KeyR if down && !event.repeat => self.reset_player(),
            KeyCode::Escape if down => event_loop.exit(),
            _ => {}
        }
    }

    fn reset_player(&mut self) {
        let (Some(e), Some((pose, vehicle))) = (self.session.player, self.reset.clone()) else {
            return;
        };
        let w = &mut self.session.app.world;
        w.insert(e, pose);
        w.insert(e, Velocity::default());
        w.insert(e, vehicle);
        w.insert(e, VehicleInput::default());
        self.input = VehicleInput::default();
        self.aids = DriverAids::default();
        self.chase.snap();
    }

    /// Smooth digital keys into analogue controls, like a pad trigger.
    fn update_input(&mut self, dt: f32) {
        let k = &self.keys;
        let (touch_throttle, touch_brake, touch_steer) = self.touch.targets();
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
            let _ = p.reply.send(self.session.handle_line(&p.line));
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let real_dt = (now - self.last).as_secs_f64().min(0.25);
        self.last = now;
        self.serve_control();
        let tick_dt = self.session.app.time().dt() as f64;
        if !self.session.paused {
            self.accumulator += real_dt;
            let mut steps = 0;
            while self.accumulator >= tick_dt && steps < 8 {
                self.update_input(tick_dt as f32);
                // The keyboard drives only while it is in use, so a control client can drive too.
                let keyboard_active =
                    self.keys.any() || self.touch.any() || self.input != VehicleInput::default();
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

        let heading = self
            .session
            .player
            .and_then(|e| self.session.app.world.get::<Pose>(e))
            .map(|p| self.chase.update(p.heading, real_dt as f32));
        let frame = extract_with(&self.session.app.world, self.camera, heading);
        let Some(gfx) = self.gfx.as_mut() else { return };
        match gfx.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex)
            | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                let view = tex
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                gfx.renderer.render(&gfx.gpu, &frame, &view);
                gfx.gpu.queue.present(tex);
                self.frames += 1;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gfx.surface.configure(&gfx.gpu.device, &gfx.config);
            }
            _ => {}
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
                    g.config.width = size.width;
                    g.config.height = size.height;
                    g.surface.configure(&g.gpu.device, &g.config);
                    g.renderer.resize(&g.gpu, size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(&event, event_loop),
            WindowEvent::Touch(t) => {
                use winit::event::TouchPhase;
                if let Some(g) = &self.gfx {
                    let size = g.window.inner_size();
                    self.touch.size = (size.width as f64, size.height as f64);
                }
                let p = (t.location.x, t.location.y);
                match t.phase {
                    TouchPhase::Started if p.1 < self.touch.size.1 * 0.12 => {
                        self.camera = self.camera.next()
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }
}
