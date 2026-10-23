//! Levels (type 212, `.level` in `H.AnimRetarget` / `B.AnimRetarget`).
//!
//! A level is a list of placed objects. Each object has a type (a path into
//! Packages.bin, or a name relative to the level's folder), a position and
//! rotation in the level's space, a scope name, and an attribute text in the
//! notation read by [`crate::ee`]. Type, position, rotation and the
//! two small integers are in the `H.` part; the attribute texts are in `B.`.
//!
//! The `H.` entry is a [`CommonHeader`] followed by (little-endian):
//!
//! ```text
//! u32 n, n x { u32 len, [len] path }     files the level refers to (packages, scripts, sub-levels)
//! u32 n, n x { u32 len, [len] path }     action files
//! u32 n, n x { u32 len, [len] path }     asset files
//! u32 n, n x { str, str, [8] }           sub-levels (empty in every level of the install)
//! u32 unknown                            0
//! u32 object_count, then per object:
//!   str type                             path, or a name relative to the level's folder
//!   f32[3] position
//!   f32[4] rotation                      quaternion x, y, z, w
//!   str scope                            "/" or "/Layer12/Prefab3/": the layer or prefab the object sits in
//!   u32 instance                         the object's `Instance=` attribute
//!   u32 unknown                          0
//! u32 n, n x u32, u32 0                  trailing ids (meaning unknown)
//! ```
//!
//! `B.` has `object_count` u32 attribute lengths, then each object's text of
//! that length plus a terminating NUL.
//!
//! The layout of the object list follows Warframe-Exporter's reader for
//! levels; the trailing ids and the instance integers are not used by it.
//!
//! # Space
//!
//! Like models, levels use a left-handed, y-up space with metres as unit:
//! object positions and the vertices of level geometry share it. Rotations
//! are quaternions in that space. [`Transform::to_right_handed`] applies the
//! same conversion as [`crate::scene`] (negate x).

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use anyhow::{bail, ensure, Context, Result};

use crate::bytes::Reader;
use crate::ee::{self, Attributes, Value};
use crate::header::CommonHeader;
use crate::material::{folder, parse_properties, resolve_path};
use crate::packages::{PackagesBin, TextReader};

pub const KIND_LEVEL: u16 = 212;

#[derive(Debug, Clone)]
pub struct RawObject {
    /// The object's type: a Packages.bin path, or a name relative to the level's folder.
    pub type_path: String,
    pub position: [f32; 3],
    /// Quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    /// Layer or prefab scope, e.g. `/Layer3/`.
    pub scope: String,
    pub instance: u32,
    pub unknown: u32,
    /// The attribute text from `B.`, without its terminating NUL.
    pub attributes: String,
}

impl RawObject {
    pub fn attrs(&self) -> Result<Attributes> {
        Attributes::parse(&self.attributes)
            .with_context(|| format!("attributes of {} {}", self.type_path, self.scope))
    }
}

#[derive(Debug, Clone)]
pub struct RawLevel {
    pub common: CommonHeader,
    pub paths: Vec<String>,
    pub actions: Vec<String>,
    pub assets: Vec<String>,
    pub sublevels: Vec<(String, String)>,
    pub objects: Vec<RawObject>,
    pub trailing: Vec<u32>,
}

fn string_list(r: &mut Reader) -> Result<Vec<String>> {
    let n = r.u32()?;
    ensure!(n < 100_000, "implausible path count {n}");
    (0..n).map(|_| r.str32(4096)).collect()
}

impl RawLevel {
    /// Parses the `H.` and `B.` parts of a level.
    pub fn parse(head: &[u8], body: &[u8]) -> Result<Self> {
        let common = CommonHeader::parse(head).context("common header")?;
        ensure!(
            common.kind == KIND_LEVEL,
            "level type {} is not supported (only {KIND_LEVEL})",
            common.kind
        );
        let mut r = Reader::at(head, common.len)?;
        let paths = string_list(&mut r).context("paths")?;
        let actions = string_list(&mut r).context("action paths")?;
        let assets = string_list(&mut r).context("asset paths")?;
        let n = r.u32()?;
        ensure!(n < 10_000, "implausible sub-level count {n}");
        let mut sublevels = Vec::new();
        for _ in 0..n {
            let a = r.str32(4096)?;
            let b = r.str32(4096)?;
            r.skip(8)?;
            sublevels.push((a, b));
        }
        ensure!(r.u32()? == 0, "unexpected value before the object list");
        let count = r.u32()? as usize;
        ensure!(count < 1_000_000, "implausible object count {count}");
        let mut objects = Vec::with_capacity(count);
        for i in 0..count {
            let type_path = r.str32(4096).with_context(|| format!("object {i}"))?;
            let position = [r.f32()?, r.f32()?, r.f32()?];
            let rotation = r.f32x4()?;
            let scope = r.str32(4096)?;
            let instance = r.u32()?;
            let unknown = r.u32()?;
            objects.push(RawObject {
                type_path,
                position,
                rotation,
                scope,
                instance,
                unknown,
                attributes: String::new(),
            });
        }
        let n = r.u32()?;
        ensure!(n < 1000, "implausible trailing id count {n}");
        let trailing = (0..n).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
        ensure!(r.u32()? == 0, "unexpected final value");
        ensure!(r.rest().is_empty(), "bytes left after the level header");

        let mut b = Reader::new(body);
        let lens = (0..count).map(|_| b.u32()).collect::<Result<Vec<_>>>()?;
        for (o, len) in objects.iter_mut().zip(lens) {
            let raw = b.bytes(len as usize + 1)?;
            if raw[len as usize] != 0 {
                bail!("attribute text of {} is not NUL-terminated", o.type_path);
            }
            o.attributes = String::from_utf8_lossy(&raw[..len as usize]).into_owned();
        }
        ensure!(b.rest().is_empty(), "bytes left after the attribute texts");
        Ok(RawLevel {
            common,
            paths,
            actions,
            assets,
            sublevels,
            objects,
            trailing,
        })
    }
}

// ---------------------------------------------------------------------------
// Types and attribute resolution

/// A Packages.bin type with every property it inherits.
#[derive(Debug, Clone)]
pub struct TypeInfo {
    pub path: String,
    /// The type, then its parents up to the root.
    pub chain: Vec<String>,
    /// Key to (value, path of the type that set it); children override parents.
    pub props: BTreeMap<String, (Value, String)>,
}

impl TypeInfo {
    /// The root of the chain: the engine class everything derives from.
    pub fn class(&self) -> &str {
        self.chain.last().map_or(&self.path, String::as_str)
    }
}

/// Resolves type paths through Packages.bin, remembering what it has seen.
pub struct TypeResolver<'a> {
    bin: &'a PackagesBin,
    texts: TextReader<'a>,
    cache: HashMap<String, Option<Rc<TypeInfo>>>,
}

impl<'a> TypeResolver<'a> {
    pub fn new(bin: &'a PackagesBin) -> Result<Self> {
        Ok(TypeResolver {
            bin,
            texts: bin.texts()?,
            cache: HashMap::new(),
        })
    }

    /// `None` when the type is not in Packages.bin.
    pub fn resolve(&mut self, path: &str) -> Result<Option<Rc<TypeInfo>>> {
        if let Some(hit) = self.cache.get(path) {
            return Ok(hit.clone());
        }
        let info = self.walk(path)?.map(Rc::new);
        self.cache.insert(path.to_string(), info.clone());
        Ok(info)
    }

    fn walk(&mut self, path: &str) -> Result<Option<TypeInfo>> {
        if !self.bin.contains(path) {
            return Ok(None);
        }
        let mut chain = vec![path.to_string()];
        while let Some(parent) = self.bin.parent(chain.last().unwrap()) {
            if chain.len() > 64 || chain.iter().any(|c| c == parent) {
                bail!("parent chain of {path} loops");
            }
            chain.push(parent.to_string());
        }
        let mut props = BTreeMap::new();
        for entity in chain.iter().rev() {
            let Some(text) = self.texts.text(entity)? else {
                continue;
            };
            for (k, raw) in parse_properties(&text) {
                let value = ee::parse_value(&raw).unwrap_or(Value::Text(raw));
                props.insert(k, (value, entity.clone()));
            }
        }
        Ok(Some(TypeInfo {
            path: path.to_string(),
            chain,
            props,
        }))
    }
}

/// An object's attributes together with what its type provides: the object's
/// own text first, then the type chain. Relative paths in an object's own
/// text are relative to the folder named after the level
/// (`/Lotus/Levels/X/Level.level` -> `/Lotus/Levels/X/Level/`); those
/// inherited from a type are relative to the type's own folder.
pub struct Resolved<'a> {
    pub own: &'a Attributes,
    pub ty: Option<&'a TypeInfo>,
    pub level_base: &'a str,
}

impl Resolved<'_> {
    /// The value and the folder its relative paths are relative to.
    pub fn get(&self, key: &str) -> Option<(&Value, &str)> {
        if let Some(v) = self.own.get(key) {
            return Some((v, self.level_base));
        }
        let (v, owner) = self.ty?.props.get(key)?;
        Some((v, folder(owner)))
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|(v, _)| v.text())
    }

    pub fn float(&self, key: &str) -> Option<f32> {
        self.get(key).and_then(|(v, _)| v.float())
    }

    pub fn floats(&self, key: &str) -> Option<Vec<f32>> {
        self.get(key).and_then(|(v, _)| v.floats())
    }

    pub fn flag(&self, key: &str) -> Option<bool> {
        self.text(key).map(|v| v != "0" && !v.is_empty())
    }

    /// An absolute path; `None` when unset or empty.
    pub fn path(&self, key: &str) -> Option<String> {
        let (v, base) = self.get(key)?;
        let t = v.text()?.trim();
        (!t.is_empty()).then(|| resolve_path(base, t))
    }

    /// Absolute paths of a list-valued attribute (`OverrideMaterial={a,b}`);
    /// empty entries stay empty so positions are kept.
    pub fn paths(&self, key: &str) -> Vec<String> {
        let Some((v, base)) = self.get(key) else {
            return Vec::new();
        };
        v.items()
            .iter()
            .map(|x| match x.text().map(str::trim) {
                Some(t) if !t.is_empty() => resolve_path(base, t),
                _ => String::new(),
            })
            .collect()
    }

    /// Every attribute as text, the object's own overriding the type's.
    pub fn all_text(&self) -> Vec<(String, String)> {
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        if let Some(ty) = self.ty {
            for (k, (v, _)) in &ty.props {
                out.insert(k.clone(), v.to_string());
            }
        }
        for (k, v) in &self.own.0 {
            out.insert(k.clone(), v.to_string());
        }
        out.into_iter().collect()
    }
}

// ---------------------------------------------------------------------------
// The level description

/// Placement in the game's left-handed space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub translation: [f32; 3],
    /// Quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    };

    /// The same placement in a right-handed space (x negated), as
    /// [`crate::scene::prepare`] does for vertices: the translation's x flips
    /// and a rotation (x, y, z, w) becomes (x, -y, -z, w).
    pub fn to_right_handed(&self) -> Transform {
        let [x, y, z, w] = self.rotation;
        Transform {
            translation: [
                -self.translation[0],
                self.translation[1],
                self.translation[2],
            ],
            rotation: [x, -y, -z, w],
            scale: self.scale,
        }
    }

    /// Row-major 4x4 matrix: scale, then rotation, then translation.
    pub fn matrix(&self) -> [[f32; 4]; 4] {
        let [x, y, z, w] = self.rotation;
        let n = (x * x + y * y + z * z + w * w).sqrt();
        let (x, y, z, w) = if n > 0.0 {
            (x / n, y / n, z / n, w / n)
        } else {
            (0.0, 0.0, 0.0, 1.0)
        };
        let r = [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
            ],
            [
                2.0 * (x * y + z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - x * w),
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ];
        std::array::from_fn(|i| {
            std::array::from_fn(|j| match (i, j) {
                (3, 3) => 1.0,
                (3, _) => 0.0,
                (_, 3) => self.translation[i],
                _ => r[i][j] * self.scale[j],
            })
        })
    }
}

/// Baked lighting attached to an object: two textures (colour and direction),
/// the rectangle of the lightmap atlas the object uses, and the scale and
/// offset that turn texel values back into light.
#[derive(Debug, Clone, PartialEq)]
pub struct Lightmap {
    pub textures: Vec<String>,
    pub coords: [f32; 4],
    pub normalize: [f32; 4],
    pub normalize1: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceRole {
    /// A CSG zone: the level's own architecture (type 105/109 geometry).
    Zone,
    /// A placed model.
    Prop,
}

#[derive(Debug, Clone)]
pub struct Instance {
    /// Index into [`LevelDescription::models`].
    pub model: usize,
    pub transform: Transform,
    /// `OverrideMaterial`: replaces the model's materials, in mesh order;
    /// an empty entry keeps the model's own.
    pub materials: Vec<String>,
    /// Index of the object in the level.
    pub object: usize,
    pub type_path: String,
    pub scope: String,
    /// `StartHidden=1`: the object is shown later by a script.
    pub hidden: bool,
    pub role: InstanceRole,
    pub lightmap: Option<Lightmap>,
}

/// A flat quad or polygon (`CsgSheet`) given by its points.
#[derive(Debug, Clone)]
pub struct Sheet {
    pub object: usize,
    pub transform: Transform,
    pub material: String,
    pub points: Vec<[f32; 3]>,
}

/// A light with the values the level gives it. Colour is as stored (0..255,
/// alpha last); nothing is defaulted.
#[derive(Debug, Clone, Default)]
pub struct Light {
    pub object: usize,
    pub type_path: String,
    pub scope: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub color: Option<[f32; 4]>,
    pub temperature: Option<f32>,
    pub color_mode: Option<String>,
    pub brightness: Option<f32>,
    pub lumens: Option<f32>,
    pub radius: Option<f32>,
    pub angle: Option<f32>,
    pub length: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub falloff: Option<f32>,
    pub falloff_exponent: Option<f32>,
    pub cull_distance: Option<f32>,
    pub light_type: Option<String>,
    /// Projection texture.
    pub texture: Option<String>,
    pub ies: Option<String>,
    /// `On=0` means the light starts switched off.
    pub on: Option<bool>,
    pub directional: Option<bool>,
    pub direction: Option<[f32; 3]>,
    /// Every attribute as text.
    pub attributes: Vec<(String, String)>,
}

/// A blocking or trigger volume with the dimensions the level gives it.
#[derive(Debug, Clone)]
pub struct Volume {
    pub object: usize,
    pub type_path: String,
    pub scope: String,
    pub transform: Transform,
    /// `Dimensions={x,y,z}`.
    pub dimensions: Option<[f32; 3]>,
    pub length: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub radius: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnKind {
    Player,
    Npc,
}

#[derive(Debug, Clone)]
pub struct Spawn {
    pub object: usize,
    pub kind: SpawnKind,
    pub type_path: String,
    pub scope: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub tag: Option<String>,
    pub attributes: Vec<(String, String)>,
}

/// What a level places, as data: models and where they stand, lights, volumes
/// and spawn points. Positions are in the game's space (see [`Transform`]).
#[derive(Debug, Clone, Default)]
pub struct LevelDescription {
    pub path: String,
    /// Distinct model paths referenced by [`Instance::model`].
    pub models: Vec<String>,
    pub instances: Vec<Instance>,
    pub sheets: Vec<Sheet>,
    pub lights: Vec<Light>,
    pub volumes: Vec<Volume>,
    pub spawns: Vec<Spawn>,
    /// Objects that were not turned into anything above, by reason or type.
    pub skipped: BTreeMap<String, usize>,
}

const CLASS_LIGHT: &str = "/EE/Types/Engine/Light";
const CLASS_ZONE: &str = "/EE/Types/Engine/CsgZone";
const CLASS_SHEET: &str = "/EE/Types/Alias/CsgSheet";
const CLASS_PLAYER_SPAWN: &str = "/EE/Types/Engine/PlayerSpawn";
const CLASS_NPC_SPAWN: &str = "/EE/Types/Npc/NpcSpawnPoint";
const CLASS_BLOCKING: [&str; 2] = [
    "/EE/Types/Engine/BlockingVolume",
    "/EE/Types/Engine/SimpleBlockingVolume",
];

fn vec3(v: Option<Vec<f32>>) -> Option<[f32; 3]> {
    let v = v?;
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

fn vec4(v: Option<Vec<f32>>) -> Option<[f32; 4]> {
    let v = v?;
    (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
}

impl LevelDescription {
    /// Builds the description of the level at `level_path`.
    pub fn build(level: &RawLevel, level_path: &str, types: &mut TypeResolver) -> Result<Self> {
        let level_folder = folder(level_path);
        let stem = level_path
            .rsplit('/')
            .next()
            .unwrap_or(level_path)
            .trim_end_matches(".level");
        let level_base = format!("{level_folder}{stem}/");
        let mut d = LevelDescription {
            path: level_path.to_string(),
            ..Default::default()
        };
        let mut model_index: HashMap<String, usize> = HashMap::new();
        for (i, o) in level.objects.iter().enumerate() {
            let own = o.attrs()?;
            let ty = types.resolve(&resolve_path(level_folder, &o.type_path))?;
            let r = Resolved {
                own: &own,
                ty: ty.as_deref(),
                level_base: &level_base,
            };
            let class = ty.as_ref().map_or(o.type_path.as_str(), |t| t.class());
            let scale = match r.floats("MeshScale").as_deref() {
                Some([s]) => [*s; 3],
                Some([x, y, z]) => [*x, *y, *z],
                _ => [1.0; 3],
            };
            let transform = Transform {
                translation: o.position,
                rotation: o.rotation,
                scale,
            };
            let mut skip = |why: String| *d.skipped.entry(why).or_default() += 1;
            match class {
                CLASS_LIGHT => {
                    d.lights.push(Light {
                        object: i,
                        type_path: o.type_path.clone(),
                        scope: o.scope.clone(),
                        position: o.position,
                        rotation: o.rotation,
                        color: vec4(r.floats("Color")),
                        temperature: r.float("Temperature"),
                        color_mode: r.text("ColorMode").map(str::to_string),
                        brightness: r.float("Brightness"),
                        lumens: r.float("Lumens"),
                        radius: r.float("Radius"),
                        angle: r.float("Angle"),
                        length: r.float("Length"),
                        width: r.float("Width"),
                        height: r.float("Height"),
                        falloff: r.float("Falloff"),
                        falloff_exponent: r.float("FalloffExponent"),
                        cull_distance: r.float("CullDistance"),
                        light_type: r.text("LightType").map(str::to_string),
                        texture: r.path("Texture"),
                        ies: r.path("IesLightData"),
                        on: r.flag("On"),
                        directional: r.flag("Directional"),
                        direction: vec3(r.floats("DirectionalLightDir")),
                        attributes: r.all_text(),
                    });
                }
                CLASS_SHEET => {
                    let points = r
                        .get("Points")
                        .map(|(v, _)| v.items().iter().filter_map(|p| vec3(p.floats())).collect())
                        .unwrap_or_default();
                    d.sheets.push(Sheet {
                        object: i,
                        transform,
                        material: r.path("Material").unwrap_or_default(),
                        points,
                    });
                }
                CLASS_PLAYER_SPAWN | CLASS_NPC_SPAWN => d.spawns.push(Spawn {
                    object: i,
                    kind: if class == CLASS_PLAYER_SPAWN {
                        SpawnKind::Player
                    } else {
                        SpawnKind::Npc
                    },
                    type_path: o.type_path.clone(),
                    scope: o.scope.clone(),
                    position: o.position,
                    rotation: o.rotation,
                    tag: r.text("Tag").map(str::to_string),
                    attributes: r.all_text(),
                }),
                c if CLASS_BLOCKING.contains(&c) => d.volumes.push(Volume {
                    object: i,
                    type_path: o.type_path.clone(),
                    scope: o.scope.clone(),
                    transform: Transform {
                        scale: [1.0; 3],
                        ..transform
                    },
                    dimensions: vec3(r.floats("Dimensions")),
                    length: r.float("Length"),
                    width: r.float("Width"),
                    height: r.float("Height"),
                    radius: r.float("Radius"),
                }),
                _ => {
                    let Some(mesh) = r.path("Mesh") else {
                        skip(format!("no mesh: {}", o.type_path));
                        continue;
                    };
                    if mesh.starts_with("/EE/Editor/") {
                        skip("editor helper mesh".to_string());
                        continue;
                    }
                    let model = *model_index.entry(mesh.clone()).or_insert_with(|| {
                        d.models.push(mesh.clone());
                        d.models.len() - 1
                    });
                    let lightmap = match r.get("LightMap") {
                        Some((v, base)) => {
                            let textures: Vec<String> = v
                                .items()
                                .iter()
                                .filter_map(|t| t.text())
                                .filter(|t| !t.is_empty())
                                .map(|t| resolve_path(base, t))
                                .collect();
                            (!textures.is_empty()).then(|| Lightmap {
                                textures,
                                coords: vec4(r.floats("LightMapCoords")).unwrap_or_default(),
                                normalize: vec4(r.floats("LightMapNormalize")).unwrap_or_default(),
                                normalize1: vec4(r.floats("LightMapNormalize1"))
                                    .unwrap_or_default(),
                            })
                        }
                        None => None,
                    };
                    d.instances.push(Instance {
                        model,
                        transform,
                        materials: r.paths("OverrideMaterial"),
                        object: i,
                        type_path: o.type_path.clone(),
                        scope: o.scope.clone(),
                        hidden: r.flag("StartHidden").unwrap_or(false),
                        role: if class == CLASS_ZONE {
                            InstanceRole::Zone
                        } else {
                            InstanceRole::Prop
                        },
                        lightmap,
                    });
                }
            }
        }
        Ok(d)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::header::tests::build;

    fn str32(out: &mut Vec<u8>, s: &str) {
        out.extend((s.len() as u32).to_le_bytes());
        out.extend(s.as_bytes());
    }

    /// A level with the given (type, position, rotation, scope, attributes) objects.
    pub fn build_level(objects: &[(&str, [f32; 3], [f32; 4], &str, &str)]) -> (Vec<u8>, Vec<u8>) {
        let mut h = build(&[], "", u32::from(KIND_LEVEL));
        for list in [
            &["/Lotus/Types/Game/!index.epkg"][..],
            &[][..],
            &["/Lotus/Levels/X/0_ports.txt"][..],
        ] {
            h.extend((list.len() as u32).to_le_bytes());
            for p in list {
                str32(&mut h, p);
            }
        }
        h.extend(0u32.to_le_bytes()); // sub-levels
        h.extend(0u32.to_le_bytes());
        h.extend((objects.len() as u32).to_le_bytes());
        let mut b = Vec::new();
        for (i, (_, ..)) in objects.iter().enumerate() {
            b.extend((objects[i].4.len() as u32).to_le_bytes());
        }
        for (i, (ty, pos, rot, scope, attrs)) in objects.iter().enumerate() {
            str32(&mut h, ty);
            for v in pos.iter().chain(rot) {
                h.extend(v.to_le_bytes());
            }
            str32(&mut h, scope);
            h.extend((i as u32).to_le_bytes());
            h.extend(0u32.to_le_bytes());
            b.extend(attrs.as_bytes());
            b.push(0);
        }
        h.extend(1u32.to_le_bytes());
        h.extend(77u32.to_le_bytes());
        h.extend(0u32.to_le_bytes());
        (h, b)
    }

    #[test]
    fn parses_objects_and_attributes() {
        let (h, b) = build_level(&[
            (
                "/EE/Types/Engine/Decoration",
                [1.0, 2.0, 3.0],
                [0.0, 0.0, 0.0, 1.0],
                "/Layer1/",
                "Instance=0\nMesh=a.fbx\n",
            ),
            ("DojoLight", [4.0, 5.0, 6.0], [0.0, 1.0, 0.0, 0.0], "/", ""),
        ]);
        let level = RawLevel::parse(&h, &b).unwrap();
        assert_eq!(level.paths, ["/Lotus/Types/Game/!index.epkg"]);
        assert_eq!(level.assets, ["/Lotus/Levels/X/0_ports.txt"]);
        assert_eq!(level.objects.len(), 2);
        let o = &level.objects[0];
        assert_eq!(
            (o.position, o.scope.as_str()),
            ([1.0, 2.0, 3.0], "/Layer1/")
        );
        assert_eq!(o.attrs().unwrap().text("Mesh"), Some("a.fbx"));
        assert_eq!(level.objects[1].rotation, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(level.objects[1].attributes, "");
        assert_eq!(level.trailing, [77]);
    }

    #[test]
    fn rejects_damaged_levels() {
        let (h, b) = build_level(&[("T", [0.0; 3], [0.0, 0.0, 0.0, 1.0], "/", "A=1\n")]);
        assert!(RawLevel::parse(&h[..h.len() - 1], &b).is_err());
        assert!(RawLevel::parse(&h, &b[..b.len() - 1]).is_err());
        let mut longer = b.clone();
        longer.push(0);
        assert!(RawLevel::parse(&h, &longer).is_err());
        let mut wrong_kind = h.clone();
        let kind_at = CommonHeader::parse(&h).unwrap().len - 4;
        wrong_kind[kind_at] = 102;
        assert!(RawLevel::parse(&wrong_kind, &b).is_err());
    }
}
