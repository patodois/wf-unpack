//! Materials: property text from [`PackagesBin`], resolved through the
//! parent chain.
//!
//! The text is one `Key=Value` per line; a value in braces may span lines.
//! An entity inherits every property of its parent and overrides some of
//! them. Material keys that matter here:
//!
//! ```text
//! TX:<Slot>=path           texture, relative to the folder of the entity that sets it
//! PS:<Name>={r,g,b,a}      shader constants, e.g. PS:DiffuseColor, PS:TintColor0..3
//! DIFFUSE_MAP=0|1          whether TX:DiffuseMap is sampled (same for ROUGHNESS_MAP,
//!                          SPECULAR_MAP, EMISSIVE_MAP)
//! PACKMAP=PACK_MAP|NONE    whether TX:PackMap replaces the diffuse/roughness/specular maps
//! DETAILING=TINT_MASK      player colours through TX:TintMask
//! NORMALS=NORMAL_MAP
//! ```
//!
//! Model attribute text names materials relative to the model's folder,
//! e.g. `PrimeGalatinePBR` next to `PrimeGalatine.fbx`.

use std::collections::BTreeMap;

use anyhow::{bail, Result};

use crate::packages::{PackagesBin, TextReader};

/// Parses property text into key/value pairs, in order.
pub fn parse_properties(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let mut value = value.to_string();
        let mut depth = value.matches('{').count() as i64 - value.matches('}').count() as i64;
        while depth > 0 {
            let Some(next) = lines.next() else { break };
            value.push('\n');
            value.push_str(next);
            depth += next.matches('{').count() as i64 - next.matches('}').count() as i64;
        }
        out.push((key.trim().to_string(), value.trim().to_string()));
    }
    out
}

/// Parses `{a,b,c,d}` (or a single number) into floats.
pub fn parse_floats(value: &str) -> Option<Vec<f32>> {
    let inner = value.trim().trim_start_matches('{').trim_end_matches('}');
    inner.split(',').map(|v| v.trim().parse().ok()).collect()
}

/// The folder part of a path, with its trailing slash.
pub fn folder(path: &str) -> &str {
    &path[..path.rfind('/').map_or(0, |i| i + 1)]
}

/// Resolves `name` against `base_folder` unless it is absolute.
pub fn resolve_path(base_folder: &str, name: &str) -> String {
    if name.starts_with('/') {
        name.to_string()
    } else {
        format!("{base_folder}{name}")
    }
}

/// A material with every inherited property.
#[derive(Debug, Clone, Default)]
pub struct Material {
    pub path: String,
    /// Key to (value, path of the entity that set it).
    pub props: BTreeMap<String, (String, String)>,
}

impl Material {
    /// Walks the parent chain of `path` and merges the properties, children
    /// last. `own` is extra text for the material itself: many materials keep
    /// their properties in the attribute text of their `H.Misc` entry
    /// instead of in Packages.bin; Packages.bin text wins where both set a key.
    pub fn resolve(
        bin: &PackagesBin,
        texts: &mut TextReader,
        path: &str,
        own: &str,
    ) -> Result<Self> {
        if !bin.contains(path) {
            bail!("material {path} is not in Packages.bin");
        }
        let mut chain = Vec::new();
        let mut cur = Some(path.to_string());
        while let Some(p) = cur {
            if chain.contains(&p) || chain.len() > 64 {
                bail!("parent chain of {path} loops");
            }
            cur = bin.parent(&p).map(str::to_string);
            chain.push(p);
        }
        let mut props = BTreeMap::new();
        for entity in chain.iter().rev() {
            if entity == path {
                for (k, v) in parse_properties(own) {
                    props.insert(k, (v, entity.clone()));
                }
            }
            let Some(text) = texts.text(entity)? else {
                continue;
            };
            for (k, v) in parse_properties(&text) {
                props.insert(k, (v, entity.clone()));
            }
        }
        Ok(Material {
            path: path.to_string(),
            props,
        })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.props.get(key).map(|(v, _)| v.as_str())
    }

    /// A flag like `DIFFUSE_MAP=1`.
    pub fn flag(&self, key: &str) -> bool {
        self.get(key).is_some_and(|v| v != "0" && v != "NONE")
    }

    /// The absolute path of a `TX:<slot>` texture.
    pub fn texture(&self, slot: &str) -> Option<String> {
        let (v, owner) = self.props.get(&format!("TX:{slot}"))?;
        (!v.is_empty()).then(|| resolve_path(folder(owner), v))
    }

    pub fn color(&self, key: &str) -> Option<[f32; 4]> {
        let v = parse_floats(self.get(key)?)?;
        Some(match v.len() {
            1 => [v[0], v[0], v[0], 1.0],
            3 => [v[0], v[1], v[2], 1.0],
            4 => [v[0], v[1], v[2], v[3]],
            _ => return None,
        })
    }

    pub fn float(&self, key: &str) -> Option<f32> {
        parse_floats(self.get(key)?)?.first().copied()
    }
}

/// Default player colours for tint masks when a material sets none
/// (a neutral grey, light grey, dark grey and gold).
pub const DEFAULT_TINTS: [[f32; 4]; 4] = [
    [0.35, 0.35, 0.35, 1.0],
    [0.8, 0.8, 0.8, 1.0],
    [0.1, 0.1, 0.1, 1.0],
    [0.84, 0.56, 0.1, 1.0],
];

/// Where a channel comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// One channel (0 = R .. 3 = A) of a texture.
    Channel(String, usize),
    /// All colour channels of a texture.
    Rgb(String),
    Constant(f32),
}

/// Where the four tint mask layers are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskLayout {
    /// A `_t` texture: layers in R, G, B, A.
    Channels,
    /// `DETAILING=TINT_MASK_PACK_MAP`: the pack map's alpha holds the four
    /// layers as a 2x2 grid of half-size copies, R top-left, G top-right,
    /// B bottom-left, A bottom-right.
    PackAlphaQuadrants,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tint {
    pub mask: String,
    pub layout: MaskLayout,
    pub colors: [[f32; 4]; 4],
    /// True when the material sets no `PS:TintColor*` and [`DEFAULT_TINTS`] are used.
    pub default_colors: bool,
}

/// How to turn a material into glTF PBR inputs (see the README for what is guessed).
#[derive(Debug, Clone)]
pub struct Plan {
    pub name: String,
    /// Albedo before tinting: a colour texture, a grey channel, or `diffuse_color`.
    pub albedo: Source,
    pub diffuse_color: [f32; 4],
    /// Tint mask (R, G, B, A layered bottom to top) and the four colours.
    pub tint: Option<Tint>,
    pub roughness: Source,
    /// Metal mask, or specular level when `specular_workflow` is set.
    pub metalness: Source,
    /// `SPECULARMAP_IS_METALMAP=0`: the metal source holds specular intensity.
    pub specular_workflow: bool,
    pub normal: Option<String>,
    /// Emissive texture, whether only its red channel is used, and its colour.
    pub emissive: Option<(String, bool, [f32; 3])>,
    /// Features seen but not reproduced, for reports.
    pub unsupported: Vec<String>,
}

impl Plan {
    /// Picks textures from a resolved material; `exists` says whether a texture path is in H.Texture.
    pub fn new(m: &Material, exists: impl Fn(&str) -> bool) -> Plan {
        let tex = |slot: &str| m.texture(slot).filter(|t| exists(t));
        let mut unsupported = Vec::new();
        let packmap = (m.get("PACKMAP") == Some("PACK_MAP") || m.flag("PACK_MAP"))
            .then(|| tex("PackMap"))
            .flatten();
        let roughness_const = m.float("PS:Roughness").unwrap_or(0.5);
        // Pack map channels, worked out from the content: R = metal mask (specular level
        // when SPECULARMAP_IS_METALMAP=0), G = grey albedo (0.99 correlation with the
        // luminance of a diffuse map made for the same mesh), B = roughness, A = tint
        // mask quadrants for TINT_MASK_PACK_MAP. Separate maps win where enabled.
        let specular_workflow = !m.flag("SPECULARMAP_IS_METALMAP");
        let from_pack = |c: usize| packmap.clone().map(|p| Source::Channel(p, c));
        let mut albedo = m
            .flag("DIFFUSE_MAP")
            .then(|| tex("DiffuseMap"))
            .flatten()
            .map(Source::Rgb)
            .or_else(|| from_pack(1))
            .unwrap_or(Source::Constant(1.0));
        let roughness = m
            .flag("ROUGHNESS_MAP")
            .then(|| tex("RoughnessMap"))
            .flatten()
            .map(|t| Source::Channel(t, 0))
            .or_else(|| from_pack(2))
            .unwrap_or(Source::Constant(roughness_const));
        let metalness = m
            .flag("SPECULAR_MAP")
            .then(|| tex("SpecularMap"))
            .flatten()
            .map(|t| Source::Channel(t, 0))
            .or_else(|| from_pack(0))
            .unwrap_or(Source::Constant(0.0));
        let mut normal = (m.get("NORMALS") == Some("NORMAL_MAP") || m.flag("NORMAL_MAP"))
            .then(|| tex("NormalMap"))
            .flatten();
        if albedo == Source::Constant(1.0) && m.get("LAYERS").is_some() {
            // Layered (terrain-like) materials blend up to three layers by vertex colour; use the first
            if let Some(t) = tex("Layer_Red_DiffuseMap").or_else(|| tex("Layer_Green_DiffuseMap")) {
                albedo = Source::Rgb(t);
                normal = normal
                    .or_else(|| tex("Layer_Red_NormalMap"))
                    .or_else(|| tex("Layer_Green_NormalMap"));
                unsupported.push("layer blending (first layer only)".into());
            }
        }
        let default_colors = (0..4).all(|i| m.color(&format!("PS:TintColor{i}")).is_none());
        let colors = std::array::from_fn(|i| {
            m.color(&format!("PS:TintColor{i}"))
                .unwrap_or(DEFAULT_TINTS[i])
        });
        let tint_from = |mask: Option<String>, layout| {
            mask.map(|mask| Tint {
                mask,
                layout,
                colors,
                default_colors,
            })
        };
        let tint = match m.get("DETAILING") {
            Some("TINT_MASK") => tint_from(tex("TintMask"), MaskLayout::Channels),
            Some("TINT_MASK_PACK_MAP" | "TINT_MASK_PACK_MAP_BLEND") => {
                let tint = tint_from(packmap.clone(), MaskLayout::PackAlphaQuadrants);
                if m.get("DETAILING") == Some("TINT_MASK_PACK_MAP_BLEND") {
                    unsupported.push("DETAILING=TINT_MASK_PACK_MAP_BLEND (blend part)".into());
                }
                tint
            }
            Some(other) if other != "NONE" => {
                unsupported.push(format!("DETAILING={other}"));
                None
            }
            _ => None,
        };
        let emissive = (m.flag("EMISSIVE_MAP") || m.flag("Emissive"))
            .then(|| tex("EmissiveMap"))
            .flatten()
            .map(|t| {
                let single = m.get("EMISSIVE_COMPRESSION") == Some("EC_COMPONENT");
                let color = m
                    .color("PS:EmissiveTintColorHi")
                    .filter(|_| m.get("EMISSIVE_FX") == Some("EMISSIVE_MULTI_TINT"))
                    .or_else(|| m.color("PS:EmissiveTintColor"))
                    .unwrap_or([1.0; 4]);
                (t, single, [color[0], color[1], color[2]])
            });
        Plan {
            name: m.path.rsplit('/').next().unwrap_or_default().to_string(),
            albedo,
            diffuse_color: m
                .color("PS:DiffuseColor")
                .unwrap_or([0.75, 0.75, 0.75, 1.0]),
            tint,
            roughness,
            metalness,
            specular_workflow,
            normal,
            emissive,
            unsupported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::tests::build;

    #[test]
    fn parses_properties() {
        let p = parse_properties("A=1\nPS:Color={0.5,1,0,1}\nNested={\n  X=1\n}\nB=two\n");
        let keys: Vec<_> = p.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["A", "PS:Color", "Nested", "B"]);
        assert_eq!(p[2].1, "{\n  X=1\n}");
        assert_eq!(parse_floats(&p[1].1).unwrap(), [0.5, 1.0, 0.0, 1.0]);
        assert_eq!(parse_floats("0.25").unwrap(), [0.25]);
        assert_eq!(folder("/A/B/c.png"), "/A/B/");
        assert_eq!(resolve_path("/A/", "c.png"), "/A/c.png");
        assert_eq!(resolve_path("/A/", "/X/c.png"), "/X/c.png");
    }

    #[test]
    fn resolves_inheritance_and_relative_textures() {
        let raw = build(&[
            ("/EE/Mat/", "Base", "", Some("DIFFUSE_MAP=1\nTX:DiffuseMap=Base_d.png\nTX:NormalMap=Base_n.png\nPS:Roughness=0.5\n")),
            ("/Lotus/W/", "MyPBR", "/EE/Mat/Base", Some("DIFFUSE_MAP=0\nTX:NormalMap=My_n.png\nPS:TintColor0={1,0,0,1}\n")),
        ]);
        let bin = PackagesBin::parse(&raw).unwrap();
        let mut texts = bin.texts().unwrap();
        let m = Material::resolve(
            &bin,
            &mut texts,
            "/Lotus/W/MyPBR",
            "TX:EmissiveMap=My_e.png\nTX:NormalMap=Ignored_n.png\n",
        )
        .unwrap();
        assert_eq!(m.texture("EmissiveMap").unwrap(), "/Lotus/W/My_e.png");
        assert!(!m.flag("DIFFUSE_MAP"));
        assert_eq!(m.texture("NormalMap").unwrap(), "/Lotus/W/My_n.png");
        // Inherited texture stays relative to the parent's folder
        assert_eq!(m.texture("DiffuseMap").unwrap(), "/EE/Mat/Base_d.png");
        assert_eq!(m.float("PS:Roughness"), Some(0.5));
        assert_eq!(m.color("PS:TintColor0"), Some([1.0, 0.0, 0.0, 1.0]));
        assert!(Material::resolve(&bin, &mut texts, "/Lotus/W/Missing", "").is_err());
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    fn material(props: &[(&str, &str)]) -> Material {
        Material {
            path: "/Lotus/W/Mat".into(),
            props: props
                .iter()
                .map(|(k, v)| (k.to_string(), (v.to_string(), "/Lotus/W/Mat".to_string())))
                .collect(),
        }
    }

    #[test]
    fn packmap_material() {
        let m = material(&[
            ("PACKMAP", "PACK_MAP"),
            ("DIFFUSE_MAP", "0"),
            ("TX:PackMap", "P"),
            ("NORMALS", "NORMAL_MAP"),
            ("TX:NormalMap", "N.png"),
            ("DETAILING", "TINT_MASK"),
            ("TX:TintMask", "T.png"),
            ("PS:TintColor0", "{1,0,0,0.5}"),
            ("EMISSIVE_MAP", "1"),
            ("EMISSIVE_COMPRESSION", "EC_COMPONENT"),
            ("EMISSIVE_FX", "EMISSIVE_MULTI_TINT"),
            ("PS:EmissiveTintColorHi", "{0,0.5,1,1}"),
            ("TX:EmissiveMap", "E.png"),
        ]);
        let plan = Plan::new(&m, |_| true);
        assert_eq!(plan.albedo, Source::Channel("/Lotus/W/P".into(), 1));
        assert_eq!(plan.roughness, Source::Channel("/Lotus/W/P".into(), 2));
        assert_eq!(plan.metalness, Source::Channel("/Lotus/W/P".into(), 0));
        assert_eq!(plan.normal.as_deref(), Some("/Lotus/W/N.png"));
        let tint = plan.tint.unwrap();
        assert_eq!(
            (tint.mask.as_str(), tint.layout),
            ("/Lotus/W/T.png", MaskLayout::Channels)
        );
        assert_eq!(tint.colors[0], [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(tint.colors[3], DEFAULT_TINTS[3]);
        assert!(!tint.default_colors);
        // No SPECULARMAP_IS_METALMAP=1: R holds specular level
        assert!(plan.specular_workflow);
        assert_eq!(
            plan.emissive,
            Some(("/Lotus/W/E.png".into(), true, [0.0, 0.5, 1.0]))
        );
    }

    #[test]
    fn tint_mask_in_pack_map() {
        let m = material(&[
            ("PACKMAP", "PACK_MAP"),
            ("TX:PackMap", "P"),
            ("DETAILING", "TINT_MASK_PACK_MAP"),
            ("SPECULARMAP_IS_METALMAP", "1"),
        ]);
        let plan = Plan::new(&m, |_| true);
        let tint = plan.tint.unwrap();
        assert_eq!(
            (tint.mask.as_str(), tint.layout),
            ("/Lotus/W/P", MaskLayout::PackAlphaQuadrants)
        );
        assert!(tint.default_colors);
        assert_eq!(tint.colors, DEFAULT_TINTS);
        assert!(plan.unsupported.is_empty());
        // Without a pack map there is no mask
        let m = material(&[("DETAILING", "TINT_MASK_PACK_MAP")]);
        assert!(Plan::new(&m, |_| true).tint.is_none());
    }

    #[test]
    fn diffuse_map_wins_over_pack_albedo() {
        let m = material(&[
            ("PACKMAP", "PACK_MAP"),
            ("TX:PackMap", "P"),
            ("DIFFUSE_MAP", "1"),
            ("TX:DiffuseMap", "D.png"),
            ("ROUGHNESS_MAP", "0"),
        ]);
        let plan = Plan::new(&m, |_| true);
        assert_eq!(plan.albedo, Source::Rgb("/Lotus/W/D.png".into()));
        assert_eq!(plan.roughness, Source::Channel("/Lotus/W/P".into(), 2));
    }

    #[test]
    fn separate_maps_and_missing_textures() {
        let m = material(&[
            ("DIFFUSE_MAP", "1"),
            ("TX:DiffuseMap", "D.png"),
            ("ROUGHNESS_MAP", "1"),
            ("TX:RoughnessMap", "R.png"),
            ("SPECULAR_MAP", "1"),
            ("SPECULARMAP_IS_METALMAP", "1"),
            ("TX:SpecularMap", "S.png"),
            ("DETAILING", "TINT_COLOR"),
            ("PS:Roughness", "0.3"),
        ]);
        let plan = Plan::new(&m, |_| true);
        assert_eq!(plan.albedo, Source::Rgb("/Lotus/W/D.png".into()));
        assert_eq!(plan.metalness, Source::Channel("/Lotus/W/S.png".into(), 0));
        assert!(!plan.specular_workflow);
        assert_eq!(plan.unsupported, ["DETAILING=TINT_COLOR"]);
        // Textures that are not in the game fall back to constants
        let plan = Plan::new(&m, |t| t.ends_with("D.png"));
        assert_eq!(plan.roughness, Source::Constant(0.3));
        assert_eq!(plan.metalness, Source::Constant(0.0));
    }
}
