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
