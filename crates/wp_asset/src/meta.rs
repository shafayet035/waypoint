//! `.asset.ron`: where an asset came from and the terms it is used under.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The licence file next to an asset: `<stem>.asset.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssetMeta {
    /// Where the asset came from: "own work", "Poly Haven", a shop's name.
    pub source: String,
    /// Who made it.
    pub author: String,
    pub licence: Licence,
    /// Line for the game's credits screen. Required by CC BY licences.
    #[serde(default)]
    pub credit: Option<String>,
    /// Page the asset was downloaded from.
    #[serde(default)]
    pub url: Option<String>,
    /// Made by an AI model. Allowed for background props only, after cleanup.
    #[serde(default)]
    pub generated: bool,
}

/// The terms an asset is used under.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum Licence {
    /// Made for this game; the game's own licence applies.
    Own,
    /// Creative Commons CC0 1.0 (public domain), e.g. Poly Haven, ambientCG.
    Cc0,
    /// Creative Commons Attribution 3.0: needs a `credit`.
    CcBy3,
    /// Creative Commons Attribution 4.0: needs a `credit`.
    CcBy4,
    /// Bought; the text names the licence that allows redistribution in games.
    Purchased(String),
}

impl Licence {
    pub fn needs_credit(&self) -> bool {
        matches!(self, Licence::CcBy3 | Licence::CcBy4)
    }

    /// Short name for listings (SPDX identifiers where they exist).
    pub fn label(&self) -> String {
        match self {
            Licence::Own => "own work".into(),
            Licence::Cc0 => "CC0-1.0".into(),
            Licence::CcBy3 => "CC-BY-3.0".into(),
            Licence::CcBy4 => "CC-BY-4.0".into(),
            Licence::Purchased(terms) => format!("purchased ({terms})"),
        }
    }
}

/// The licence file for an asset path: same folder, same stem, `.asset.ron`.
pub fn meta_path(asset: &str) -> String {
    let (dir, file) = match asset.rsplit_once('/') {
        Some((d, f)) => (format!("{d}/"), f),
        None => (String::new(), asset),
    };
    let stem = file.split_once('.').map_or(file, |(s, _)| s);
    format!("{dir}{stem}.asset.ron")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_paths() {
        assert_eq!(
            meta_path("assets/vehicles/hatchback.glb"),
            "assets/vehicles/hatchback.asset.ron"
        );
        assert_eq!(meta_path("bus.gltf"), "bus.asset.ron");
    }

    #[test]
    fn parses() {
        let m: AssetMeta = wp_data::parse_ron(
            "a.asset.ron",
            r#"(source: "Poly Haven", author: "A. Artist", licence: CcBy4, credit: Some("Bus by A. Artist"))"#,
        )
        .unwrap();
        assert!(m.licence.needs_credit());
        assert_eq!(m.licence.label(), "CC-BY-4.0");
    }
}
