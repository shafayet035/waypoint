//! `wp docs`: offline documentation topics and the error catalogue.

use super::*;

// ---- wp docs --------------------------------------------------------------

const DOCS_CLI: &str = "\
wp — Waypoint command line. Every command takes --json and --project <dir>.

  wp new <dir>                 create a project from the sandbox template
  wp check                     validate all files (schemas + cross-references)
  wp schema <type>             JSON Schema: game | vehicle | trailer | road | scene | scenario
  wp docs [topic]              offline docs: cli | errors | errors/<CODE> | scenario | vehicle
  wp run [drive opts]          drive in a window: W/S throttle-brake, A/D steer, C camera (incl. cab view), R reset, P pause
  wp run --mission route_1     play a mission (its scene, vehicle and spawn); the title shows the objective
  wp run --quality low         quality tier: low|mid|flagship (default game.toml [quality] tier)
                               art style: game.toml [look] style = \"realistic\"|\"toon\"|\"flat\", plus colour
                               overrides (sky_top, ground, asphalt... as [r, g, b] 0–255); see `wp schema game`
  wp run --headless [opts]     simulate without a window; prints the final state (+ --trace, --seconds, --ticks)
  wp run --control <port>      also accept control-protocol requests on 127.0.0.1:<port> (0 = any free port)
  wp shot --at 12.5s [opts]    render one frame to PNG, headless: --camera chase|cab|top|side|showcase --out f.png
  wp audio [opts] --out f.wav  render the player vehicle's sound headless (engine, road, skids); wp run --mute
  wp serve [drive opts]        control protocol (JSON-RPC 2.0) on stdin/stdout, one request per line
  wp mcp                       Model Context Protocol server on stdin/stdout (tools, resources, screenshots)
  wp test [filter]             run scenario tests; exit 1 if any fail (golden replays included)
  wp replay <file.replay.ron>  play a recording back headless; exit 1 at the first checkpoint that differs
  wp replay record --scenario N  golden replay next to the scenario (tests/N.replay.ron); `wp test N` checks it
  wp run --record f.replay.ron record a drive (window or --headless): inputs, R resets, a world hash per second
  wp vehicle test <name> [--manoeuvre accel_0_100|brake_100_0|skidpad|all]
  wp import osm <file.osm> --name N [--drive-side right|left] [--lang en] [--force]
                               OpenStreetMap → roads/N.road.ron + scenes/N.scene.ron (spawn \"start\");
                               export an area at openstreetmap.org (Export → .osm). Keep the ODbL credit.
  wp import opendrive <file.xodr> --name N [--force]
                               ASAM OpenDRIVE → the same two files; drive side from the file's rule.
  wp asset info <file.glb>     what the engine sees in a model: triangles, size, materials and roles
  wp asset list                every model, its licence and the vehicles using it
  wp credits                   the credits screen: asset credits and map attributions
  wp mod new <name>            start a mod in mods/<name>/ (wp docs mods)
  wp mod list                  mods in load order, active or not, and the files each adds or replaces
  wp systems                   resolved system schedule
  wp plugins                   loaded plugins in load order

Drive options (run and shot): --scenario NAME, or --scene PATH --spawn NAME --vehicle NAME
  --throttle 0..1 --brake 0..1 --steer -1..1 --seed N

Agent loop: edit a file → wp check --json → wp test --json → on failure, wp shot --scenario <name> --at <t>
and look at the image → fix. For live work, `wp serve` or `wp mcp`, then call rpc.methods.

Control protocol methods: rpc.methods, sim.status, sim.step, sim.pause, sim.resume, sim.snapshot,
sim.restore, world.entities, world.get, world.set, world.find, world.spawn, world.despawn,
world.resources, input.set, vehicle.telemetry, events.poll, events.subscribe, events.unsubscribe,
render.screenshot, replay.start, replay.stop. events.subscribe {kinds?, entity?} pushes notifications
{\"method\": \"events.event\", \"params\": {subscription, event}} as events happen (serve and --control).
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
  Autopilot                     the traffic AI drives the scenario's `mission` (set mission: Some(\"route_1\"))
  Cruise(kmh: 40.0, steer: 0.25)                                              speed controller + fixed steer
  FollowRoute(to_road: \"east_curve\", kmh: 50.0)  plans a lane route on the scene's road network and drives it
  ReverseToDock(dock: \"bay_1\", kmh: 3.0)       backs the towed trailer into a scene dock (pulls up to retry)

Assertions
  ReachSpeed(kmh: 50.0, within_s: 15.0, not_before_s: 8.0)   (not_before_s is optional)
  BrakingDistance(max_m: 36.7)    Mfdd(min_mps2: 5.0)    StoppedBy(s: 14.0)    TurningCircle(max_m: 25.0)
  MaxSpeed(130.0)   MaxLateralG(0.35)   SteadyLateralG(min: 0.4, max: 1.05)
  MaxSideslipDeg(9.0)   no spin or big slide   MaxDecelG(0.4)   no harsh braking
  TrafficFlows(min_trips: 10)   the scene's AI traffic keeps reaching destinations (no gridlock)
  NoRedLightViolations   no vehicle enters a signalled junction on red
  NoPedestrianHits   no vehicle hits a person on foot
  MissionComplete   every stop served   MissionScore(min: 70)   punctuality, comfort and safety (out of 100)
  FinalPositionWithin(min: (70.0, -3.0), max: (78.0, 3.0))   where the player ends up
  NoEvent(\"WheelLift\")   ExpectEvent(\"AbsEngaged\")   NoNaN   Deterministic   NoCollisions
  RouteFollowed(max_offset_m: 0.6)   FollowRoute finished, never further than this from the lane centre
  Docked(dock: \"bay_1\", max_offset_m: 0.3, max_angle_deg: 3.0)   trailer rear at the dock, square, stopped

Events: GearShift, AbsEngaged, AbsReleased, EscEngaged, EscReleased, WheelLift, WheelLanded, LowAirPressure, RolledOver, Jackknife,
Collision (bodies touching; impact_kmh, and a dent in each: telemetry health_pct), RanRedLight, HitPedestrian.

Scenes: traffic: Some((count: 12, vehicles: [\"hatchback\"], radius_m: Some(300.0))) (radius: keep traffic
near the player on big maps), weather: Some((rain: 0.8)) (wet roads: grip down to 70 %; drivers slow;
\"Weather\" can be changed live with world.set), pedestrians: Some((count: 30)) (they walk
roads with sidewalk_m > 0 and cross at zebras), parked: [(vehicle: \"city_bus\", position: (80.0, 0.0))],
time_of_day: Some((hour: 21.5, hours_per_hour: 60.0)) (night: headlights and street lamps; \"TimeOfDay\" is live-editable).

Junction control (road files, on a node): control: Signals((green_s: 20.0, amber_s: 3.0, all_red_s: 2.0))
or control: Priority(major: [\"high_street\"]); default Auto (take turns).
Regulation limits used in the sandbox: UN ECE R13 Type-0 braking — buses (M3) from 60 km/h
≤ 0.15v + v²/130 = 36.7 m and MFDD ≥ 5.0 m/s²; cars (M1, R13-H) from 100 km/h ≤ 70 m and
MFDD ≥ 6.43 m/s². EU turning circle for buses: within 25 m.

Runs are headless at 60 Hz gameplay / 480 Hz physics, and bit-identical for the same build and seed.
Golden replays: tests/<name>.replay.ron (from `wp replay record --scenario <name>`) holds a world hash per
second; `wp test` adds MatchesGoldenReplay and names the second the run first differed. Re-record after an
intended behaviour change.
";

const DOCS_VEHICLE: &str = "\
Vehicle files: vehicles/<name>.vehicle.ron  (full schema with units: wp schema vehicle)

Units are in field names (mass_kg, radius_m, drag_area_m2). Model: brush tyres with relaxation
length and sliding friction, body roll and pitch on springs, tipping about the outer wheels,
torque-curve engine with optional speed governor, automatic gearbox, open differential, brakes
(hydraulic, or air with reservoir, compressor and chamber lag) with optional ABS, aerodynamic drag.
Sound: engine.cylinders sets the note; sound: (volume, pitch, roughness, rumble, intake, road, skid) tunes it
(multipliers, 1 = default; a diesel bus: pitch 0.85, roughness 1.6, rumble 2.2).
Optional sections: suspension, body (with style: Car | Bus | TruckCab), brakes.air, brakes.abs,
brakes.traction_control, brakes.esc (stability control),
engine.governed_speed_kmh, gearbox.reverse_ratio, hitch (fifth wheel for semi-trailers).
Semi-trailers live in trailers/<name>.trailer.ron (`wp schema trailer`). Couple one in a scenario
with player.trailer: Some(\"semi_trailer\"), or `wp run --vehicle tractor --trailer semi_trailer`.
Check behaviour with `wp vehicle test <name>` (100 km/h tests are skipped for governed vehicles):
  accel_0_50 / accel_0_100   time from rest
  brake_60_0 / brake_100_0   stopping distance and ECE R13 MFDD
  skidpad                    steady lateral g at 40 km/h with fixed steering
  turning_circle             diameter of the outer front wheel's path on full lock
";

const DOCS_ASSETS: &str = "\
Models: glTF 2.0 under assets/, e.g. assets/vehicles/hatchback.glb (Blender: File → Export → glTF 2.0,
format .glb, +Y up). Each needs a licence file with the same stem (wp schema asset):

  // assets/vehicles/hatchback.asset.ron
  (source: \"Poly Haven\", author: \"A. Artist\", licence: CcBy4, credit: Some(\"Hatchback by A. Artist, CC BY 4.0\"))

  licence: Own | Cc0 | CcBy3 | CcBy4 | Purchased(\"terms\")   CC BY needs `credit`; generated: true marks AI models
  (background props only). `wp credits` prints the credits screen.

Vehicles: model: Some(\"assets/vehicles/hatchback.glb\") in the .vehicle.ron. Origin on the ground midway
between the axles, metres, sized to the vehicle (`body`); the engine draws the wheels. Material names set
roles: paint… (drawn in the vehicle's colour: player red, traffic liveries), glass…, brake…/tail… (glow
when braking), head… (headlamps); anything else keeps its colour. Textures are not drawn yet (M4).

wp check: licence file present (WP-F003) and complete (WP-F004), model loads (WP-F001/F002), size and
origin match the vehicle (WP-F006/F007), triangles within the [quality] tier's budget (WP-F008):
player vehicle 40k / 80k / 150k, traffic 8k / 15k / 30k (low / mid / flagship).
`wp asset info <file>` shows what the engine sees; `wp shot` shows how it looks.
tools/models/vehicles.py builds the sandbox's low-poly models from code.
";

const DOCS_MODS: &str = "\
Mods: folders under mods/, each with a mod.toml (wp schema mod):

  name = \"night_bus\"   version = \"1.0.0\"   author = \"...\"   description = \"...\"   priority = 0

A mod mirrors the game's layout. A file at the same path replaces the game's
(mods/night_bus/vehicles/city_bus.vehicle.ron), a new path adds one (mods/night_bus/scenes/night.scene.ron).
Vehicles, trailers, roads, scenes, scenarios, missions and their Lua scripts, models (with licence files)
and replays all layer. Mods load by priority, then folder name; later wins. Every check runs on the
result, and errors point at the file inside the mod. Switch one off with game.toml [mods] disabled = [\"night_bus\"]
or enabled = false in its mod.toml. `wp mod list` shows what each mod adds and replaces.
Not yet: WASM modules and per-mod capability permissions.
";

const DOCS_SCRIPTING: &str = "\
Mission scripts: Lua (Luau, sandboxed). Set `script: Some(\"missions/route_1.lua\")` in a mission.
`wp check` compiles every script (WP-L001 with the line); a runtime error fails the mission.

Hooks (define the ones you need):
  on_start()                     once, at the first tick
  on_tick(dt)                    every tick (60 Hz); dt in seconds
  on_arrive(stop)                doors open at a stop (stop id)
  on_depart(stop, info)          info: late_s, boarded, alighted, left_behind
  on_complete(score)             the route is done (score 0–100)
  on_fail(reason)

The `mission` table:
  mission.objective(text)        replace the objective shown while driving
  mission.message(text)          show a message
  mission.add_passengers(stop, n) more people wait at the next visit to a stop
  mission.bonus(points)          add (or, if negative, take) points
  mission.fail(reason)
  mission.log(text)              a line in the run's log (wp test --json: metrics.mission.log)
  mission.state()                { time_s, speed_kmh, on_board, next_stop, stops_served }

No `os`, files or clock; math.random is seeded from the run's seed, so runs stay deterministic.
Luau extras over Lua 5.1: `+=`, `continue`, string interpolation, optional types.
";

pub fn docs(ctx: &Ctx, topic: Option<&str>) -> CmdResult {
    let text = match topic {
        None => "topics: cli, errors, errors/<CODE>, assets, mods, scenario, scripting, vehicle\nexample: wp docs errors/WP-V012\n".to_string(),
        Some("scripting") => DOCS_SCRIPTING.to_string(),
        Some("assets") => DOCS_ASSETS.to_string(),
        Some("mods") => DOCS_MODS.to_string(),
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
            return Err(cli_error(format!("unknown docs topic `{other}`"), "topics: cli, errors, errors/<CODE>, assets, mods, scenario, scripting, vehicle"));
        }
    };
    if ctx.json {
        print_json(&json!({ "topic": topic, "text": text }));
    } else {
        print!("{text}");
    }
    Ok(0)
}
