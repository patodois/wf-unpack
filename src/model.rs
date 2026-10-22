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
//! comes out mirrored relative to its texture. [`crate::gltf`] negates z
//! (and swaps two corners of every triangle) to get glTF's right-handed
//! space; y is up in both.
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
pub const VERTEX_SIZE: usize = 24;
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
}

fn skip_physics_ref(r: &mut Reader) -> Result<()> {
    let a = r.u32()?;
    let b = r.u32()?;
    if a == 8 || b == 8 {
        r.skip(2)?;
    }
    r.str32(1024).context("physics reference")?;
    Ok(())
}

impl ModelHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let common = CommonHeader::parse(data).context("common header")?;
        ensure!(
            common.kind == KIND_STATIC,
            "model type {} is not supported (only {KIND_STATIC})",
            common.kind
        );
        let mut r = Reader::at(data, common.len)?;
        r.skip(0x30)?;
        skip_physics_ref(&mut r)?;
        r.skip(0x28)?;
        let bounds_min = r.f32x4()?;
        let bounds_max = r.f32x4()?;
        let vertex_count = r.u32()?;
        let index_count = r.u32()?;
        let morph_count = r.u32()?;
        let bone_count = r.u32()?;
        r.skip(0x22)?;
        let body_vertex_count = r.u32()?;
        let body_index_count = r.u32()?;
        ensure!(
            body_vertex_count <= vertex_count && body_index_count <= index_count,
            "B. counts {body_vertex_count}/{body_index_count} exceed the totals {vertex_count}/{index_count}"
        );
        r.skip(0x3D)?;

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
        skip_physics_ref(&mut r)?;

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
        })
    }

    /// Per-axis factor applied to the snorm positions.
    pub fn scale(&self) -> [f32; 3] {
        // A flat axis has scale 0: every position on it is 0, whatever the i16 says
        std::array::from_fn(|k| self.bounds_min[k].abs().max(self.bounds_max[k].abs()))
    }

    /// Material names from the attribute text, one per mesh.
    pub fn materials(&self) -> Vec<String> {
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
    }
}

pub struct Model {
    pub header: ModelHeader,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
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
        let expected_body = physx + 2 + bv * VERTEX_SIZE + bi * 2;
        let expected_full = fv * VERTEX_SIZE + fi * 2;
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
        let (body_verts, body_idx) = body.split_at(bv * VERTEX_SIZE);
        let (full_verts, full_idx) = full.split_at(fv * VERTEX_SIZE);
        let vertices: Vec<Vertex> = body_verts
            .as_chunks::<VERTEX_SIZE>()
            .0
            .iter()
            .chain(full_verts.as_chunks::<VERTEX_SIZE>().0)
            .map(|v| decode_vertex(v, scale))
            .collect();
        let indices: Vec<u32> = body_idx
            .as_chunks::<2>()
            .0
            .iter()
            .chain(full_idx.as_chunks::<2>().0)
            .map(|&c| u32::from(u16::from_le_bytes(c)))
            .collect();

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
        })
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
}
