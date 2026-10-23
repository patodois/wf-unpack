//! Static models (`.fbx` paths of type 102), split across `H.Misc`, `B.Misc`
//! and `F.Misc`.
//!
//! The `H.` entry is a [`CommonHeader`] followed by (little-endian):
//!
//! ```text
//! [0x30]  unknown
//! physics reference:
//!   u32 a, u32 b, [2 bytes if a or b is 8], u32 len, [len] path
//! [0x28]  unknown
//! f32[4] bounds_min, f32[4] bounds_max   object-space bounding box (w unused)
//! u32 vertex_count
//! u32 index_count
//! u32 morph_count
//! u32 bone_count
//! [0x22]  unknown
//! u32 body_vertex_count    vertices stored in B. (the rest are in F.)
//! u32 body_index_count     indices stored in B.
//! [0x3D]  unknown
//! u32 mesh_count, then per mesh (one per material):
//!   f32[4] min, f32[4] max
//!   u32 len, [len] name
//!   u32 lod_offset[5], u32 lod_count[5]   index ranges, LOD 0 (finest) to 4
//!   [0x20], u32 len, [len] unknown name, [0x10]
//! u32 n, u16[n]
//! physics reference (as above)
//! u32 physx_count, then per PhysX shape:
//!   u32 type, [0x4C if type is 1, 0x60 if 7, else 0x50],
//!   f32[4], f32[4], [4 bytes unless type is 0, 2 or 3], [4],
//!   u32 data_len, [8]
//! u32 message_count, then u32 len + text each (build warnings)
//! ```
//!
//! The material names are in the common header's attribute text, as
//! `Materials={\n    Name,\n    Name\n}`, one per mesh; names without a
//! leading `/` are relative to the model's folder.
//!
//! `B.` holds the PhysX shapes (`data_len` bytes each), a u16, then
//! `body_vertex_count` vertices and `body_index_count` u16 indices. `F.`
//! holds the remaining vertices and then the remaining indices. B. carries
//! the coarsest LOD so the game can draw something before F. streams in.
//! Each vertex is 24 bytes:
//!
//! ```text
//! i16[4]  position   x, y, z as snorm (/ 32767), times the scale;
//!                    w is +255 or -255, the sign of the bitangent
//! u8[3]   normal     octahedral, see below
//! u8      unknown    0 or 255
//! u8[3]   tangent    octahedral
//! u8      ambient occlusion (probably)
//! f16[2]  uv0
//! f16[2]  uv1
//! ```
//!
//! The scale per axis is the largest absolute value of the bounding box on
//! that axis, so the quantised positions use the full i16 range.
//!
//! Normals and tangents are octahedral unit vectors in 24 bits: read the
//! three bytes as a big-endian number, the top 12 bits are u and the low 12
//! bits v, each mapped from 0..4095 to -1..1. Then x = u, y = v,
//! z = 1 - |u| - |v|, and when z < 0 the lower hemisphere is unfolded:
//! x = (1 - |v|) * sign(u), y = (1 - |u|) * sign(v). Against normals
//! rebuilt from the triangles of a sword, the decoded normals average a
//! cosine of 0.998, and decoded tangents are perpendicular to them.
//!
//! Coordinates are left-handed: exported as they are, a sign's lettering
//! comes out mirrored relative to its texture. [`crate::gltf`] negates x
//! (and swaps two corners of every triangle) to get glTF's right-handed
//! space; y is up in both, and characters then face +z as glTF expects.
//!
//! # Level meshes (types 105 and 109)
//!
//! The geometry of levels uses the same container. The header differs in
//! three places: the physics reference tests its two words as bit masks (`& 8`)
//! instead of comparing them with 8, a list of material paths follows the
//! mesh list (absolute `/EE/...` or `/Lotus/...` paths, one per mesh), and
//! there is no skeleton.
//!
//! - Type 105 (`N_a.fbx`) has the vertex layout of type 102, in `B.` and `F.`.
//! - Type 109 (`N_c.fbx`, the geometry of a level's CSG zones) keeps
//!   everything in `B.` and has 32-byte vertices with float positions:
//!
//! ```text
//! f32[3]  position   in level units; the fourth float is 1.0
//! u8[3]   normal     octahedral, then an unused 0
//! u8[3]   tangent    octahedral, then 0 (no bitangent sign is stored)
//! f16[2]  uv0
//! f16[2]  uv1        the lightmap coordinates
//! ```
//!
//! After the PhysX data come zero or more 0x120-byte blocks of unknown
//! meaning (this reader skips as many as the part size implies), then two
//! bytes, the vertices and the u16 indices. Computed from the UV
//! derivatives of all 9,005 vertices of one level mesh, `cross(normal,
//! tangent)` always points towards increasing v, which is the `+1` sign of
//! type 102.
//!
//! The layout follows Warframe-Exporter's reader for type 102; the checks in
//! [`Model::from_parts`] (header consumed exactly, part sizes, decoded
//! bounds equal to the header's) hold for every model `check-models` reads.

use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

use crate::bytes::{f16_to_f32, Reader};
use crate::cache::Package;
use crate::header::CommonHeader;

pub const KIND_STATIC: u16 = 102;
pub const KIND_SKINNED: u16 = 281;
/// Level geometry with the packed vertices of type 102 (`N_a.fbx`).
pub const KIND_LEVEL_PACKED: u16 = 105;
/// Level geometry with float positions, all in `B.` (`N_c.fbx`, CSG zones).
pub const KIND_LEVEL_FLOAT: u16 = 109;
pub const VERTEX_SIZE: usize = 24;
/// Type 109 vertices have float positions.
pub const FLOAT_VERTEX_SIZE: usize = 32;
/// Unknown blocks between the PhysX data and the vertices of some type 109 models.
pub const EXTRA_BLOCK: usize = 0x120;
/// Skinned vertices add four bone indices and packed weights.
pub const SKINNED_VERTEX_SIZE: usize = 32;
pub const LOD_COUNT: usize = 5;

#[derive(Debug, Clone)]
pub struct MeshInfo {
    pub name: String,
    pub min: [f32; 4],
    pub max: [f32; 4],
    /// Index ranges per LOD, finest first.
    pub lod_offsets: [u32; LOD_COUNT],
    pub lod_counts: [u32; LOD_COUNT],
}

/// One entry of a skinned model's bone tree.
#[derive(Debug, Clone, PartialEq)]
pub struct BoneNode {
    pub name: String,
    /// Index of the parent; the root points at itself.
    pub parent: u16,
    pub unknown: u16,
}

/// The skeleton part of a type-281 header.
#[derive(Debug, Clone, Default)]
pub struct SkeletonHeader {
    /// Bones that carry weights, in the order of the inverse bind matrices.
    pub weighted: Vec<String>,
    pub tree: Vec<BoneNode>,
    /// Per mesh: local bone index (as stored in vertices) to weighted bone index.
    pub bone_maps: Vec<Vec<u32>>,
    /// Bytes between the inverse bind matrices and the rest poses in B.
    pub rest_skip: u32,
    /// Morph data in B. before the indices, in units of 8 bytes.
    pub morph_skip: u32,
}

#[derive(Debug, Clone)]
pub struct ModelHeader {
    pub common: CommonHeader,
    pub bounds_min: [f32; 4],
    pub bounds_max: [f32; 4],
    pub vertex_count: u32,
    pub index_count: u32,
    pub morph_count: u32,
    pub bone_count: u32,
    pub body_vertex_count: u32,
    pub body_index_count: u32,
    pub meshes: Vec<MeshInfo>,
    /// Sizes of the PhysX shapes at the start of the B. part.
    pub physx_lens: Vec<u32>,
    pub messages: Vec<String>,
    /// Present for skinned models (type 281).
    pub skeleton: Option<SkeletonHeader>,
    /// Level meshes (105, 109): one material path per mesh.
    pub material_paths: Vec<String>,
}

fn read_meshes(r: &mut Reader) -> Result<Vec<MeshInfo>> {
    let mesh_count = r.u32()?;
    ensure!(mesh_count <= 64, "implausible mesh count {mesh_count}");
    let mut meshes = Vec::with_capacity(mesh_count as usize);
    for _ in 0..mesh_count {
        let min = r.f32x4()?;
        let max = r.f32x4()?;
        let name = r.str32(1024).context("mesh name")?;
        let mut lod_offsets = [0; LOD_COUNT];
        let mut lod_counts = [0; LOD_COUNT];
        for v in &mut lod_offsets {
            *v = r.u32()?;
        }
        for v in &mut lod_counts {
            *v = r.u32()?;
        }
        r.skip(0x20)?;
        r.str32(1024).context("mesh second name")?;
        r.skip(0x10)?;
        meshes.push(MeshInfo {
            name,
            min,
            max,
            lod_offsets,
            lod_counts,
        });
    }
    let n = r.u32()?;
    ensure!(n <= 64, "implausible u16 array length {n}");
    r.skip(2 * n as usize)?;
    Ok(meshes)
}

/// Skips the per-mesh morph structures and returns the size of the morph data in B. (8-byte units).
fn skip_morph_structs(r: &mut Reader, meshes: usize) -> Result<u32> {
    let count = r.u32()? as usize;
    ensure!(
        count == 0 || count == meshes,
        "{count} morph structures for {meshes} meshes"
    );
    let mut total = 0u32;
    for _ in 0..count {
        ensure!(r.u32()? == 5, "unexpected morph structure");
        let mult = r.u32()?;
        r.skip(16)?;
        ensure!(r.u32()? == 5, "unexpected morph structure");
        r.skip(20)?;
        ensure!(r.u32()? == 5, "unexpected morph structure");
        r.skip(20)?;
        ensure!(r.u32()? == 5, "unexpected morph structure");
        let base = r.u32()?;
        r.skip(16)?;
        total = total.saturating_add(base.saturating_mul(mult.max(1)));
        let names = r.u32()?;
        ensure!(names <= 64, "implausible morph name count {names}");
        for _ in 0..names {
            r.str32(1024)?;
        }
        let n = r.u32()?;
        ensure!(n <= 64, "implausible morph array {n}");
        r.skip(4 * n as usize)?;
    }
    Ok(total)
}

fn skip_physics_ref(r: &mut Reader, bit_mask: bool) -> Result<()> {
    let a = r.u32()?;
    let b = r.u32()?;
    let flagged = |v: u32| if bit_mask { v & 8 != 0 } else { v == 8 };
    if flagged(a) || flagged(b) {
        r.skip(2)?;
    }
    r.str32(1024).context("physics reference")?;
    Ok(())
}

fn read_material_paths(r: &mut Reader) -> Result<Vec<String>> {
    let n = r.u32()?;
    ensure!(n <= 100, "implausible material count {n}");
    (0..n).map(|_| r.str32(1024)).collect()
}

impl ModelHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let common = CommonHeader::parse(data).context("common header")?;
        ensure!(
            matches!(
                common.kind,
                KIND_STATIC | KIND_SKINNED | KIND_LEVEL_PACKED | KIND_LEVEL_FLOAT
            ),
            "model type {} is not supported (only {KIND_STATIC}, {KIND_SKINNED}, {KIND_LEVEL_PACKED} and {KIND_LEVEL_FLOAT})",
            common.kind
        );
        let level_mesh = matches!(common.kind, KIND_LEVEL_PACKED | KIND_LEVEL_FLOAT);
        let mut r = Reader::at(data, common.len)?;
        r.skip(0x30)?;
        skip_physics_ref(&mut r, level_mesh)?;
        let (
            vertex_count,
            index_count,
            morph_count,
            bone_count,
            body_vertex_count,
            body_index_count,
        );
        let (bounds_min, bounds_max, meshes, skeleton);
        let mut material_paths = Vec::new();
        if common.kind == KIND_LEVEL_FLOAT {
            r.skip(0x28)?;
            bounds_min = r.f32x4()?;
            bounds_max = r.f32x4()?;
            vertex_count = r.u32()?;
            index_count = r.u32()?;
            morph_count = r.u32()?;
            bone_count = r.u32()?;
            ensure!(
                morph_count == 0 && bone_count == 0,
                "level mesh with {morph_count} morphs and {bone_count} bones"
            );
            r.skip(0x67)?;
            // Everything is in B.
            (body_vertex_count, body_index_count) = (vertex_count, index_count);
            meshes = read_meshes(&mut r)?;
            material_paths = read_material_paths(&mut r)?;
            skeleton = None;
        } else if common.kind == KIND_STATIC || common.kind == KIND_LEVEL_PACKED {
            r.skip(0x28)?;
            bounds_min = r.f32x4()?;
            bounds_max = r.f32x4()?;
            vertex_count = r.u32()?;
            index_count = r.u32()?;
            morph_count = r.u32()?;
            bone_count = r.u32()?;
            r.skip(0x22)?;
            body_vertex_count = r.u32()?;
            body_index_count = r.u32()?;
            r.skip(0x3D)?;
            meshes = read_meshes(&mut r)?;
            if common.kind == KIND_LEVEL_PACKED {
                material_paths = read_material_paths(&mut r)?;
            }
            skeleton = None;
        } else {
            r.skip(0x2C)?;
            bounds_min = r.f32x4()?;
            bounds_max = r.f32x4()?;
            let n = r.u32()?;
            ensure!(n <= 1024, "implausible weighted bone count {n}");
            let weighted = (0..n).map(|_| r.str32(1024)).collect::<Result<Vec<_>>>()?;
            index_count = r.u32()?;
            bone_count = r.u32()?;
            vertex_count = r.u32()?;
            morph_count = r.u32()? & 0xFFFF;
            r.skip(0x13)?;
            body_vertex_count = r.u32()?;
            body_index_count = r.u32()?;
            r.skip(0x25)?;
            let n = r.u32()?;
            ensure!(n <= 2048, "implausible bone tree size {n}");
            let mut tree = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let name = r.str32(1024)?;
                let parent = r.u16()?;
                let unknown = r.u16()?;
                tree.push(BoneNode {
                    name,
                    parent,
                    unknown,
                });
            }
            let unknown_structs = r.u32()?;
            ensure!(
                unknown_structs <= 1024,
                "implausible structure count {unknown_structs}"
            );
            for _ in 0..unknown_structs {
                r.skip(0x10)?;
                r.str32(1024)?;
                r.skip(0x4C)?;
            }
            r.skip(0x1A)?;
            let rest_skip = r.u32()?;
            ensure!(rest_skip <= 1 << 20, "implausible skip {rest_skip}");
            r.skip(0x10 * unknown_structs as usize + 8)?;
            meshes = read_meshes(&mut r)?;
            let morph_names = r.u32()?;
            ensure!(
                morph_names <= 4096,
                "implausible morph name count {morph_names}"
            );
            for _ in 0..morph_names {
                r.str32(1024)?;
                r.skip(4)?;
            }
            let maps = r.u32()?;
            ensure!(maps <= 64, "implausible bone map count {maps}");
            let mut bone_maps = Vec::with_capacity(maps as usize);
            for _ in 0..maps {
                let k = r.u32()?;
                ensure!(k <= 1024, "implausible bone map size {k}");
                bone_maps.push((0..k).map(|_| r.u32()).collect::<Result<Vec<_>>>()?);
            }
            let morph_skip = skip_morph_structs(&mut r, meshes.len())?;
            r.skip(0x2F)?;
            skip_physics_ref(&mut r, false)?;
            skeleton = Some(SkeletonHeader {
                weighted,
                tree,
                bone_maps,
                rest_skip,
                morph_skip,
            });
        }
        ensure!(
            body_vertex_count <= vertex_count && body_index_count <= index_count,
            "B. counts {body_vertex_count}/{body_index_count} exceed the totals {vertex_count}/{index_count}"
        );
        if common.kind != KIND_SKINNED {
            skip_physics_ref(&mut r, level_mesh)?;
        }

        let physx_count = r.u32()?;
        ensure!(
            physx_count <= 1024,
            "implausible PhysX shape count {physx_count}"
        );
        let mut physx_lens = Vec::with_capacity(physx_count as usize);
        for _ in 0..physx_count {
            let ty = r.u32()?;
            r.skip(match ty {
                1 => 0x4C,
                7 => 0x60,
                _ => 0x50,
            })?;
            r.skip(32)?;
            if !matches!(ty, 0 | 2 | 3) {
                r.skip(4)?;
            }
            r.skip(4)?;
            physx_lens.push(r.u32()?);
            r.skip(8)?;
        }

        let message_count = r.u32()?;
        ensure!(
            message_count <= 1024,
            "implausible message count {message_count}"
        );
        let messages = (0..message_count)
            .map(|_| r.str32(1 << 16))
            .collect::<Result<_>>()?;
        let left = r.rest().len();
        ensure!(left == 0, "{left} bytes left after the model header");

        Ok(ModelHeader {
            common,
            bounds_min,
            bounds_max,
            vertex_count,
            index_count,
            morph_count,
            bone_count,
            body_vertex_count,
            body_index_count,
            meshes,
            physx_lens,
            messages,
            skeleton,
            material_paths,
        })
    }

    /// Per-axis factor applied to the snorm positions.
    pub fn scale(&self) -> [f32; 3] {
        // A flat axis has scale 0: every position on it is 0, whatever the i16 says
        std::array::from_fn(|k| self.bounds_min[k].abs().max(self.bounds_max[k].abs()))
    }

    /// Material names, one per mesh: the header's list for level meshes, else the attribute text's.
    pub fn materials(&self) -> Vec<String> {
        if !self.material_paths.is_empty() {
            return self.material_paths.clone();
        }
        let attrs = &self.common.attributes;
        let Some(start) = attrs.find("Materials={") else {
            return Vec::new();
        };
        let block = &attrs[start + "Materials={".len()..];
        let block = &block[..block.find('}').unwrap_or(block.len())];
        block
            .lines()
            .map(|l| l.trim().trim_end_matches(','))
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// xyz plus the bitangent sign in w, as glTF's TANGENT expects.
    pub tangent: [f32; 4],
    pub ao: f32,
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    /// Skinned models: joint (weighted bone) indices and their weights; zero otherwise.
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

/// Decodes a 24-bit octahedral unit vector (see the module docs).
pub fn decode_octahedral(b: [u8; 3]) -> [f32; 3] {
    let bits = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
    let u = (bits >> 12) as f32 / 4095.0 * 2.0 - 1.0;
    let v = (bits & 0xFFF) as f32 / 4095.0 * 2.0 - 1.0;
    let z = 1.0 - u.abs() - v.abs();
    let (x, y) = if z < 0.0 {
        ((1.0 - v.abs()).copysign(u), (1.0 - u.abs()).copysign(v))
    } else {
        (u, v)
    };
    let len = (x * x + y * y + z * z).sqrt();
    [x / len, y / len, z / len]
}

/// Decodes one 24-byte vertex.
pub fn decode_vertex(raw: &[u8], scale: [f32; 3]) -> Vertex {
    let i16_at = |o: usize| f32::from(i16::from_le_bytes([raw[o], raw[o + 1]]));
    let half_at = |o: usize| f16_to_f32(u16::from_le_bytes([raw[o], raw[o + 1]]));
    Vertex {
        position: std::array::from_fn(|k| i16_at(2 * k) / 32767.0 * scale[k]),
        normal: decode_octahedral([raw[8], raw[9], raw[10]]),
        tangent: {
            let [x, y, z] = decode_octahedral([raw[12], raw[13], raw[14]]);
            [x, y, z, if i16_at(6) < 0.0 { -1.0 } else { 1.0 }]
        },
        ao: f32::from(raw[15]) / 255.0,
        uv0: [half_at(16), half_at(18)],
        uv1: [half_at(20), half_at(22)],
        joints: [0; 4],
        weights: [0.0; 4],
    }
}

/// Decodes one 32-byte type 109 vertex (see the module docs).
pub fn decode_float_vertex(raw: &[u8]) -> Vertex {
    let f32_at = |o: usize| f32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
    let half_at = |o: usize| f16_to_f32(u16::from_le_bytes([raw[o], raw[o + 1]]));
    Vertex {
        position: [f32_at(0), f32_at(4), f32_at(8)],
        normal: decode_octahedral([raw[16], raw[17], raw[18]]),
        tangent: {
            let [x, y, z] = decode_octahedral([raw[20], raw[21], raw[22]]);
            [x, y, z, 1.0]
        },
        ao: f32::from(raw[23]) / 255.0,
        uv0: [half_at(24), half_at(26)],
        uv1: [half_at(28), half_at(30)],
        joints: [0; 4],
        weights: [0.0; 4],
    }
}

/// Decodes one 32-byte skinned vertex: the 24-byte layout, then four u8 bone
/// indices (local to the mesh's bone map) and a u32 holding three 10-bit
/// weights (lowest bits first); the fourth weight is what is left of 1.
pub fn decode_skinned_vertex(raw: &[u8], scale: [f32; 3]) -> Vertex {
    let mut v = decode_vertex(&raw[..VERTEX_SIZE], scale);
    v.joints = [raw[24], raw[25], raw[26], raw[27]].map(u16::from);
    let packed = u32::from_le_bytes(raw[28..32].try_into().unwrap());
    let mut w = [0.0f32; 4];
    for (k, item) in w.iter_mut().take(3).enumerate() {
        *item = ((packed >> (10 * k)) & 0x3FF) as f32 / 1023.0;
    }
    w[3] = (1.0 - w[0] - w[1] - w[2]).max(0.0);
    if w[3] < 1e-4 {
        w[3] = 0.0;
    }
    let sum: f32 = w.iter().sum();
    if sum > 0.0 {
        w = w.map(|x| x / sum);
    }
    v.weights = w;
    v
}

/// A bone of the rest pose, in the game's (left-handed) space.
#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    /// Local rotation as a quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    /// Local translation.
    pub translation: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
    /// Bone index of each joint (weighted bone); vertex joint indices point here.
    pub joints: Vec<usize>,
    /// Inverse bind matrix per joint, 3x4 row-major (the last row is 0, 0, 0, 1).
    pub inverse_binds: Vec<[f32; 12]>,
}

impl Skeleton {
    /// Local transform of a bone as a row-major 4x4 matrix (translation x rotation).
    pub fn local_matrix(&self, bone: usize) -> [[f32; 4]; 4] {
        let [x, y, z, w] = self.bones[bone].rotation;
        let t = self.bones[bone].translation;
        [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
                t[0],
            ],
            [
                2.0 * (x * y + z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - x * w),
                t[1],
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                1.0 - 2.0 * (x * x + y * y),
                t[2],
            ],
            [0.0, 0.0, 0.0, 1.0],
        ]
    }

    /// Largest difference between `inverse_bind x rest-pose global transform`
    /// and the identity over all joints; near zero when the rest pose and the
    /// inverse bind matrices agree.
    pub fn bind_error(&self) -> f32 {
        self.joint_bind_errors().into_iter().fold(0.0, f32::max)
    }

    /// Per joint: largest difference from the identity of `inverse_bind x global`.
    pub fn joint_bind_errors(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.joints.len());
        for (j, &bone) in self.joints.iter().enumerate() {
            let mut worst = 0.0f32;
            let ib = &self.inverse_binds[j];
            let ib = [
                [ib[0], ib[1], ib[2], ib[3]],
                [ib[4], ib[5], ib[6], ib[7]],
                [ib[8], ib[9], ib[10], ib[11]],
                [0.0, 0.0, 0.0, 1.0],
            ];
            let p = mat_mul(&ib, &self.global_matrix(bone));
            for (r, row) in p.iter().enumerate() {
                for (c, v) in row.iter().enumerate() {
                    worst = worst.max((v - if r == c { 1.0 } else { 0.0 }).abs());
                }
            }
            out.push(worst);
        }
        out
    }

    /// Local transforms (translation, rotation, scale) that put every joint at
    /// its bind pose (the inverse of its inverse bind matrix); bones without
    /// one keep their stored rest transform. Some models store a rest pose
    /// that differs from the bind pose; exporting the bind pose keeps the
    /// mesh in shape.
    pub fn bind_pose_locals(&self) -> Vec<([f32; 3], [f32; 4], [f32; 3])> {
        let mut joint_of = vec![None; self.bones.len()];
        for (j, &b) in self.joints.iter().enumerate() {
            joint_of[b] = Some(j);
        }
        let mut globals: Vec<[[f32; 4]; 4]> = Vec::with_capacity(self.bones.len());
        let mut out = Vec::with_capacity(self.bones.len());
        for (i, bone) in self.bones.iter().enumerate() {
            let parent_global = bone.parent.map(|p| globals[p]);
            let global = match joint_of[i] {
                Some(j) => affine_inverse(&row_major(&self.inverse_binds[j])),
                None => {
                    let local = self.local_matrix(i);
                    parent_global.map_or(local, |pg| mat_mul(&pg, &local))
                }
            };
            let local = parent_global.map_or(global, |pg| mat_mul(&affine_inverse(&pg), &global));
            out.push(decompose(&local));
            globals.push(global);
        }
        out
    }

    /// Model-space transform of a bone in the rest pose.
    pub fn global_matrix(&self, bone: usize) -> [[f32; 4]; 4] {
        let local = self.local_matrix(bone);
        match self.bones[bone].parent {
            Some(p) => mat_mul(&self.global_matrix(p), &local),
            None => local,
        }
    }
}

/// A 3x4 row-major matrix as 4x4.
pub fn row_major(m: &[f32; 12]) -> [[f32; 4]; 4] {
    [
        [m[0], m[1], m[2], m[3]],
        [m[4], m[5], m[6], m[7]],
        [m[8], m[9], m[10], m[11]],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Inverse of an affine 4x4 matrix (last row 0, 0, 0, 1).
pub fn affine_inverse(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let a = |r: usize, c: usize| f64::from(m[r][c]);
    let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
        - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
        + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
    let inv_det = if det.abs() < 1e-12 { 0.0 } else { 1.0 / det };
    // Inverse of the 3x3 part: transposed cofactors over the determinant
    let r: [[f64; 3]; 3] = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let (r0, r1) = ((j + 1) % 3, (j + 2) % 3);
            let (c0, c1) = ((i + 1) % 3, (i + 2) % 3);
            (a(r0, c0) * a(r1, c1) - a(r0, c1) * a(r1, c0)) * inv_det
        })
    });
    let t = [a(0, 3), a(1, 3), a(2, 3)];
    let mut out = [[0.0f32; 4]; 4];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = r[i][j] as f32;
        }
        out[i][3] = -(r[i][0] * t[0] + r[i][1] * t[1] + r[i][2] * t[2]) as f32;
    }
    out[3][3] = 1.0;
    out
}

/// Splits an affine matrix into translation, rotation quaternion (x, y, z, w) and scale.
pub fn decompose(m: &[[f32; 4]; 4]) -> ([f32; 3], [f32; 4], [f32; 3]) {
    let t = [m[0][3], m[1][3], m[2][3]];
    let scale: [f32; 3] =
        std::array::from_fn(|c| (m[0][c] * m[0][c] + m[1][c] * m[1][c] + m[2][c] * m[2][c]).sqrt());
    let r = |i: usize, j: usize| {
        if scale[j] > 0.0 {
            m[i][j] / scale[j]
        } else {
            0.0
        }
    };
    let trace = r(0, 0) + r(1, 1) + r(2, 2);
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (r(2, 1) - r(1, 2)) / s,
            (r(0, 2) - r(2, 0)) / s,
            (r(1, 0) - r(0, 1)) / s,
            0.25 * s,
        ]
    } else if r(0, 0) > r(1, 1) && r(0, 0) > r(2, 2) {
        let s = (1.0 + r(0, 0) - r(1, 1) - r(2, 2)).sqrt() * 2.0;
        [
            0.25 * s,
            (r(0, 1) + r(1, 0)) / s,
            (r(0, 2) + r(2, 0)) / s,
            (r(2, 1) - r(1, 2)) / s,
        ]
    } else if r(1, 1) > r(2, 2) {
        let s = (1.0 + r(1, 1) - r(0, 0) - r(2, 2)).sqrt() * 2.0;
        [
            (r(0, 1) + r(1, 0)) / s,
            0.25 * s,
            (r(1, 2) + r(2, 1)) / s,
            (r(0, 2) - r(2, 0)) / s,
        ]
    } else {
        let s = (1.0 + r(2, 2) - r(0, 0) - r(1, 1)).sqrt() * 2.0;
        [
            (r(0, 2) + r(2, 0)) / s,
            (r(1, 2) + r(2, 1)) / s,
            0.25 * s,
            (r(1, 0) - r(0, 1)) / s,
        ]
    };
    let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    (t, q.map(|v| v / n), scale)
}

pub fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..4).map(|k| a[i][k] * b[k][j]).sum()))
}

pub struct Model {
    pub header: ModelHeader,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub skeleton: Option<Skeleton>,
}

/// Reads inverse binds and rest poses, and maps vertex bone indices from
/// each mesh's local numbering to joint indices.
fn read_skeleton(
    sh: &SkeletonHeader,
    data: &[u8],
    meshes: &[MeshInfo],
    indices: &[u32],
    vertices: &mut [Vertex],
) -> Result<Skeleton> {
    let mut r = Reader::new(data);
    let f = |r: &mut Reader| r.f32();
    let mut inverse_binds = Vec::with_capacity(sh.weighted.len());
    for _ in 0..sh.weighted.len() {
        let mut m = [0.0; 12];
        for v in &mut m {
            *v = f(&mut r)?;
        }
        inverse_binds.push(m);
    }
    r.skip(sh.rest_skip as usize)?;
    let mut bones = Vec::with_capacity(sh.tree.len());
    for (i, node) in sh.tree.iter().enumerate() {
        let rotation = [f(&mut r)?, f(&mut r)?, f(&mut r)?, f(&mut r)?];
        let translation = [f(&mut r)?, f(&mut r)?, f(&mut r)?];
        r.skip(12)?;
        let parent = usize::from(node.parent);
        ensure!(
            parent < sh.tree.len(),
            "bone {} has parent {parent} of {}",
            node.name,
            sh.tree.len()
        );
        ensure!(parent <= i, "bone {} comes before its parent", node.name);
        bones.push(Bone {
            name: node.name.clone(),
            parent: (parent != i).then_some(parent),
            rotation,
            translation,
        });
    }
    // Joints are found by name. An unnamed joint is the bone at its own index;
    // a name missing from the tree (attachments that ride another model's
    // skeleton) gets a root-level bone placed at its bind pose.
    let mut joints = Vec::with_capacity(sh.weighted.len());
    for (j, w) in sh.weighted.iter().enumerate() {
        let found = if w.is_empty() {
            (j < sh.tree.len()).then_some(j)
        } else {
            bones.iter().position(|b| &b.name == w)
        };
        let index = match found {
            Some(i) => i,
            None => {
                let g = affine_inverse(&row_major(&inverse_binds[j]));
                let (translation, rotation, _) = decompose(&g);
                bones.push(Bone {
                    name: if w.is_empty() {
                        format!("joint{j}")
                    } else {
                        w.clone()
                    },
                    parent: None,
                    rotation,
                    translation,
                });
                bones.len() - 1
            }
        };
        joints.push(index);
    }

    // Each vertex takes the bone map of the first mesh that uses it
    let mut owner = vec![usize::MAX; vertices.len()];
    for (m, mesh) in meshes.iter().enumerate() {
        for lod in 0..LOD_COUNT {
            let start = mesh.lod_offsets[lod] as usize;
            let end = (start + mesh.lod_counts[lod] as usize).min(indices.len());
            for &i in &indices[start.min(end)..end] {
                if owner[i as usize] == usize::MAX {
                    owner[i as usize] = m;
                }
            }
        }
    }
    for (v, &m) in vertices.iter_mut().zip(&owner) {
        if m == usize::MAX {
            // Not used by any mesh or LOD
            v.joints = [0; 4];
            continue;
        }
        let map = sh.bone_maps.get(m);
        for k in 0..4 {
            if v.weights[k] == 0.0 {
                v.joints[k] = 0;
                continue;
            }
            let local = usize::from(v.joints[k]);
            let joint = match map {
                Some(map) => *map.get(local).with_context(|| {
                    format!("bone index {local} outside mesh {m}'s map of {}", map.len())
                })? as usize,
                None => local,
            };
            ensure!(
                joint < joints.len(),
                "joint {joint} out of range ({} joints)",
                joints.len()
            );
            v.joints[k] = joint as u16;
        }
    }
    Ok(Skeleton {
        bones,
        joints,
        inverse_binds,
    })
}

impl Model {
    pub fn from_parts(header: ModelHeader, body: &[u8], full: &[u8]) -> Result<Self> {
        let (mut bv, mut bi) = (
            header.body_vertex_count as usize,
            header.body_index_count as usize,
        );
        if bv == 0 && bi == 0 && full.is_empty() {
            // Small models have no F. part and leave the B. counts at zero
            (bv, bi) = (header.vertex_count as usize, header.index_count as usize);
        }
        let (fv, fi) = (
            header.vertex_count as usize - bv,
            header.index_count as usize - bi,
        );
        let physx: usize = header.physx_lens.iter().map(|&l| l as usize).sum();
        let float_positions = header.common.kind == KIND_LEVEL_FLOAT;
        let vsize = if header.skeleton.is_some() {
            SKINNED_VERTEX_SIZE
        } else if float_positions {
            FLOAT_VERTEX_SIZE
        } else {
            VERTEX_SIZE
        };
        // Skinned models put the skeleton between the PhysX shapes and the vertices
        let (skel_len, morph_len) = match &header.skeleton {
            Some(s) => {
                let bones = s.tree.len();
                let len = s.weighted.len() * 48
                    + s.rest_skip as usize
                    + bones * 40
                    + bones * 4
                    + if bones > 0 { 8 } else { 0 };
                (len, s.morph_skip as usize * 8)
            }
            None => (0, 0),
        };
        let mut expected_body = physx + 2 + skel_len + bv * vsize + morph_len + bi * 2;
        // Some type 109 models carry whole unknown blocks after the PhysX data
        let mut physx = physx;
        if float_positions && body.len() > expected_body {
            let extra = body.len() - expected_body;
            ensure!(
                extra.is_multiple_of(EXTRA_BLOCK),
                "B. part is {} bytes, expected {expected_body} plus whole blocks of {EXTRA_BLOCK}",
                body.len()
            );
            expected_body += extra;
            physx += extra;
        }
        let expected_full = fv * vsize + fi * 2;
        if header.morph_count > 0 && (body.len(), full.len()) != (expected_body, expected_full) {
            bail!(
                "models with morph targets are not supported ({} morphs)",
                header.morph_count
            );
        }
        ensure!(
            body.len() == expected_body,
            "B. part is {} bytes, expected {expected_body}",
            body.len()
        );
        ensure!(
            full.len() == expected_full,
            "F. part is {} bytes, expected {expected_full}",
            full.len()
        );

        let scale = header.scale();
        let body = &body[physx + 2..];
        let (skel_data, body) = body.split_at(skel_len);
        let (body_verts, rest) = body.split_at(bv * vsize);
        let body_idx = &rest[morph_len..];
        let (full_verts, full_idx) = full.split_at(fv * vsize);
        let decode = |v: &[u8]| {
            if header.skeleton.is_some() {
                decode_skinned_vertex(v, scale)
            } else if float_positions {
                decode_float_vertex(v)
            } else {
                decode_vertex(v, scale)
            }
        };
        let mut vertices: Vec<Vertex> = body_verts
            .chunks_exact(vsize)
            .chain(full_verts.chunks_exact(vsize))
            .map(decode)
            .collect();
        let indices: Vec<u32> = body_idx
            .as_chunks::<2>()
            .0
            .iter()
            .chain(full_idx.as_chunks::<2>().0)
            .map(|&c| u32::from(u16::from_le_bytes(c)))
            .collect();
        let skeleton = match &header.skeleton {
            Some(sh) => Some(read_skeleton(
                sh,
                skel_data,
                &header.meshes,
                &indices,
                &mut vertices,
            )?),
            None => None,
        };

        if let Some(&max) = indices.iter().max() {
            ensure!(
                (max as usize) < vertices.len(),
                "index {max} out of range for {} vertices",
                vertices.len()
            );
        }
        ensure!(
            indices.len().is_multiple_of(3),
            "index count {} is not a multiple of 3",
            indices.len()
        );
        for (i, m) in header.meshes.iter().enumerate() {
            for lod in 0..LOD_COUNT {
                let end = m.lod_offsets[lod] as usize + m.lod_counts[lod] as usize;
                ensure!(
                    end <= indices.len(),
                    "mesh {i} LOD {lod} ends at index {end} of {}",
                    indices.len()
                );
            }
        }
        Ok(Model {
            header,
            vertices,
            indices,
            skeleton,
        })
    }

    /// Loads a model by path from already opened `H.Misc`, `B.Misc` and `F.Misc` packages.
    pub fn load_from(
        head: &mut Package,
        body: Option<&mut Package>,
        full: Option<&mut Package>,
        path: &str,
    ) -> Result<Self> {
        let h = head
            .read_path(path)?
            .with_context(|| format!("{path} is not in H.Misc"))?;
        let header = ModelHeader::parse(&h).with_context(|| path.to_string())?;
        let body = match body {
            Some(p) => p.read_path(path)?.unwrap_or_default(),
            None => Vec::new(),
        };
        let full = match full {
            Some(p) => p.read_path(path)?.unwrap_or_default(),
            None => Vec::new(),
        };
        Model::from_parts(header, &body, &full).with_context(|| path.to_string())
    }

    /// Loads a model by path from a `Cache.Windows` folder.
    pub fn load(dir: &Path, path: &str) -> Result<Self> {
        let read = |name: &str| -> Result<Option<Vec<u8>>> {
            match Package::open_in(dir, name)? {
                Some(mut pkg) => pkg.read_path(path),
                None => Ok(None),
            }
        };
        let h = read("H.Misc")?.with_context(|| format!("{path} is not in H.Misc"))?;
        let header = ModelHeader::parse(&h).with_context(|| path.to_string())?;
        let body = read("B.Misc")?.unwrap_or_default();
        let full = read("F.Misc")?.unwrap_or_default();
        Model::from_parts(header, &body, &full).with_context(|| path.to_string())
    }

    /// Checks that the decoded positions lie inside the header's bounding box,
    /// up to the quantisation step. The box is often a little larger than the
    /// geometry, so this cannot prove the scale, but a wrong vertex layout
    /// fails it.
    pub fn check_bounds(&self) -> Result<()> {
        if self.vertices.is_empty() {
            return Ok(());
        }
        let (lo, hi) = self.bounds();
        let scale = self.header.scale();
        for k in 0..3 {
            let tol = scale[k] / 32767.0 * 2.0 + 1e-6;
            let (want_lo, want_hi) = (self.header.bounds_min[k], self.header.bounds_max[k]);
            ensure!(
                lo[k] >= want_lo - tol && hi[k] <= want_hi + tol,
                "axis {k}: decoded {}..{} outside the header's {want_lo}..{want_hi}",
                lo[k],
                hi[k]
            );
        }
        Ok(())
    }

    /// Bounding box of the decoded positions.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &self.vertices {
            for k in 0..3 {
                lo[k] = lo[k].min(v.position[k]);
                hi[k] = hi[k].max(v.position[k]);
            }
        }
        (lo, hi)
    }

    /// The triangles of one mesh at one LOD, as indices into `vertices`.
    pub fn lod_indices(&self, mesh: usize, lod: usize) -> Result<&[u32]> {
        let m = self.header.meshes.get(mesh).context("no such mesh")?;
        if lod >= LOD_COUNT {
            bail!("LOD {lod} out of range");
        }
        let start = m.lod_offsets[lod] as usize;
        Ok(&self.indices[start..start + m.lod_counts[lod] as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertex_bytes(
        pos: [i16; 4],
        normal: [u8; 4],
        tangent_ao: [u8; 4],
        uv0: [u16; 2],
        uv1: [u16; 2],
    ) -> Vec<u8> {
        let mut v = Vec::new();
        for p in pos {
            v.extend(p.to_le_bytes());
        }
        v.extend(normal);
        v.extend(tangent_ao);
        for h in uv0.into_iter().chain(uv1) {
            v.extend(h.to_le_bytes());
        }
        v
    }

    #[test]
    fn octahedral_vectors() {
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-3);
        // Centre of the square is +z, the corners are -z
        assert!(near(decode_octahedral([0x7F, 0xF7, 0xFF]), [0.0, 0.0, 1.0]));
        assert!(near(
            decode_octahedral([0xFF, 0xFF, 0xFF]),
            [0.0, 0.0, -1.0]
        ));
        assert!(near(
            decode_octahedral([0x00, 0x00, 0x00]),
            [0.0, 0.0, -1.0]
        ));
        // Edge midpoints are on the equator: u = -1, v = 0 is -x; u = 0, v = -1 is -y
        assert!(near(
            decode_octahedral([0x00, 0x07, 0xFF]),
            [-1.0, 0.0, 0.0]
        ));
        assert!(near(
            decode_octahedral([0x7F, 0xF0, 0x00]),
            [0.0, -1.0, 0.0]
        ));
        // A lower-hemisphere point unfolds: u = 0.5, v = 0.75 => (0.25, 0.5, -0.25) normalised
        let (u, v) = (
            (0.75f32 * 4095.0).round() as u32,
            (0.875f32 * 4095.0).round() as u32,
        );
        let bits = u << 12 | v;
        let n = decode_octahedral([(bits >> 16) as u8, (bits >> 8) as u8, bits as u8]);
        let l = (0.25f32 * 0.25 + 0.5 * 0.5 + 0.25 * 0.25).sqrt();
        assert!(near(n, [0.25 / l, 0.5 / l, -0.25 / l]), "{n:?}");
    }

    #[test]
    fn decodes_vertex() {
        // Normal: u = 4095 (+1), v = 2047.5 rounded down (~0) => +x; tangent: u ~0, v = 4095 => +y
        let raw = vertex_bytes(
            [32767, -32767, 0, -255],
            [0xFF, 0xF7, 0xFF, 0],
            [0x7F, 0xFF, 0xFF, 51],
            [0x3800, 0x3C00],
            [0, 0xBC00],
        );
        assert_eq!(raw.len(), VERTEX_SIZE);
        let v = decode_vertex(&raw, [2.0, 0.5, 1.0]);
        assert_eq!(v.position, [2.0, -0.5, 0.0]);
        let near = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
        assert!(near(&v.normal, &[1.0, 0.0, 0.0]), "{:?}", v.normal);
        assert!(near(&v.tangent, &[0.0, 1.0, 0.0, -1.0]), "{:?}", v.tangent);
        assert!((v.ao - 0.2).abs() < 1e-6);
        assert_eq!(v.uv0, [0.5, 1.0]);
        assert_eq!(v.uv1, [0.0, -1.0]);
    }

    /// A header with one mesh and the given counts, built field by field as documented above.
    fn header_bytes(vertices: u32, indices: u32, body_vertices: u32, body_indices: u32) -> Vec<u8> {
        let mut h =
            crate::header::tests::build(&["/Lotus/m.fbx"], "Materials={\n    MyMat\n}\n", 102);
        h.extend([0; 0x30]);
        h.extend([0; 12]); // physics reference: a, b, empty path
        h.extend([0; 0x28]);
        for f in [-1.0f32, -2.0, -0.5, 0.0, 1.0, 4.0, 0.5, 0.0] {
            h.extend(f.to_le_bytes());
        }
        for v in [vertices, indices, 0, 0] {
            h.extend(v.to_le_bytes());
        }
        h.extend([0; 0x22]);
        h.extend(body_vertices.to_le_bytes());
        h.extend(body_indices.to_le_bytes());
        h.extend([0; 0x3D]);
        h.extend(1u32.to_le_bytes());
        h.extend([0; 32]);
        h.extend(4u32.to_le_bytes());
        h.extend(b"mesh");
        for v in [
            body_indices,
            0,
            0,
            0,
            0,
            indices - body_indices,
            body_indices,
            0,
            0,
            0,
        ] {
            h.extend(v.to_le_bytes());
        }
        h.extend([0; 0x20]);
        h.extend(0u32.to_le_bytes());
        h.extend([0; 0x10]);
        h.extend(0u32.to_le_bytes()); // u16 array
        h.extend([0; 12]); // physics reference
        h.extend(0u32.to_le_bytes()); // PhysX shapes
        h.extend(0u32.to_le_bytes()); // messages
        h
    }

    #[test]
    fn parses_header_and_splits_parts() {
        let header = ModelHeader::parse(&header_bytes(4, 6, 3, 3)).unwrap();
        assert_eq!(
            (
                header.vertex_count,
                header.index_count,
                header.body_vertex_count,
                header.body_index_count
            ),
            (4, 6, 3, 3)
        );
        assert_eq!(header.scale(), [1.0, 4.0, 0.5]);
        assert!(Model::from_parts(header.clone(), &[0, 0], &[]).is_err());
        assert_eq!(header.materials(), ["MyMat"]);
        assert_eq!(header.meshes[0].name, "mesh");

        let v = |x: i16| vertex_bytes([x, 0, 0, 0], [128; 4], [128; 4], [0; 2], [0; 2]);
        let mut body = vec![0, 0];
        for x in [0, 32767, -32767] {
            body.extend(v(x));
        }
        for i in [0u16, 1, 2] {
            body.extend(i.to_le_bytes());
        }
        let mut full = v(16384);
        for i in [1u16, 3, 2] {
            full.extend(i.to_le_bytes());
        }
        let model = Model::from_parts(header.clone(), &body, &full).unwrap();
        assert_eq!(model.vertices.len(), 4);
        assert_eq!(model.vertices[1].position[0], 1.0);
        assert_eq!(model.indices, [0, 1, 2, 1, 3, 2]);
        assert_eq!(model.lod_indices(0, 0).unwrap(), [1, 3, 2]);
        assert_eq!(model.lod_indices(0, 1).unwrap(), [0, 1, 2]);

        // No F. part and zero B. counts: everything is in B.
        let mut all_b = ModelHeader::parse(&header_bytes(4, 6, 0, 0)).unwrap();
        all_b.meshes[0].lod_offsets = [0; LOD_COUNT];
        all_b.meshes[0].lod_counts = [6, 0, 0, 0, 0];
        let mut body_all = body[..2 + 3 * VERTEX_SIZE].to_vec();
        body_all.extend(v(16384));
        for i in [0u16, 1, 2, 1, 3, 2] {
            body_all.extend(i.to_le_bytes());
        }
        let model = Model::from_parts(all_b, &body_all, &[]).unwrap();
        assert_eq!((model.vertices.len(), model.indices.len()), (4, 6));

        // Wrong part sizes and out-of-range indices are errors
        assert!(Model::from_parts(header.clone(), &body[..body.len() - 1], &full).is_err());
        let mut bad = full.clone();
        let n = bad.len();
        bad[n - 2..].copy_from_slice(&9u16.to_le_bytes());
        assert!(Model::from_parts(header, &body, &bad).is_err());
    }

    #[test]
    fn rejects_other_types_and_trailing_bytes() {
        let mut raw = header_bytes(4, 6, 3, 3);
        raw.push(0);
        assert!(ModelHeader::parse(&raw).is_err());
        let raw = crate::header::tests::build(&[], "", 281);
        assert!(ModelHeader::parse(&raw).is_err());
    }

    fn s32(h: &mut Vec<u8>, s: &str) {
        h.extend((s.len() as u32).to_le_bytes());
        h.extend(s.as_bytes());
    }

    #[test]
    fn decodes_skinned_vertex() {
        let mut raw = vertex_bytes(
            [0, 0, 0, 255],
            [0x7F, 0xF7, 0xFF, 0],
            [0x7F, 0xFF, 0xFF, 0],
            [0; 2],
            [0; 2],
        );
        raw.extend([3, 1, 0, 0]);
        // Weights 512/1023 and 256/1023 in the low 20 bits; the third is 0; the fourth is the rest
        let packed: u32 = 512 | 256 << 10;
        raw.extend(packed.to_le_bytes());
        let v = decode_skinned_vertex(&raw, [1.0; 3]);
        assert_eq!(v.joints, [3, 1, 0, 0]);
        assert!((v.weights[0] - 512.0 / 1023.0).abs() < 1e-6);
        assert!((v.weights[1] - 256.0 / 1023.0).abs() < 1e-6);
        assert_eq!(v.weights[2], 0.0);
        assert!((v.weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        // Three full weights leave nothing for the fourth
        let mut raw2 = raw.clone();
        raw2[28..32].copy_from_slice(&(341u32 | 341 << 10 | 341 << 20).to_le_bytes());
        let v = decode_skinned_vertex(&raw2, [1.0; 3]);
        assert!(v.weights[3] < 0.01 && (v.weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    fn two_bone_skeleton() -> Skeleton {
        // Root at the origin; child 1 m up, turned 90 degrees about z
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let bones = vec![
            Bone {
                name: "root".into(),
                parent: None,
                rotation: [0.0, 0.0, 0.0, 1.0],
                translation: [0.0; 3],
            },
            Bone {
                name: "arm".into(),
                parent: Some(0),
                rotation: [0.0, 0.0, h, h],
                translation: [0.0, 1.0, 0.0],
            },
        ];
        let mut sk = Skeleton {
            bones,
            joints: vec![0, 1],
            inverse_binds: Vec::new(),
        };
        sk.inverse_binds = (0..2)
            .map(|b| {
                let inv = affine_inverse(&sk.global_matrix(b));
                std::array::from_fn(|k| inv[k / 4][k % 4])
            })
            .collect();
        sk
    }

    #[test]
    fn bone_hierarchy_and_bind_pose() {
        let sk = two_bone_skeleton();
        let g = sk.global_matrix(1);
        // The child's x axis points along the parent's y after the turn
        assert!((g[1][0] - 1.0).abs() < 1e-6 && (g[1][3] - 1.0).abs() < 1e-6);
        assert!(sk.bind_error() < 1e-6);
        let locals = sk.bind_pose_locals();
        assert!((locals[1].0[1] - 1.0).abs() < 1e-6);
        assert!((locals[1].1[2] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        // A rest pose that differs from the bind pose: the export follows the bind pose
        let mut moved = sk.clone();
        moved.bones[1].translation = [0.0, 3.0, 0.0];
        assert!(moved.bind_error() > 1.0);
        assert!((moved.bind_pose_locals()[1].0[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn affine_round_trip() {
        let sk = two_bone_skeleton();
        let g = sk.global_matrix(1);
        let p = mat_mul(&g, &affine_inverse(&g));
        for (r, row) in p.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                assert!((v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-6);
            }
        }
        let (t, q, s) = decompose(&g);
        assert_eq!(t, [0.0, 1.0, 0.0]);
        assert!(s.iter().all(|v| (v - 1.0).abs() < 1e-6));
        assert!((q[2].abs() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    }

    /// A type-281 header with two weighted bones, one mesh whose bone map
    /// sends local 0 -> joint 1 and local 1 -> joint 0, all data in B.
    fn skinned_header_bytes() -> Vec<u8> {
        let mut h = crate::header::tests::build(&["/Lotus/c.fbx"], "", 281);
        h.extend([0; 0x30]);
        h.extend([0; 12]);
        h.extend([0; 0x2C]);
        for f in [-1.0f32, -1.0, -1.0, 0.0, 1.0, 1.0, 1.0, 0.0] {
            h.extend(f.to_le_bytes());
        }
        h.extend(2u32.to_le_bytes());
        s32(&mut h, "root");
        s32(&mut h, "arm");
        for v in [3u32, 2, 3, 0] {
            h.extend(v.to_le_bytes()); // indices, bones, vertices, morphs
        }
        h.extend([0; 0x13]);
        h.extend([0u8; 8]); // B. counts zero: everything in B.
        h.extend([0; 0x25]);
        h.extend(2u32.to_le_bytes());
        s32(&mut h, "root");
        h.extend([0, 0, 0, 0]);
        s32(&mut h, "arm");
        h.extend([0, 0, 0, 0]);
        h.extend(0u32.to_le_bytes()); // unknown structures
        h.extend([0; 0x1A]);
        h.extend(0u32.to_le_bytes()); // rest skip
        h.extend([0; 8]);
        h.extend(1u32.to_le_bytes());
        h.extend([0; 32]);
        s32(&mut h, "mesh");
        for v in [0u32, 0, 0, 0, 0, 3, 0, 0, 0, 0] {
            h.extend(v.to_le_bytes());
        }
        h.extend([0; 0x20]);
        h.extend(0u32.to_le_bytes());
        h.extend([0; 0x10]);
        h.extend(0u32.to_le_bytes()); // u16 array
        h.extend(0u32.to_le_bytes()); // morph names
        h.extend(1u32.to_le_bytes()); // bone maps
        h.extend(2u32.to_le_bytes());
        h.extend(1u32.to_le_bytes());
        h.extend(0u32.to_le_bytes());
        h.extend(0u32.to_le_bytes()); // morph structures
        h.extend([0; 0x2F]);
        h.extend([0; 12]);
        h.extend(0u32.to_le_bytes()); // PhysX
        h.extend(0u32.to_le_bytes()); // messages
        h
    }

    #[test]
    fn parses_skinned_model() {
        let header = ModelHeader::parse(&skinned_header_bytes()).unwrap();
        let sh = header.skeleton.clone().unwrap();
        assert_eq!(sh.weighted, ["root", "arm"]);
        assert_eq!(sh.tree[1].name, "arm");
        assert_eq!(sh.bone_maps, [vec![1, 0]]);

        let sk = two_bone_skeleton();
        let mut body = vec![0, 0];
        for ib in &sk.inverse_binds {
            for f in ib {
                body.extend(f.to_le_bytes());
            }
        }
        for b in &sk.bones {
            for f in b.rotation.iter().chain(&b.translation).chain(&[0.0; 3]) {
                body.extend(f.to_le_bytes());
            }
        }
        body.extend([0; 2 * 4 + 8]); // two u16 per bone twice, then 8 bytes
        for local in [0u8, 1, 0] {
            let mut v = vertex_bytes([0; 4], [128; 4], [128; 4], [0; 2], [0; 2]);
            v.extend([local, 0, 0, 0]);
            v.extend(1023u32.to_le_bytes());
            body.extend(v);
        }
        for i in [0u16, 1, 2] {
            body.extend(i.to_le_bytes());
        }
        let model = Model::from_parts(header, &body, &[]).unwrap();
        let sk2 = model.skeleton.unwrap();
        assert_eq!(sk2.joints, [0, 1]);
        assert_eq!(sk2.bones[1].parent, Some(0));
        assert!(sk2.bind_error() < 1e-6);
        // Local 0 maps to joint 1 through the bone map
        let joints: Vec<u16> = model.vertices.iter().map(|v| v.joints[0]).collect();
        assert_eq!(joints, [1, 0, 1]);
        assert_eq!(model.vertices[0].weights, [1.0, 0.0, 0.0, 0.0]);
    }
}
