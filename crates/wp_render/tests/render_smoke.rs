//! Renders the sandbox hatchback headless. Set WP_RENDER_OUT=<dir> to keep the PNGs.
//! Skips (with a message) when no GPU adapter is available.

use std::path::Path;
use wp_app::App;
use wp_core::glam::Vec2;
use wp_render::{CameraMode, extract, render_rgba};
use wp_vehicle::{VehicleDef, VehicleInput};

fn world_with_car() -> App {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sandbox");
    let def: VehicleDef = wp_data::load_ron(&root, "vehicles/hatchback.vehicle.ron")
        .unwrap()
        .value;
    let mut app = App::new();
    app.add_plugin(wp_physics::PhysicsPlugin)
        .add_plugin(wp_world::WorldPlugin)
        .add_plugin(wp_vehicle::VehiclePlugin);
    app.finish().unwrap();
    let e = wp_vehicle::spawn_vehicle(&mut app.world, &def, Vec2::ZERO, 0.3);
    app.world.insert(e, wp_world::Player);
    *app.world.get_mut::<VehicleInput>(e).unwrap() = VehicleInput {
        throttle: 0.6,
        brake: 0.0,
        steer: 0.35,
        reverse: false,
    };
    app.run_ticks(150);
    app
}

#[test]
fn renders_a_car() {
    let gpu = match wp_rhi::Gpu::headless() {
        Ok(g) => g,
        Err(d) => {
            eprintln!("skipping render test: {}", d.render());
            return;
        }
    };
    let app = world_with_car();
    let (w, h) = (960, 540);
    for (name, mode) in CameraMode::ALL {
        let frame = extract(&app.world, mode);
        assert!(
            frame.items.len() >= 10,
            "vehicle produced {} draw items",
            frame.items.len()
        );
        let rgba = render_rgba(&gpu, &frame, w, h).unwrap();
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        if let Ok(dir) = std::env::var("WP_RENDER_OUT") {
            wp_rhi::write_png(&Path::new(&dir).join(format!("{name}.png")), w, h, &rgba).unwrap();
        }
        // The image must not be a flat colour: count distinct coarse colours.
        let mut seen = std::collections::BTreeSet::new();
        for px in rgba.chunks(4).step_by(97) {
            seen.insert((px[0] / 16, px[1] / 16, px[2] / 16));
        }
        assert!(
            seen.len() > 20,
            "{name}: only {} colours, image looks empty",
            seen.len()
        );
    }
    eprintln!("rendered on {:?}", gpu.info());
}

#[test]
fn upscaled_frames_match_full_resolution() {
    let gpu = match wp_rhi::Gpu::headless() {
        Ok(g) => g,
        Err(d) => {
            eprintln!("skipping render test: {}", d.render());
            return;
        }
    };
    let app = world_with_car();
    let frame = extract(&app.world, CameraMode::Chase);
    let (w, h) = (960, 540);
    let full = render_rgba(&gpu, &frame, w, h).unwrap();
    // The same frame drawn at 270 lines and scaled up.
    let target = wp_rhi::OffscreenTarget::new(&gpu, w, h);
    let mut r = wp_render::Renderer::new(&gpu, wp_rhi::OFFSCREEN_FORMAT, w, h);
    r.set_max_height(&gpu, Some(270));
    assert_eq!(r.internal_size(), (480, 270));
    r.render(&gpu, &frame, &target.view);
    let low = target.read_rgba(&gpu).unwrap();
    assert_eq!(low.len(), full.len());
    // Softer, but the same picture: small mean difference per channel.
    let diff: f64 = full
        .iter()
        .zip(&low)
        .map(|(a, b)| (*a as f64 - *b as f64).abs())
        .sum::<f64>()
        / full.len() as f64;
    assert!(diff < 6.0, "mean difference {diff:.2} per channel");
    if let Ok(dir) = std::env::var("WP_RENDER_OUT") {
        wp_rhi::write_png(&Path::new(&dir).join("upscaled.png"), w, h, &low).unwrap();
    }
}

/// A 6 km straight road: windows keep only the tiles near the camera on the
/// GPU, built on a worker thread; offscreen renders keep the whole road.
#[test]
fn road_tiles_stream_around_the_camera() {
    let gpu = match wp_rhi::Gpu::headless() {
        Ok(g) => g,
        Err(d) => {
            eprintln!("skipping render test: {}", d.render());
            return;
        }
    };
    let def: wp_road::RoadNetworkDef = wp_data::parse_ron(
        "long.road.ron",
        r#"(name: "long", nodes: [(id: "a", position: (-3000.0, 0.0)), (id: "b", position: (3000.0, 0.0))],
            roads: [(id: "r", from: "a", to: "b", lanes_forward: 2, lanes_backward: 2, sidewalk_m: 2.0)])"#,
    )
    .unwrap();
    let mut app = world_with_car();
    app.world
        .insert_resource(wp_world::Roads(std::sync::Arc::new(
            wp_road::RoadNetwork::build(&def),
        )));
    let frame = extract(&app.world, CameraMode::Chase);
    let (w, h) = (320, 180);
    let target = wp_rhi::OffscreenTarget::new(&gpu, w, h);

    let mut r = wp_render::Renderer::new(&gpu, wp_rhi::OFFSCREEN_FORMAT, w, h);
    r.render(&gpu, &frame, &target.view);
    let all = r.stats();
    assert!(all.road_tiles_total >= 15, "{all:?}");
    assert_eq!(all.road_tiles_resident, all.road_tiles_total);
    assert!(
        all.road_tiles_drawn < all.road_tiles_total,
        "tiles behind the camera are culled: {all:?}"
    );

    let mut s = wp_render::Renderer::new(&gpu, wp_rhi::OFFSCREEN_FORMAT, w, h);
    s.set_streaming(Some(700.0));
    let start = std::time::Instant::now();
    loop {
        s.render(&gpu, &frame, &target.view);
        let st = s.stats();
        if st.road_tiles_total > 0 && st.road_tiles_resident >= 4 {
            break;
        }
        assert!(
            start.elapsed().as_secs() < 30,
            "tiles never arrived: {st:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // A few more frames: everything in range arrives, nothing far away does.
    for _ in 0..5 {
        s.render(&gpu, &frame, &target.view);
    }
    let st = s.stats();
    assert!(st.road_tiles_resident <= 10, "{st:?}");
    assert!(st.road_tiles_resident < st.road_tiles_total / 2, "{st:?}");
    assert!(st.road_tiles_drawn >= 1, "{st:?}");
}

/// The sun's shadow map darkens the ground beside the car; turning it off
/// falls back to blob shadows.
#[test]
fn sun_shadows_darken_the_ground() {
    let gpu = match wp_rhi::Gpu::headless() {
        Ok(g) => g,
        Err(d) => {
            eprintln!("skipping render test: {}", d.render());
            return;
        }
    };
    let app = world_with_car();
    let frame = extract(&app.world, CameraMode::Showcase);
    let (w, h) = (480, 270);
    let target = wp_rhi::OffscreenTarget::new(&gpu, w, h);
    let draw = |shadows: Option<wp_render::ShadowSettings>| {
        let mut r = wp_render::Renderer::new(&gpu, wp_rhi::OFFSCREEN_FORMAT, w, h);
        r.set_shadows(&gpu, shadows);
        r.render(&gpu, &frame, &target.view);
        let px = target.read_rgba(&gpu).unwrap();
        px.chunks(4)
            .map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64)
            .sum::<u64>()
    };
    let lit = draw(None);
    let shadowed = draw(Some(wp_render::ShadowSettings::default()));
    assert!(
        shadowed < lit,
        "shadow map should darken the frame: {shadowed} vs {lit}"
    );
}

/// At night the sky darkens, headlights come on and light the road ahead.
#[test]
fn night_has_headlights() {
    let gpu = match wp_rhi::Gpu::headless() {
        Ok(g) => g,
        Err(d) => {
            eprintln!("skipping render test: {}", d.render());
            return;
        }
    };
    let mut app = world_with_car();
    let day = extract(&app.world, CameraMode::Chase);
    assert!(day.lights.is_empty(), "no lights by day");
    app.world.insert_reflected_resource(
        "TimeOfDay",
        wp_world::TimeOfDay {
            hour: 22.0,
            rate: 0.0,
        },
    );
    let night = extract(&app.world, CameraMode::Chase);
    assert!(night.environment.darkness() > 0.9);
    assert_eq!(night.lights.len(), 4, "two headlamps and two tail lamps");
    let (w, h) = (320, 180);
    let mean = |f: &wp_render::FrameView| {
        let px = render_rgba(&gpu, f, w, h).unwrap();
        px.chunks(4)
            .map(|p| p[0] as f64 + p[1] as f64 + p[2] as f64)
            .sum::<f64>()
            / (w * h) as f64
    };
    let mut unlit = night.clone();
    unlit.lights.clear();
    let (d, n, u) = (mean(&day), mean(&night), mean(&unlit));
    assert!(n < d * 0.6, "night should be darker: {n:.0} vs {d:.0}");
    assert!(
        n > u * 1.05,
        "headlights should light the scene: {n:.0} vs {u:.0}"
    );
}

/// A game's look changes the picture: toon bands the light, flat drops highlights.
#[test]
fn art_styles_change_the_picture() {
    let gpu = match wp_rhi::Gpu::headless() {
        Ok(g) => g,
        Err(d) => {
            eprintln!("skipping render test: {}", d.render());
            return;
        }
    };
    let mut app = world_with_car();
    let (w, h) = (320, 180);
    let mut shot = |style: wp_world::ArtStyle| {
        let look = wp_world::LookDef {
            style,
            ..Default::default()
        }
        .resolve();
        app.world.insert_reflected_resource("Look", look);
        let frame = extract(&app.world, CameraMode::Showcase);
        assert_eq!(frame.look.style, style);
        render_rgba(&gpu, &frame, w, h).unwrap()
    };
    let real = shot(wp_world::ArtStyle::Realistic);
    let toon = shot(wp_world::ArtStyle::Toon);
    let flat = shot(wp_world::ArtStyle::Flat);
    let differ = |a: &[u8], b: &[u8]| {
        a.iter()
            .zip(b)
            .filter(|(x, y)| x.abs_diff(**y) > 12)
            .count()
    };
    assert!(
        differ(&real, &toon) > (w * h) as usize,
        "toon looks different"
    );
    assert!(
        differ(&real, &flat) > (w * h) as usize,
        "flat looks different"
    );
    assert!(differ(&toon, &flat) > (w * h) as usize);
}

/// In the cab the driver can turn their head; other cameras ignore it.
#[test]
fn cab_view_looks_around() {
    let app = world_with_car();
    let look = |yaw: f32| {
        let f = wp_render::extract_view(
            &app.world,
            CameraMode::Cab,
            &wp_render::ViewOptions {
                look: (yaw, 0.0),
                ..Default::default()
            },
        );
        (f.camera.target - f.camera.eye).normalize()
    };
    let ahead = look(0.0);
    let left = look(1.4);
    assert!(
        ahead.dot(left) < 0.3,
        "a glance turns the view: {ahead} vs {left}"
    );
    assert!(ahead.cross(left).z > 0.0, "positive yaw looks left");
    let frame = extract(&app.world, CameraMode::Cab);
    assert!(frame.items.len() > 20, "the cab adds a dashboard and wheel");
}

/// Distant models draw a simpler level of detail.
#[test]
fn distant_models_use_fewer_triangles() {
    let gpu = match wp_rhi::Gpu::headless() {
        Ok(g) => g,
        Err(d) => {
            eprintln!("skipping render test: {}", d.render());
            return;
        }
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sandbox");
    let path = "assets/vehicles/hatchback.glb";
    let model = wp_asset::load_model(&root, path).unwrap();
    assert_eq!(model.lods.len(), 2);
    let mut app = world_with_car();
    app.world
        .insert_resource(wp_asset::Models(std::sync::Arc::new(
            wp_asset::ModelLibrary::new([(path.to_string(), std::sync::Arc::new(model))]),
        )));
    let near = extract(&app.world, CameraMode::Showcase);
    let mut far = near.clone();
    let back = (far.camera.eye - far.camera.target).normalize() * 150.0;
    far.camera.eye += back;
    let (w, h) = (320, 180);
    let target = wp_rhi::OffscreenTarget::new(&gpu, w, h);
    let triangles = |f: &wp_render::FrameView| {
        let mut r = wp_render::Renderer::new(&gpu, wp_rhi::OFFSCREEN_FORMAT, w, h);
        r.set_shadows(&gpu, None);
        r.render(&gpu, f, &target.view);
        r.stats().triangles
    };
    let (n, f) = (triangles(&near), triangles(&far));
    assert!(f < n, "far {f} vs near {n} triangles");
}
