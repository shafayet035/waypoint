use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use wp_core::Diagnostic;
use wp_data::Loaded;

/// A scenario test: `tests/<name>.scenario.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioDef {
    /// Scene file, relative to the project root.
    pub scene: String,
    /// Seed for every random number generator in the run.
    #[serde(default)]
    pub seed: u64,
    pub player: PlayerDef,
    /// Simulated length of the test, seconds.
    pub duration_s: f32,
    /// Everything that must hold for the test to pass.
    pub assert: Vec<Assertion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlayerDef {
    /// Vehicle name, as in its `.vehicle.ron` file.
    pub vehicle: String,
    /// Spawn point name in the scene.
    pub spawn: String,
    /// Semi-trailer to couple to the vehicle's fifth wheel, by name.
    #[serde(default)]
    pub trailer: Option<String>,
    /// Starting speed along the spawn heading, km/h.
    #[serde(default)]
    pub initial_kmh: f32,
    pub driver: Driver,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum Driver {
    /// Timed inputs. Each key holds until the next one.
    Script(Vec<InputKey>),
    /// Hold a speed with a speed controller and a fixed steering input.
    Cruise {
        kmh: f32,
        #[serde(default)]
        steer: f32,
    },
    /// Reverse the towed trailer into a scene dock and stop with its rear at
    /// the bumpers. Needs `player.trailer`. `kmh` is the reversing speed.
    ReverseToDock {
        dock: String,
        #[serde(default = "default_dock_kmh")]
        kmh: f32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputKey {
    pub at_s: f32,
    /// 0 to 1.
    #[serde(default)]
    pub throttle: f32,
    /// 0 to 1.
    #[serde(default)]
    pub brake: f32,
    /// −1 (right) to 1 (left).
    #[serde(default)]
    pub steer: f32,
    /// Select reverse gear (engages below 2 km/h).
    #[serde(default)]
    pub reverse: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum Assertion {
    /// Speed reaches `kmh` no later than `within_s` seconds (and, if set, no
    /// sooner than `not_before_s`, to catch implausibly fast vehicles).
    ReachSpeed {
        kmh: f32,
        within_s: f32,
        #[serde(default)]
        not_before_s: f32,
    },
    /// From the first full brake application to standstill, the car travels at most `max_m`.
    BrakingDistance { max_m: f32 },
    /// Mean fully developed deceleration (UN ECE R13 Annex 4) of the first full
    /// brake application is at least this, m/s² (M1 cars 6.43, M3 buses 5.0).
    Mfdd { min_mps2: f32 },
    /// The outer front wheel's path while circling fits in this diameter, m
    /// (EU rule for buses and trucks: 25 m).
    TurningCircle { max_m: f32 },
    /// No vehicle event of this kind happens to the player (e.g. "WheelLift").
    NoEvent(String),
    /// At least one vehicle event of this kind happens to the player.
    ExpectEvent(String),
    /// At the end, the trailer's rear is within `max_offset_m` of the dock
    /// point, its heading within `max_angle_deg` of the dock's, and it is stopped.
    Docked {
        dock: String,
        max_offset_m: f32,
        max_angle_deg: f32,
    },
    /// The vehicle is stopped (below 0.5 km/h) by `s` seconds and stays stopped.
    StoppedBy { s: f32 },
    /// Speed never exceeds this, km/h.
    MaxSpeed(f32),
    /// Peak lateral acceleration never exceeds this, in g (passenger comfort).
    MaxLateralG(f32),
    /// Mean lateral acceleration over the second half of the run lies in [min, max], in g.
    SteadyLateralG { min: f32, max: f32 },
    /// No NaN or infinite values appear in the vehicle state.
    NoNaN,
    /// A second run with the same seed produces identical state hashes.
    Deterministic,
}

fn default_dock_kmh() -> f32 {
    3.0
}

impl Assertion {
    pub fn describe(&self) -> String {
        match self {
            Self::ReachSpeed {
                kmh,
                within_s,
                not_before_s,
            } if *not_before_s > 0.0 => {
                format!("ReachSpeed({kmh} km/h in {not_before_s}–{within_s} s)")
            }
            Self::ReachSpeed { kmh, within_s, .. } => {
                format!("ReachSpeed({kmh} km/h within {within_s} s)")
            }
            Self::BrakingDistance { max_m } => format!("BrakingDistance(≤ {max_m} m)"),
            Self::StoppedBy { s } => format!("StoppedBy({s} s)"),
            Self::MaxSpeed(k) => format!("MaxSpeed({k} km/h)"),
            Self::MaxLateralG(g) => format!("MaxLateralG({g} g)"),
            Self::SteadyLateralG { min, max } => format!("SteadyLateralG({min}–{max} g)"),
            Self::Mfdd { min_mps2 } => format!("Mfdd(≥ {min_mps2} m/s²)"),
            Self::TurningCircle { max_m } => format!("TurningCircle(≤ {max_m} m)"),
            Self::NoEvent(k) => format!("NoEvent({k})"),
            Self::ExpectEvent(k) => format!("ExpectEvent({k})"),
            Self::Docked {
                dock,
                max_offset_m,
                max_angle_deg,
            } => {
                format!("Docked({dock}, ≤ {max_offset_m} m, ≤ {max_angle_deg}°)")
            }
            Self::NoNaN => "NoNaN".into(),
            Self::Deterministic => "Deterministic".into(),
        }
    }
}

impl ScenarioDef {
    /// Checks that need only this file. Cross-file references are checked by the project loader.
    pub fn validate(file: &Loaded<ScenarioDef>) -> Vec<Diagnostic> {
        let s = &file.value;
        let mut out = Vec::new();
        if !(s.duration_s > 0.0 && s.duration_s <= 3600.0) {
            out.push(
                file.error_at("WP-S013", "duration_s", "scenario duration is out of range")
                    .expected(format!("0 < duration_s ≤ 3600, found {}", s.duration_s)),
            );
        }
        if s.assert.is_empty() {
            out.push(
                file.warning_at(
                    "WP-S015",
                    "assert",
                    "scenario has no assertions, so it can never fail",
                )
                .help("add at least NoNaN, e.g. assert: [NoNaN]"),
            );
        }
        for a in &s.assert {
            if let Assertion::NoEvent(k) | Assertion::ExpectEvent(k) = a
                && !wp_vehicle::VehicleEvent::KINDS.contains(&k.as_str())
            {
                out.push(
                    file.error_at("WP-S017", "assert", format!("unknown event kind `{k}`"))
                        .help(format!(
                            "kinds: {}",
                            wp_vehicle::VehicleEvent::KINDS.join(", ")
                        )),
                );
            }
        }
        if let Driver::Script(keys) = &s.player.driver {
            if keys.windows(2).any(|w| w[1].at_s < w[0].at_s) {
                out.push(
                    file.error_at("WP-S014", "at_s", "script keys must be in time order")
                        .help("sort the keys by at_s"),
                );
            }
            for k in keys {
                if !(0.0..=1.0).contains(&k.throttle)
                    || !(0.0..=1.0).contains(&k.brake)
                    || !(-1.0..=1.0).contains(&k.steer)
                {
                    out.push(
                        file.error_at(
                            "WP-S016",
                            "at_s",
                            format!("input at {} s is out of range", k.at_s),
                        )
                        .expected("throttle and brake in 0..1, steer in -1..1"),
                    );
                }
            }
        }
        if !(s.player.initial_kmh >= 0.0 && s.player.initial_kmh <= 400.0) {
            out.push(
                file.error_at("WP-S016", "initial_kmh", "initial speed is out of range")
                    .expected(format!(
                        "0 ≤ initial_kmh ≤ 400, found {}",
                        s.player.initial_kmh
                    )),
            );
        }
        if let Driver::Cruise { kmh, steer } = &s.player.driver
            && (!(*kmh >= 0.0) || !(-1.0..=1.0).contains(steer))
        {
            out.push(
                file.error_at("WP-S016", "driver", "cruise settings are out of range")
                    .expected("kmh ≥ 0 and steer in -1..1"),
            );
        }
        out
    }
}
