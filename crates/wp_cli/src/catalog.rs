//! Every diagnostic code the engine can emit, for `wp docs errors/<code>`.
//! A test checks that every code used in the source is listed here.

pub struct ErrorDoc {
    pub code: &'static str,
    pub title: &'static str,
    pub fix: &'static str,
}

pub const ERRORS: &[ErrorDoc] = &[
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
        code: "WP-C002",
        title: "Windowed mode is not available yet",
        fix: "Use `wp run --headless`. The renderer arrives in the next milestone step.",
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
        title: "Centre of mass is high for the track width (warning)",
        fix: "Check cg_height_m; above track_width_m / 2 the vehicle may roll over before it slides.",
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
];

pub fn find(code: &str) -> Option<&'static ErrorDoc> {
    ERRORS.iter().find(|e| e.code.eq_ignore_ascii_case(code))
}
