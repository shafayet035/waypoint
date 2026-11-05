//! `settings.toml`: choices that belong to this device, not to the game.
//!
//! The player keeps it in its data folder (one per game). Today it holds the
//! quality tier: measured once with the renderer benchmark on first launch
//! (`wp_render::bench`), then reused. Edit `tier` to override it; delete the
//! file to measure again.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use wp_world::QualityTier;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub quality: Option<DeviceQuality>,
}

/// The tier this device plays at, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceQuality {
    /// low, mid or flagship. Edit to override the benchmark's choice.
    pub tier: QualityTier,
    /// "benchmark" when the engine chose it.
    #[serde(default)]
    pub chosen_by: String,
    /// The GPU it was measured on.
    #[serde(default)]
    pub gpu: String,
    /// The benchmark's median frame time, ms (1280×720, Mid settings).
    #[serde(default)]
    pub bench_ms: Option<f32>,
    /// The device's RAM, GB.
    #[serde(default)]
    pub memory_gb: Option<f32>,
}

const HEADER: &str = "# Waypoint device settings. `tier` was measured on first launch;\n\
# change it to low, mid or flagship to override, or delete this file to measure again.\n\n";

/// Read `path`; a missing file is empty settings.
pub fn load(path: &Path) -> Result<Settings, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    let text = toml::to_string(settings).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, format!("{HEADER}{text}")).map_err(|e| format!("{}: {e}", path.display()))
}

/// Where a game's settings live on this platform, under `data_dir` (the
/// app's private storage on phones), or the user's data folder.
pub fn settings_path(data_dir: Option<&Path>, game: &str) -> Option<PathBuf> {
    let base = match data_dir {
        Some(d) => d.to_path_buf(),
        None => user_data_dir()?,
    };
    let safe: String = game
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(base.join(safe).join("settings.toml"))
}

fn user_data_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_vendor = "apple") {
        // On iOS, HOME is the app's own container.
        return Some(home?.join("Library/Application Support/Waypoint"));
    }
    if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        return Some(PathBuf::from(x).join("waypoint"));
    }
    if let Some(a) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(a).join("Waypoint"));
    }
    Some(home?.join(".local/share/waypoint"))
}

/// How the tier was decided.
#[derive(Clone, Debug)]
pub enum TierChoice {
    /// Read from settings.toml.
    Saved(DeviceQuality),
    /// Measured now (and saved, unless saving failed: `saved` holds why).
    Measured {
        quality: DeviceQuality,
        report: Box<wp_render::bench::BenchReport>,
        saved: Result<(), String>,
    },
    /// Measuring failed; the game's own tier is used.
    Fallback(String),
}

impl TierChoice {
    /// One line for the log.
    pub fn describe(&self, tier: QualityTier) -> String {
        match self {
            Self::Saved(q) => format!(
                "quality {} (saved: {} on {})",
                tier.name(),
                q.chosen_by,
                q.gpu
            ),
            Self::Measured { quality, saved, .. } => format!(
                "quality {} (benchmark {:.1} ms on {}, {} RAM){}",
                tier.name(),
                quality.bench_ms.unwrap_or(0.0),
                quality.gpu,
                quality
                    .memory_gb
                    .map_or("unknown".into(), |m| format!("{m:.1} GB")),
                match saved {
                    Ok(()) => String::new(),
                    Err(e) => format!("; not saved: {e}"),
                }
            ),
            Self::Fallback(why) => format!("quality {} (game default; {why})", tier.name()),
        }
    }
}

/// Measure this device: run the renderer benchmark on a headless GPU and
/// pick a tier from it and the device's memory.
pub fn measure() -> Result<(DeviceQuality, wp_render::bench::BenchReport), String> {
    let gpu = wp_rhi::Gpu::headless().map_err(|d| d.message.clone())?;
    let report = wp_render::bench::run(&gpu).map_err(|d| d.message.clone())?;
    let memory_gb = wp_rhi::system_memory_gb();
    let tier = QualityTier::for_device(report.frame_ms, memory_gb, report.software());
    Ok((
        DeviceQuality {
            tier,
            chosen_by: "benchmark".into(),
            gpu: report.gpu.name.clone(),
            bench_ms: Some(report.frame_ms),
            memory_gb,
        },
        report,
    ))
}

/// The tier to play at: the one in settings.toml at `path`, else measured
/// now and saved there; `fallback` if measuring fails.
pub fn device_tier(path: Option<&Path>, fallback: QualityTier) -> (QualityTier, TierChoice) {
    let mut settings = path.and_then(|p| load(p).ok()).unwrap_or_default();
    if let Some(q) = &settings.quality {
        return (q.tier, TierChoice::Saved(q.clone()));
    }
    match measure() {
        Ok((quality, report)) => {
            settings.quality = Some(quality.clone());
            let saved = match path {
                Some(p) => save(p, &settings),
                None => Err("no data folder".into()),
            };
            (
                quality.tier,
                TierChoice::Measured {
                    quality,
                    report: Box::new(report),
                    saved,
                },
            )
        }
        Err(e) => (fallback, TierChoice::Fallback(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_missing_is_empty() {
        let dir = std::env::temp_dir().join(format!("wp_settings_{}", std::process::id()));
        let path = dir.join("game").join("settings.toml");
        assert_eq!(load(&path).unwrap(), Settings::default());
        let s = Settings {
            quality: Some(DeviceQuality {
                tier: QualityTier::Low,
                chosen_by: "benchmark".into(),
                gpu: "Test GPU".into(),
                bench_ms: Some(14.5),
                memory_gb: Some(3.7),
            }),
        };
        save(&path, &s).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("tier = \"low\""), "{text}");
        assert_eq!(load(&path).unwrap(), s);
        // A saved tier is used without measuring again.
        let (tier, how) = device_tier(Some(&path), QualityTier::Flagship);
        assert_eq!(tier, QualityTier::Low);
        assert!(matches!(how, TierChoice::Saved(_)));
        // A hand-edited file with only a tier works too.
        std::fs::write(&path, "[quality]\ntier = \"flagship\"\n").unwrap();
        assert_eq!(
            device_tier(Some(&path), QualityTier::Low).0,
            QualityTier::Flagship
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn paths_are_per_game() {
        let p = settings_path(Some(Path::new("/data")), "Harbour Shuttle").unwrap();
        assert_eq!(p, Path::new("/data/Harbour_Shuttle/settings.toml"));
    }
}
