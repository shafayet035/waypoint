//! Every diagnostic code the engine can emit, for `wp docs errors/<code>`.
//! A test checks that every code used in the source is listed here.

pub struct ErrorDoc {
    pub code: &'static str,
    pub title: &'static str,
    pub fix: &'static str,
}

pub const ERRORS: &[ErrorDoc] = &[
    ErrorDoc {
        code: "WP-O001",
        title: "A screenshot test refers to a scenario, scene or vehicle that does not exist",
        fix: "Name an existing scenario, or a scene (and optionally spawn and vehicle); not both.",
    },
    ErrorDoc {
        code: "WP-O002",
        title: "A screenshot test has no approved picture (warning), or its picture changed",
        fix: "Look at the new picture and the diff that `wp test` writes; if the change is intended, run `wp test --approve <name>`.",
    },
    ErrorDoc {
        code: "WP-O003",
        title: "A screenshot test's size, time or tolerance is out of range",
        fix: "Use 16–4096 px a side, at_s ≥ 0, tolerance pixel 0–1 and changed_pct 0–100.",
    },
    ErrorDoc {
        code: "WP-B001",
        title: "A building's footprint has no area",
        fix: "List three or more distinct corners in order round the building, (x, y) in metres.",
    },
    ErrorDoc {
        code: "WP-B002",
        title: "A building or street uses an unknown style",
        fix: "Use a built-in style (plaster, brick, concrete, office, industrial, shophouse) or define it under `styles`.",
    },
    ErrorDoc {
        code: "WP-B003",
        title: "A building's height is out of range",
        fix: "Use height_m above 0 and at most 600.",
    },
    ErrorDoc {
        code: "WP-B004",
        title: "A street row names a road that is not there, or the scene has no roads",
        fix: "Use road ids from the scene's .road.ron (or leave `roads` empty for every road with a sidewalk); street rows need a road network.",
    },
    ErrorDoc {
        code: "WP-B005",
        title: "A building style or street size range is invalid",
        fix: "Styles need wall colours, storey_m 1.5–10, bay_m 0.5–10, window and lit 0–1; size ranges are (min, max) with 0 < min ≤ max.",
    },
    ErrorDoc {
        code: "WP-B006",
        title: "A scene's scenery file does not exist or failed to load",
        fix: "Point `scenery` at a scenery/<name>.scenery.ron file, relative to the project root.",
    },
    ErrorDoc {
        code: "WP-B009",
        title: "A tree stands on a road (warning)",
        fix: "Move it off the carriageway and out of junctions: trunks are solid.",
    },
    ErrorDoc {
        code: "WP-B008",
        title: "A tree or tree row has an invalid size",
        fix: "Tree heights run from 1 to 80 m; tree rows need spacing_m ≥ 3 and height_m (min, max).",
    },
    ErrorDoc {
        code: "WP-B007",
        title: "A building stands on a road or sidewalk (warning)",
        fix: "Move its footprint back from the road; vehicles and pedestrians use that space.",
    },
    ErrorDoc {
        code: "WP-A001",
        title: "A plugin depends on a plugin that is not loaded",
        fix: "Add the missing plugin, or disable the dependent plugin too in game.toml [plugins] disabled.",
    },
    ErrorDoc {
        code: "WP-A002",
        title: "The same plugin was added twice",
        fix: "Remove the duplicate add_plugin call.",
    },
    ErrorDoc {
        code: "WP-A003",
        title: "Plugin dependencies form a cycle",
        fix: "Break the cycle; two plugins cannot each require the other.",
    },
    ErrorDoc {
        code: "WP-A004",
        title: "game.toml disables a plugin that does not exist",
        fix: "Check the name against `wp plugins`.",
    },
    ErrorDoc {
        code: "WP-A010",
        title: "System ordering is invalid (duplicate name, unknown target or cycle)",
        fix: "Run `wp systems` and fix the before/after constraints named in the message.",
    },
    ErrorDoc {
        code: "WP-C001",
        title: "Unknown name passed on the command line",
        fix: "Run the command with --help, or `wp docs cli`, to see valid values.",
    },
    ErrorDoc {
        code: "WP-C003",
        title: "No scenario matches the filter",
        fix: "Run `wp test` without a filter to list all scenarios.",
    },
    ErrorDoc {
        code: "WP-C004",
        title: "Target folder for `wp new` already exists",
        fix: "Pick a new name or delete the folder.",
    },
    ErrorDoc {
        code: "WP-D001",
        title: "A file could not be read",
        fix: "Check the path; paths in project files are relative to the project root.",
    },
    ErrorDoc {
        code: "WP-D002",
        title: "A RON file has a syntax or structure error",
        fix: "Look at the line and column given; `wp schema <type>` shows the expected fields. Unknown fields are errors.",
    },
    ErrorDoc {
        code: "WP-D003",
        title: "A TOML file has a syntax or structure error",
        fix: "Look at the line and column given; `wp schema game` shows the expected fields.",
    },
    ErrorDoc {
        code: "WP-D004",
        title: "Data could not be written as RON",
        fix: "This is an engine bug; report it with the command you ran.",
    },
    ErrorDoc {
        code: "WP-F001",
        title: "A model file cannot be read or is not valid glTF 2.0",
        fix: "Export from Blender with File → Export → glTF 2.0 (format .glb); check the path is relative to the project root.",
    },
    ErrorDoc {
        code: "WP-F002",
        title: "A glTF file's buffer is embedded as base64 or missing",
        fix: "Export a single .glb, or keep the .bin next to the .gltf.",
    },
    ErrorDoc {
        code: "WP-F003",
        title: "An asset has no licence file",
        fix: "Add <stem>.asset.ron next to it with source, author and licence (`wp schema asset`).",
    },
    ErrorDoc {
        code: "WP-F004",
        title: "An asset's licence needs a credit line",
        fix: "CC BY licences require attribution: add credit: Some(\"<title> by <author>, CC BY 4.0\").",
    },
    ErrorDoc {
        code: "WP-F005",
        title: "A vehicle's model does not exist or failed to load",
        fix: "Check the path in `model` (relative to the project root) and run `wp asset list`.",
    },
    ErrorDoc {
        code: "WP-F006",
        title: "A vehicle model's size does not match the vehicle",
        fix: "Model in metres to the vehicle's dimensions (set `body` for exact sizes); engine X forward is glTF +Z. Mirrors may add up to 0.5 m of width.",
    },
    ErrorDoc {
        code: "WP-F007",
        title: "A vehicle model's origin is not on the ground midway between the axles",
        fix: "Move the origin (Blender: Set Origin, then apply transforms); the lowest point must sit between the ground and the wheel radius.",
    },
    ErrorDoc {
        code: "WP-F008",
        title: "A model has more triangles than its quality tier allows",
        fix: "Decimate the mesh, or raise game.toml [quality] tier; traffic vehicles have smaller budgets than the player's.",
    },
    ErrorDoc {
        code: "WP-F009",
        title: "An AI-generated model is used as a vehicle",
        fix: "AI models are allowed for background props only; model vehicles by hand or use CC0 or bought models.",
    },
    ErrorDoc {
        code: "WP-F010",
        title: "Some model primitives are not triangles",
        fix: "Triangulate before export (Blender: Triangulate modifier); lines and points are not drawn.",
    },
    ErrorDoc {
        code: "WP-G001",
        title: "No game.toml found",
        fix: "Run inside a project, pass --project <dir>, or create one with `wp new <name>`.",
    },
    ErrorDoc {
        code: "WP-G002",
        title: "The command needs a project without errors",
        fix: "Run `wp check` and fix the errors it lists.",
    },
    ErrorDoc {
        code: "WP-G010",
        title: "game.toml start_scene does not exist",
        fix: "Point start_scene at a .scene.ron file, relative to the project root.",
    },
    ErrorDoc {
        code: "WP-G011",
        title: "The project has no vehicles",
        fix: "Add a vehicles/<name>.vehicle.ron file (copy one from `wp new`).",
    },
    ErrorDoc {
        code: "WP-K001",
        title: "The control request names an entity that does not exist",
        fix: "Call world.entities for valid ids (strings like \"3v0\"), or omit `entity` to use the player.",
    },
    ErrorDoc {
        code: "WP-K003",
        title: "world.spawn names an unknown vehicle",
        fix: "Use a vehicle name from the project's vehicles/ folder.",
    },
    ErrorDoc {
        code: "WP-K004",
        title: "No snapshot with that id, or it could not be restored",
        fix: "Use an id returned by sim.snapshot in this session.",
    },
    ErrorDoc {
        code: "WP-K005",
        title: "A component value could not be set",
        fix: "Read the current value with world.get and send the same shape; use merge: true to change single fields.",
    },
    ErrorDoc {
        code: "WP-K006",
        title: "sim.step asked for too many ticks at once",
        fix: "Step at most 36000 ticks (10 minutes) per call.",
    },
    ErrorDoc {
        code: "WP-K007",
        title: "The entity lacks the component or is not a vehicle",
        fix: "Check its components with world.get.",
    },
    ErrorDoc {
        code: "WP-K008",
        title: "replay.stop was called with no recording in progress",
        fix: "Call replay.start first; recording covers the ticks between the two calls.",
    },
    ErrorDoc {
        code: "WP-K009",
        title: "events.subscribe needs a transport that can push lines",
        fix: "Subscribe over `wp serve` or `wp run --control`; through MCP, call events.poll instead.",
    },
    ErrorDoc {
        code: "WP-I001",
        title: "Map data could not be imported",
        fix: "Give an OSM XML file (.osm) containing highway=* ways and the nodes they use; export a small area from openstreetmap.org or the Overpass API.",
    },
    ErrorDoc {
        code: "WP-K010",
        title: "The control port could not be opened",
        fix: "Pick another port with --control <port>, or 0 for any free port.",
    },
    ErrorDoc {
        code: "WP-L001",
        title: "A Lua script does not compile",
        fix: "Fix the syntax at the line given (Luau: Lua 5.1 plus `+=`, types, `continue`). `wp docs scripting` lists the hooks and the `mission` API.",
    },
    ErrorDoc {
        code: "WP-M001",
        title: "A mission's scene does not exist",
        fix: "Point `scene` at a scenes/<name>.scene.ron file.",
    },
    ErrorDoc {
        code: "WP-M002",
        title: "A mission's vehicle does not exist",
        fix: "Use a vehicle name that has a vehicles/<name>.vehicle.ron file.",
    },
    ErrorDoc {
        code: "WP-M003",
        title: "A mission's spawn point is not in its scene",
        fix: "Use a name from the scene's `spawns`.",
    },
    ErrorDoc {
        code: "WP-M004",
        title: "A mission's stop is not in the scene's road network, or it has none",
        fix: "Add the stop to the road file's `stops` (id, name, road, at_m), or fix the id.",
    },
    ErrorDoc {
        code: "WP-M005",
        title: "A mission's script file does not exist",
        fix: "Paths are relative to the project root, e.g. missions/route_1.lua.",
    },
    ErrorDoc {
        code: "WP-N001",
        title: "A road network node is defined twice",
        fix: "Give every node a unique id.",
    },
    ErrorDoc {
        code: "WP-N002",
        title: "A road is defined twice",
        fix: "Give every road a unique id.",
    },
    ErrorDoc {
        code: "WP-N003",
        title: "A road refers to a node that does not exist",
        fix: "Add the node to `nodes`, or fix the road's from/to.",
    },
    ErrorDoc {
        code: "WP-N004",
        title: "A road starts and ends at the same node",
        fix: "Split a loop into two roads with a node in between.",
    },
    ErrorDoc {
        code: "WP-N005",
        title: "A road has no lanes",
        fix: "Set lanes_forward and/or lanes_backward to at least 1.",
    },
    ErrorDoc {
        code: "WP-N006",
        title: "A road dimension is out of range",
        fix: "lane_width_m 2–6, speed_limit_kmh 5–200, sidewalk_m ≥ 0.",
    },
    ErrorDoc {
        code: "WP-N007",
        title: "An arc's radius is too small for its endpoints",
        fix: "Use |radius_m| of at least half the distance between the nodes.",
    },
    ErrorDoc {
        code: "WP-N008",
        title: "A junction's major road does not meet that node",
        fix: "List, under Priority(major: [...]), roads whose from or to is this node.",
    },
    ErrorDoc {
        code: "WP-N009",
        title: "A junction's signal timing is out of range",
        fix: "Use green_s 5–180, amber_s 2–8 and all_red_s 0–10 seconds.",
    },
    ErrorDoc {
        code: "WP-N010",
        title: "A bus stop is defined twice",
        fix: "Give every stop a unique id.",
    },
    ErrorDoc {
        code: "WP-N011",
        title: "A bus stop is on a road that does not exist, or has no lane in its direction",
        fix: "Use an existing road id; set `forward` to the direction buses travel (from → to is true).",
    },
    ErrorDoc {
        code: "WP-W005",
        title: "A scene refers to a road network that does not exist",
        fix: "Point `roads` at a roads/<name>.road.ron file, relative to the project root.",
    },
    ErrorDoc {
        code: "WP-W006",
        title: "A scene's traffic names a vehicle that does not exist, or none at all",
        fix: "List vehicle names that have a vehicles/<name>.vehicle.ron file.",
    },
    ErrorDoc {
        code: "WP-W007",
        title: "A scene has traffic but no road network",
        fix: "Traffic drives on roads: add `roads: Some(\"roads/<name>.road.ron\")` to the scene, or remove `traffic`.",
    },
    ErrorDoc {
        code: "WP-W008",
        title: "A scene parks a vehicle that does not exist",
        fix: "Use a vehicle name that has a vehicles/<name>.vehicle.ron file.",
    },
    ErrorDoc {
        code: "WP-P001",
        title: "A window could not be opened",
        fix: "Run on a machine with a display, or use `wp run --headless` / `wp shot`.",
    },
    ErrorDoc {
        code: "WP-P002",
        title: "The window event loop failed",
        fix: "Rerun; if it persists, report the message with your OS version.",
    },
    ErrorDoc {
        code: "WP-R001",
        title: "No usable GPU adapter",
        fix: "Install GPU drivers; on headless Linux CI install mesa lavapipe (software Vulkan). WGPU_BACKEND selects a backend.",
    },
    ErrorDoc {
        code: "WP-R002",
        title: "The GPU refused to create a device",
        fix: "Update GPU drivers; the message names the missing feature or limit.",
    },
    ErrorDoc {
        code: "WP-R003",
        title: "Reading a rendered frame back from the GPU failed",
        fix: "Rerun; if it persists, try another backend with WGPU_BACKEND.",
    },
    ErrorDoc {
        code: "WP-R004",
        title: "Cannot render to the window surface",
        fix: "Update GPU drivers or try another backend with WGPU_BACKEND.",
    },
    ErrorDoc {
        code: "WP-R010",
        title: "A screenshot could not be written",
        fix: "Check the --out path and that its folder is writable.",
    },
    ErrorDoc {
        code: "WP-S010",
        title: "A scenario refers to a scene that does not exist",
        fix: "Use a path from the list in the message, relative to the project root.",
    },
    ErrorDoc {
        code: "WP-S011",
        title: "A scenario refers to a vehicle that does not exist",
        fix: "Use a vehicle `name` from a .vehicle.ron file.",
    },
    ErrorDoc {
        code: "WP-S012",
        title: "A scenario refers to a spawn point that is not in its scene",
        fix: "Use a spawn name from the scene's `spawns` list.",
    },
    ErrorDoc {
        code: "WP-S013",
        title: "Scenario duration is out of range",
        fix: "Set duration_s between 0 and 3600.",
    },
    ErrorDoc {
        code: "WP-S014",
        title: "Script keys are not in time order",
        fix: "Sort the keys by at_s.",
    },
    ErrorDoc {
        code: "WP-S015",
        title: "A scenario has no assertions",
        fix: "Add at least NoNaN so the test can fail.",
    },
    ErrorDoc {
        code: "WP-S016",
        title: "A driver input or speed is out of range",
        fix: "throttle and brake are 0..1, steer is -1..1, speeds are 0 or more.",
    },
    ErrorDoc {
        code: "WP-V001",
        title: "Vehicle mass must be positive",
        fix: "Set mass_kg to the total mass including driver, e.g. 1250.0 for a car.",
    },
    ErrorDoc {
        code: "WP-V002",
        title: "Vehicle name does not match its file name",
        fix: "Rename the file or the vehicle; scenarios refer to vehicles by name.",
    },
    ErrorDoc {
        code: "WP-V003",
        title: "Yaw inertia must be positive",
        fix: "A good first estimate is mass_kg × wheelbase_m² / 4.",
    },
    ErrorDoc {
        code: "WP-V004",
        title: "Two vehicles have the same name",
        fix: "Give each vehicle a unique name.",
    },
    ErrorDoc {
        code: "WP-V010",
        title: "Centre of mass is not between the axles",
        fix: "cg_to_front_axle_m must be greater than 0 and less than wheelbase_m.",
    },
    ErrorDoc {
        code: "WP-V011",
        title: "A body dimension is invalid",
        fix: "wheelbase_m and track_width_m must be positive; cg_height_m cannot be negative.",
    },
    ErrorDoc {
        code: "WP-V012",
        title: "Gearbox is invalid",
        fix: "Give at least one positive ratio, a positive final_drive and efficiency in (0, 1].",
    },
    ErrorDoc {
        code: "WP-V013",
        title: "Gear ratios do not decrease",
        fix: "List ratios from first gear (largest) to top gear (smallest).",
    },
    ErrorDoc {
        code: "WP-V014",
        title: "Shift points are inconsistent",
        fix: "Need idle_rpm < shift_down_rpm < shift_up_rpm ≤ redline_rpm.",
    },
    ErrorDoc {
        code: "WP-V020",
        title: "Engine torque curve is invalid",
        fix: "At least 2 (rpm, Nm) points, rpm increasing, torque not negative.",
    },
    ErrorDoc {
        code: "WP-V021",
        title: "Idle speed is not below the redline",
        fix: "Set 0 < idle_rpm < redline_rpm.",
    },
    ErrorDoc {
        code: "WP-V030",
        title: "Tyre parameter is out of range",
        fix: "friction about 1.0, stiffness_per_load 12–20, relaxation_length_m 0.2–0.6, rolling_resistance about 0.012.",
    },
    ErrorDoc {
        code: "WP-V031",
        title: "Wheel radius or inertia is not positive",
        fix: "Typical car: radius_m 0.31, inertia_kgm2 1.1.",
    },
    ErrorDoc {
        code: "WP-V040",
        title: "Brake settings are invalid",
        fix: "max_torque_nm ≥ 0 and front_bias between 0 and 1.",
    },
    ErrorDoc {
        code: "WP-V050",
        title: "Steering settings are invalid",
        fix: "max_angle_deg in (0, 60] and a positive rate_deg_per_s.",
    },
    ErrorDoc {
        code: "WP-V060",
        title: "Drag area is negative",
        fix: "drag_area_m2 is Cd × frontal area, e.g. 0.65 for a car.",
    },
    ErrorDoc {
        code: "WP-V070",
        title: "The vehicle would roll over before its tyres slide (warning)",
        fix: "Check cg_height_m and track_width_m: the static stability factor track / (2 × cg height) is below the tyre friction.",
    },
    ErrorDoc {
        code: "WP-V022",
        title: "Governed speed is out of range",
        fix: "Set governed_speed_kmh between 5 and 400, or remove it.",
    },
    ErrorDoc {
        code: "WP-V041",
        title: "Air brake settings are inconsistent",
        fix: "Need 0 < chamber_full_bar ≤ reservoir_max_bar, 0 < response_time_s ≤ 2, consumption_ratio in 0..1, warning_bar < reservoir_max_bar.",
    },
    ErrorDoc {
        code: "WP-V080",
        title: "Suspension settings are out of range",
        fix: "Gradients in (0, 20] deg/g, damping_ratio in (0, 2], front_roll_share 0..1, roll centre below the centre of mass.",
    },
    ErrorDoc {
        code: "WP-V090",
        title: "Body dimensions do not fit the chassis",
        fix: "length_m must exceed wheelbase_m + front_overhang_m, and height_m must exceed cg_height_m.",
    },
    ErrorDoc {
        code: "WP-S017",
        title: "Unknown event kind in an assertion",
        fix: "Use one of: GearShift, AbsEngaged, AbsReleased, WheelLift, WheelLanded, LowAirPressure, RolledOver.",
    },
    ErrorDoc {
        code: "WP-S018",
        title: "A scenario refers to a trailer that does not exist",
        fix: "Use a trailer `name` from a trailers/*.trailer.ron file.",
    },
    ErrorDoc {
        code: "WP-S019",
        title: "The scenario's vehicle cannot tow a semi-trailer",
        fix: "Use a tractor unit, or add hitch: Some((ahead_of_rear_axle_m: 0.5)) to the vehicle.",
    },
    ErrorDoc {
        code: "WP-S020",
        title: "A scenario refers to a dock that is not in its scene",
        fix: "Use a dock name from the scene's `docks` list.",
    },
    ErrorDoc {
        code: "WP-S021",
        title: "A scenario's mission does not exist, or is set in another scene",
        fix: "Name a missions/<name>.mission.ron whose `scene` is the scenario's scene.",
    },
    ErrorDoc {
        code: "WP-S022",
        title: "The Autopilot driver has no mission, or no route to its first stop",
        fix: "Give the scenario a `mission`, and spawn the vehicle on a lane that leads to the first stop.",
    },
    ErrorDoc {
        code: "WP-S021",
        title: "ReverseToDock is used without a trailer",
        fix: "Add trailer: Some(\"semi_trailer\") to the player, and use a tractor unit.",
    },
    ErrorDoc {
        code: "WP-T001",
        title: "A trailer quantity must be positive",
        fix: "Check mass_kg, yaw_inertia_kgm2, cg_height_m, track_width_m and the wheel.",
    },
    ErrorDoc {
        code: "WP-T002",
        title: "Trailer axles are not listed front to back behind the kingpin",
        fix: "List axles_behind_kingpin_m in increasing order, all positive.",
    },
    ErrorDoc {
        code: "WP-T003",
        title: "The trailer's centre of mass is not between kingpin and axles",
        fix: "Set 0 < cg_behind_kingpin_m < the mean axle distance.",
    },
    ErrorDoc {
        code: "WP-T004",
        title: "Trailer body dimensions do not fit its axles",
        fix: "kingpin_from_front_m must be inside the body, and the last axle too.",
    },
    ErrorDoc {
        code: "WP-V091",
        title: "The fifth wheel is not between the axles",
        fix: "Set 0 ≤ ahead_of_rear_axle_m < wheelbase_m.",
    },
    ErrorDoc {
        code: "WP-W001",
        title: "Two spawn points share a name",
        fix: "Give every spawn point a unique name.",
    },
    ErrorDoc {
        code: "WP-W002",
        title: "Ground friction_scale is out of range",
        fix: "Use 0 < friction_scale ≤ 2 (1.0 dry asphalt, 0.7 wet, 0.3 snow).",
    },
    ErrorDoc {
        code: "WP-W003",
        title: "Gravity is not positive",
        fix: "Use 9.81 for Earth.",
    },
    ErrorDoc {
        code: "WP-W004",
        title: "Scene has no spawn points (warning)",
        fix: "Add at least one entry to `spawns`.",
    },
    ErrorDoc {
        code: "WP-Y001",
        title: "A replay refers to a scenario, scene, vehicle or mission that does not exist",
        fix: "Re-record it (`wp replay record --scenario <name>` or `wp run --record`), or restore what it refers to.",
    },
    ErrorDoc {
        code: "WP-Y002",
        title: "A replay no longer plays back identically",
        fix: "Something changed the simulation. The divergence tick says when; look with `wp shot --at <t>`. If the change is intended, re-record the replay.",
    },
    ErrorDoc {
        code: "WP-X001",
        title: "A folder under mods/ has no mod.toml",
        fix: "Create the mod with `wp mod new <name>`, or add mods/<name>/mod.toml with name and version.",
    },
    ErrorDoc {
        code: "WP-X002",
        title: "game.toml disables a mod that is not in mods/",
        fix: "Remove it from [mods] disabled, or check the folder name (`wp mod list`).",
    },
    ErrorDoc {
        code: "WP-N012",
        title: "Traffic cannot drive into and out of a road joined at both ends",
        fix: "A junction could not connect it: give it a gentler angle where it meets the next road, or move `through` points away from its ends; check with a FollowRoute scenario.",
    },
    ErrorDoc {
        code: "WP-X003",
        title: "A mod's code cannot be used",
        fix: "Check the path in `code` (relative to the mod's folder); modules must compile to WebAssembly (.wasm or .wat) and export on_tick(dt: f32).",
    },
    ErrorDoc {
        code: "WP-X004",
        title: "A mod asks for an unknown capability",
        fix: "Capabilities: log, read_vehicles, drive_vehicles.",
    },
];

pub fn find(code: &str) -> Option<&'static ErrorDoc> {
    ERRORS.iter().find(|e| e.code.eq_ignore_ascii_case(code))
}
