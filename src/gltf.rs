//! Minimal glTF 2.0 writer for a prepared [`Scene`]: one primitive per mesh
//! at the chosen LOD, sharing one set of vertex attributes; images are files
//! next to the `.gltf`.

use std::fs;
use std::path::Path;

use anyhow::Result;
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
        gl_materials.push(mat);
    }
    let textures: Vec<Value> = (0..images.len())
        .map(|i| json!({"source": i, "sampler": 0}))
        .collect();

    // Skeleton: bones become nodes 1..
    let mut nodes = vec![json!({"name": name, "mesh": 0})];
    let mut scene_nodes = vec![0];
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
            if b.parent.is_none() {
                scene_nodes.push(i + 1);
            }
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
        if let Some(root) = sk.bones.iter().position(|b| b.parent.is_none()) {
            skin["skeleton"] = json!(root + 1);
        }
        skins.push(skin);
    }

    // Animations: bone i is node i + 1. Channels that share key times share one input accessor.
    let mut animations = Vec::new();
    for clip in clips {
        let mut inputs: Vec<(Vec<u32>, usize)> = Vec::new();
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        for ch in &clip.channels {
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
