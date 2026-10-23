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
use std::path::Path;
use std::rc::Rc;

use anyhow::{bail, ensure, Context, Result};

use crate::bytes::Reader;
use crate::cache::Package;
use crate::ee::{self, Attributes, Value};
use crate::header::CommonHeader;
use crate::material::{folder, parse_properties, resolve_path};
use crate::packages::{PackagesBin, TextReader};
use crate::physx::Shape;

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
// Static levels (type 30, `.static`)

pub const KIND_STATIC_LEVEL: u16 = 30;

/// One placed model of a static level.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticObject {
    pub position: [f32; 3],
    /// Quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    pub scale: f32,
    /// Index into [`StaticLevel::models`].
    pub model: u16,
    /// Index into [`StaticLevel::natural_paths`] (meaning unknown).
    pub natural: u16,
    /// Index into [`StaticLevel::materials`].
    pub material: u16,
    /// Index into the level's matrix table (not read).
    pub matrix: u16,
}

/// A batch of placed models stored apart from the object list: hub and
/// mission levels with thousands of props keep them in a `0.static` next to
/// the level, which its CSG zones refer to (`StaticLevel=`).
///
/// The `H.` entry is a [`CommonHeader`] followed by (little-endian):
///
/// ```text
/// u32 n, n x str                   unknown paths
/// u32 0
/// u32 object_count
/// u32 n, n x { u32, u32 }          index group sizes; their sum is the number of u16 "indices 1"
/// u32 n, n x str                   model paths (absolute)
/// u32 n, n x str                   unknown paths
/// u32 n, n x str                   texture names
/// u32 n, n x str                   texture names
/// u32 n, n x str                   natural paths
/// u32 n, n x str                   attribute texts
/// u32 n, n x str                   light paths
/// u32 n, n x str                   material paths
/// u32 indices_2, u32 unknown_1, u32 unknown_2, u32 0, u32 matrices
/// u32 object_count, u32 0
/// ```
///
/// `B.` holds `object_count` records of 48 bytes (position, 4 unknown bytes,
/// rotation, scale, 12 unknown bytes), then the tables (`indices 1` and
/// `indices 2` as u16, `unknown_1` x 20 bytes, `unknown_2` x 48 bytes,
/// matrices x 36 bytes), then `object_count` records of 176 bytes whose
/// fields at 120, 128, 130 and 132 are the model, natural, material and
/// matrix indices (u16).
///
/// Learned from Warframe-Exporter's reader for this type; no code was copied.
#[derive(Debug, Clone)]
pub struct StaticLevel {
    pub common: CommonHeader,
    pub models: Vec<String>,
    pub natural_paths: Vec<String>,
    pub attributes: Vec<String>,
    pub materials: Vec<String>,
    pub objects: Vec<StaticObject>,
    /// Pairs of counts, one per group; the groups' `u16` entries follow each other in `group_indices`.
    pub group_sizes: Vec<(u32, u32)>,
    pub group_indices: Vec<u16>,
}

impl StaticLevel {
    pub fn parse(head: &[u8], body: &[u8]) -> Result<Self> {
        let common = CommonHeader::parse(head).context("common header")?;
        ensure!(
            common.kind == KIND_STATIC_LEVEL,
            "static level type {} is not supported (only {KIND_STATIC_LEVEL})",
            common.kind
        );
        let mut r = Reader::at(head, common.len)?;
        let _unknown_paths = string_list(&mut r).context("unknown paths")?;
        ensure!(r.u32()? == 0, "unexpected value before the object count");
        let count = r.u32()? as usize;
        ensure!(count < 1_000_000, "implausible object count {count}");
        let groups = r.u32()?;
        ensure!(groups < 1_000_000, "implausible index group count {groups}");
        let mut indices1 = 0usize;
        let mut group_sizes = Vec::new();
        for _ in 0..groups {
            let (a, b) = (r.u32()?, r.u32()?);
            indices1 += a as usize + b as usize;
            group_sizes.push((a, b));
        }
        let models = string_list(&mut r).context("model paths")?;
        let _ = string_list(&mut r).context("unknown paths")?;
        let _ = string_list(&mut r).context("texture names")?;
        let _ = string_list(&mut r).context("texture names")?;
        let natural_paths = string_list(&mut r).context("natural paths")?;
        let attributes = string_list(&mut r).context("attributes")?;
        let _lights = string_list(&mut r).context("light paths")?;
        let materials = string_list(&mut r).context("material paths")?;
        let indices2 = r.u32()? as usize;
        let unknown1 = r.u32()? as usize;
        let unknown2 = r.u32()? as usize;
        ensure!(r.u32()? == 0, "unexpected value before the matrix count");
        let matrices = r.u32()? as usize;
        ensure!(
            r.u32()? as usize == count,
            "second object count differs from the first"
        );
        ensure!(r.u32()? == 0, "unexpected final value");
        ensure!(r.rest().is_empty(), "bytes left after the header");

        let mut b = Reader::new(body);
        let mut objects = Vec::with_capacity(count);
        for _ in 0..count {
            let position = [b.f32()?, b.f32()?, b.f32()?];
            b.skip(4)?;
            let rotation = b.f32x4()?;
            let scale = b.f32()?;
            b.skip(12)?;
            objects.push(StaticObject {
                position,
                rotation,
                scale,
                model: 0,
                natural: 0,
                material: 0,
                matrix: 0,
            });
        }
        let group_indices = (0..indices1).map(|_| b.u16()).collect::<Result<Vec<_>>>()?;
        b.skip(indices2 * 2 + unknown1 * 20 + unknown2 * 48 + matrices * 36)?;
        for o in &mut objects {
            let rec = b.bytes(176)?;
            let u16_at = |i: usize| u16::from_le_bytes([rec[i], rec[i + 1]]);
            o.model = u16_at(120);
            o.natural = u16_at(128);
            o.material = u16_at(130);
            o.matrix = u16_at(132);
            ensure!(
                (o.model as usize) < models.len(),
                "model index {} out of range ({} models)",
                o.model,
                models.len()
            );
        }
        ensure!(b.rest().is_empty(), "bytes left after the object records");
        Ok(StaticLevel {
            common,
            models,
            natural_paths,
            attributes,
            materials,
            objects,
            group_sizes,
            group_indices,
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
    /// `OverrideMaterial` (or a lone `Material`): replaces the model's
    /// materials, in mesh order; an empty entry keeps the model's own.
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

/// A connection point of a tile (`LevelPortal`): where the level joins
/// another one. Procedural missions chain tiles by matching portals.
#[derive(Debug, Clone)]
pub struct Portal {
    pub object: usize,
    pub type_path: String,
    pub scope: String,
    pub transform: Transform,
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// Every attribute as text.
    pub attributes: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentKind {
    /// `LevelInfo`: settings of the whole level.
    Level,
    /// `ZoneAttribs`: settings of a region (ambient light, fog, reflection probe, ambience).
    Zone,
}

/// Sky, fog and ambient settings as the level gives them. Colours are as
/// stored (0..255, alpha last).
#[derive(Debug, Clone)]
pub struct Environment {
    pub object: usize,
    pub kind: EnvironmentKind,
    pub type_path: String,
    pub scope: String,
    pub position: [f32; 3],
    pub ambient_light: Option<[f32; 4]>,
    /// `FogColor`, or `PostProcessInfo.fogColor` of a level.
    pub fog_color: Option<[f32; 4]>,
    /// `DistanceFogDensity`, or `PostProcessInfo.distanceFogDensity`.
    pub fog_density: Option<f32>,
    pub override_level_fog: Option<bool>,
    /// The reflection probe's cube map (`ReflectionMap`).
    pub reflection_map: Option<String>,
    /// The sky or distant scenery to draw (`BackDropId`).
    pub backdrop: Option<String>,
    /// Every attribute as text.
    pub attributes: Vec<(String, String)>,
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
    pub environments: Vec<Environment>,
    pub portals: Vec<Portal>,
    /// Objects that were not turned into anything above, by reason or type.
    pub skipped: BTreeMap<String, usize>,
    /// `.static` files the level's zones refer to (`StaticLevel=`); their
    /// objects are added with [`LevelDescription::add_static_level`].
    pub static_levels: Vec<String>,
    /// PhysX shapes of each model, indexed like `models`; empty until
    /// `load_collision` (in [`crate::level_export`]) has read them. `None`
    /// for models that no visible instance uses or that failed to read.
    pub collision: Vec<Option<Rc<Vec<Shape>>>>,
    model_index: HashMap<String, usize>,
}

const CLASS_LIGHT: &str = "/EE/Types/Engine/Light";
const CLASS_ZONE: &str = "/EE/Types/Engine/CsgZone";
const CLASS_SHEET: &str = "/EE/Types/Alias/CsgSheet";
const CLASS_HLOD: &str = "/EE/Types/Engine/HLODAggregateEntity";
const CLASS_PORTAL: &str = "/EE/Types/Framework/LevelPortal";
const CLASS_LEVEL_INFO: &str = "/EE/Types/Engine/LevelInfo";
const CLASS_ZONE_ATTRIBS: &str = "/EE/Types/Engine/ZoneAttribs";
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
    /// The collision shapes of a model, once loaded.
    pub fn collision_shapes(&self, model: usize) -> Option<&[Shape]> {
        self.collision.get(model)?.as_deref().map(Vec::as_slice)
    }

    /// The index of a model path, adding it on first use.
    fn model(&mut self, path: &str) -> usize {
        if let Some(&i) = self.model_index.get(path) {
            return i;
        }
        self.models.push(path.to_string());
        self.model_index
            .insert(path.to_string(), self.models.len() - 1);
        self.models.len() - 1
    }

    /// Adds the objects of a static level as instances. Every object is added
    /// once, though a static level lists some of them under several zones.
    pub fn add_static_level(&mut self, path: &str, s: &StaticLevel) {
        for (i, o) in s.objects.iter().enumerate() {
            if s.models[o.model as usize].starts_with("/EE/Editor/") {
                *self
                    .skipped
                    .entry("editor helper mesh".to_string())
                    .or_default() += 1;
                continue;
            }
            let model = self.model(&s.models[o.model as usize]);
            self.instances.push(Instance {
                model,
                transform: Transform {
                    translation: o.position,
                    rotation: o.rotation,
                    scale: [o.scale; 3],
                },
                materials: Vec::new(),
                object: i,
                type_path: "(static level)".to_string(),
                scope: path.to_string(),
                hidden: false,
                role: InstanceRole::Prop,
                lightmap: None,
            });
        }
    }

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
                CLASS_PORTAL => d.portals.push(Portal {
                    object: i,
                    type_path: o.type_path.clone(),
                    scope: o.scope.clone(),
                    transform: Transform {
                        scale: [1.0; 3],
                        ..transform
                    },
                    width: r.float("Width"),
                    height: r.float("Height"),
                    attributes: r.all_text(),
                }),
                CLASS_LEVEL_INFO | CLASS_ZONE_ATTRIBS => {
                    let post = r.get("PostProcessInfo").map(|(v, _)| v.clone());
                    let from_post = |key: &str| post.as_ref().and_then(|p| p.get(key)).cloned();
                    d.environments.push(Environment {
                        object: i,
                        kind: if class == CLASS_LEVEL_INFO {
                            EnvironmentKind::Level
                        } else {
                            EnvironmentKind::Zone
                        },
                        type_path: o.type_path.clone(),
                        scope: o.scope.clone(),
                        position: o.position,
                        ambient_light: vec4(r.floats("AmbientLight")),
                        fog_color: vec4(r.floats("FogColor"))
                            .or_else(|| vec4(from_post("fogColor").and_then(|v| v.floats()))),
                        fog_density: r
                            .float("DistanceFogDensity")
                            .or_else(|| from_post("distanceFogDensity").and_then(|v| v.float())),
                        override_level_fog: r.flag("OverrideLevelFog"),
                        reflection_map: r.path("ReflectionMap"),
                        backdrop: r.text("BackDropId").map(str::to_string),
                        attributes: r.all_text(),
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
                CLASS_HLOD => skip(
                    "HLOD aggregate (a low-detail stand-in; type 108 models are not read)"
                        .to_string(),
                ),
                _ => {
                    let Some(mesh) = r.path("Mesh") else {
                        skip(format!("no mesh: {}", o.type_path));
                        continue;
                    };
                    if mesh.starts_with("/EE/Editor/") {
                        skip("editor helper mesh".to_string());
                        continue;
                    }
                    let model = d.model(&mesh);
                    if class == CLASS_ZONE {
                        if let Some(p) = r.path("StaticLevel") {
                            if !d.static_levels.contains(&p) {
                                d.static_levels.push(p);
                            }
                        }
                    }
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
                        materials: {
                            let list = r.paths("OverrideMaterial");
                            // A lone `Material` replaces the first slot, as an override list of one would
                            match (list.is_empty(), r.path("Material")) {
                                (true, Some(m)) => vec![m],
                                _ => list,
                            }
                        },
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

/// Reads the level at `level_path` (and the static levels its zones refer
/// to) from a `Cache.Windows` folder and describes it.
pub fn load_description(game: &Path, level_path: &str) -> Result<LevelDescription> {
    let read = |package: &str, path: &str| -> Result<Vec<u8>> {
        let mut pkg =
            Package::open_in(game, package)?.with_context(|| format!("no {package} package"))?;
        Ok(pkg.read_path(path)?.unwrap_or_default())
    };
    let head = read("H.AnimRetarget", level_path)?;
    ensure!(!head.is_empty(), "{level_path} is not in H.AnimRetarget");
    let body = read("B.AnimRetarget", level_path)?;
    let raw = RawLevel::parse(&head, &body).with_context(|| level_path.to_string())?;
    let bin = PackagesBin::load(game)?;
    let mut types = TypeResolver::new(&bin)?;
    let mut d = LevelDescription::build(&raw, level_path, &mut types)?;
    for path in d.static_levels.clone() {
        let (h, b) = (
            read("H.AnimRetarget", &path)?,
            read("B.AnimRetarget", &path)?,
        );
        ensure!(!h.is_empty(), "{path} is not in H.AnimRetarget");
        let s = StaticLevel::parse(&h, &b).with_context(|| path.clone())?;
        d.add_static_level(&path, &s);
    }
    Ok(d)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::header::tests::build;
    use crate::packages::PackagesBin;

    fn str32(out: &mut Vec<u8>, s: &str) {
        out.extend((s.len() as u32).to_le_bytes());
        out.extend(s.as_bytes());
    }

    /// A level with the given (type, position, rotation, scope, attributes) objects.
    /// (type, position, rotation, scope, attributes)
    pub type TestObject<'a> = (&'a str, [f32; 3], [f32; 4], &'a str, &'a str);

    pub fn build_level(objects: &[TestObject]) -> (Vec<u8>, Vec<u8>) {
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
        for o in objects {
            b.extend((o.4.len() as u32).to_le_bytes());
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
    fn transform_matrix_and_handedness() {
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let t = Transform {
            translation: [1.0, 2.0, 3.0],
            rotation: [0.0, q, 0.0, q], // 90 degrees about y: +z goes to +x
            scale: [2.0, 1.0, 1.0],
        };
        let m = t.matrix();
        let apply = |m: &[[f32; 4]; 4], p: [f32; 3]| -> [f32; 3] {
            std::array::from_fn(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3])
        };
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        // Scale first (x doubles), then rotate (x axis goes to -z), then move
        assert!(near(apply(&m, [1.0, 0.0, 0.0]), [1.0, 2.0, 1.0]));
        assert!(near(apply(&m, [0.0, 0.0, 1.0]), [2.0, 2.0, 3.0]));
        // Mirroring x: the right-handed matrix is S * M * S with S = diag(-1, 1, 1)
        let rh = t.to_right_handed().matrix();
        for p in [[1.0, 0.0, 0.0], [0.3, -2.0, 5.0], [0.0, 0.0, 1.0]] {
            let want = apply(&m, [-p[0], p[1], p[2]]);
            assert!(near(apply(&rh, p), [-want[0], want[1], want[2]]), "{p:?}");
        }
        assert_eq!(Transform::IDENTITY.matrix()[1][1], 1.0);
    }

    fn type_bin() -> Vec<u8> {
        crate::packages::tests::build(&[
            ("/EE/Types/Engine/", "Light", "", None),
            (
                "/EE/Types/Engine/",
                "Decoration",
                "",
                Some("UseLightMap=1\n"),
            ),
            ("/EE/Types/Engine/", "CsgZone", "", None),
            ("/EE/Types/Engine/", "PlayerSpawn", "", None),
            ("/EE/Types/Engine/", "LevelInfo", "", None),
            (
                "/EE/Types/Framework/",
                "LevelPortal",
                "",
                Some("Mesh=/EE/Editor/X.fbx\n"),
            ),
            (
                "/Lotus/Levels/Portals/",
                "Door5x3",
                "/EE/Types/Framework/LevelPortal",
                Some("Width=5\nHeight=3\n"),
            ),
            ("/EE/Types/Engine/", "ZoneAttribs", "", None),
            (
                "/Lotus/Levels/",
                "RoomAttribs",
                "/EE/Types/Engine/ZoneAttribs",
                Some("AmbientLight={50,50,50,0}\n"),
            ),
            ("/EE/Types/Engine/", "SimpleBlockingVolume", "", None),
            ("/EE/Types/Alias/", "CsgSheet", "", None),
            (
                "/Lotus/Levels/",
                "DojoLight",
                "/EE/Types/Engine/Light",
                Some("AllowPortalWalks=0\n"),
            ),
            (
                "/Lotus/Deco/",
                "Chair",
                "/EE/Types/Engine/Decoration",
                Some("Mesh=Chair.fbx\nMeshScale=0.5\n"),
            ),
            (
                "/Lotus/Deco/",
                "RedChair",
                "Chair",
                Some("Mesh=/Lotus/Other/Red.fbx\n"),
            ),
        ])
    }

    #[test]
    fn builds_a_description() {
        let id = [0.0, 0.0, 0.0, 1.0];
        let (h, b) = build_level(&[
            (
                "/EE/Types/Engine/CsgZone",
                [0.0; 3],
                id,
                "/",
                "Mesh=0_c.fbx\nStaticLevel=0.static\nLightMap={a_clr.png,/L/b_dir.png}\nLightMapCoords={0,0,0.5,0.5}\n",
            ),
            (
                "/EE/Types/Engine/Decoration",
                [1.0, 2.0, 3.0],
                id,
                "/Layer1/",
                "Mesh=/Lotus/Objects/A.fbx\nOverrideMaterial={/M/One,\"\",Rel}\n",
            ),
            (
                "/EE/Types/Engine/Decoration",
                [0.0; 3],
                id,
                "/Layer1/",
                "Mesh=/Lotus/Objects/A.fbx\nStartHidden=1\n",
            ),
            ("/Lotus/Deco/Chair", [0.0; 3], id, "/", ""),
            (
                "/EE/Types/Engine/Decoration",
                [0.0; 3],
                id,
                "/",
                "Mesh=/Lotus/Objects/A.fbx\nMaterial=/M/Solo\n",
            ),
            ("/Lotus/Deco/RedChair", [0.0; 3], id, "/", "MeshScale={1,2,3}\n"),
            (
                "DojoLight",
                [4.0, 5.0, 6.0],
                [0.0, 1.0, 0.0, 0.0],
                "/Layer3/",
                "Color={204,220,255,255}\nBrightness=6.25\nLumens=10000\nOn=0\nTexture=t.png\nDirectionalLightDir={1,0,0}\n",
            ),
            ("/EE/Types/Engine/PlayerSpawn", [7.0, 1.0, 8.0], id, "/", "Tag=Start\n"),
            (
                "/EE/Types/Engine/SimpleBlockingVolume",
                [0.0; 3],
                id,
                "/",
                "Length=4\nWidth=2\nHeight=3\nRadius=1.5\n",
            ),
            (
                "/EE/Types/Alias/CsgSheet",
                [0.0; 3],
                id,
                "/",
                "Material=/M/Sheet\nPoints={{0,0,0},{1,0,0},{1,1,0}}\n",
            ),
            ("/EE/Types/Engine/Decoration", [0.0; 3], id, "/", "Mesh=/EE/Editor/Darkitect/Objects/X.fbx\n"),
            ("/Lotus/Unknown/Thing", [0.0; 3], id, "/", "Foo=1\n"),
            (
                "/EE/Types/Engine/LevelInfo",
                [0.0; 3],
                id,
                "/",
                "PostProcessInfo={fogColor={102,110,126,255}\ndistanceFogDensity=0.17\n}\n",
            ),
            ("Portals/Door5x3", [2.0, 0.0, -20.0], [0.0, 1.0, 0.0, 0.0], "/", "Tag=Entry\n"),
            (
                "RoomAttribs",
                [1.0, 8.0, 10.0],
                id,
                "/",
                "FogColor={53,88,105,60}\nDistanceFogDensity=0.435\nReflectionMap=0_cross.hdr\nBackDropId=Sky\n",
            ),
        ]);
        let level = RawLevel::parse(&h, &b).unwrap();
        let bin = PackagesBin::parse(&type_bin()).unwrap();
        let mut types = TypeResolver::new(&bin).unwrap();
        let d = LevelDescription::build(&level, "/Lotus/Levels/X.level", &mut types).unwrap();

        // Models: the zone's mesh is relative to the folder named after the level
        assert_eq!(
            d.models,
            [
                "/Lotus/Levels/X/0_c.fbx",
                "/Lotus/Objects/A.fbx",
                // inherited from the type, relative to the type's folder
                "/Lotus/Deco/Chair.fbx",
                "/Lotus/Other/Red.fbx"
            ]
        );
        assert_eq!(d.instances.len(), 6);
        let zone = &d.instances[0];
        assert_eq!(zone.role, InstanceRole::Zone);
        let lm = zone.lightmap.as_ref().unwrap();
        assert_eq!(lm.textures, ["/Lotus/Levels/X/a_clr.png", "/L/b_dir.png"]);
        assert_eq!(lm.coords, [0.0, 0.0, 0.5, 0.5]);
        assert_eq!(d.static_levels, ["/Lotus/Levels/X/0.static"]);
        let a = &d.instances[1];
        assert_eq!(a.transform.translation, [1.0, 2.0, 3.0]);
        assert_eq!(a.materials, ["/M/One", "", "/Lotus/Levels/X/Rel"]);
        assert!(!a.hidden && d.instances[2].hidden);
        assert_eq!(d.instances[3].transform.scale, [0.5; 3]);
        assert_eq!(d.instances[3].model, 2);
        // A lone Material replaces the first slot
        assert_eq!(d.instances[4].materials, ["/M/Solo"]);
        // The child type overrides Mesh and keeps the scale; the object overrides the scale
        assert_eq!(d.instances[5].model, 3);
        assert_eq!(d.instances[5].transform.scale, [1.0, 2.0, 3.0]);

        // The relative type name resolves against the level's folder
        assert_eq!(d.lights.len(), 1);
        let l = &d.lights[0];
        assert_eq!(l.color, Some([204.0, 220.0, 255.0, 255.0]));
        assert_eq!(
            (l.brightness, l.lumens, l.on),
            (Some(6.25), Some(10000.0), Some(false))
        );
        assert_eq!(l.texture.as_deref(), Some("/Lotus/Levels/X/t.png"));
        assert_eq!(l.direction, Some([1.0, 0.0, 0.0]));
        assert!(l
            .attributes
            .contains(&("AllowPortalWalks".into(), "0".into())));
        assert_eq!(l.position, [4.0, 5.0, 6.0]);

        assert_eq!(d.spawns.len(), 1);
        assert_eq!(
            (d.spawns[0].kind, d.spawns[0].tag.as_deref()),
            (SpawnKind::Player, Some("Start"))
        );
        assert_eq!(d.volumes.len(), 1);
        assert_eq!(
            (d.volumes[0].length, d.volumes[0].width, d.volumes[0].height),
            (Some(4.0), Some(2.0), Some(3.0))
        );
        assert_eq!(d.sheets.len(), 1);
        assert_eq!(d.sheets[0].points.len(), 3);
        assert_eq!(d.sheets[0].material, "/M/Sheet");
        assert_eq!(d.skipped["editor helper mesh"], 1);
        assert_eq!(d.skipped["no mesh: /Lotus/Unknown/Thing"], 1);

        assert_eq!(d.portals.len(), 1);
        let p = &d.portals[0];
        assert_eq!((p.width, p.height), (Some(5.0), Some(3.0)));
        assert_eq!(p.transform.translation, [2.0, 0.0, -20.0]);
        assert!(p.attributes.contains(&("Tag".into(), "Entry".into())));

        assert_eq!(d.environments.len(), 2);
        let (info, zone) = (&d.environments[0], &d.environments[1]);
        assert_eq!(info.kind, EnvironmentKind::Level);
        assert_eq!(info.fog_color, Some([102.0, 110.0, 126.0, 255.0]));
        assert_eq!(info.fog_density, Some(0.17));
        assert_eq!(zone.kind, EnvironmentKind::Zone);
        assert_eq!(zone.ambient_light, Some([50.0, 50.0, 50.0, 0.0]));
        assert_eq!(zone.fog_color, Some([53.0, 88.0, 105.0, 60.0]));
        assert_eq!(
            zone.reflection_map.as_deref(),
            Some("/Lotus/Levels/X/0_cross.hdr")
        );
        assert_eq!(zone.backdrop.as_deref(), Some("Sky"));
        assert_eq!(zone.position, [1.0, 8.0, 10.0]);

        // Static levels add their objects once each
        let s = StaticLevel {
            common: level.common.clone(),
            models: vec!["/Lotus/Objects/A.fbx".into(), "/EE/Editor/X.fbx".into()],
            natural_paths: Vec::new(),
            attributes: Vec::new(),
            materials: Vec::new(),
            objects: vec![
                StaticObject {
                    position: [9.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: 3.0,
                    model: 0,
                    natural: 0,
                    material: 0,
                    matrix: 0,
                },
                StaticObject {
                    position: [0.0; 3],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: 1.0,
                    model: 1,
                    natural: 0,
                    material: 0,
                    matrix: 0,
                },
            ],
            group_sizes: Vec::new(),
            group_indices: Vec::new(),
        };
        let mut d = d;
        d.add_static_level("/Lotus/Levels/X/0.static", &s);
        assert_eq!(d.instances.len(), 7);
        assert_eq!(d.instances[6].transform.scale, [3.0; 3]);
        assert_eq!(d.instances[6].model, 1);
        assert_eq!(d.skipped["editor helper mesh"], 2);
        assert_eq!(d.models.len(), 4);
    }

    fn build_static(objects: &[(u16, [f32; 3])], models: &[&str]) -> (Vec<u8>, Vec<u8>) {
        let mut h = build(&[], "", u32::from(KIND_STATIC_LEVEL));
        let list = |h: &mut Vec<u8>, items: &[&str]| {
            h.extend((items.len() as u32).to_le_bytes());
            for p in items {
                str32(h, p);
            }
        };
        list(&mut h, &["/unknown"]);
        h.extend(0u32.to_le_bytes());
        h.extend((objects.len() as u32).to_le_bytes());
        h.extend(1u32.to_le_bytes()); // one index group
        h.extend(2u32.to_le_bytes());
        h.extend(1u32.to_le_bytes());
        list(&mut h, models);
        for _ in 0..4 {
            list(&mut h, &[]); // unknown paths, two texture lists, natural paths
        }
        list(&mut h, &[]); // attributes
        list(&mut h, &[]); // lights
        list(&mut h, &["/mat"]);
        // indices 2, unknown 1, unknown 2, 0, matrices
        for v in [1u32, 1, 1, 0, 1] {
            h.extend(v.to_le_bytes());
        }
        h.extend((objects.len() as u32).to_le_bytes());
        h.extend(0u32.to_le_bytes());

        let mut b = Vec::new();
        for (_, pos) in objects {
            for v in pos {
                b.extend(v.to_le_bytes());
            }
            b.extend([0; 4]);
            for v in [0.0f32, 0.0, 0.0, 1.0, 2.0] {
                b.extend(v.to_le_bytes());
            }
            b.extend([0; 12]);
        }
        b.extend([0; 3 * 2]); // group indices (2 + 1)
        b.extend([0; 2]); // indices 2
        b.extend([0; 20 + 48 + 36]);
        for (model, _) in objects {
            let mut rec = [0u8; 176];
            rec[120..122].copy_from_slice(&model.to_le_bytes());
            rec[128..130].copy_from_slice(&7u16.to_le_bytes());
            b.extend(rec);
        }
        (h, b)
    }

    #[test]
    fn parses_static_levels() {
        let (h, b) = build_static(
            &[(1, [1.0, 2.0, 3.0]), (0, [4.0, 5.0, 6.0])],
            &["/A.fbx", "/B.fbx"],
        );
        let s = StaticLevel::parse(&h, &b).unwrap();
        assert_eq!(s.models, ["/A.fbx", "/B.fbx"]);
        assert_eq!(s.materials, ["/mat"]);
        assert_eq!(s.objects.len(), 2);
        assert_eq!(
            (s.objects[0].model, s.objects[0].position),
            (1, [1.0, 2.0, 3.0])
        );
        assert_eq!((s.objects[1].scale, s.objects[1].natural), (2.0, 7));
        assert_eq!(
            (s.group_sizes.clone(), s.group_indices.len()),
            (vec![(2, 1)], 3)
        );
        // A model index past the list and trailing bytes are errors
        let (h2, b2) = build_static(&[(5, [0.0; 3])], &["/A.fbx"]);
        assert!(StaticLevel::parse(&h2, &b2).is_err());
        let mut longer = b.clone();
        longer.push(0);
        assert!(StaticLevel::parse(&h, &longer).is_err());
        assert!(StaticLevel::parse(&h, &b[..b.len() - 1]).is_err());
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
