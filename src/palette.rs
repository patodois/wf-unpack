//! A Warframe's default colours: where the game keeps them.
//!
//! A suit (`/Lotus/Powersuits/Excalibur/Excalibur`, whose parent chain ends in
//! `.../ExcaliburBaseSuit` and `/Lotus/Types/Game/PowerSuits/PlayerPowerSuit`)
//! sets
//!
//! ```text
//! Mesh=/Lotus/Characters/Tenno/Excalibur/ExcaliburBody_skel.fbx
//! DefaultEnergyColor={77,176,224,255}        energy colour 1, 0..255 with alpha last
//! DefaultEnergyColor1={54,146,190,255}       energy colour 2
//! DefaultCustomization={Helmet={ItemType=...ExcaliburHelmet} BodySkin={...}}
//! ```
//!
//! A new account has no colours picked, so what a Warframe looks like by
//! default comes from its materials: every material of the body, helmet and
//! other parts sets its four tint colours (`PS:TintColor0..3`, linear RGB with
//! the specularity modulation in alpha) next to the mask in its pack map, and
//! the glowing parts use `PS:EmissiveTintColorHi` / `Lo`. The game's colour
//! picker overrides them slot by slot (primary, secondary, tertiary,
//! accents, emissive, energy 1 and 2); `/Lotus/Upgrades/Skins/Armor/WarframeDefaults/EmptyCustomization`
//! is the "nothing picked" entry. The suit's two energy colours are what the
//! energy slots start as.
//!
//! This module reads those values: [`SuitPalette::resolve`] follows the suit
//! to its meshes and returns each material's four tints and emissive colours
//! and the two energy colours. Applying it is [`PaletteOverride`] in the
//! material baker: the tints already come from the materials, and the energy
//! colours replace the emissive colour of materials that tint their emission
//! (`EMISSIVE_FX=EMISSIVE_MULTI_TINT`).

use std::path::Path;

use anyhow::{Context, Result};

use crate::cache::Package;
use crate::ee::Value;
use crate::level::TypeResolver;
use crate::material::{folder, resolve_path, Material};
use crate::model::ModelHeader;
use crate::packages::PackagesBin;

/// The colours one material sets.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaterialColours {
    pub path: String,
    /// `PS:TintColor0..3`, linear RGB and the specularity modulation; `None` where the material sets none.
    pub tint: [Option<[f32; 4]>; 4],
    pub emissive_hi: Option<[f32; 4]>,
    pub emissive_lo: Option<[f32; 4]>,
    pub emissive_tint: Option<[f32; 4]>,
    /// `EMISSIVE_FX=EMISSIVE_MULTI_TINT`: the emission is tinted by the energy colours.
    pub multi_tint_emission: bool,
}

/// One mesh of a Warframe and the colours of its materials.
#[derive(Debug, Clone, PartialEq)]
pub struct PartColours {
    /// `body`, or the slot of the default customization it came from (`Helmet`, ...).
    pub part: String,
    pub mesh: String,
    pub materials: Vec<MaterialColours>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SuitPalette {
    pub suit: String,
    /// The entity of the suit's parent chain that names the body mesh.
    pub base_suit: Option<String>,
    /// `DefaultEnergyColor`, `DefaultEnergyColor1`: 0..255, alpha last.
    pub energy: [Option<[f32; 4]>; 2],
    pub parts: Vec<PartColours>,
}

/// What to put over a material's own colours when baking.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PaletteOverride {
    /// Replaces the four tint colours of tinted materials (linear RGB, specularity alpha).
    pub tint: Option<[[f32; 4]; 4]>,
    /// Replaces the emissive colour of multi-tint materials; sRGB 0..1 as the colour picker stores them.
    pub energy: [Option<[f32; 3]>; 2],
}

fn colour(v: &Value) -> Option<[f32; 4]> {
    let f = v.floats()?;
    match f.len() {
        3 => Some([f[0], f[1], f[2], 255.0]),
        4 => Some([f[0], f[1], f[2], f[3]]),
        _ => None,
    }
}

fn material_colours(m: &Material) -> MaterialColours {
    MaterialColours {
        path: m.path.clone(),
        tint: std::array::from_fn(|i| m.color(&format!("PS:TintColor{i}"))),
        emissive_hi: m.color("PS:EmissiveTintColorHi"),
        emissive_lo: m.color("PS:EmissiveTintColorLo"),
        emissive_tint: m.color("PS:EmissiveTintColor"),
        multi_tint_emission: m.get("EMISSIVE_FX") == Some("EMISSIVE_MULTI_TINT"),
    }
}

impl SuitPalette {
    /// Resolves a suit (any entity of its chain works, e.g. `/Lotus/Powersuits/Excalibur/Excalibur`).
    pub fn resolve(game: &Path, suit: &str) -> Result<Self> {
        let bin = PackagesBin::load(game)?;
        let mut types = TypeResolver::new(&bin)?;
        let ty = types
            .resolve(suit)?
            .with_context(|| format!("{suit} is not in Packages.bin"))?;
        let base_suit = ty
            .chain
            .iter()
            .find(|c| {
                // the first entity of the chain whose own text names a mesh
                types
                    .resolve(c)
                    .ok()
                    .flatten()
                    .is_some_and(|t| t.props.get("Mesh").is_some_and(|(_, owner)| owner == *c))
            })
            .cloned();
        let energy = ["DefaultEnergyColor", "DefaultEnergyColor1"]
            .map(|k| ty.props.get(k).and_then(|(v, _)| colour(v)));
        let mut meshes: Vec<(String, String)> = Vec::new();
        if let Some((v, owner)) = ty.props.get("Mesh") {
            if let Some(t) = v.text() {
                meshes.push(("body".into(), resolve_path(folder(owner), t.trim())));
            }
        }
        // The helmet and other parts of the default customization attach decorations with their own meshes
        if let Some((Value::Table(slots), _)) = ty.props.get("DefaultCustomization") {
            for (slot, sv) in slots {
                let Some(item) = sv.get("ItemType").and_then(Value::text) else {
                    continue;
                };
                let Some(item_ty) = types.resolve(item.trim())? else {
                    continue;
                };
                let children = item_ty
                    .props
                    .get("THIRD_PERSON_Attachment")
                    .and_then(|(v, _)| v.get("UpgradeAutoChildren"));
                for child in children.map(Value::items).unwrap_or_default() {
                    let Some(deco) = child.get("Type").and_then(Value::text) else {
                        continue;
                    };
                    let Some(deco_ty) = types.resolve(deco.trim())? else {
                        continue;
                    };
                    if let Some((v, owner)) = deco_ty.props.get("Mesh") {
                        if let Some(t) = v.text() {
                            meshes.push((slot.clone(), resolve_path(folder(owner), t.trim())));
                        }
                    }
                }
            }
        }
        let mut misc = Package::open(&game.join("H.Misc.toc"))?;
        let mut parts = Vec::new();
        for (part, mesh) in meshes {
            let Some(raw) = misc.read_path(&mesh)? else {
                continue;
            };
            let Ok(header) = ModelHeader::parse(&raw) else {
                continue;
            };
            let mut materials = Vec::new();
            for name in header.materials() {
                let path = resolve_path(folder(&mesh), &name);
                let own = match misc.read_path(&path)? {
                    Some(r) => crate::header::CommonHeader::parse(&r)
                        .map(|h| h.attributes)
                        .unwrap_or_default(),
                    None => String::new(),
                };
                // The text reader of `types` is private; a second one is cheap
                let mut texts = bin.texts()?;
                if let Ok(m) = Material::resolve(&bin, &mut texts, &path, &own) {
                    materials.push(material_colours(&m));
                }
            }
            parts.push(PartColours {
                part,
                mesh,
                materials,
            });
        }
        Ok(SuitPalette {
            suit: suit.to_string(),
            base_suit,
            energy,
            parts,
        })
    }

    /// The four tint colours of the first tinted material of the body.
    pub fn body_tint(&self) -> Option<[[f32; 4]; 4]> {
        let body = self.parts.iter().find(|p| p.part == "body")?;
        body.materials.iter().find_map(|m| {
            m.tint
                .iter()
                .any(Option::is_some)
                .then(|| std::array::from_fn(|i| m.tint[i].unwrap_or([0.0, 0.0, 0.0, 0.5])))
        })
    }

    /// The energy colours as the override the material baker applies (sRGB 0..1).
    pub fn energy_override(&self) -> PaletteOverride {
        PaletteOverride {
            tint: None,
            energy: self
                .energy
                .map(|e| e.map(|c| [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ee::parse_value;

    #[test]
    fn colours_from_values() {
        let v = parse_value("{77,176,224,255}").unwrap();
        assert_eq!(colour(&v), Some([77.0, 176.0, 224.0, 255.0]));
        assert_eq!(
            colour(&parse_value("{1,2,3}").unwrap()),
            Some([1.0, 2.0, 3.0, 255.0])
        );
        assert_eq!(colour(&parse_value("{1,2}").unwrap()), None);
    }

    #[test]
    fn material_colours_from_properties() {
        let mut m = Material::default();
        m.path = "/Lotus/X/M".into();
        for (k, v) in [
            ("PS:TintColor0", "{0.35,0.35,0.35,0.5}"),
            ("PS:TintColor3", "{0.26,0.0075,0.0037,0.5}"),
            ("PS:EmissiveTintColorHi", "{0,0.81,1,1}"),
            ("EMISSIVE_FX", "EMISSIVE_MULTI_TINT"),
        ] {
            m.props.insert(k.into(), (v.into(), "/Lotus/X/M".into()));
        }
        let c = material_colours(&m);
        assert_eq!(c.tint[0], Some([0.35, 0.35, 0.35, 0.5]));
        assert_eq!(c.tint[1], None);
        assert_eq!(c.tint[3].unwrap()[1], 0.0075);
        assert_eq!(c.emissive_hi, Some([0.0, 0.81, 1.0, 1.0]));
        assert!(c.multi_tint_emission);
        let part = SuitPalette {
            suit: "s".into(),
            base_suit: None,
            energy: [Some([255.0, 0.0, 51.0, 255.0]), None],
            parts: vec![PartColours {
                part: "body".into(),
                mesh: "m".into(),
                materials: vec![c],
            }],
        };
        assert_eq!(part.body_tint().unwrap()[1], [0.0, 0.0, 0.0, 0.5]);
        let o = part.energy_override();
        assert_eq!(o.energy[0], Some([1.0, 0.0, 0.2]));
        assert_eq!(o.energy[1], None);
    }
}
