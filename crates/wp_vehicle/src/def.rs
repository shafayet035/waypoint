//! The `.vehicle.ron` format. Units are in field names so files read unambiguously.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use wp_core::Diagnostic;
use wp_data::Loaded;

/// A drivable vehicle. One file per vehicle: `vehicles/<name>.vehicle.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VehicleDef {
    /// Unique name; scenarios refer to the vehicle by it. Should match the file name.
    pub name: String,
    /// Total mass including driver and fuel, kg.
    pub mass_kg: f32,
    /// Moment of inertia about the vertical axis, kg·m². Roughly mass × wheelbase² / 4.
    pub yaw_inertia_kgm2: f32,
    /// Height of the centre of mass above the ground, m.
    pub cg_height_m: f32,
    /// Distance between front and rear axles, m.
    pub wheelbase_m: f32,
    /// Distance from the centre of mass back from the front axle, m. Must be inside the wheelbase.
    pub cg_to_front_axle_m: f32,
    /// Distance between left and right wheel centres, m.
    pub track_width_m: f32,
    /// Which axle the engine drives.
    pub drive: Drive,
    pub wheel: WheelDef,
    pub tyre: TyreDef,
    pub engine: EngineDef,
    pub gearbox: GearboxDef,
    pub brakes: BrakeDef,
    pub steering: SteeringDef,
    pub aero: AeroDef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Drive {
    Front,
    Rear,
    All,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WheelDef {
    /// Loaded rolling radius, m.
    pub radius_m: f32,
    /// Rotational inertia of one wheel plus its share of the driveline, kg·m².
    pub inertia_kgm2: f32,
}

/// Brush tyre model parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TyreDef {
    /// Peak friction coefficient on dry asphalt (about 1.0 for road tyres).
    pub friction: f32,
    /// Slip stiffness per unit load, 1/slip. Typical 12–20; higher means a sharper tyre.
    pub stiffness_per_load: f32,
    /// Distance the tyre rolls before its force builds up, m (0.2–0.6).
    pub relaxation_length_m: f32,
    /// Rolling resistance coefficient (0.008–0.015 for cars).
    pub rolling_resistance: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EngineDef {
    pub idle_rpm: f32,
    /// Fuel cut above this speed.
    pub redline_rpm: f32,
    /// Full-throttle torque curve as (rpm, newton-metres) points in ascending rpm.
    pub torque_curve: Vec<(f32, f32)>,
    /// Engine-braking torque at redline with the throttle closed, N·m.
    #[serde(default = "default_engine_brake")]
    pub engine_brake_nm: f32,
}

fn default_engine_brake() -> f32 {
    30.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GearboxDef {
    /// Forward gear ratios, first gear first, each smaller than the last.
    pub ratios: Vec<f32>,
    /// Final drive (differential) ratio.
    pub final_drive: f32,
    /// Driveline efficiency, 0–1.
    pub efficiency: f32,
    /// Automatic upshift engine speed.
    pub shift_up_rpm: f32,
    /// Automatic downshift engine speed.
    pub shift_down_rpm: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrakeDef {
    /// Total brake torque at full pedal, all wheels together, N·m.
    pub max_torque_nm: f32,
    /// Share of brake torque on the front axle, 0–1.
    pub front_bias: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SteeringDef {
    /// Front-wheel angle at full lock, degrees.
    pub max_angle_deg: f32,
    /// How fast the wheels can turn, degrees per second.
    pub rate_deg_per_s: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AeroDef {
    /// Drag coefficient × frontal area, m² (0.6–0.8 for cars, 5–8 for buses).
    pub drag_area_m2: f32,
}

impl VehicleDef {
    pub fn rear_axle_from_cg(&self) -> f32 {
        self.wheelbase_m - self.cg_to_front_axle_m
    }

    pub fn peak_torque_nm(&self) -> f32 {
        self.engine
            .torque_curve
            .iter()
            .map(|p| p.1)
            .fold(0.0, f32::max)
    }

    /// Check physical plausibility. Diagnostics point at the offending field.
    pub fn validate(file: &Loaded<VehicleDef>) -> Vec<Diagnostic> {
        let v = &file.value;
        let mut out = Vec::new();
        let mut positive = |key: &str, value: f32, code: &str| {
            if !(value > 0.0 && value.is_finite()) {
                out.push(
                    file.error_at(code, key, format!("`{key}` must be a positive number"))
                        .expected(format!("{key} > 0, found {value}")),
                );
            }
        };
        positive("mass_kg", v.mass_kg, "WP-V001");
        positive("yaw_inertia_kgm2", v.yaw_inertia_kgm2, "WP-V003");
        positive("wheelbase_m", v.wheelbase_m, "WP-V011");
        positive("track_width_m", v.track_width_m, "WP-V011");
        positive("radius_m", v.wheel.radius_m, "WP-V031");
        positive("inertia_kgm2", v.wheel.inertia_kgm2, "WP-V031");
        positive("stiffness_per_load", v.tyre.stiffness_per_load, "WP-V030");
        positive("relaxation_length_m", v.tyre.relaxation_length_m, "WP-V030");
        positive("final_drive", v.gearbox.final_drive, "WP-V012");
        positive("rate_deg_per_s", v.steering.rate_deg_per_s, "WP-V050");

        if v.cg_height_m < 0.0 {
            out.push(file.error_at("WP-V011", "cg_height_m", "`cg_height_m` cannot be negative"));
        }
        if !(v.cg_to_front_axle_m > 0.0 && v.cg_to_front_axle_m < v.wheelbase_m) {
            out.push(
                file.error_at(
                    "WP-V010",
                    "cg_to_front_axle_m",
                    "centre of mass is not between the axles",
                )
                .expected(format!(
                    "0 < cg_to_front_axle_m < wheelbase_m ({})",
                    v.wheelbase_m
                ))
                .help("measure from the front axle backwards to the centre of mass"),
            );
        }
        if v.track_width_m > 0.0 && v.cg_height_m > v.track_width_m / 2.0 {
            out.push(
                file.warning_at("WP-V070", "cg_height_m", "centre of mass is high for the track width; the vehicle may roll over in corners")
                    .expected(format!("cg_height_m below track_width_m / 2 ({:.2})", v.track_width_m / 2.0)),
            );
        }
        if !(v.tyre.friction > 0.0 && v.tyre.friction <= 3.0) {
            out.push(
                file.error_at("WP-V030", "friction", "tyre friction is out of range")
                    .expected(format!("0 < friction ≤ 3, found {}", v.tyre.friction))
                    .help("road tyres are about 1.0 on dry asphalt"),
            );
        }
        if !(0.0..0.1).contains(&v.tyre.rolling_resistance) {
            out.push(
                file.error_at(
                    "WP-V030",
                    "rolling_resistance",
                    "rolling resistance is out of range",
                )
                .expected(format!(
                    "0 ≤ rolling_resistance < 0.1, found {}",
                    v.tyre.rolling_resistance
                )),
            );
        }

        // Engine.
        let e = &v.engine;
        if !(e.idle_rpm > 0.0 && e.idle_rpm < e.redline_rpm) {
            out.push(
                file.error_at(
                    "WP-V021",
                    "idle_rpm",
                    "idle speed must be positive and below the redline",
                )
                .expected(format!("0 < idle_rpm < redline_rpm ({})", e.redline_rpm)),
            );
        }
        if e.torque_curve.len() < 2 {
            out.push(
                file.error_at(
                    "WP-V020",
                    "torque_curve",
                    "torque curve needs at least 2 points",
                )
                .help("e.g. torque_curve: [(1000.0, 150.0), (4000.0, 200.0), (6500.0, 160.0)]"),
            );
        } else if e.torque_curve.windows(2).any(|w| w[1].0 <= w[0].0) {
            out.push(
                file.error_at(
                    "WP-V020",
                    "torque_curve",
                    "torque curve rpm values must increase",
                )
                .help("sort the points by rpm and remove duplicates"),
            );
        }
        if e.torque_curve.iter().any(|p| p.1 < 0.0 || !p.1.is_finite()) {
            out.push(file.error_at(
                "WP-V020",
                "torque_curve",
                "torque values cannot be negative",
            ));
        }
        if e.engine_brake_nm < 0.0 {
            out.push(file.error_at(
                "WP-V020",
                "engine_brake_nm",
                "engine braking torque cannot be negative",
            ));
        }

        // Gearbox.
        let g = &v.gearbox;
        if g.ratios.is_empty() || g.ratios.iter().any(|r| !(*r > 0.0)) {
            out.push(
                file.error_at(
                    "WP-V012",
                    "ratios",
                    "gearbox needs at least one positive ratio",
                )
                .help("e.g. ratios: [3.6, 2.1, 1.4, 1.05, 0.85]"),
            );
        } else if g.ratios.windows(2).any(|w| w[1] >= w[0]) {
            out.push(
                file.error_at(
                    "WP-V013",
                    "ratios",
                    "gear ratios must get smaller from first gear upwards",
                )
                .expected("each ratio smaller than the one before it"),
            );
        }
        if !(g.efficiency > 0.0 && g.efficiency <= 1.0) {
            out.push(file.error_at(
                "WP-V012",
                "efficiency",
                "driveline efficiency must be in (0, 1]",
            ));
        }
        if !(g.shift_down_rpm > e.idle_rpm
            && g.shift_down_rpm < g.shift_up_rpm
            && g.shift_up_rpm <= e.redline_rpm)
        {
            out.push(
                file.error_at("WP-V014", "shift_up_rpm", "shift points are inconsistent")
                    .expected(format!(
                        "idle_rpm ({}) < shift_down_rpm ({}) < shift_up_rpm ({}) ≤ redline_rpm ({})",
                        e.idle_rpm, g.shift_down_rpm, g.shift_up_rpm, e.redline_rpm
                    )),
            );
        }

        // Brakes and steering.
        if !(0.0..=1.0).contains(&v.brakes.front_bias) {
            out.push(file.error_at(
                "WP-V040",
                "front_bias",
                "brake front_bias must be between 0 and 1",
            ));
        }
        if v.brakes.max_torque_nm < 0.0 {
            out.push(file.error_at(
                "WP-V040",
                "max_torque_nm",
                "brake torque cannot be negative",
            ));
        }
        if !(v.steering.max_angle_deg > 0.0 && v.steering.max_angle_deg <= 60.0) {
            out.push(
                file.error_at("WP-V050", "max_angle_deg", "steering lock is out of range")
                    .expected(format!(
                        "0 < max_angle_deg ≤ 60, found {}",
                        v.steering.max_angle_deg
                    )),
            );
        }
        if v.aero.drag_area_m2 < 0.0 {
            out.push(file.error_at("WP-V060", "drag_area_m2", "drag area cannot be negative"));
        }
        out
    }
}
