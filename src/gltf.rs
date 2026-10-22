//! Minimal glTF 2.0 writer for a [`Model`]: one primitive per mesh at the
//! chosen LOD, sharing one set of vertex attributes; images are files next
//! to the `.gltf`.

use std::fs;
use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use crate::model::Model;

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

pub struct Options {
    pub lod: usize,
    /// Negate z, turning the game's left-handed space into glTF's right-handed one.
    pub mirror_z: bool,
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-6 {
        [0.0, 1.0, 0.0]
    } else {
        v.map(|c| c / len)
    }
}

pub fn write(
    model: &Model,
    name: &str,
    materials: &[Material],
    opts: &Options,
    out_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(out_dir)?;
    let z = if opts.mirror_z { -1.0 } else { 1.0 };

    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut push_view = |bin: &mut Vec<u8>, bytes: &[u8], target: u32| -> usize {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        views.push(json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len(), "target": target}));
        bin.extend_from_slice(bytes);
        views.len() - 1
    };
    let floats =
        |vals: &mut dyn Iterator<Item = f32>| vals.flat_map(f32::to_le_bytes).collect::<Vec<u8>>();

    let positions: Vec<[f32; 3]> = model
        .vertices
        .iter()
        .map(|v| [v.position[0], v.position[1], v.position[2] * z])
        .collect();
    let normals: Vec<[f32; 3]> = model
        .vertices
        .iter()
        .map(|v| normalize([v.normal[0], v.normal[1], v.normal[2] * z]))
        .collect();
    // glTF's bitangent is cross(normal, tangent) * w and must point to the top
    // of the texture (decreasing v). As stored, cross(normal, tangent) * w
    // points the other way on every triangle checked, and mirroring flips it
    // once more; hence the sign.
    let tangents: Vec<[f32; 4]> = model
        .vertices
        .iter()
        .map(|v| {
            let [x, y, w] = normalize([v.tangent[0], v.tangent[1], v.tangent[2] * z]);
            [x, y, w, -v.tangent[3]]
        })
        .collect();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let n = positions.len();
    let pos_view = push_view(
        &mut bin,
        &floats(&mut positions.iter().flatten().copied()),
        34962,
    );
    let nrm_view = push_view(
        &mut bin,
        &floats(&mut normals.iter().flatten().copied()),
        34962,
    );
    let tan_view = push_view(
        &mut bin,
        &floats(&mut tangents.iter().flatten().copied()),
        34962,
    );
    let uv0_view = push_view(
        &mut bin,
        &floats(&mut model.vertices.iter().flat_map(|v| v.uv0)),
        34962,
    );
    let uv1_view = push_view(
        &mut bin,
        &floats(&mut model.vertices.iter().flat_map(|v| v.uv1)),
        34962,
    );
    let mut accessors = vec![
        json!({"bufferView": pos_view, "componentType": 5126, "count": n, "type": "VEC3", "min": lo, "max": hi}),
        json!({"bufferView": nrm_view, "componentType": 5126, "count": n, "type": "VEC3"}),
        json!({"bufferView": uv0_view, "componentType": 5126, "count": n, "type": "VEC2"}),
        json!({"bufferView": uv1_view, "componentType": 5126, "count": n, "type": "VEC2"}),
        json!({"bufferView": tan_view, "componentType": 5126, "count": n, "type": "VEC4"}),
    ];

    let mut primitives = Vec::new();
    for mesh in 0..model.header.meshes.len() {
        let tris = model.lod_indices(mesh, opts.lod)?;
        if tris.is_empty() {
            continue;
        }
        let mut idx: Vec<u32> = tris.to_vec();
        if opts.mirror_z {
            // Mirroring flips the winding; swap two corners to keep faces pointing out
            for t in idx.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
        }
        let bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
        let view = push_view(&mut bin, &bytes, 34963);
        accessors.push(json!({"bufferView": view, "componentType": 5125, "count": idx.len(), "type": "SCALAR"}));
        let mut prim = json!({
            "attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2, "TEXCOORD_1": 3, "TANGENT": 4},
            "indices": accessors.len() - 1,
        });
        if mesh < materials.len() {
            prim["material"] = json!(mesh);
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

    let bin_name = format!("{name}.bin");
    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "wf-unpack"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": name, "mesh": 0}],
        "meshes": [{"name": name, "primitives": primitives}],
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"uri": bin_name, "byteLength": bin.len()}],
    });
    if !gl_materials.is_empty() {
        doc["materials"] = json!(gl_materials);
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
