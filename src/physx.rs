//! Collision shapes of models: the PhysX entries of a model header and the
//! cooked PhysX data at the start of the `B.` part.
//!
//! A model header lists its shapes (see [`crate::model::PhysxShape`]): a type,
//! a 0x4C, 0x50 or 0x60 byte record, two vectors and the size of the cooked
//! data, which is stored at the start of `B.` in header order. The record
//! of the primitive shapes holds their pose and size (little-endian floats):
//!
//! ```text
//! 0x00 f32[3] position  (then one float, 0 or 1)
//! 0x10 f32[4] rotation  quaternion x, y, z, w
//! type 1 (sphere):  0x20 f32 radius
//! type 2 (capsule): 0x20 f32 radius, 0x24 f32 half height
//! type 3 (box):     0x28 f32[3] half extents
//! ```
//!
//! Types 0 (no size stored), 1, 2 and 3 have no cooked data. Types 5 (convex
//! mesh) and 6 (triangle mesh) carry the shape's data in cooked PhysX 3.4 form
//! and a pose of zero translation and no rotation (the vertices are in the
//! model's space). Both start with `NXS\x01` and a four-letter tag.
//!
//! A triangle mesh (`MESH`):
//!
//! ```text
//! u32 version (15), u32 0, u32 flags, u32 vertex_count, u32 triangle_count
//! f32[3] vertices
//! triangles: u8, u16 or u32 indices, three per triangle (flags 4, 8, else 0)
//! u16 material index per triangle (flag 1)
//! ... then a remap table (flag 2), adjacency (16) and search structures
//! ```
//!
//! A convex mesh (`CVXM`):
//!
//! ```text
//! u32 version (13), u32 0
//! "ICE\x01" "CLHL" u32 9
//! u32 vertex_count, u32 edge_count, u32 polygon_count, u32 index_count
//! f32[3] vertices
//! polygon_count x { f32[4] plane (normal, d), u16 first, u8 count, u8 min_index }
//! u8 indices   index_count of them; a polygon's corners are indices[first..first + count]
//! ... then edge and adjacency tables
//! ```
//!
//! Layouts were read from the data and checked on every shape in the game
//! files (indices in range, vertices inside the model's bounding box); the
//! primitives' sizes were compared with the models' bounds for a few
//! examples only.
//! The record fields beyond those listed (and capsule orientation) are not
//! understood.

use anyhow::{bail, ensure, Context, Result};

use crate::bytes::Reader;
use crate::model::PhysxShape;

#[derive(Debug, Clone, PartialEq)]
pub struct TriangleMesh {
    pub vertices: Vec<[f32; 3]>,
    /// Three vertex indices per triangle.
    pub indices: Vec<u32>,
    /// Material index per triangle, when the mesh has them.
    pub materials: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConvexMesh {
    pub vertices: Vec<[f32; 3]>,
    /// Each polygon's corners as indices into `vertices`, and its plane (normal, d).
    pub polygons: Vec<(Vec<u32>, [f32; 4])>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Collider {
    /// A shape of type 0: the header stores no size for it.
    Empty,
    Sphere {
        radius: f32,
    },
    Capsule {
        radius: f32,
        half_height: f32,
    },
    Box {
        half_extents: [f32; 3],
    },
    Convex(ConvexMesh),
    Triangles(TriangleMesh),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub translation: [f32; 3],
    /// Quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    pub collider: Collider,
}

const SERIAL_MATERIALS: u32 = 1;
const SERIAL_FACE_REMAP: u32 = 2;
const SERIAL_8BIT: u32 = 4;
const SERIAL_16BIT: u32 = 8;

fn read_vertices(r: &mut Reader, n: usize) -> Result<Vec<[f32; 3]>> {
    ensure!(n <= 1 << 22, "implausible vertex count {n}");
    (0..n).map(|_| Ok([r.f32()?, r.f32()?, r.f32()?])).collect()
}

pub fn parse_triangle_mesh(data: &[u8]) -> Result<TriangleMesh> {
    let mut r = Reader::new(data);
    ensure!(r.bytes(8)? == b"NXS\x01MESH", "not a cooked triangle mesh");
    let version = r.u32()?;
    ensure!(
        version == 15,
        "triangle mesh version {version} (expected 15)"
    );
    r.skip(4)?;
    let flags = r.u32()?;
    let (nv, nt) = (r.u32()? as usize, r.u32()? as usize);
    ensure!(nt <= 1 << 22, "implausible triangle count {nt}");
    let vertices = read_vertices(&mut r, nv)?;
    let indices: Vec<u32> = (0..nt * 3)
        .map(|_| {
            Ok(if flags & SERIAL_8BIT != 0 {
                u32::from(r.u8()?)
            } else if flags & SERIAL_16BIT != 0 {
                u32::from(r.u16()?)
            } else {
                r.u32()?
            })
        })
        .collect::<Result<_>>()?;
    if let Some(&max) = indices.iter().max() {
        ensure!(
            (max as usize) < nv,
            "index {max} out of range for {nv} vertices"
        );
    }
    let materials = if flags & SERIAL_MATERIALS != 0 {
        (0..nt).map(|_| r.u16()).collect::<Result<_>>()?
    } else {
        Vec::new()
    };
    // The remap table follows when flagged; the rest is search structures
    let _ = SERIAL_FACE_REMAP;
    Ok(TriangleMesh {
        vertices,
        indices,
        materials,
    })
}

pub fn parse_convex_mesh(data: &[u8]) -> Result<ConvexMesh> {
    let mut r = Reader::new(data);
    ensure!(r.bytes(8)? == b"NXS\x01CVXM", "not a cooked convex mesh");
    let version = r.u32()?;
    ensure!(version == 13, "convex mesh version {version} (expected 13)");
    r.skip(4)?;
    ensure!(r.bytes(8)? == b"ICE\x01CLHL", "missing convex hull tag");
    let hull_version = r.u32()?;
    ensure!(
        hull_version == 9,
        "convex hull version {hull_version} (expected 9)"
    );
    let (nv, _edges, np, ni) = (
        r.u32()? as usize,
        r.u32()?,
        r.u32()? as usize,
        r.u32()? as usize,
    );
    ensure!(
        np <= 1 << 16 && ni <= 1 << 20,
        "implausible convex mesh size"
    );
    let vertices = read_vertices(&mut r, nv)?;
    let mut raw = Vec::with_capacity(np);
    for _ in 0..np {
        let plane = r.f32x4()?;
        let first = r.u16()? as usize;
        let count = r.u8()? as usize;
        r.skip(1)?;
        raw.push((plane, first, count));
    }
    let indices = r.bytes(ni)?;
    let mut polygons = Vec::with_capacity(np);
    for (plane, first, count) in raw {
        let corners = indices
            .get(first..first + count)
            .context("polygon indices run past the index table")?;
        ensure!(
            corners.iter().all(|&i| (i as usize) < nv),
            "polygon corner out of range"
        );
        polygons.push((corners.iter().map(|&i| u32::from(i)).collect(), plane));
    }
    Ok(ConvexMesh { vertices, polygons })
}

/// The shapes of a model: `shapes` from its header and `body`, the `B.` part.
pub fn read_shapes(shapes: &[PhysxShape], body: &[u8]) -> Result<Vec<Shape>> {
    let mut offset = 0usize;
    let mut out = Vec::with_capacity(shapes.len());
    for s in shapes {
        let f = |o: usize| f32::from_le_bytes(s.raw[o..o + 4].try_into().unwrap());
        // The record holds the pose after the type word: record[0..] here starts at the position
        let (translation, rotation) = ([f(0), f(4), f(8)], [f(16), f(20), f(24), f(28)]);
        let data = body
            .get(offset..offset + s.len as usize)
            .with_context(|| format!("PhysX data of {} bytes at {offset} runs past B.", s.len))?;
        offset += s.len as usize;
        let collider = match s.kind {
            0 => Collider::Empty,
            1 => Collider::Sphere { radius: f(32) },
            2 => Collider::Capsule {
                radius: f(32),
                half_height: f(36),
            },
            3 => Collider::Box {
                half_extents: [f(40), f(44), f(48)],
            },
            5 => Collider::Convex(parse_convex_mesh(data)?),
            6 => Collider::Triangles(parse_triangle_mesh(data)?),
            other => bail!("PhysX shape type {other} is not understood"),
        };
        out.push(Shape {
            translation,
            rotation,
            collider,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32s(out: &mut Vec<u8>, vals: &[f32]) {
        for v in vals {
            out.extend(v.to_le_bytes());
        }
    }

    fn u32s(out: &mut Vec<u8>, vals: &[u32]) {
        for v in vals {
            out.extend(v.to_le_bytes());
        }
    }

    /// A cooked triangle mesh of two triangles with 16-bit indices and materials.
    fn mesh_blob(flags: u32) -> Vec<u8> {
        let mut b = b"NXS\x01MESH".to_vec();
        u32s(&mut b, &[15, 0, flags, 4, 2]);
        f32s(
            &mut b,
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
        );
        let idx = [0u32, 1, 2, 0, 2, 3];
        for i in idx {
            if flags & SERIAL_8BIT != 0 {
                b.push(i as u8);
            } else if flags & SERIAL_16BIT != 0 {
                b.extend((i as u16).to_le_bytes());
            } else {
                b.extend(i.to_le_bytes());
            }
        }
        if flags & SERIAL_MATERIALS != 0 {
            b.extend([3, 0, 4, 0]);
        }
        b.extend([0xEE; 40]); // search structures, ignored
        b
    }

    #[test]
    fn reads_triangle_meshes() {
        for flags in [9, 5, 0, 8] {
            let m = parse_triangle_mesh(&mesh_blob(flags)).unwrap();
            assert_eq!(m.vertices.len(), 4);
            assert_eq!(m.indices, [0, 1, 2, 0, 2, 3]);
            assert_eq!(m.vertices[2], [1.0, 1.0, 0.0]);
            assert_eq!(
                m.materials,
                if flags & SERIAL_MATERIALS != 0 {
                    vec![3, 4]
                } else {
                    vec![]
                }
            );
        }
        // An index past the vertices, a wrong tag and a short blob fail
        let mut bad = mesh_blob(9);
        let at = 28 + 4 * 12;
        bad[at..at + 2].copy_from_slice(&9u16.to_le_bytes());
        assert!(parse_triangle_mesh(&bad).is_err());
        assert!(parse_triangle_mesh(&mesh_blob(9)[..40]).is_err());
        let mut other = mesh_blob(9);
        other[4..8].copy_from_slice(b"CVXM");
        assert!(parse_triangle_mesh(&other).is_err());
    }

    /// A cooked convex mesh: a square pyramid with a square base.
    fn convex_blob() -> Vec<u8> {
        let mut b = b"NXS\x01CVXM".to_vec();
        u32s(&mut b, &[13, 0]);
        b.extend(b"ICE\x01CLHL");
        // 5 vertices, 8 edges, 5 polygons, 16 indices
        u32s(&mut b, &[9, 5, 8, 5, 16]);
        f32s(
            &mut b,
            &[
                -1.0, 0.0, -1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 1.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0,
            ],
        );
        // Base then four sides; each polygon: plane, first index (u16), count, min index
        let polys: [(usize, u8); 5] = [(0, 4), (4, 3), (7, 3), (10, 3), (13, 3)];
        for (first, count) in polys {
            f32s(&mut b, &[0.0, -1.0, 0.0, 0.0]);
            b.extend((first as u16).to_le_bytes());
            b.extend([count, 0]);
        }
        b.extend([0, 1, 2, 3, 0, 1, 4, 1, 2, 4, 2, 3, 4, 3, 0, 4]);
        b.extend([0xEE; 30]);
        b
    }

    #[test]
    fn reads_convex_meshes() {
        let m = parse_convex_mesh(&convex_blob()).unwrap();
        assert_eq!(m.vertices.len(), 5);
        assert_eq!(m.polygons.len(), 5);
        assert_eq!(m.polygons[0].0, [0, 1, 2, 3]);
        assert_eq!(m.polygons[2].0, [1, 2, 4]);
        assert_eq!(m.polygons[4].0, [3, 0, 4]);
        assert!(parse_convex_mesh(&convex_blob()[..60]).is_err());
        assert!(parse_convex_mesh(&mesh_blob(9)).is_err());
    }

    fn shape(kind: u32, raw_len: usize, len: u32, set: &[(usize, f32)]) -> PhysxShape {
        let mut raw = vec![0u8; raw_len];
        for &(at, v) in set {
            raw[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        PhysxShape {
            kind,
            raw,
            vector1: [0.0; 4],
            vector2: [0.0; 4],
            len,
        }
    }

    #[test]
    fn reads_shapes_in_header_order() {
        let mesh = mesh_blob(9);
        let convex = convex_blob();
        let shapes = [
            // box: position, rotation, half extents
            shape(
                3,
                0x50,
                0,
                &[
                    (0, 1.0),
                    (4, 2.0),
                    (8, 3.0),
                    (28, 1.0),
                    (40, 8.0),
                    (44, 2.0),
                    (48, 1.5),
                ],
            ),
            shape(6, 0x50, mesh.len() as u32, &[(28, 1.0)]),
            shape(1, 0x4C, 0, &[(28, 1.0), (32, 5.0)]),
            shape(5, 0x50, convex.len() as u32, &[(28, 1.0)]),
            shape(2, 0x50, 0, &[(28, 1.0), (32, 0.37), (36, 1.3)]),
            shape(0, 0x50, 0, &[]),
        ];
        let mut body = mesh.clone();
        body.extend(&convex);
        let out = read_shapes(&shapes, &body).unwrap();
        assert_eq!(out[0].translation, [1.0, 2.0, 3.0]);
        assert_eq!(out[0].rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(
            out[0].collider,
            Collider::Box {
                half_extents: [8.0, 2.0, 1.5]
            }
        );
        assert!(matches!(&out[1].collider, Collider::Triangles(m) if m.indices.len() == 6));
        assert_eq!(out[2].collider, Collider::Sphere { radius: 5.0 });
        assert!(matches!(&out[3].collider, Collider::Convex(m) if m.vertices.len() == 5));
        assert_eq!(
            out[4].collider,
            Collider::Capsule {
                radius: 0.37,
                half_height: 1.3
            }
        );
        assert_eq!(out[5].collider, Collider::Empty);
        // Data past the end of B. and unknown types fail
        assert!(read_shapes(&shapes, &body[..body.len() - 1]).is_err());
        assert!(read_shapes(&[shape(4, 0x50, 0, &[])], &[]).is_err());
    }
}
