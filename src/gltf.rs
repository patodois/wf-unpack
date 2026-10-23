//! Minimal glTF 2.0 writer for a prepared [`Scene`]: one primitive per mesh
//! at the chosen LOD, sharing one set of vertex attributes; images are files
//! next to the `.gltf`.

use std::fs;
use std::path::Path;

use anyhow::{ensure, Result};
use serde_json::{json, Value};

use crate::animation::{Channel, Interpolation, Target};
use crate::scene::Scene;

/// Texture slots a material can fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    BaseColor,
    MetallicRoughness,
    Normal,
    Emissive,
}

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    /// (slot, image file name relative to the .gltf)
    pub images: Vec<(Slot, String)>,
    pub base_color_factor: [f32; 4],
    pub metallic_factor: f32,
    pub roughness_factor: f32,
    pub emissive_factor: [f32; 3],
    /// `KHR_materials_emissive_strength`, written when it is not 1.
    pub emissive_strength: f32,
}

impl Material {
    pub fn named(name: impl Into<String>) -> Self {
        Material {
            name: name.into(),
            images: Vec::new(),
            base_color_factor: [1.0; 4],
            metallic_factor: 0.0,
            roughness_factor: 0.6,
            emissive_factor: [1.0; 3],
            emissive_strength: 1.0,
        }
    }
}

/// An animation to write: channels whose `node` is a bone index of the
/// scene's skin (already converted to the scene's handedness).
pub struct Clip {
    pub name: String,
    pub channels: Vec<Channel>,
    /// Written as the animation's `extras` (events, root motion, ...).
    pub extras: Option<Value>,
}

pub fn write(scene: &Scene, name: &str, materials: &[Material], out_dir: &Path) -> Result<()> {
    write_animated(scene, name, materials, &[], out_dir)
}

/// Like [`write`], plus glTF animations driving the skeleton's bones.
pub fn write_animated(
    scene: &Scene,
    name: &str,
    materials: &[Material],
    clips: &[Clip],
    out_dir: &Path,
) -> Result<()> {
    ensure!(
        !scene.primitives.is_empty(),
        "{name} has no triangles to write (an empty \"primitives\" list draws nothing)"
    );
    fs::create_dir_all(out_dir)?;

    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut push_view = |bin: &mut Vec<u8>, bytes: &[u8], target: Option<u32>| -> usize {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let mut view = json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            view["target"] = json!(t);
        }
        views.push(view);
        bin.extend_from_slice(bytes);
        views.len() - 1
    };
    let floats =
        |vals: &mut dyn Iterator<Item = f32>| vals.flat_map(f32::to_le_bytes).collect::<Vec<u8>>();
    const VERTICES: Option<u32> = Some(34962);

    let n = scene.positions.len();
    let (lo, hi) = scene.bounds;
    let pos_view = push_view(
        &mut bin,
        &floats(&mut scene.positions.iter().flatten().copied()),
        VERTICES,
    );
    let nrm_view = push_view(
        &mut bin,
        &floats(&mut scene.normals.iter().flatten().copied()),
        VERTICES,
    );
    let tan_view = push_view(
        &mut bin,
        &floats(&mut scene.tangents.iter().flatten().copied()),
        VERTICES,
    );
    let uv0_view = push_view(
        &mut bin,
        &floats(&mut scene.uv0.iter().flatten().copied()),
        VERTICES,
    );
    let uv1_view = push_view(
        &mut bin,
        &floats(&mut scene.uv1.iter().flatten().copied()),
        VERTICES,
    );
    let mut accessors = vec![
        json!({"bufferView": pos_view, "componentType": 5126, "count": n, "type": "VEC3", "min": lo, "max": hi}),
        json!({"bufferView": nrm_view, "componentType": 5126, "count": n, "type": "VEC3"}),
        json!({"bufferView": uv0_view, "componentType": 5126, "count": n, "type": "VEC2"}),
        json!({"bufferView": uv1_view, "componentType": 5126, "count": n, "type": "VEC2"}),
        json!({"bufferView": tan_view, "componentType": 5126, "count": n, "type": "VEC4"}),
    ];
    let mut attributes =
        json!({"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2, "TEXCOORD_1": 3, "TANGENT": 4});
    if scene.skin.is_some() {
        let joints: Vec<u8> = scene
            .joints
            .iter()
            .flatten()
            .flat_map(|j| j.to_le_bytes())
            .collect();
        let view = push_view(&mut bin, &joints, VERTICES);
        accessors
            .push(json!({"bufferView": view, "componentType": 5123, "count": n, "type": "VEC4"}));
        attributes["JOINTS_0"] = json!(accessors.len() - 1);
        let view = push_view(
            &mut bin,
            &floats(&mut scene.weights.iter().flatten().copied()),
            VERTICES,
        );
        accessors
            .push(json!({"bufferView": view, "componentType": 5126, "count": n, "type": "VEC4"}));
        attributes["WEIGHTS_0"] = json!(accessors.len() - 1);
    }

    let mut primitives = Vec::new();
    for p in &scene.primitives {
        let bytes: Vec<u8> = p.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let view = push_view(&mut bin, &bytes, Some(34963));
        accessors.push(json!({"bufferView": view, "componentType": 5125, "count": p.indices.len(), "type": "SCALAR"}));
        let mut prim = json!({"attributes": attributes, "indices": accessors.len() - 1});
        if p.mesh < materials.len() {
            prim["material"] = json!(p.mesh);
        }
        primitives.push(prim);
    }

    let mut images: Vec<Value> = Vec::new();
    let mut image_index = |file: &str| -> usize {
        if let Some(i) = images.iter().position(|v| v["uri"] == file) {
            return i;
        }
        images.push(json!({ "uri": file }));
        images.len() - 1
    };
    let mut gl_materials = Vec::new();
    let mut uses_emissive_strength = false;
    for m in materials {
        let mut pbr = json!({
            "baseColorFactor": m.base_color_factor,
            "metallicFactor": m.metallic_factor,
            "roughnessFactor": m.roughness_factor,
        });
        let mut mat = json!({"name": m.name});
        for (slot, file) in &m.images {
            let tex = json!({"index": image_index(file)});
            match slot {
                Slot::BaseColor => pbr["baseColorTexture"] = tex,
                Slot::MetallicRoughness => pbr["metallicRoughnessTexture"] = tex,
                Slot::Normal => mat["normalTexture"] = tex,
                Slot::Emissive => {
                    mat["emissiveTexture"] = tex;
                    mat["emissiveFactor"] = json!(m.emissive_factor);
                }
            }
        }
        mat["pbrMetallicRoughness"] = pbr;
        if m.emissive_strength != 1.0 && mat.get("emissiveTexture").is_some() {
            mat["extensions"] = json!({"KHR_materials_emissive_strength": {"emissiveStrength": m.emissive_strength}});
            uses_emissive_strength = true;
        }
        gl_materials.push(mat);
    }
    let textures: Vec<Value> = (0..images.len())
        .map(|i| json!({"source": i, "sampler": 0}))
        .collect();

    // Skeleton: bones become nodes 1..
    let mut nodes = vec![json!({"name": name, "mesh": 0})];
    let scene_nodes = [0];
    let mut skins = Vec::new();
    if let Some(sk) = &scene.skin {
        nodes[0]["skin"] = json!(0);
        for (i, b) in sk.bones.iter().enumerate() {
            let mut node =
                json!({"name": b.name, "translation": b.translation, "rotation": b.rotation});
            if b.scale.iter().any(|s| (s - 1.0).abs() > 1e-4) {
                node["scale"] = json!(b.scale);
            }
            let children: Vec<usize> = sk
                .bones
                .iter()
                .enumerate()
                .filter(|(_, c)| c.parent == Some(i))
                .map(|(c, _)| c + 1)
                .collect();
            if !children.is_empty() {
                node["children"] = json!(children);
            }
            nodes.push(node);
        }
        // Bones without a parent hang from the model's node, as in the bridge's model, so that
        // every joint has the one common root glTF requires of a skin, however many roots the
        // skeleton has (attachments that ride another skeleton get root bones of their own)
        let roots: Vec<usize> = sk
            .bones
            .iter()
            .enumerate()
            .filter(|(_, b)| b.parent.is_none())
            .map(|(i, _)| i + 1)
            .collect();
        if !roots.is_empty() {
            nodes[0]["children"] = json!(roots);
        }
        // Column-major, as glTF stores matrices
        let ibm: Vec<u8> = sk
            .inverse_binds
            .iter()
            .flat_map(|m| (0..16).map(move |k| m[k % 4][k / 4]))
            .flat_map(f32::to_le_bytes)
            .collect();
        let view = push_view(&mut bin, &ibm, None);
        accessors.push(json!({"bufferView": view, "componentType": 5126, "count": sk.inverse_binds.len(), "type": "MAT4"}));
        let mut skin = json!({
            "joints": sk.joints.iter().map(|j| j + 1).collect::<Vec<_>>(),
            "inverseBindMatrices": accessors.len() - 1,
        });
        skin["skeleton"] = json!(0);
        skins.push(skin);
    }

    // Animations: bone i is node i + 1. Channels that share key times share one input accessor.
    let mut animations = Vec::new();
    for clip in clips {
        let mut inputs: Vec<(Vec<u32>, usize)> = Vec::new();
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        for ch in &clip.channels {
            // A channel without keys would be an accessor of no elements, which glTF forbids
            if ch.times.is_empty() {
                continue;
            }
            let key = ch.times.iter().map(|t| t.to_bits()).collect::<Vec<u32>>();
            let input = match inputs.iter().find(|(k, _)| *k == key) {
                Some((_, a)) => *a,
                None => {
                    let view = push_view(&mut bin, &floats(&mut ch.times.iter().copied()), None);
                    let lo = ch.times.iter().copied().fold(f32::MAX, f32::min);
                    let hi = ch.times.iter().copied().fold(f32::MIN, f32::max);
                    accessors.push(json!({"bufferView": view, "componentType": 5126, "count": ch.times.len(), "type": "SCALAR", "min": [lo], "max": [hi]}));
                    inputs.push((key, accessors.len() - 1));
                    accessors.len() - 1
                }
            };
            let (path, kind) = match ch.target {
                Target::Translation => ("translation", "VEC3"),
                Target::Rotation => ("rotation", "VEC4"),
                Target::Scale => ("scale", "VEC3"),
            };
            let view = push_view(&mut bin, &floats(&mut ch.values.iter().copied()), None);
            accessors.push(json!({"bufferView": view, "componentType": 5126, "count": ch.times.len(), "type": kind}));
            samplers.push(json!({
                "input": input,
                "output": accessors.len() - 1,
                "interpolation": match ch.interpolation {
                    Interpolation::Linear => "LINEAR",
                    Interpolation::Step => "STEP",
                },
            }));
            channels.push(json!({
                "sampler": samplers.len() - 1,
                "target": {"node": ch.node + 1, "path": path},
            }));
        }
        let mut animation = json!({"name": clip.name, "samplers": samplers, "channels": channels});
        if let Some(extras) = &clip.extras {
            animation["extras"] = extras.clone();
        }
        animations.push(animation);
    }

    let bin_name = format!("{name}.bin");
    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "wf-unpack"},
        "scene": 0,
        "scenes": [{"nodes": scene_nodes}],
        "nodes": nodes,
        "meshes": [{"name": name, "primitives": primitives}],
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"uri": bin_name, "byteLength": bin.len()}],
    });
    if !gl_materials.is_empty() {
        doc["materials"] = json!(gl_materials);
        if uses_emissive_strength {
            doc["extensionsUsed"] = json!(["KHR_materials_emissive_strength"]);
        }
    }
    if !skins.is_empty() {
        doc["skins"] = json!(skins);
    }
    if !animations.is_empty() {
        doc["animations"] = json!(animations);
    }
    if !images.is_empty() {
        doc["images"] = json!(images);
        doc["textures"] = json!(textures);
        doc["samplers"] = json!([{"wrapS": 10497, "wrapT": 10497}]);
    }
    fs::write(out_dir.join(&bin_name), &bin)?;
    fs::write(
        out_dir.join(format!("{name}.gltf")),
        serde_json::to_vec_pretty(&doc)?,
    )?;
    Ok(())
}

/// A model's vertex data in a [`LevelWriter`]: shared by every mesh and node that uses it.
pub struct ModelHandle {
    attributes: Value,
    /// (material slot, index accessor) per primitive.
    primitives: Vec<(usize, usize)>,
}

/// Writes a whole level as one glTF: each model's vertices are stored once,
/// meshes reuse them (one per distinct material set), and nodes place meshes.
#[derive(Default)]
pub struct LevelWriter {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    meshes: Vec<Value>,
    materials: Vec<Value>,
    images: Vec<Value>,
    nodes: Vec<Value>,
    scene_nodes: Vec<usize>,
    lights: Vec<Value>,
    scene_extras: Option<Value>,
    uses_emissive_strength: bool,
}

impl LevelWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Size of the binary buffer so far.
    pub fn buffer_len(&self) -> usize {
        self.bin.len()
    }

    fn push_view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut view =
            json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            view["target"] = json!(t);
        }
        self.views.push(view);
        self.bin.extend_from_slice(bytes);
        self.views.len() - 1
    }

    fn push_accessor(&mut self, view: usize, ty: &str, component: u32, count: usize) -> usize {
        self.accessors.push(
            json!({"bufferView": view, "componentType": component, "count": count, "type": ty}),
        );
        self.accessors.len() - 1
    }

    fn image_index(&mut self, file: &str) -> usize {
        if let Some(i) = self.images.iter().position(|v| v["uri"] == file) {
            return i;
        }
        self.images.push(json!({ "uri": file }));
        self.images.len() - 1
    }

    /// Adds a material; returns its index.
    pub fn add_material(&mut self, m: &Material) -> usize {
        let mut pbr = json!({
            "baseColorFactor": m.base_color_factor,
            "metallicFactor": m.metallic_factor,
            "roughnessFactor": m.roughness_factor,
        });
        let mut mat = json!({"name": m.name});
        for (slot, file) in &m.images {
            let tex = json!({"index": self.image_index(file)});
            match slot {
                Slot::BaseColor => pbr["baseColorTexture"] = tex,
                Slot::MetallicRoughness => pbr["metallicRoughnessTexture"] = tex,
                Slot::Normal => mat["normalTexture"] = tex,
                Slot::Emissive => {
                    mat["emissiveTexture"] = tex;
                    mat["emissiveFactor"] = json!(m.emissive_factor);
                }
            }
        }
        mat["pbrMetallicRoughness"] = pbr;
        if m.emissive_strength != 1.0 && mat.get("emissiveTexture").is_some() {
            mat["extensions"] = json!({"KHR_materials_emissive_strength": {"emissiveStrength": m.emissive_strength}});
            self.uses_emissive_strength = true;
        }
        self.materials.push(mat);
        self.materials.len() - 1
    }

    /// Stores the vertices and indices of a static model.
    pub fn add_model(&mut self, scene: &Scene) -> ModelHandle {
        let n = scene.positions.len();
        let (lo, hi) = scene.bounds;
        let floats = |vals: &mut dyn Iterator<Item = f32>| {
            vals.flat_map(f32::to_le_bytes).collect::<Vec<u8>>()
        };
        const VERTICES: Option<u32> = Some(34962);
        let mut attributes = json!({});
        let sets: [(&str, &str, Vec<u8>); 5] = [
            (
                "POSITION",
                "VEC3",
                floats(&mut scene.positions.iter().flatten().copied()),
            ),
            (
                "NORMAL",
                "VEC3",
                floats(&mut scene.normals.iter().flatten().copied()),
            ),
            (
                "TEXCOORD_0",
                "VEC2",
                floats(&mut scene.uv0.iter().flatten().copied()),
            ),
            (
                "TEXCOORD_1",
                "VEC2",
                floats(&mut scene.uv1.iter().flatten().copied()),
            ),
            (
                "TANGENT",
                "VEC4",
                floats(&mut scene.tangents.iter().flatten().copied()),
            ),
        ];
        for (name, ty, bytes) in sets {
            let view = self.push_view(&bytes, VERTICES);
            let acc = self.push_accessor(view, ty, 5126, n);
            if name == "POSITION" {
                self.accessors[acc]["min"] = json!(lo);
                self.accessors[acc]["max"] = json!(hi);
            }
            attributes[name] = json!(acc);
        }
        let primitives = scene
            .primitives
            .iter()
            .map(|p| {
                let bytes: Vec<u8> = p.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
                let view = self.push_view(&bytes, Some(34963));
                (
                    p.mesh,
                    self.push_accessor(view, "SCALAR", 5125, p.indices.len()),
                )
            })
            .collect();
        ModelHandle {
            attributes,
            primitives,
        }
    }

    /// A mesh of positions and triangles only (collision shapes), stored with
    /// its own vertices; returns the mesh index.
    pub fn add_simple_mesh(
        &mut self,
        name: &str,
        positions: &[[f32; 3]],
        indices: &[u32],
    ) -> usize {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let bytes: Vec<u8> = positions
            .iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let view = self.push_view(&bytes, Some(34962));
        let pos = self.push_accessor(view, "VEC3", 5126, positions.len());
        self.accessors[pos]["min"] = json!(lo);
        self.accessors[pos]["max"] = json!(hi);
        let bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let view = self.push_view(&bytes, Some(34963));
        let idx = self.push_accessor(view, "SCALAR", 5125, indices.len());
        self.meshes.push(json!({"name": name, "primitives": [{"attributes": {"POSITION": pos}, "indices": idx}]}));
        self.meshes.len() - 1
    }

    /// A mesh over a stored model; `materials[slot]` is the glTF material of each mesh slot.
    pub fn add_mesh(
        &mut self,
        name: &str,
        model: &ModelHandle,
        materials: &[Option<usize>],
    ) -> usize {
        let primitives: Vec<Value> = model
            .primitives
            .iter()
            .map(|&(slot, indices)| {
                let mut prim = json!({"attributes": model.attributes, "indices": indices});
                if let Some(Some(m)) = materials.get(slot) {
                    prim["material"] = json!(m);
                }
                prim
            })
            .collect();
        self.meshes
            .push(json!({"name": name, "primitives": primitives}));
        self.meshes.len() - 1
    }

    /// Adds a node to the scene; `node` holds its name, transform and extras.
    pub fn add_node(&mut self, node: Value) -> usize {
        self.nodes.push(node);
        let i = self.nodes.len() - 1;
        self.scene_nodes.push(i);
        i
    }

    /// Data stored in the scene's `extras` (level-wide settings).
    pub fn set_scene_extras(&mut self, extras: Value) {
        self.scene_extras = Some(extras);
    }

    /// Adds a KHR_lights_punctual light definition; returns its index.
    pub fn add_light(&mut self, light: Value) -> usize {
        self.lights.push(light);
        self.lights.len() - 1
    }

    /// Writes `<name>.gltf` and `<name>.bin` into `out_dir`.
    pub fn finish(self, name: &str, out_dir: &Path) -> Result<()> {
        fs::create_dir_all(out_dir)?;
        let bin_name = format!("{name}.bin");
        let mut doc = json!({
            "asset": {"version": "2.0", "generator": "wf-unpack"},
            "scene": 0,
            "scenes": [{"name": name, "nodes": self.scene_nodes}],
            "nodes": self.nodes,
        });
        if let Some(extras) = &self.scene_extras {
            doc["scenes"][0]["extras"] = extras.clone();
        }
        if !self.meshes.is_empty() {
            doc["meshes"] = json!(self.meshes);
            doc["accessors"] = json!(self.accessors);
            doc["bufferViews"] = json!(self.views);
            doc["buffers"] = json!([{"uri": bin_name, "byteLength": self.bin.len()}]);
        }
        if !self.materials.is_empty() {
            doc["materials"] = json!(self.materials);
        }
        if !self.images.is_empty() {
            let textures: Vec<Value> = (0..self.images.len())
                .map(|i| json!({"source": i, "sampler": 0}))
                .collect();
            doc["images"] = json!(self.images);
            doc["textures"] = json!(textures);
            doc["samplers"] = json!([{"wrapS": 10497, "wrapT": 10497}]);
        }
        let mut used: Vec<&str> = Vec::new();
        if !self.lights.is_empty() {
            used.push("KHR_lights_punctual");
            doc["extensions"] = json!({"KHR_lights_punctual": {"lights": self.lights}});
        }
        if self.uses_emissive_strength {
            used.push("KHR_materials_emissive_strength");
        }
        if !used.is_empty() {
            doc["extensionsUsed"] = json!(used);
        }
        fs::write(out_dir.join(&bin_name), &self.bin)?;
        fs::write(
            out_dir.join(format!("{name}.gltf")),
            serde_json::to_vec_pretty(&doc)?,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Primitive, SceneBone, SceneSkin};

    fn bone(name: &str, parent: Option<usize>) -> SceneBone {
        SceneBone {
            name: name.into(),
            parent,
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }

    /// One triangle skinned to three bones: A (root) with child B, and a second root C.
    fn scene(with_triangle: bool) -> Scene {
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        Scene {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            tangents: vec![[1.0, 0.0, 0.0, 1.0]; 3],
            uv0: vec![[0.0; 2]; 3],
            uv1: vec![[0.0; 2]; 3],
            joints: vec![[0, 1, 2, 0]; 3],
            weights: vec![[0.5, 0.25, 0.25, 0.0]; 3],
            primitives: if with_triangle {
                vec![Primitive {
                    mesh: 0,
                    indices: vec![0, 1, 2],
                }]
            } else {
                Vec::new()
            },
            skin: Some(SceneSkin {
                bones: vec![bone("A", None), bone("B", Some(0)), bone("C", None)],
                joints: vec![0, 1, 2],
                inverse_binds: vec![identity; 3],
            }),
            bounds: ([0.0; 3], [1.0, 1.0, 0.0]),
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wf-unpack-gltf-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_skeleton_with_two_roots_has_one_common_root() {
        let dir = temp_dir("roots");
        write_animated(&scene(true), "m", &[], &[], &dir).unwrap();
        let doc: Value = serde_json::from_slice(&fs::read(dir.join("m.gltf")).unwrap()).unwrap();
        let _ = fs::remove_dir_all(&dir);
        // Bones are nodes 1..=3; both roots (A = 1, C = 3) hang from the model's node 0
        assert_eq!(doc["nodes"][0]["children"], json!([1, 3]));
        assert_eq!(doc["nodes"][1]["children"], json!([2]));
        assert_eq!(doc["scenes"][0]["nodes"], json!([0]));
        assert_eq!(doc["skins"][0]["skeleton"], json!(0));
        assert_eq!(doc["meshes"][0]["primitives"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_scene_without_triangles_is_not_written() {
        let dir = temp_dir("empty");
        let err = write_animated(&scene(false), "m", &[], &[], &dir).unwrap_err();
        assert!(err.to_string().contains("no triangles"), "{err:#}");
        assert!(!dir.join("m.gltf").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
