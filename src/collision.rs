//! Collision shapes as triangles, for physics or debug drawing in another engine.
//!
//! [`crate::physx`] reads a model's shapes: cooked triangle and convex meshes and the
//! primitive records (sphere, capsule, box). A physics engine can use the meshes as they
//! are; [`shape_mesh`] gives every kind a triangle list, so one code path serves them all:
//!
//! | shape | triangles |
//! |---|---|
//! | triangle mesh | the cooked mesh as is |
//! | convex mesh | each polygon as a fan of triangles |
//! | box | the 12 triangles of its 8 corners |
//! | sphere | a UV sphere of 10 rings and 16 segments (an approximation) |
//! | capsule | the same with the poles moved out by the half height, along the shape's local x axis, as PhysX lays capsules out (an approximation) |
//!
//! Vertices are in the model's space (the shape's own translation and rotation applied),
//! in the game's left-handed space, with the cooked winding. The caller mirrors x and
//! swaps two corners of each triangle for a right-handed space, as [`crate::scene`] does.
//! A shape of type 0 stores no size and gives `None`.
//!
//! The primitives keep their exact parameters in [`ShapeMesh::kind`] so an engine that has
//! real boxes, spheres and capsules can use them instead of the tessellation.

use crate::physx::{Collider, Shape};

/// One shape as triangles.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeMesh {
    /// `triangles`, `convex`, or the primitive with its parameters:
    /// `box half_extents=[1,2,3]`, `sphere radius=0.5`, `capsule radius=0.5 half_height=1`.
    pub kind: String,
    pub vertices: Vec<[f32; 3]>,
    /// Three indices per triangle.
    pub indices: Vec<u32>,
}

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    // v' = v + 2 w (u x v) + 2 u x (u x v) with u = (x, y, z); q is normalised first
    let n = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    let [x, y, z, w] = if n > 0.0 {
        q.map(|c| c / n)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    let u = [x, y, z];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(u, v);
    let t = [2.0 * t[0], 2.0 * t[1], 2.0 * t[2]];
    let c = cross(u, t);
    std::array::from_fn(|k| v[k] + w * t[k] + c[k])
}

/// A sphere, or a capsule along the local x axis (PhysX's convention) when `half_height` > 0.
///
/// Rings run from the pole at +x to the pole at -x. The upper hemisphere's rings sit `half_height`
/// out along +x, the lower one's `half_height` out along -x, and both have an equator ring, so
/// the band between the two equators is the straight wall of the capsule. A sphere has a single
/// equator.
pub(crate) fn uv_sphere(
    radius: f32,
    half_height: f32,
    place: &dyn Fn([f32; 3]) -> [f32; 3],
) -> (Vec<[f32; 3]>, Vec<u32>) {
    const HALF_RINGS: u32 = 5;
    const SEGMENTS: u32 = 16;
    let quarter = std::f32::consts::FRAC_PI_2;
    let mut rings: Vec<(f32, f32)> = Vec::new(); // polar angle from +x, shift along x
    for i in 0..=HALF_RINGS {
        rings.push((quarter * i as f32 / HALF_RINGS as f32, half_height));
    }
    for i in 0..=HALF_RINGS {
        if i == 0 && half_height == 0.0 {
            continue;
        }
        rings.push((
            quarter + quarter * i as f32 / HALF_RINGS as f32,
            -half_height,
        ));
    }
    let mut verts = Vec::new();
    for &(phi, shift) in &rings {
        // The poles are on the x axis
        let (x, rr) = (phi.cos() * radius + shift, phi.sin() * radius);
        for seg in 0..SEGMENTS {
            let a = std::f32::consts::TAU * seg as f32 / SEGMENTS as f32;
            verts.push(place([x, rr * a.cos(), rr * a.sin()]));
        }
    }
    let mut idx = Vec::new();
    for ring in 0..rings.len() as u32 - 1 {
        for seg in 0..SEGMENTS {
            let (a, b) = (
                ring * SEGMENTS + seg,
                ring * SEGMENTS + (seg + 1) % SEGMENTS,
            );
            let (c, e) = (a + SEGMENTS, b + SEGMENTS);
            idx.extend([a, b, c, b, e, c]);
        }
    }
    (verts, idx)
}

/// The triangles of one shape in the model's space, or `None` for a shape without a size.
pub fn shape_mesh(s: &Shape) -> Option<ShapeMesh> {
    let place = |p: [f32; 3]| -> [f32; 3] {
        let r = rotate(s.rotation, p);
        std::array::from_fn(|k| r[k] + s.translation[k])
    };
    let (kind, vertices, indices) = match &s.collider {
        Collider::Empty => return None,
        Collider::Triangles(m) => (
            "triangles".to_string(),
            m.vertices.iter().map(|&p| place(p)).collect(),
            m.indices.clone(),
        ),
        Collider::Convex(m) => {
            let mut idx = Vec::new();
            for (corners, _) in &m.polygons {
                for k in 1..corners.len().saturating_sub(1) {
                    idx.extend([corners[0], corners[k], corners[k + 1]]);
                }
            }
            (
                "convex".to_string(),
                m.vertices.iter().map(|&p| place(p)).collect(),
                idx,
            )
        }
        Collider::Box { half_extents: h } => {
            let verts: Vec<[f32; 3]> = (0..8)
                .map(|i| {
                    place([
                        if i & 1 == 0 { -h[0] } else { h[0] },
                        if i & 2 == 0 { -h[1] } else { h[1] },
                        if i & 4 == 0 { -h[2] } else { h[2] },
                    ])
                })
                .collect();
            let quads = [
                [0, 1, 3, 2],
                [4, 6, 7, 5],
                [0, 2, 6, 4],
                [1, 5, 7, 3],
                [0, 4, 5, 1],
                [2, 3, 7, 6],
            ];
            let idx = quads
                .iter()
                .flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]])
                .collect();
            (format!("box half_extents={h:?}"), verts, idx)
        }
        Collider::Sphere { radius } => {
            let (v, i) = uv_sphere(*radius, 0.0, &place);
            (format!("sphere radius={radius}"), v, i)
        }
        Collider::Capsule {
            radius,
            half_height,
        } => {
            let (v, i) = uv_sphere(*radius, *half_height, &place);
            (
                format!("capsule radius={radius} half_height={half_height}"),
                v,
                i,
            )
        }
    };
    Some(ShapeMesh {
        kind,
        vertices,
        indices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physx::{ConvexMesh, TriangleMesh};

    fn shape(collider: Collider) -> Shape {
        Shape {
            translation: [10.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            collider,
        }
    }

    #[test]
    fn a_box_has_twelve_triangles_around_its_pose() {
        let m = shape_mesh(&shape(Collider::Box {
            half_extents: [1.0, 2.0, 3.0],
        }))
        .unwrap();
        assert_eq!((m.vertices.len(), m.indices.len()), (8, 36));
        assert!(m
            .vertices
            .iter()
            .all(|p| (9.0..=11.0).contains(&p[0]) && p[1].abs() <= 2.0 && p[2].abs() <= 3.0));
        assert!(m.indices.iter().all(|&k| (k as usize) < m.vertices.len()));
        assert!(m.kind.starts_with("box half_extents=[1.0, 2.0, 3.0]"));
    }

    #[test]
    fn rotation_applies_before_translation() {
        // A quarter turn about z swaps the x and y extents of a box
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let s = Shape {
            rotation: [0.0, 0.0, q, q],
            translation: [0.0; 3],
            collider: Collider::Box {
                half_extents: [1.0, 2.0, 3.0],
            },
        };
        let m = shape_mesh(&s).unwrap();
        let max_x = m.vertices.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
        assert!((max_x - 2.0).abs() < 1e-5);
        // +x turns into +y
        assert!(rotate([0.0, 0.0, q, q], [1.0, 0.0, 0.0])[1] > 0.999);
    }

    #[test]
    fn spheres_and_capsules_are_tessellated_around_their_centre() {
        let m = shape_mesh(&shape(Collider::Sphere { radius: 2.0 })).unwrap();
        assert!(m.vertices.iter().all(|p| {
            (((p[0] - 10.0).powi(2) + p[1].powi(2) + p[2].powi(2)).sqrt() - 2.0).abs() < 1e-4
        }));
        assert!(m.indices.iter().all(|&k| (k as usize) < m.vertices.len()));
        // The capsule's poles sit half a height out along x
        let c = shape_mesh(&shape(Collider::Capsule {
            radius: 1.0,
            half_height: 3.0,
        }))
        .unwrap();
        let (lo, hi) = c
            .vertices
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
        assert!((lo - 6.0).abs() < 1e-4 && (hi - 14.0).abs() < 1e-4);
        assert!(c.kind.starts_with("capsule radius=1 half_height=3"));
    }

    #[test]
    fn meshes_keep_their_data_and_empty_shapes_give_nothing() {
        let tri = TriangleMesh {
            vertices: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            materials: Vec::new(),
        };
        let m = shape_mesh(&shape(Collider::Triangles(tri))).unwrap();
        assert_eq!(
            (m.kind.as_str(), m.indices.as_slice()),
            ("triangles", &[0, 1, 2][..])
        );
        assert_eq!(m.vertices[1], [11.0, 0.0, 0.0]);
        // A quad polygon becomes two triangles
        let cvx = ConvexMesh {
            vertices: vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            polygons: vec![(vec![0, 1, 2, 3], [0.0, 0.0, 1.0, 0.0])],
        };
        let m = shape_mesh(&shape(Collider::Convex(cvx))).unwrap();
        assert_eq!(
            (m.kind.as_str(), m.indices),
            ("convex", vec![0, 1, 2, 0, 2, 3])
        );
        assert!(shape_mesh(&shape(Collider::Empty)).is_none());
    }

    fn ident(p: [f32; 3]) -> [f32; 3] {
        p
    }

    /// Distance from the segment `(-h, 0, 0)..(h, 0, 0)`.
    fn axis_distance(p: [f32; 3], h: f32) -> f32 {
        let along = (p[0].abs() - h).max(0.0);
        (along * along + p[1] * p[1] + p[2] * p[2]).sqrt()
    }

    #[test]
    fn a_capsule_has_a_straight_wall_between_two_equators() {
        let (r, h) = (0.4f32, 0.7f32);
        let (verts, idx) = uv_sphere(r, h, &ident);
        // Every vertex is on the capsule's surface and the poles are at +-(h + r)
        for v in &verts {
            assert!((axis_distance(*v, h) - r).abs() < 1e-5, "{v:?}");
        }
        let (lo, hi) = verts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| {
            (lo.min(v[0]), hi.max(v[0]))
        });
        assert!((lo + h + r).abs() < 1e-5 && (hi - h - r).abs() < 1e-5);
        // A full-radius ring at each end of the wall: 16 vertices each, none in between
        let on_wall: Vec<&[f32; 3]> = verts
            .iter()
            .filter(|v| (v[1] * v[1] + v[2] * v[2]).sqrt() > r - 1e-5)
            .collect();
        assert_eq!(on_wall.len(), 32);
        assert_eq!(
            on_wall.iter().filter(|v| (v[0] - h).abs() < 1e-5).count(),
            16
        );
        assert_eq!(
            on_wall.iter().filter(|v| (v[0] + h).abs() < 1e-5).count(),
            16
        );
        // The wall's triangles are parallel to the axis: no vertex of a triangle with both
        // ends on the wall is off it
        let wall_tris = idx
            .chunks(3)
            .filter(|t| t.iter().all(|&i| on_wall.contains(&&verts[i as usize])))
            .count();
        assert_eq!(wall_tris, 32);
        assert!(idx.iter().all(|&i| (i as usize) < verts.len()));
        assert_eq!(idx.len() % 3, 0);
    }

    #[test]
    fn a_sphere_has_one_equator_and_no_flat_band() {
        let (verts, idx) = uv_sphere(2.0, 0.0, &ident);
        assert_eq!(verts.len(), 11 * 16);
        for v in &verts {
            assert!((v.iter().map(|c| c * c).sum::<f32>().sqrt() - 2.0).abs() < 1e-5);
        }
        // No zero-area triangles from a doubled ring
        let area = |t: &[u32]| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| verts[i as usize]);
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let x = [
                u[1] * w[2] - u[2] * w[1],
                u[2] * w[0] - u[0] * w[2],
                u[0] * w[1] - u[1] * w[0],
            ];
            x.iter().map(|c| c * c).sum::<f32>().sqrt() / 2.0
        };
        // The cap fans at the poles have a collapsed side, everything else has area
        let degenerate = idx.chunks(3).filter(|t| area(t) < 1e-6).count();
        assert_eq!(degenerate, 2 * 16);
    }
}
