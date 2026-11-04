//! Screenshot tests: `tests/<name>.shot.ron` renders one frame of a scenario
//! (or a scene) at a fixed time and camera, and compares it with the
//! approved picture `tests/<name>.png`. Rendering and comparing happen in
//! the presentation layer (`wp test`); this is the file format.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use wp_core::Diagnostic;
use wp_data::Loaded;

/// `tests/<name>.shot.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShotDef {
    /// A scenario whose scene, vehicle, spawn and driver to use.
    #[serde(default)]
    pub scenario: Option<String>,
    /// Without a scenario: a scene path (default: game.toml's start_scene),
    /// spawn point, vehicle and constant controls.
    #[serde(default)]
    pub scene: Option<String>,
    #[serde(default)]
    pub spawn: Option<String>,
    #[serde(default)]
    pub vehicle: Option<String>,
    #[serde(default)]
    pub throttle: f32,
    #[serde(default)]
    pub steer: f32,
    #[serde(default)]
    pub seed: u64,
    /// Simulated time of the frame, s.
    pub at_s: f32,
    #[serde(default)]
    pub camera: ShotCamera,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    /// Render at a quality tier (resolution cap, shadows, lights, detail);
    /// absent: the offscreen defaults `wp shot` uses.
    #[serde(default)]
    pub quality: Option<wp_world::QualityTier>,
    /// How different the picture may be before the test fails.
    #[serde(default)]
    pub tolerance: Tolerance,
}

/// Where the camera is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ShotCamera {
    #[default]
    Chase,
    Cab,
    Top,
    Side,
    Showcase,
}

impl ShotCamera {
    /// The name `wp shot --camera` takes.
    pub fn name(self) -> &'static str {
        match self {
            Self::Chase => "chase",
            Self::Cab => "cab",
            Self::Top => "top",
            Self::Side => "side",
            Self::Showcase => "showcase",
        }
    }
}

/// Limits on how much a picture may change.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tolerance {
    /// A pixel counts as changed above this colour difference (Oklab
    /// distance: 0.02 is just noticeable).
    #[serde(default = "default_pixel")]
    pub pixel: f32,
    /// At most this share of pixels may change, %.
    #[serde(default = "default_changed_pct")]
    pub changed_pct: f32,
}

impl Default for Tolerance {
    fn default() -> Self {
        Self {
            pixel: default_pixel(),
            changed_pct: default_changed_pct(),
        }
    }
}

fn default_width() -> u32 {
    640
}
fn default_height() -> u32 {
    360
}
fn default_pixel() -> f32 {
    0.06
}
fn default_changed_pct() -> f32 {
    0.5
}

impl ShotDef {
    pub fn validate(file: &Loaded<ShotDef>) -> Vec<Diagnostic> {
        let s = &file.value;
        let mut out = Vec::new();
        if !(16..=4096).contains(&s.width) || !(16..=4096).contains(&s.height) {
            out.push(
                file.error_at(
                    "WP-O003",
                    "width",
                    format!("picture size {}×{} is out of range", s.width, s.height),
                )
                .help("use 16 to 4096 pixels a side; 640×360 is plenty for most tests"),
            );
        }
        if !(s.at_s >= 0.0 && s.at_s.is_finite()) {
            out.push(
                file.error_at("WP-O003", "at_s", format!("at_s {} is not a time", s.at_s))
                    .help("seconds from the start, 0 or more"),
            );
        }
        let t = s.tolerance;
        if !(t.pixel > 0.0 && t.pixel <= 1.0) || !(0.0..=100.0).contains(&t.changed_pct) {
            out.push(
                file.error_at("WP-O003", "tolerance", "tolerance is out of range")
                    .help("pixel: 0–1 (0.06 default), changed_pct: 0–100 (0.5 default)"),
            );
        }
        if s.scenario.is_some() && (s.scene.is_some() || s.vehicle.is_some() || s.spawn.is_some()) {
            out.push(
                file.warning_at(
                    "WP-O001",
                    "scenario",
                    "a scenario sets its own scene, vehicle and spawn; the others are ignored",
                )
                .help("give either `scenario` or `scene`/`spawn`/`vehicle`"),
            );
        }
        out
    }
}
