//! `wp docs`: offline documentation topics and the error catalogue.

use super::*;

// ---- wp docs --------------------------------------------------------------

const DOCS_CLI: &str = "\
wp — Waypoint command line. Every command takes --json and --project <dir>.

  wp new <dir>                 create a project from the sandbox template
  wp check                     validate all files (schemas + cross-references)
  wp schema <type>             JSON Schema: game | vehicle | trailer | road | scene | scenario
  wp docs [topic]              offline docs: cli | errors | errors/<CODE> | scenario | vehicle
  wp run [drive opts]          drive in a window: W/S throttle-brake, A/D steer, C camera, R reset, P pause
  wp run --headless [opts]     simulate without a window; prints the final state (+ --trace, --seconds, --ticks)
  wp run --control <port>      also accept control-protocol requests on 127.0.0.1:<port> (0 = any free port)
  wp shot --at 12.5s [opts]    render one frame to PNG, headless: --camera chase|top|side|showcase --out f.png
  wp serve [drive opts]        control protocol (JSON-RPC 2.0) on stdin/stdout, one request per line
  wp mcp                       Model Context Protocol server on stdin/stdout (tools, resources, screenshots)
  wp test [filter]             run scenario tests; exit 1 if any fail
  wp vehicle test <name> [--manoeuvre accel_0_100|brake_100_0|skidpad|all]
  wp import osm <file.osm> --name N [--drive-side right|left] [--lang en] [--force]
                               OpenStreetMap → roads/N.road.ron + scenes/N.scene.ron (spawn \"start\");
                               export an area at openstreetmap.org (Export → .osm). Keep the ODbL credit.
  wp systems                   resolved system schedule
  wp plugins                   loaded plugins in load order

Drive options (run and shot): --scenario NAME, or --scene PATH --spawn NAME --vehicle NAME
  --throttle 0..1 --brake 0..1 --steer -1..1 --seed N

Agent loop: edit a file → wp check --json → wp test --json → on failure, wp shot --scenario <name> --at <t>
and look at the image → fix. For live work, `wp serve` or `wp mcp`, then call rpc.methods.

Control protocol methods: rpc.methods, sim.status, sim.step, sim.pause, sim.resume, sim.snapshot,
sim.restore, world.entities, world.get, world.set, world.find, world.spawn, world.despawn,
world.resources, input.set, vehicle.telemetry, events.poll, render.screenshot.
";

const DOCS_SCENARIO: &str = "\
Scenario files: tests/<name>.scenario.ron  (full schema: wp schema scenario)

  (
    scene: \"scenes/flat.scene.ron\",
    seed: 7,
    player: (vehicle: \"hatchback\", spawn: \"start\", initial_kmh: 0.0, driver: <Driver>),
    duration_s: 20.0,
    assert: [<Assertion>, ...],
  )

Drivers
  Script([(at_s: 0.0, throttle: 1.0), (at_s: 9.0, brake: 1.0, steer: 0.0)])   keys hold until the next
  Keyboard([(at_s: 1.0, steer: 1.0), (at_s: 2.0, brake: 1.0)])   key presses through the player's driver aids
  Cruise(kmh: 40.0, steer: 0.25)                                              speed controller + fixed steer
  FollowRoute(to_road: \"east_curve\", kmh: 50.0)  plans a lane route on the scene's road network and drives it
  ReverseToDock(dock: \"bay_1\", kmh: 3.0)       backs the towed trailer into a scene dock (pulls up to retry)

Assertions
  ReachSpeed(kmh: 50.0, within_s: 15.0, not_before_s: 8.0)   (not_before_s is optional)
  BrakingDistance(max_m: 36.7)    Mfdd(min_mps2: 5.0)    StoppedBy(s: 14.0)    TurningCircle(max_m: 25.0)
  MaxSpeed(130.0)   MaxLateralG(0.35)   SteadyLateralG(min: 0.4, max: 1.05)
  MaxSideslipDeg(9.0)   no spin or big slide   MaxDecelG(0.4)   no harsh braking
  NoEvent(\"WheelLift\")   ExpectEvent(\"AbsEngaged\")   NoNaN   Deterministic
  RouteFollowed(max_offset_m: 0.6)   FollowRoute finished, never further than this from the lane centre
  Docked(dock: \"bay_1\", max_offset_m: 0.3, max_angle_deg: 3.0)   trailer rear at the dock, square, stopped

Events: GearShift, AbsEngaged, AbsReleased, WheelLift, WheelLanded, LowAirPressure, RolledOver, Jackknife.
Regulation limits used in the sandbox: UN ECE R13 Type-0 braking — buses (M3) from 60 km/h
≤ 0.15v + v²/130 = 36.7 m and MFDD ≥ 5.0 m/s²; cars (M1, R13-H) from 100 km/h ≤ 70 m and
MFDD ≥ 6.43 m/s². EU turning circle for buses: within 25 m.

Runs are headless at 60 Hz gameplay / 480 Hz physics, and bit-identical for the same build and seed.
";

const DOCS_VEHICLE: &str = "\
Vehicle files: vehicles/<name>.vehicle.ron  (full schema with units: wp schema vehicle)

Units are in field names (mass_kg, radius_m, drag_area_m2). Model: brush tyres with relaxation
length and sliding friction, body roll and pitch on springs, tipping about the outer wheels,
torque-curve engine with optional speed governor, automatic gearbox, open differential, brakes
(hydraulic, or air with reservoir, compressor and chamber lag) with optional ABS, aerodynamic drag.
Optional sections: suspension, body (with style: Car | Bus | TruckCab), brakes.air, brakes.abs,
engine.governed_speed_kmh, gearbox.reverse_ratio, hitch (fifth wheel for semi-trailers).
Semi-trailers live in trailers/<name>.trailer.ron (`wp schema trailer`). Couple one in a scenario
with player.trailer: Some(\"semi_trailer\"), or `wp run --vehicle tractor --trailer semi_trailer`.
Check behaviour with `wp vehicle test <name>` (100 km/h tests are skipped for governed vehicles):
  accel_0_50 / accel_0_100   time from rest
  brake_60_0 / brake_100_0   stopping distance and ECE R13 MFDD
  skidpad                    steady lateral g at 40 km/h with fixed steering
  turning_circle             diameter of the outer front wheel's path on full lock
";

pub fn docs(ctx: &Ctx, topic: Option<&str>) -> CmdResult {
    let text = match topic {
        None => "topics: cli, errors, errors/<CODE>, scenario, vehicle\nexample: wp docs errors/WP-V012\n".to_string(),
        Some("cli") => DOCS_CLI.to_string(),
        Some("scenario") => DOCS_SCENARIO.to_string(),
        Some("vehicle") => DOCS_VEHICLE.to_string(),
        Some("errors") => {
            if ctx.json {
                let list: Vec<_> = catalog::ERRORS
                    .iter()
                    .map(|e| json!({ "code": e.code, "title": e.title, "fix": e.fix }))
                    .collect();
                print_json(&json!(list));
                return Ok(0);
            }
            catalog::ERRORS.iter().map(|e| format!("{}  {}\n", e.code, e.title)).collect()
        }
        Some(t) if t.starts_with("errors/") => {
            let code = &t["errors/".len()..];
            let Some(e) = catalog::find(code) else {
                return Err(cli_error(format!("unknown error code `{code}`"), "list all codes with `wp docs errors`"));
            };
            if ctx.json {
                print_json(&json!({ "code": e.code, "title": e.title, "fix": e.fix }));
                return Ok(0);
            }
            format!("{}: {}\n\nHow to fix: {}\n", e.code, e.title, e.fix)
        }
        Some(other) => {
            return Err(cli_error(format!("unknown docs topic `{other}`"), "topics: cli, errors, errors/<CODE>, scenario, vehicle"));
        }
    };
    if ctx.json {
        print_json(&json!({ "topic": topic, "text": text }));
    } else {
        print!("{text}");
    }
    Ok(0)
}
