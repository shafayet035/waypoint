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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum Assertion {
    /// Speed reaches `kmh` no later than `within_s` seconds.
    ReachSpeed { kmh: f32, within_s: f32 },
    /// From the first full brake application to standstill, the car travels at most `max_m`.
    BrakingDistance { max_m: f32 },
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

impl Assertion {
    pub fn describe(&self) -> String {
        match self {
            Self::ReachSpeed { kmh, within_s } => {
                format!("ReachSpeed({kmh} km/h within {within_s} s)")
            }
            Self::BrakingDistance { max_m } => format!("BrakingDistance(≤ {max_m} m)"),
            Self::StoppedBy { s } => format!("StoppedBy({s} s)"),
            Self::MaxSpeed(k) => format!("MaxSpeed({k} km/h)"),
            Self::MaxLateralG(g) => format!("MaxLateralG({g} g)"),
            Self::SteadyLateralG { min, max } => format!("SteadyLateralG({min}–{max} g)"),
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
