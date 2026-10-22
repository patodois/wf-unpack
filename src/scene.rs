//! A [`Model`] converted to a right-handed, y-up space with flat vertex
//! arrays, ready for glTF or a renderer. Both the glTF writer and the viewer
//! use it, so they agree on the handedness conversion.
//!
//! The game's space is left-handed; negating x makes it right-handed and
//! keeps characters facing +z (glTF's front). Mirroring also flips triangle
//! winding (two corners of each triangle are swapped), bone translations
//! (x), bone rotations (the quaternion's y and z) and the inverse bind
//! matrices (S * M * S with S = diag(-1, 1, 1)).

use anyhow::Result;

use crate::model::Model;

pub struct Primitive {
    /// Index of the model mesh (and of its material).
    pub mesh: usize,
    pub indices: Vec<u32>,
}

pub struct SceneBone {
    pub name: String,
    pub parent: Option<usize>,
    pub translation: [f32; 3],
    /// Quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

pub struct SceneSkin {
    pub bones: Vec<SceneBone>,
    /// Bone index of each joint; vertex joint indices point into this list.
    pub joints: Vec<usize>,
    /// Inverse bind matrix per joint, row-major.
    pub inverse_binds: Vec<[[f32; 4]; 4]>,
}

pub struct Scene {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// xyz and the bitangent sign, as glTF's TANGENT expects.
    pub tangents: Vec<[f32; 4]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    /// Joints and weights, empty for static models.
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub primitives: Vec<Primitive>,
    pub skin: Option<SceneSkin>,
    pub bounds: ([f32; 3], [f32; 3]),
}

impl Scene {
    pub fn triangles(&self) -> usize {
        self.primitives.iter().map(|p| p.indices.len() / 3).sum()
    }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-6 {
        [0.0, 1.0, 0.0]
    } else {
        v.map(|c| c / len)
    }
}

/// Converts `model` at `lod`; `mirror_x` turns the game's left-handed space
/// into a right-handed one (always wanted except for debugging).
pub fn prepare(model: &Model, lod: usize, mirror_x: bool) -> Result<Scene> {
    let mx = if mirror_x { -1.0 } else { 1.0 };
    let v = &model.vertices;
    let positions: Vec<[f32; 3]> = v
        .iter()
        .map(|v| [v.position[0] * mx, v.position[1], v.position[2]])
        .collect();
    let normals = v
        .iter()
        .map(|v| normalize([v.normal[0] * mx, v.normal[1], v.normal[2]]))
        .collect();
    // glTF's bitangent is cross(normal, tangent) * w and must point to the top
    // of the texture (decreasing v). As stored, cross(normal, tangent) * w
    // points the other way; the mirror reverses the cross product, which puts
    // it right. Checked against the UV derivatives of every triangle.
    let tangents = v
        .iter()
        .map(|v| {
            let [x, y, z] = normalize([v.tangent[0] * mx, v.tangent[1], v.tangent[2]]);
            [
                x,
                y,
                z,
                if mirror_x {
                    v.tangent[3]
                } else {
                    -v.tangent[3]
                },
            ]
        })
        .collect();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }

    let mut primitives = Vec::new();
    for mesh in 0..model.header.meshes.len() {
        let mut indices = model.lod_indices(mesh, lod)?.to_vec();
        if indices.is_empty() {
            continue;
        }
        if mirror_x {
            // Mirroring flips the winding; swap two corners to keep faces pointing out
            for t in indices.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
        }
        primitives.push(Primitive { mesh, indices });
    }

    let skin = model.skeleton.as_ref().map(|sk| {
        let locals = sk.bind_pose_locals();
        let bones = sk
            .bones
            .iter()
            .zip(locals)
            .map(|(b, (t, [x, y, z, w], scale))| SceneBone {
                name: b.name.clone(),
                parent: b.parent,
                translation: [t[0] * mx, t[1], t[2]],
                // Reflecting x maps the quaternion (x, y, z, w) to (x, -y, -z, w)
                rotation: [x, y * mx, z * mx, w],
                scale,
            })
            .collect();
        let inverse_binds = sk
            .inverse_binds
            .iter()
            .map(|m| {
                let sign = |r: usize, c: usize| if (r == 0) != (c == 0) { mx } else { 1.0 };
                std::array::from_fn(|r| {
                    std::array::from_fn(|c| {
                        if r == 3 {
                            if c == 3 {
                                1.0
                            } else {
                                0.0
                            }
                        } else {
                            m[r * 4 + c] * sign(r, c)
                        }
                    })
                })
            })
            .collect();
        SceneSkin {
            bones,
            joints: sk.joints.clone(),
            inverse_binds,
        }
    });
    let skinned = skin.is_some();

    Ok(Scene {
        positions,
        normals,
        tangents,
        uv0: v.iter().map(|v| v.uv0).collect(),
        uv1: v.iter().map(|v| v.uv1).collect(),
        joints: if skinned {
            v.iter().map(|v| v.joints).collect()
        } else {
            Vec::new()
        },
        weights: if skinned {
            v.iter().map(|v| v.weights).collect()
        } else {
            Vec::new()
        },
        primitives,
        skin,
        bounds: (lo, hi),
    })
}
