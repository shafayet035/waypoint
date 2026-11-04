//! A game's art style: game.toml `[look]`, resolved into the reflected
//! `Look` resource the renderer reads (agents can change it live).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How surfaces are shaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtStyle {
    /// Physically based light, reflections and soft gradients.
    #[default]
    Realistic,
    /// Cel shading: light falls in hard bands, crisp highlights, bold colour.
    Toon,
    /// Low-poly pastel: matte colours, no reflections, soft light.
    Flat,
}

/// game.toml `[look]`: a style preset plus optional overrides. Colours are
/// sRGB, 0–255.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LookDef {
    #[serde(default)]
    pub style: ArtStyle,
    /// Sky overhead.
    #[serde(default)]
    pub sky_top: Option<[u8; 3]>,
    /// Sky at the horizon (also the fog colour).
    #[serde(default)]
    pub sky_horizon: Option<[u8; 3]>,
    /// Open ground off the roads.
    #[serde(default)]
    pub ground: Option<[u8; 3]>,
    /// The metric grid drawn on open ground.
    #[serde(default)]
    pub grid: Option<bool>,
    #[serde(default)]
    pub asphalt: Option<[u8; 3]>,
    #[serde(default)]
    pub markings: Option<[u8; 3]>,
    #[serde(default)]
    pub sidewalk: Option<[u8; 3]>,
    /// Colour saturation, 1 = as authored.
    #[serde(default)]
    pub saturation: Option<f32>,
    /// Haze, as a multiple of the style's.
    #[serde(default)]
    pub fog: Option<f32>,
    /// Brightness, as a multiple of the style's.
    #[serde(default)]
    pub exposure: Option<f32>,
    /// Light bands for the toon style (2–6).
    #[serde(default)]
    pub toon_bands: Option<u8>,
}

/// The resolved look (linear colours).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Look {
    pub style: ArtStyle,
    pub sky_top: [f32; 3],
    pub sky_horizon: [f32; 3],
    pub ground: [f32; 3],
    pub ground_lines: [f32; 3],
    pub grid: bool,
    pub asphalt: [f32; 3],
    pub markings: [f32; 3],
    pub sidewalk: [f32; 3],
    pub saturation: f32,
    pub fog: f32,
    pub exposure: f32,
    /// 0 for smooth light; otherwise the number of toon bands.
    pub bands: f32,
}

impl Default for Look {
    fn default() -> Self {
        LookDef::default().resolve()
    }
}

/// sRGB 0–255 to linear.
pub fn srgb(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| {
        let x = v as f32 / 255.0;
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    })
}

impl LookDef {
    pub fn resolve(&self) -> Look {
        // Realistic: the engine's measured defaults.
        let mut look = Look {
            style: self.style,
            sky_top: [0.16, 0.34, 0.78],
            sky_horizon: [0.62, 0.72, 0.86],
            ground: [0.055, 0.057, 0.062],
            ground_lines: [0.5, 0.5, 0.46],
            grid: true,
            asphalt: [0.045, 0.046, 0.05],
            markings: [0.78, 0.78, 0.76],
            sidewalk: [0.26, 0.255, 0.24],
            saturation: 1.0,
            fog: 1.0,
            exposure: 1.0,
            bands: 0.0,
        };
        match self.style {
            ArtStyle::Realistic => {}
            ArtStyle::Toon => {
                look.sky_top = srgb([70, 150, 255]);
                look.sky_horizon = srgb([190, 225, 255]);
                look.ground = srgb([110, 175, 90]);
                look.grid = false;
                look.asphalt = srgb([80, 84, 96]);
                look.markings = srgb([255, 250, 235]);
                look.sidewalk = srgb([215, 205, 190]);
                look.saturation = 1.25;
                look.fog = 0.6;
                look.bands = 3.0;
            }
            ArtStyle::Flat => {
                look.sky_top = srgb([125, 185, 245]);
                look.sky_horizon = srgb([240, 225, 215]);
                look.ground = srgb([150, 190, 125]);
                look.grid = false;
                look.asphalt = srgb([120, 120, 130]);
                look.markings = srgb([250, 245, 230]);
                look.sidewalk = srgb([230, 215, 200]);
                look.saturation = 0.95;
                look.fog = 0.8;
                look.exposure = 0.55;
            }
        }
        let set = |slot: &mut [f32; 3], c: Option<[u8; 3]>| {
            if let Some(c) = c {
                *slot = srgb(c);
            }
        };
        set(&mut look.sky_top, self.sky_top);
        set(&mut look.sky_horizon, self.sky_horizon);
        set(&mut look.ground, self.ground);
        set(&mut look.asphalt, self.asphalt);
        set(&mut look.markings, self.markings);
        set(&mut look.sidewalk, self.sidewalk);
        if let Some(g) = self.grid {
            look.grid = g;
        }
        if let Some(s) = self.saturation {
            look.saturation = s.max(0.0);
        }
        if let Some(f) = self.fog {
            look.fog = f.max(0.0);
        }
        if let Some(e) = self.exposure {
            look.exposure = e.max(0.01);
        }
        if let Some(b) = self.toon_bands {
            look.bands = b.clamp(2, 8) as f32;
        }
        look
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_overrides() {
        let real = LookDef::default().resolve();
        assert_eq!(real.bands, 0.0);
        assert!(real.grid);
        let toon = LookDef {
            style: ArtStyle::Toon,
            toon_bands: Some(4),
            asphalt: Some([255, 0, 0]),
            ..Default::default()
        }
        .resolve();
        assert_eq!(toon.bands, 4.0);
        assert_eq!(toon.asphalt, [1.0, 0.0, 0.0]);
        assert!(!toon.grid);
        assert!((srgb([128, 128, 128])[0] - 0.2158).abs() < 0.001);
    }
}
