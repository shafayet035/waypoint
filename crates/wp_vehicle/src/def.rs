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
    /// Body roll and pitch. Defaults suit a passenger car.
    #[serde(default)]
    pub suspension: SuspensionDef,
    /// Outer body dimensions for drawing. Derived from the chassis if absent.
    #[serde(default)]
    pub body: Option<BodyDef>,
    /// A fifth wheel for towing a semi-trailer (tractor units).
    #[serde(default)]
    pub hitch: Option<HitchDef>,
    /// How the vehicle sounds. Defaults suit a petrol car; the engine's note
    /// follows `engine.cylinders` and the revs.
    #[serde(default)]
    pub sound: SoundDef,
    /// The driver's view (cab camera). Absent: placed from the body style.
    #[serde(default)]
    pub cab: Option<CabDef>,
    /// Body model to draw (glTF `.glb`, a project path such as
    /// `assets/vehicles/hatchback.glb`), origin on the ground midway between
    /// the axles, sized to `body`. Wheels stay procedural. Without one the
    /// body is drawn from boxes.
    #[serde(default)]
    pub model: Option<String>,
}

/// Where a semi-trailer's kingpin couples to this vehicle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HitchDef {
    /// Fifth-wheel position ahead of the rear axle, m (typically 0.2–0.7).
    pub ahead_of_rear_axle_m: f32,
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
    /// Friction while fully sliding, as a share of peak friction (0.7–0.8 dry).
    /// Below 1.0 a locked wheel stops worse than one held at peak slip, which is why ABS helps.
    #[serde(default = "default_sliding_ratio")]
    pub sliding_friction_ratio: f32,
}

fn default_sliding_ratio() -> f32 {
    0.75
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
    /// Speed governor: throttle is cut above this road speed, km/h (buses and trucks).
    #[serde(default)]
    pub governed_speed_kmh: Option<f32>,
    /// Number of cylinders, for the engine's sound (firing frequency).
    #[serde(default = "default_cylinders")]
    pub cylinders: u8,
}

fn default_engine_brake() -> f32 {
    30.0
}

fn default_cylinders() -> u8 {
    4
}

/// The driver's seat, for the cab (first-person) camera.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CabDef {
    /// Driver's eye: metres forward of the centre of mass, to the driver's
    /// side of the centreline (mirrored for left- or right-hand traffic),
    /// and above the ground.
    pub eye_m: (f32, f32, f32),
    /// Vertical field of view, degrees.
    #[serde(default = "default_cab_fov")]
    pub fov_deg: f32,
}

fn default_cab_fov() -> f32 {
    62.0
}

/// The synthesized vehicle sound. Every level is a multiplier: 1 is the default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SoundDef {
    /// Overall loudness.
    #[serde(default = "one")]
    pub volume: f32,
    /// Engine note pitch: 1 follows the firing frequency; below 1 sounds bigger.
    #[serde(default = "one")]
    pub pitch: f32,
    /// Higher harmonics: below 1 is smooth (petrol), above 1 rasps (diesel, sports exhaust).
    #[serde(default = "one")]
    pub roughness: f32,
    /// Low half-order rumble (big diesels, V8s).
    #[serde(default = "one")]
    pub rumble: f32,
    /// Intake and exhaust hiss with throttle.
    #[serde(default = "one")]
    pub intake: f32,
    /// Tyre roar with speed.
    #[serde(default = "one")]
    pub road: f32,
    /// Squeal when sliding.
    #[serde(default = "one")]
    pub skid: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for SoundDef {
    fn default() -> Self {
        SoundDef {
            volume: 1.0,
            pitch: 1.0,
            roughness: 1.0,
            rumble: 1.0,
            intake: 1.0,
            road: 1.0,
            skid: 1.0,
        }
    }
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
    /// Reverse gear ratio (positive number). Defaults to the first-gear ratio.
    #[serde(default)]
    pub reverse_ratio: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrakeDef {
    /// Total brake torque at full pedal, all wheels together, N·m.
    pub max_torque_nm: f32,
    /// Share of brake torque on the front axle, 0–1.
    pub front_bias: f32,
    /// Anti-lock braking: releases a wheel that starts to lock and reapplies it.
    #[serde(default)]
    pub abs: bool,
    /// Traction control: cuts engine torque while a driven wheel spins
    /// (more than 12 % faster than the road).
    #[serde(default)]
    pub traction_control: bool,
    /// Electronic stability control: compares the yaw rate with what the
    /// steering asks for and brakes one wheel to stop a slide (outer front)
    /// or a plough (inner rear), cutting engine torque meanwhile.
    #[serde(default)]
    pub esc: bool,
    /// Compressed-air brakes (buses, trucks). Absent means hydraulic brakes with no lag.
    #[serde(default)]
    pub air: Option<AirBrakeDef>,
}

/// An air brake system: compressor → reservoir → brake chambers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AirBrakeDef {
    /// Governor cut-out pressure of the reservoir, bar (typically 8–10).
    pub reservoir_max_bar: f32,
    /// Chamber pressure that gives max_torque_nm, bar (typically 6–8).
    pub chamber_full_bar: f32,
    /// Time for chamber pressure to reach about 63% of the demand, s (0.15–0.4).
    pub response_time_s: f32,
    /// Compressor refill rate with the engine running, bar/s.
    pub compressor_bar_per_s: f32,
    /// Reservoir pressure lost per bar of chamber pressure applied (chamber/reservoir volume, 0.02–0.15).
    pub consumption_ratio: f32,
    /// Below this reservoir pressure a low-air warning event is raised, bar.
    pub warning_bar: f32,
}

/// Body roll and pitch, as springs and dampers about the roll and pitch axes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SuspensionDef {
    /// Body roll per g of lateral acceleration, degrees (cars 3–6, buses 5–8).
    pub roll_gradient_deg_per_g: f32,
    /// Body pitch per g of braking or acceleration, degrees (1.5–4).
    pub pitch_gradient_deg_per_g: f32,
    /// Damping as a share of critical damping (0.2–0.7).
    pub damping_ratio: f32,
    /// Height of the roll axis above the ground, m.
    pub roll_center_height_m: f32,
    /// Share of roll stiffness on the front axle, 0–1 (more front → more understeer).
    pub front_roll_share: f32,
}

impl Default for SuspensionDef {
    fn default() -> Self {
        Self {
            roll_gradient_deg_per_g: 4.5,
            pitch_gradient_deg_per_g: 2.5,
            damping_ratio: 0.35,
            roll_center_height_m: 0.12,
            front_roll_share: 0.6,
        }
    }
}

/// Outer body size, for drawing and (later) collision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BodyDef {
    pub length_m: f32,
    pub width_m: f32,
    pub height_m: f32,
    /// From the front bumper back to the front axle, m.
    pub front_overhang_m: f32,
    /// How to draw it. `Auto` picks car or bus from the height.
    #[serde(default)]
    pub style: BodyStyle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum BodyStyle {
    #[default]
    Auto,
    Car,
    Bus,
    /// A tractor unit: tall cab at the front, open chassis behind.
    TruckCab,
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
        // Static stability factor: below the tyre friction the vehicle tips
        // before it slides (true for some trucks and coaches when laden).
        let ssf = v.track_width_m / (2.0 * v.cg_height_m.max(1e-3));
        if v.track_width_m > 0.0 && ssf < v.tyre.friction {
            out.push(
                file.warning_at("WP-V070", "cg_height_m", "this vehicle would roll over before its tyres slide")
                    .expected(format!(
                        "static stability factor track_width_m / (2 × cg_height_m) = {ssf:.2} at least the tyre friction ({})",
                        v.tyre.friction
                    ))
                    .help("check cg_height_m and track_width_m; if they are right, expect WheelLift events in hard corners"),
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
        out.extend(validate_extras(file));
        out
    }
}

fn validate_extras(file: &Loaded<VehicleDef>) -> Vec<Diagnostic> {
    let v = &file.value;
    let mut out = Vec::new();
    if !(0.3..=1.0).contains(&v.tyre.sliding_friction_ratio) {
        out.push(
            file.error_at(
                "WP-V030",
                "sliding_friction_ratio",
                "sliding friction ratio is out of range",
            )
            .expected(format!(
                "0.3 ≤ sliding_friction_ratio ≤ 1.0, found {}",
                v.tyre.sliding_friction_ratio
            )),
        );
    }
    if let Some(g) = v.engine.governed_speed_kmh
        && !(g > 5.0 && g <= 400.0)
    {
        out.push(
            file.error_at(
                "WP-V022",
                "governed_speed_kmh",
                "governed speed is out of range",
            )
            .expected("5 < governed_speed_kmh ≤ 400"),
        );
    }
    if let Some(a) = &v.brakes.air {
        let ok = a.reservoir_max_bar > 0.0
            && a.chamber_full_bar > 0.0
            && a.chamber_full_bar <= a.reservoir_max_bar
            && a.response_time_s > 0.0
            && a.response_time_s <= 2.0
            && a.compressor_bar_per_s >= 0.0
            && (0.0..=1.0).contains(&a.consumption_ratio)
            && a.warning_bar >= 0.0
            && a.warning_bar < a.reservoir_max_bar;
        if !ok {
            out.push(
                file.error_at("WP-V041", "air", "air brake settings are inconsistent")
                    .expected("0 < chamber_full_bar ≤ reservoir_max_bar, 0 < response_time_s ≤ 2, 0 ≤ consumption_ratio ≤ 1, warning_bar < reservoir_max_bar"),
            );
        }
    }
    let s = &v.suspension;
    if !(s.roll_gradient_deg_per_g > 0.0 && s.roll_gradient_deg_per_g <= 20.0)
        || !(s.pitch_gradient_deg_per_g > 0.0 && s.pitch_gradient_deg_per_g <= 20.0)
        || !(s.damping_ratio > 0.0 && s.damping_ratio <= 2.0)
        || !(0.0..=1.0).contains(&s.front_roll_share)
        || s.roll_center_height_m < 0.0
        || s.roll_center_height_m >= v.cg_height_m
    {
        out.push(
            file.error_at("WP-V080", "suspension", "suspension settings are out of range")
                .expected(format!(
                    "gradients in (0, 20] deg/g, damping_ratio in (0, 2], front_roll_share in 0..1, 0 ≤ roll_center_height_m < cg_height_m ({})",
                    v.cg_height_m
                )),
        );
    }
    if let Some(hitch) = &v.hitch
        && !(0.0..v.wheelbase_m).contains(&hitch.ahead_of_rear_axle_m)
    {
        out.push(
            file.error_at(
                "WP-V091",
                "ahead_of_rear_axle_m",
                "the fifth wheel must sit between the axles",
            )
            .expected(format!(
                "0 ≤ ahead_of_rear_axle_m < wheelbase_m ({})",
                v.wheelbase_m
            )),
        );
    }
    if let Some(r) = v.gearbox.reverse_ratio
        && !(r > 0.0)
    {
        out.push(file.error_at(
            "WP-V012",
            "reverse_ratio",
            "reverse_ratio must be a positive number",
        ));
    }
    if let Some(b) = &v.body
        && (!(b.length_m > v.wheelbase_m && b.width_m > 0.0 && b.height_m > v.cg_height_m)
            || !(b.front_overhang_m >= 0.0 && b.front_overhang_m + v.wheelbase_m < b.length_m))
    {
        out.push(
                file.error_at("WP-V090", "body", "body dimensions do not fit the chassis")
                    .expected("length_m > wheelbase_m + front_overhang_m, height_m > cg_height_m, width_m > 0"),
            );
    }
    out
}
