//! [`rewrite_bridge::Provider`] for Warframe: lists the static (type 102)
//! and skinned (type 281) models of a `Cache.Windows` folder and loads them
//! as bridge [`Model`]s.
//!
//! Geometry goes through [`crate::scene::prepare`], the same conversion as
//! the glTF export: x is negated to turn the game's left-handed space into
//! the bridge's right-handed one, which also keeps characters facing +z.
//! Warframe's units are already metres (Excalibur stands 1.84 m tall), so
//! nothing is scaled. Materials are baked with [`crate::pbr`] as for glTF.
//!
//! Every mesh of a model shares one vertex buffer in the game; here each
//! bridge mesh gets only the vertices its LOD 0 triangles use.
//!
//! # Ids
//!
//! Ids are asset paths, the ones `list` returns.
//!
//! - A **model** id is the path of a static (type 102) or skinned (type 281) model.
//! - An **animated** id is a skinned model's path followed by `?animations=FILTER`.
//!   [`Provider::load_animated`] then also returns the clips whose path matches `FILTER`
//!   (a substring, or a glob with `*`, `**` and `?`; several filters can be given
//!   separated by `|`) and whose bones are all bones of the model: that is how the game
//!   retargets, and what `wf-unpack anim <model>` lists. Without the suffix no clip is
//!   loaded, since a Warframe skeleton fits about 25,000 clips. [`animated_id`] builds the
//!   id, and `load(id)` ignores the suffix so it stays the same model.
//!   Details of the clips are in [`crate::bridge_anim`].
//! - A **level** id is the path of a `.level`; `list` returns them as [`AssetKind::Level`] and
//!   [`Provider::load_scene`] builds the scene. Details are in [`crate::bridge_scene`]. The
//!   suffix `?collision` (see [`collision_id`]) gives the level's collision as a scene of its
//!   own.
//!
//! # Example
//!
//! ```no_run
//! use rewrite_bridge::{Provider, SceneModel, Target};
//! use wf_unpack::bridge::{animated_id, collision_id, Warframe};
//! use wf_unpack::player::Player;
//!
//! # fn main() -> Result<(), rewrite_bridge::BoxError> {
//! let game = std::path::Path::new("/path/to/Cache.Windows");
//! // Textures are baked at full size by default; a limit keeps big levels small
//! let wf = Warframe::open(game)?.with_max_texture(Some(1024));
//!
//! // A character with the clips of one stance. The clips animate the model's own nodes.
//! let sword = "/Lotus/Animations/Tenno/Movement/MeleeWeapon/Sword/";
//! let excalibur = wf.load_animated(&animated_id(
//!     "/Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx",
//!     sword,
//! ))?;
//! let jog = excalibur
//!     .animation(&format!("{sword}JogForward_anim.fbx"))
//!     .unwrap();
//! // Root motion is the root bone's translation channel; events are in a side table
//! let root = excalibur.model.nodes.iter().position(|n| n.name == "GAME_C1_ROOT").unwrap();
//! let root_track = jog
//!     .channels
//!     .iter()
//!     .find(|c| c.node == root && c.target == Target::Translation)
//!     .unwrap();
//! let info = wf.clip_info(&jog.name).unwrap();
//! println!(
//!     "{} s, {} keys of root motion, StepL at {:?} s",
//!     jog.duration_s,
//!     root_track.times.len(),
//!     info.event_time("StepL")
//! );
//!
//! // A level as a scene: models once, instances many times, lights in candela
//! let level = "/Lotus/Levels/ClanDojo/ClanObstacleCourseA.level";
//! let scene = wf.load_scene(level)?;
//! let SceneModel::Inline(first) = &scene.models[scene.instances[0].model] else {
//!     unreachable!()
//! };
//! println!("{} instances of {} models; first: {}", scene.instances.len(), scene.models.len(), first.name);
//! // Its collision, as a second scene to build static physics bodies from
//! let collision = wf.load_scene(&collision_id(level))?;
//! println!("{} collision instances", collision.instances.len());
//!
//! // The movement values, each with the Packages.bin entity and key it came from
//! let player = Player::load(game)?;
//! println!(
//!     "max speed {} ({}), jump {}, gravity {}",
//!     player.ground.max_speed.value,
//!     player.ground.max_speed.cite(),
//!     player.jump.height.value,
//!     player.gravity.world.value[1]
//! );
//! # Ok(())
//! # }
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, ensure, Context, Result};
use rewrite_bridge::{
    AnimatedModel, AssetInfo, AssetKind, BoxError, Image, Material, Mesh, Model, Node, Provider,
    Skin,
};

use crate::animation::{self, AnimHeader, KIND_ANIMATION};
use crate::bridge_anim::{self, ClipInfo};
use crate::cache::Package;
use crate::clipspec::clip_spec;
use crate::header::CommonHeader;
use crate::level::TypeResolver;
use crate::model::{self, KIND_SKINNED, KIND_STATIC};
use crate::packages::PackagesBin;
use crate::pbr::{self, Baked, MaterialBaker};
use crate::scene::{self, Scene};
use crate::toc::{glob_match, Entry};

/// The suffix that asks for animation clips: `<model path>?animations=<filter>`.
pub const ANIMATIONS_SUFFIX: &str = "?animations=";
/// The suffix that asks for a level's collision: `<level path>?collision`.
pub const COLLISION_SUFFIX: &str = "?collision";
/// The most clips one `load_animated` returns by default; asking for more is an error that says
/// to narrow the filter (see [`Warframe::with_max_clips`]).
pub const DEFAULT_MAX_CLIPS: usize = 2000;

/// The id that loads `model` with the clips matching `filter`.
pub fn animated_id(model: &str, filter: &str) -> String {
    format!("{model}{ANIMATIONS_SUFFIX}{filter}")
}

/// The id that loads the collision scene of a level.
pub fn collision_id(level: &str) -> String {
    format!("{level}{COLLISION_SUFFIX}")
}

/// Splits an id into the asset path and the clip filter of an animated id.
pub fn split_animated_id(id: &str) -> (&str, Option<&str>) {
    match id.split_once(ANIMATIONS_SUFFIX) {
        Some((model, filter)) => (model, Some(filter)),
        None => (id, None),
    }
}

/// True for the paths [`Warframe::list`] reports as levels.
pub fn is_level(path: &str) -> bool {
    path.ends_with(".level")
}

/// `filter` is a substring, or a glob when it has `*` or `?`.
fn matches(filter: &str, path: &str) -> bool {
    if filter.contains(['*', '?']) {
        glob_match(filter, path)
    } else {
        path.contains(filter)
    }
}

/// The Warframe provider: models, animated models and level scenes of one `Cache.Windows`
/// folder (see the module docs for the ids). It opens the packages once and can be shared
/// between threads; loads are serialised.
pub struct Warframe {
    game: PathBuf,
    textures: bool,
    max_texture: Option<u32>,
    max_clips: usize,
    inner: Mutex<Inner>,
    /// The side table of [`Provider::load_animated`]: clip name to its extra data
    clips: Mutex<BTreeMap<String, ClipInfo>>,
}

struct Inner {
    head: Package,
    body: Option<Package>,
    full: Option<Package>,
    /// Opened on the first load that bakes textures
    baker: Option<MaterialBaker>,
    /// Packages.bin for the clips' entities, read when the baker is not there to share it
    bin: Option<PackagesBin>,
    /// Model paths with their type, read once
    index: Option<Vec<(String, u16)>>,
}

/// A clip that was found for a model but not loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedClip {
    pub path: String,
    pub reason: String,
}

/// What [`Warframe::load_animated_clips`] returns: the bridge's [`AnimatedModel`] and the
/// data that has no place in it.
#[derive(Debug, Clone)]
pub struct AnimatedClips {
    pub model: AnimatedModel,
    /// Extra data of each clip, by `Animation::name` (see [`crate::bridge_anim`]): events, root
    /// motion, play rate and the Packages.bin entity.
    pub info: BTreeMap<String, ClipInfo>,
    /// Compatible clips that were left out, with the reason (undecoded formats, values the
    /// bridge cannot hold).
    pub skipped: Vec<SkippedClip>,
    /// Animation files that matched the filter but animate another skeleton.
    pub other_skeletons: usize,
}

/// The asset kind a model path suggests.
pub fn kind_of(path: &str) -> AssetKind {
    if path.starts_with("/Lotus/Weapons/") {
        AssetKind::Weapon
    } else if path.starts_with("/Lotus/Characters/") {
        AssetKind::Character
    } else {
        AssetKind::Prop
    }
}

impl Warframe {
    /// Opens the model packages of a `Cache.Windows` folder.
    pub fn open(game: &Path) -> Result<Self> {
        let open = |name: &str| Package::open_in(game, name);
        Ok(Warframe {
            game: game.to_path_buf(),
            textures: true,
            max_texture: None,
            max_clips: DEFAULT_MAX_CLIPS,
            inner: Mutex::new(Inner {
                head: open("H.Misc")?.context("no H.Misc package")?,
                body: open("B.Misc")?,
                full: open("F.Misc")?,
                baker: None,
                bin: None,
                index: None,
            }),
            clips: Mutex::new(BTreeMap::new()),
        })
    }

    /// Shrinks baked textures to at most this many pixels on a side (a box filter over 2x2
    /// blocks, halving until they fit). Default: no limit. A level's models each own copies of
    /// their images (the bridge's `Material` owns them), so big levels need a limit: the dojo's
    /// 39 models hold 1 GB of images at full size.
    pub fn with_max_texture(mut self, max: Option<u32>) -> Self {
        self.max_texture = max;
        self
    }

    /// The most clips one animated load may return (default [`DEFAULT_MAX_CLIPS`]).
    pub fn with_max_clips(mut self, max: usize) -> Self {
        self.max_clips = max;
        self
    }

    /// The Warframe game folder.
    pub fn game_dir(&self) -> &Path {
        &self.game
    }

    /// The extra data (events, root motion, play rate, Packages.bin entity) of a clip loaded by
    /// [`Provider::load_animated`], by the clip's `Animation::name`. The table collects every
    /// clip loaded through this provider; [`Warframe::load_animated_clips`] returns the same data
    /// for one call without touching it.
    pub fn clip_info(&self, name: &str) -> Option<ClipInfo> {
        self.clips.lock().unwrap().get(name).cloned()
    }

    /// With `false`, materials keep their names but get no textures, which
    /// makes loading much faster (for checks).
    pub fn with_textures(mut self, textures: bool) -> Self {
        self.textures = textures;
        self
    }

    /// Paths and types of every static and skinned model, reading each header once.
    pub fn models(&self) -> Result<Vec<(String, u16)>> {
        let mut inner = self.inner.lock().unwrap();
        if inner.index.is_none() {
            let mut entries: Vec<_> = inner.head.toc.entries.clone();
            entries.sort_by_key(|e| e.offset);
            let mut index = Vec::new();
            for e in &entries {
                // Duplicate paths: only the live (later) entry counts
                if inner.head.toc.get(&e.path).map(|live| live.offset) != Some(e.offset) {
                    continue;
                }
                let Ok(raw) = inner.head.read(e) else {
                    continue;
                };
                let Ok(common) = CommonHeader::parse(&raw) else {
                    continue;
                };
                if common.kind == KIND_STATIC || common.kind == KIND_SKINNED {
                    index.push((e.path.clone(), common.kind));
                }
            }
            index.sort();
            inner.index = Some(index);
        }
        Ok(inner.index.clone().unwrap())
    }

    fn load_model(&self, id: &str) -> Result<Model> {
        let name = id.rsplit('/').next().unwrap_or(id).trim_end_matches(".fbx");
        self.load_model_as(id, name, &[], None)
    }

    /// Loads a model under `name`, with `overrides` replacing its materials slot by slot (an
    /// empty entry keeps the model's own; entries are absolute material paths). This is how a
    /// level instance with an `OverrideMaterial` list gets its own variant of a shared model.
    ///
    /// `cache` remembers baked materials by their resolved path, so a level that places many
    /// models sharing materials bakes each material once (the images are cloned into every
    /// model, as the bridge's `Material` owns its images).
    pub(crate) fn load_model_as(
        &self,
        id: &str,
        name: &str,
        overrides: &[String],
        mut cache: Option<&mut HashMap<String, Material>>,
    ) -> Result<Model> {
        let mut inner = self.inner.lock().unwrap();
        let Inner {
            head,
            body,
            full,
            baker,
            ..
        } = &mut *inner;
        let model = model::Model::load_from(head, body.as_mut(), full.as_mut(), id)?;
        let scene = scene::prepare(&model, 0, true)?;
        let names = model.header.materials();
        let mut materials = Vec::with_capacity(model.header.meshes.len());
        for i in 0..model.header.meshes.len() {
            let own = names.get(i).cloned();
            let wanted = match overrides.get(i) {
                Some(o) if !o.is_empty() => Some(o.clone()),
                _ => own,
            };
            let label = wanted.clone().unwrap_or_else(|| format!("material{i}"));
            let short = label.rsplit('/').next().unwrap_or(&label).to_string();
            let key = wanted
                .as_ref()
                .map(|w| crate::material::resolve_path(crate::material::folder(id), w));
            if let Some(hit) = key
                .as_ref()
                .and_then(|k| cache.as_deref().and_then(|c| c.get(k)))
            {
                materials.push(hit.clone());
                continue;
            }
            let baked = if self.textures && wanted.is_some() {
                if baker.is_none() {
                    *baker = Some(MaterialBaker::open(&self.game)?);
                }
                let b = baker.as_mut().unwrap();
                // A material that cannot be resolved still gets a plain one
                b.plan(id, &label).and_then(|plan| b.bake(&plan)).ok()
            } else {
                None
            };
            let material = to_material(short, baked, self.max_texture);
            if let (Some(k), Some(c)) = (key, cache.as_deref_mut()) {
                c.insert(k, material.clone());
            }
            materials.push(material);
        }
        Ok(convert(&scene, name.to_string(), materials))
    }
}

impl Warframe {
    /// The paths of every `.level` (type 212 in `H.AnimRetarget`), sorted.
    pub fn levels(&self) -> Result<Vec<String>> {
        let pkg = Package::open_in(&self.game, "H.AnimRetarget")?;
        let mut levels: Vec<String> = pkg
            .map(|p| {
                p.toc
                    .entries
                    .iter()
                    .filter(|e| is_level(&e.path))
                    .map(|e| e.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        levels.sort();
        levels.dedup();
        Ok(levels)
    }

    /// Loads a model with the clips named by an animated id (see the module docs), and the data
    /// the bridge's types have no field for. [`Provider::load_animated`] is this without `info`
    /// and `skipped`.
    pub fn load_animated_clips(&self, id: &str) -> Result<AnimatedClips> {
        let (path, filter) = split_animated_id(id);
        let model = self.load_model(path)?;
        let mut out = AnimatedClips {
            model: AnimatedModel::new(model),
            info: BTreeMap::new(),
            skipped: Vec::new(),
            other_skeletons: 0,
        };
        if let Some(filter) = filter {
            ensure!(
                !out.model.model.skins.is_empty(),
                "{path} is not a skinned model, so no clip fits it"
            );
            self.add_clips(&mut out, filter)?;
        }
        out.model.validate()?;
        Ok(out)
    }

    /// Adds every compatible clip whose path matches one of the `|`-separated filters.
    fn add_clips(&self, out: &mut AnimatedClips, filter: &str) -> Result<()> {
        // The model's node 0 is the model; the bones follow
        let node_of: HashMap<String, usize> = out
            .model
            .model
            .nodes
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, n)| (n.name.clone(), i))
            .collect();
        let node_count = out.model.model.nodes.len();
        let filters: Vec<&str> = filter.split('|').collect();

        let mut inner = self.inner.lock().unwrap();
        let Inner {
            head,
            body,
            baker,
            bin,
            ..
        } = &mut *inner;
        let body = body.as_mut().context("no B.Misc package")?;
        if baker.is_none() && bin.is_none() {
            *bin = Some(PackagesBin::load(&self.game)?);
        }
        let bin: &PackagesBin = match baker.as_ref() {
            Some(b) => &b.bin,
            None => bin.as_ref().unwrap(),
        };
        let mut types = TypeResolver::new(bin)?;

        // Animations live under /Lotus/Animations/; a few have no .fbx extension
        let mut entries: Vec<Entry> = head
            .toc
            .entries
            .iter()
            .filter(|e| e.path.contains("Animations/") || e.path.ends_with("_anim.fbx"))
            .filter(|e| filters.iter().any(|f| matches(f, &e.path)))
            // Duplicate paths: only the live (later) entry counts
            .filter(|e| head.toc.get(&e.path).map(|live| live.offset) == Some(e.offset))
            .cloned()
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        for e in &entries {
            let raw = head.read(e)?;
            let Ok(common) = CommonHeader::parse(&raw) else {
                continue;
            };
            if common.kind != KIND_ANIMATION {
                continue;
            }
            // A header that cannot be read (a few dozen files) may or may not fit the model
            let header = match AnimHeader::parse(&raw) {
                Ok(h) => h,
                Err(err) => {
                    out.skipped.push(SkippedClip {
                        path: e.path.clone(),
                        reason: format!("header not read: {err:#}"),
                    });
                    continue;
                }
            };
            let usable: Vec<usize> = header
                .skeletons
                .iter()
                .enumerate()
                .filter(|(_, s)| s.bones.iter().all(|b| node_of.contains_key(&b.name)))
                .map(|(i, _)| i)
                .collect();
            if usable.is_empty() {
                out.other_skeletons += 1;
                continue;
            }
            let wanted: usize = header
                .skeletons
                .iter()
                .enumerate()
                .filter(|(i, _)| usable.contains(i))
                .map(|(_, s)| s.actions.len())
                .sum();
            if out.model.animations.len() + out.skipped.len() + wanted > self.max_clips {
                bail!(
                    "the filter {filter:?} matches more than {} clips for this model; narrow it \
                     (or raise the limit with with_max_clips)",
                    self.max_clips
                );
            }
            let decoded = match body.read_path(&e.path) {
                Ok(Some(bytes)) => animation::decode(&e.path, &raw, &bytes),
                Ok(None) => Err(anyhow::anyhow!("no B.Misc part")),
                Err(err) => Err(err),
            };
            let clips = match decoded {
                Ok(c) => c,
                Err(err) => {
                    out.skipped.push(SkippedClip {
                        path: e.path.clone(),
                        reason: format!("{err:#}"),
                    });
                    continue;
                }
            };
            let single = clips.len() == 1;
            // The entity's keys, then the clip's own header text over them
            let mut spec = clip_spec(&mut types, &e.path)?;
            spec.apply_header(&common.attributes)
                .with_context(|| e.path.clone())?;
            for c in clips.iter().filter(|c| usable.contains(&c.skeleton_index)) {
                let name = if single {
                    e.path.clone()
                } else {
                    format!("{}#{}:{}", e.path, c.skeleton, c.name)
                };
                let nodes: Vec<usize> = c.bones.iter().map(|b| node_of[&b.name]).collect();
                let converted = bridge_anim::convert(c, &nodes, &name, Some(spec.clone()))
                    .and_then(|(a, info)| {
                        a.validate(node_count).map_err(|e| anyhow::anyhow!("{e}"))?;
                        Ok((a, info))
                    });
                match converted {
                    Ok((a, info)) => {
                        out.info.insert(name, info);
                        out.model.animations.push(a);
                    }
                    Err(err) => out.skipped.push(SkippedClip {
                        path: name,
                        reason: format!("{err:#}"),
                    }),
                }
            }
        }
        Ok(())
    }
}

/// Halves an image until it fits `max` (a box filter over 2x2 blocks).
fn shrink(img: pbr::Image, max: Option<u32>) -> pbr::Image {
    let Some(max) = max else { return img };
    let mut cur = img;
    while cur.width.max(cur.height) > max && cur.width > 1 && cur.height > 1 {
        let (w, h) = (cur.width / 2, cur.height / 2);
        let mut out = pbr::Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let i = (((2 * y + dy) * cur.width + 2 * x + dx) * 4) as usize;
                    for (a, &v) in acc.iter_mut().zip(&cur.rgba[i..i + 4]) {
                        *a += u32::from(v);
                    }
                }
                let o = ((y * w + x) * 4) as usize;
                for (dst, a) in out.rgba[o..o + 4].iter_mut().zip(acc) {
                    *dst = ((a + 2) / 4) as u8;
                }
            }
        }
        cur = out;
    }
    cur
}

fn to_image(img: pbr::Image, max: Option<u32>) -> Image {
    let img = shrink(img, max);
    Image {
        width: img.width,
        height: img.height,
        rgba: img.rgba,
    }
}

fn to_material(name: String, baked: Option<Baked>, max: Option<u32>) -> Material {
    match baked {
        None => Material {
            name,
            metallic: 0.0,
            roughness: 0.6,
            ..Material::default()
        },
        Some(b) => Material {
            name,
            base_color_factor: b.base_color_factor,
            base_color: b.base_color.map(|i| to_image(i, max)),
            metallic_roughness: b.metallic_roughness.map(|i| to_image(i, max)),
            metallic: b.metallic_factor,
            roughness: b.roughness_factor,
            normal: b.normal.map(|i| to_image(i, max)),
            emissive_factor: if b.emissive.is_some() {
                b.emissive_factor
            } else {
                [0.0; 3]
            },
            emissive: b.emissive.map(|i| to_image(i, max)),
            ..Material::default()
        },
    }
}

/// The entries of `v` at `used` (empty stays empty).
fn pick<T: Copy>(v: &[T], used: &[usize]) -> Vec<T> {
    if v.is_empty() {
        Vec::new()
    } else {
        used.iter().map(|&i| v[i]).collect()
    }
}

/// Builds the bridge model: node 0 is the model, bones follow as nodes 1..
pub fn convert(scene: &Scene, name: String, mut materials: Vec<Material>) -> Model {
    let mut nodes = vec![Node::new(name.clone(), None)];
    let mut skins = Vec::new();
    if let Some(sk) = &scene.skin {
        for b in &sk.bones {
            nodes.push(Node {
                name: b.name.clone(),
                parent: Some(b.parent.map_or(0, |p| p + 1)),
                translation: b.translation,
                rotation: b.rotation,
                scale: b.scale,
            });
        }
        skins.push(Skin {
            joints: sk.joints.iter().map(|j| j + 1).collect(),
            // Row-major in the scene, column-major in the bridge
            inverse_bind: sk
                .inverse_binds
                .iter()
                .map(|m| std::array::from_fn(|k| m[k % 4][k / 4]))
                .collect(),
        });
    }

    let skinned = !skins.is_empty();
    let mut meshes = Vec::with_capacity(scene.primitives.len());
    for p in &scene.primitives {
        // Keep only the vertices this mesh uses
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let mut used = Vec::new();
        let indices: Vec<u32> = p
            .indices
            .iter()
            .map(|&i| {
                *remap.entry(i).or_insert_with(|| {
                    used.push(i as usize);
                    (used.len() - 1) as u32
                })
            })
            .collect();
        if p.mesh >= materials.len() {
            materials.resize_with(p.mesh + 1, || Material {
                name: "material".into(),
                metallic: 0.0,
                roughness: 0.6,
                ..Material::default()
            });
        }
        meshes.push(Mesh {
            name: format!("{name}_{}", p.mesh),
            node: 0,
            material: p.mesh,
            positions: pick(&scene.positions, &used),
            normals: pick(&scene.normals, &used),
            tangents: pick(&scene.tangents, &used),
            uvs: pick(&scene.uv0, &used),
            joints: if skinned {
                pick(&scene.joints, &used)
            } else {
                Vec::new()
            },
            weights: if skinned {
                pick(&scene.weights, &used)
            } else {
                Vec::new()
            },
            skin: skinned.then_some(0),
            indices,
        });
    }
    Model {
        name,
        nodes,
        meshes,
        materials,
        skins,
    }
}

impl Provider for Warframe {
    fn game(&self) -> &str {
        "Warframe"
    }

    fn list(&self, filter: &str) -> Result<Vec<AssetInfo>, BoxError> {
        let stem = |path: &str| {
            path.rsplit('/')
                .next()
                .unwrap_or(path)
                .trim_end_matches(".fbx")
                .trim_end_matches(".level")
                .to_string()
        };
        let mut out: Vec<AssetInfo> = self
            .models()?
            .into_iter()
            .filter(|(path, _)| path.contains(filter))
            .map(|(path, _)| AssetInfo {
                name: stem(&path),
                kind: kind_of(&path),
                id: path,
            })
            .collect();
        out.extend(
            self.levels()?
                .into_iter()
                .filter(|path| path.contains(filter))
                .map(|path| AssetInfo {
                    name: stem(&path),
                    kind: AssetKind::Level,
                    id: path,
                }),
        );
        Ok(out)
    }

    /// The model of `id`; an animated id's clip filter is ignored here.
    fn load(&self, id: &str) -> Result<Model, BoxError> {
        let model = self.load_model(split_animated_id(id).0)?;
        model.validate()?;
        Ok(model)
    }

    /// A skinned model's id with `?animations=FILTER` returns the model and its compatible
    /// clips (see the module docs); any other model id returns the model alone. Each clip's
    /// events, root motion and play rate are in [`Warframe::clip_info`].
    fn load_animated(&self, id: &str) -> Result<AnimatedModel, BoxError> {
        let loaded = self.load_animated_clips(id)?;
        self.clips.lock().unwrap().extend(loaded.info);
        Ok(loaded.model)
    }

    fn load_scene(&self, id: &str) -> Result<rewrite_bridge::Scene, BoxError> {
        Ok(crate::bridge_scene::load_scene(self, id)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Primitive, SceneBone, SceneSkin};

    #[test]
    fn kinds_from_paths() {
        assert_eq!(
            kind_of("/Lotus/Weapons/Tenno/Melee/Swords/X.fbx"),
            AssetKind::Weapon
        );
        assert_eq!(
            kind_of("/Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx"),
            AssetKind::Character
        );
        assert_eq!(kind_of("/Lotus/Objects/Lasria/Sign.fbx"), AssetKind::Prop);
    }

    #[test]
    fn ids_carry_the_clip_filter_and_the_collision_flag() {
        let id = animated_id("/Lotus/X/Body_skel.fbx", "/Lotus/Animations/Sword/");
        assert_eq!(
            id,
            "/Lotus/X/Body_skel.fbx?animations=/Lotus/Animations/Sword/"
        );
        assert_eq!(
            split_animated_id(&id),
            ("/Lotus/X/Body_skel.fbx", Some("/Lotus/Animations/Sword/"))
        );
        // Without a suffix there is no filter; a glob's `?` stays in the filter
        assert_eq!(split_animated_id("/Lotus/X.fbx"), ("/Lotus/X.fbx", None));
        assert_eq!(
            split_animated_id("/Lotus/X.fbx?animations=/A/Jump?_anim.fbx|/B/"),
            ("/Lotus/X.fbx", Some("/A/Jump?_anim.fbx|/B/"))
        );
        assert_eq!(
            collision_id("/Lotus/Levels/A.level"),
            "/Lotus/Levels/A.level?collision"
        );
        assert!(is_level("/Lotus/Levels/A.level") && !is_level("/Lotus/A.fbx"));
    }

    #[test]
    fn the_provider_can_be_shared_between_threads() {
        fn shared<T: Send + Sync>() {}
        shared::<Warframe>();
    }

    #[test]
    fn filters_are_substrings_or_globs() {
        let p = "/Lotus/Animations/Tenno/Movement/MeleeWeapon/Sword/JogForward_anim.fbx";
        assert!(matches("/Sword/", p));
        assert!(matches("", p));
        assert!(!matches("/Katana/", p));
        assert!(matches("/Lotus/Animations/**/Jog*_anim.fbx", p));
        assert!(matches("**/Sword/Jog?orward_anim.fbx", p));
        assert!(!matches("/Lotus/*/Jog*", p));
    }

    #[test]
    fn textures_shrink_by_a_box_filter() {
        let mut img = pbr::Image::new(4, 2);
        for (i, px) in img.rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            px.copy_from_slice(&[(i * 10) as u8, 100, 0, 255]);
        }
        let s = shrink(img.clone(), Some(2));
        assert_eq!((s.width, s.height), (2, 1));
        // Pixels 0, 1, 4, 5 average to (0 + 10 + 40 + 50) / 4 = 25
        assert_eq!(s.rgba[..4], [25, 100, 0, 255]);
        assert_eq!(shrink(img.clone(), Some(4)).width, 4);
        assert_eq!(shrink(img, None).width, 4);
    }

    fn scene() -> Scene {
        let n = 4;
        Scene {
            positions: (0..n).map(|i| [i as f32, 0.0, 0.0]).collect(),
            normals: vec![[0.0, 0.0, 1.0]; n],
            tangents: vec![[1.0, 0.0, 0.0, 1.0]; n],
            uv0: vec![[0.0; 2]; n],
            uv1: vec![[0.0; 2]; n],
            joints: vec![[1, 0, 0, 0]; n],
            weights: vec![[1.0, 0.0, 0.0, 0.0]; n],
            primitives: vec![
                Primitive {
                    mesh: 0,
                    indices: vec![0, 1, 2],
                },
                Primitive {
                    mesh: 1,
                    indices: vec![3, 2, 1],
                },
            ],
            skin: Some(SceneSkin {
                bones: vec![
                    SceneBone {
                        name: "root".into(),
                        parent: None,
                        translation: [0.0; 3],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0; 3],
                    },
                    SceneBone {
                        name: "arm".into(),
                        parent: Some(0),
                        translation: [0.0, 1.0, 0.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0; 3],
                    },
                ],
                joints: vec![0, 1],
                inverse_binds: vec![
                    [
                        [1.0, 0.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0, 0.0],
                        [0.0, 0.0, 0.0, 1.0],
                    ],
                    [
                        [1.0, 0.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0, -1.0],
                        [0.0, 0.0, 1.0, 0.0],
                        [0.0, 0.0, 0.0, 1.0],
                    ],
                ],
            }),
            bounds: ([0.0; 3], [3.0, 0.0, 0.0]),
        }
    }

    #[test]
    fn converts_to_a_valid_bridge_model() {
        let m = convert(&scene(), "test".into(), vec![Material::default()]);
        assert_eq!(m.validate(), Ok(()));
        // Model node, then the bones under it
        assert_eq!(m.nodes.len(), 3);
        assert_eq!((m.nodes[1].parent, m.nodes[2].parent), (Some(0), Some(1)));
        assert_eq!(m.skins[0].joints, [1, 2]);
        // Column-major: the translation sits in elements 12..15
        assert_eq!(m.skins[0].inverse_bind[1][13], -1.0);
        // Each mesh keeps only its own vertices, renumbered
        assert_eq!(
            m.meshes[1].positions,
            [[3.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.0, 0.0]]
        );
        assert_eq!(m.meshes[1].indices, [0, 1, 2]);
        // A missing material is filled in
        assert_eq!(m.materials.len(), 2);
    }
}
