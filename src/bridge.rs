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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use rewrite_bridge::{
    AssetInfo, AssetKind, BoxError, Image, Material, Mesh, Model, Node, Provider, Skin,
};

use crate::cache::Package;
use crate::header::CommonHeader;
use crate::model::{self, KIND_SKINNED, KIND_STATIC};
use crate::pbr::{self, Baked, MaterialBaker};
use crate::scene::{self, Scene};

pub struct Warframe {
    game: PathBuf,
    textures: bool,
    inner: Mutex<Inner>,
}

struct Inner {
    head: Package,
    body: Option<Package>,
    full: Option<Package>,
    /// Opened on the first load that bakes textures
    baker: Option<MaterialBaker>,
    /// Model paths with their type, read once
    index: Option<Vec<(String, u16)>>,
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
            inner: Mutex::new(Inner {
                head: open("H.Misc")?.context("no H.Misc package")?,
                body: open("B.Misc")?,
                full: open("F.Misc")?,
                baker: None,
                index: None,
            }),
        })
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
            let name = names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("material{i}"));
            let short = name.rsplit('/').next().unwrap_or(&name).to_string();
            let baked = if self.textures && names.get(i).is_some() {
                if baker.is_none() {
                    *baker = Some(MaterialBaker::open(&self.game)?);
                }
                let b = baker.as_mut().unwrap();
                // A material that cannot be resolved still gets a plain one
                b.plan(id, &name).and_then(|plan| b.bake(&plan)).ok()
            } else {
                None
            };
            materials.push(to_material(short, baked));
        }
        let name = id
            .rsplit('/')
            .next()
            .unwrap_or(id)
            .trim_end_matches(".fbx")
            .to_string();
        Ok(convert(&scene, name, materials))
    }
}

fn to_image(img: pbr::Image) -> Image {
    Image {
        width: img.width,
        height: img.height,
        rgba: img.rgba,
    }
}

fn to_material(name: String, baked: Option<Baked>) -> Material {
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
            base_color: b.base_color.map(to_image),
            metallic_roughness: b.metallic_roughness.map(to_image),
            metallic: b.metallic_factor,
            roughness: b.roughness_factor,
            normal: b.normal.map(to_image),
            emissive_factor: if b.emissive.is_some() {
                b.emissive_factor
            } else {
                [0.0; 3]
            },
            emissive: b.emissive.map(to_image),
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
        Ok(self
            .models()?
            .into_iter()
            .filter(|(path, _)| path.contains(filter))
            .map(|(path, _)| AssetInfo {
                name: path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&path)
                    .trim_end_matches(".fbx")
                    .to_string(),
                kind: kind_of(&path),
                id: path,
            })
            .collect())
    }

    fn load(&self, id: &str) -> Result<Model, BoxError> {
        let model = self.load_model(id)?;
        model.validate()?;
        Ok(model)
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
