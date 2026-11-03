//! Interactive play: a window, keyboard input and a fixed-timestep loop.
//!
//! The simulation always advances in whole 60 Hz ticks (so it stays
//! deterministic); rendering runs at display rate and only reads the world.
//! Controls: W/↑ throttle, S/↓ brake, A/← and D/→ steer, C camera,
//! R reset, P pause, Esc quit.

use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};
use wp_app::App;
use wp_core::Diagnostic;
use wp_ecs::Entity;
use wp_physics::{Pose, Velocity};
use wp_render::{CameraMode, Renderer, extract};
use wp_rhi::{Gpu, GpuInfo, wgpu};
use wp_vehicle::{Vehicle, VehicleInput, VehicleLibrary};

pub struct WindowOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub camera: CameraMode,
    /// Close automatically after this many real seconds (for smoke tests).
    pub exit_after_s: Option<f32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WindowSummary {
    pub frames: u64,
    pub ticks: u64,
    pub real_seconds: f64,
    pub sim_seconds: f64,
    pub gpu: Option<GpuInfo>,
}

/// Run `app` in a window until the user closes it. `player` receives keyboard input.
pub fn run_windowed(
    app: App,
    player: Option<Entity>,
    opts: WindowOptions,
) -> Result<WindowSummary, Diagnostic> {
    let event_loop = EventLoop::new().map_err(|e| {
        Diagnostic::error("WP-P001", format!("cannot open a window: {e}"))
            .help("use --headless on machines without a display")
    })?;
    let reset = player.and_then(|p| snapshot(&app, p));
    let mut handler = Handler {
        app,
        player,
        reset,
        opts,
        gfx: None,
        error: None,
        keys: Keys::default(),
        input: VehicleInput::default(),
        camera: CameraMode::Chase,
        paused: false,
        started: Instant::now(),
        last: Instant::now(),
        accumulator: 0.0,
        frames: 0,
        ticks: 0,
        last_title: Instant::now(),
    };
    handler.camera = handler.opts.camera;
    event_loop
        .run_app(&mut handler)
        .map_err(|e| Diagnostic::error("WP-P002", format!("window loop failed: {e}")))?;
    if let Some(e) = handler.error {
        return Err(e);
    }
    Ok(WindowSummary {
        frames: handler.frames,
        ticks: handler.ticks,
        real_seconds: handler.started.elapsed().as_secs_f64(),
        sim_seconds: handler.app.time().seconds(),
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

/// Pose and part state to restore on R.
type Snapshot = (Pose, Vehicle);

fn snapshot(app: &App, e: Entity) -> Option<Snapshot> {
    Some((
        *app.world.get::<Pose>(e)?,
        app.world.get::<Vehicle>(e)?.clone(),
    ))
}

struct Handler {
    app: App,
    player: Option<Entity>,
    reset: Option<Snapshot>,
    opts: WindowOptions,
    gfx: Option<Gfx>,
    error: Option<Diagnostic>,
    keys: Keys,
    input: VehicleInput,
    camera: CameraMode,
    paused: bool,
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
        let renderer = Renderer::new(&gpu, config.format, config.width, config.height);
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
            KeyCode::KeyP if down && !event.repeat => self.paused = !self.paused,
            KeyCode::KeyR if down && !event.repeat => self.reset_player(),
            KeyCode::Escape if down => event_loop.exit(),
            _ => {}
        }
    }

    fn reset_player(&mut self) {
        let (Some(e), Some((pose, vehicle))) = (self.player, self.reset.clone()) else {
            return;
        };
        let w = &mut self.app.world;
        w.insert(e, pose);
        w.insert(e, Velocity::default());
        w.insert(e, vehicle);
        self.input = VehicleInput::default();
    }

    /// Smooth digital keys into analogue controls, like a pad trigger.
    fn update_input(&mut self, dt: f32) {
        let approach =
            |v: f32, target: f32, rate: f32| v + (target - v).clamp(-rate * dt, rate * dt);
        let k = &self.keys;
        self.input.throttle =
            approach(self.input.throttle, if k.throttle { 1.0 } else { 0.0 }, 3.0);
        self.input.brake = approach(self.input.brake, if k.brake { 1.0 } else { 0.0 }, 5.0);
        let steer_target = match (k.left, k.right) {
            (true, false) => 1.0,
            (false, true) => -1.0,
            _ => 0.0,
        };
        // Steering returns to centre faster than it winds on.
        let rate = if steer_target == 0.0 { 3.5 } else { 1.8 };
        self.input.steer = approach(self.input.steer, steer_target, rate);
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let real_dt = (now - self.last).as_secs_f64().min(0.25);
        self.last = now;
        let tick_dt = self.app.time().dt() as f64;
        if !self.paused {
            self.accumulator += real_dt;
            let mut steps = 0;
            while self.accumulator >= tick_dt && steps < 8 {
                self.update_input(tick_dt as f32);
                if let Some(e) = self.player
                    && let Some(mut i) = self.app.world.get_mut::<VehicleInput>(e)
                {
                    *i = self.input;
                }
                self.app.tick();
                self.ticks += 1;
                self.accumulator -= tick_dt;
                steps += 1;
            }
            if steps == 8 {
                self.accumulator = 0.0; // fell behind; drop time rather than spiral
            }
        }

        let frame = extract(&self.app.world, self.camera);
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
            let status = self
                .player
                .and_then(|e| wp_vehicle::telemetry(&self.app.world, e))
                .map(|t| {
                    format!(
                        "{:.0} km/h · gear {} · {:.0} rpm · lat {:+.2} g",
                        t.speed_kmh, t.gear, t.engine_rpm, t.lateral_g
                    )
                })
                .unwrap_or_default();
            let model = self
                .player
                .and_then(|e| self.app.world.get::<Vehicle>(e).map(|v| v.model.clone()))
                .filter(|m| {
                    self.app
                        .world
                        .get_resource::<VehicleLibrary>()
                        .is_some_and(|l| l.0.contains_key(m))
                })
                .unwrap_or_default();
            let paused = if self.paused { " · PAUSED" } else { "" };
            gfx.window.set_title(&format!(
                "{} · {model} · {status} · camera {:?}{paused}   [W/S drive, A/D steer, C camera, R reset, P pause]",
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
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
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
