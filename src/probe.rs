//! Reflection probes: the cube maps of levels (`<N>_cross.hdr` in
//! `H.LightMap` / `B.LightMap`, asset type 183).
//!
//! A zone names its probe in `ZoneAttribs.ReflectionMap`. The texture is a
//! BC6H (unsigned half-float RGB) cube map. The `B.` part stores its mip
//! levels smallest first, and one level is its six faces one after the other
//! (so the largest level, the last in the part, is six consecutive faces);
//! the header sizes are those of one face, e.g. 128 x 128 with 8 mips, 21,872
//! bytes per face and 131,232 for the six.
//!
//! Faces are in Direct3D's order, +X, -X, +Y, -Y, +Z, -Z, with Direct3D's
//! orientation: for a face texel with `u` to the right and `v` down, both in
//! -1..1 across the face, the direction is
//!
//! ```text
//! +X (1, -v, -u)   -X (-1, -v, u)   +Y (u, 1, v)   -Y (u, -1, -v)   +Z (u, -v, 1)   -Z (-u, -v, -1)
//! ```
//!
//! in the game's left-handed, y-up space. Checked on the dojo's probe: with
//! this convention the texels on both sides of all 12 cube edges agree
//! (mean difference of log luminance 0.07 against 0.44 to 0.59 for random
//! permutations of the faces), and flipping or turning any face makes it
//! worse. The values are linear radiance: the dojo's brightest texel is 1.74
//! and the brightest of all 8,688 probes of the game is 10,944; none is at the
//! half float maximum (65,504). Decoders that saturate the wrong endpoint value
//! (`texture2ddecoder` 0.1.2 does) show such texels, see `bc6h.rs`.
//!
//! An equirectangular version is produced by [`Probe::to_equirect`]
//! (azimuth from the +Z axis towards +X, polar angle from +Y), and
//! [`radiance_hdr`] writes it, or any float image, as a Radiance `.hdr` file.

use std::path::Path;

use anyhow::{ensure, Result};

use crate::bc6h::decode_bc6h;
use crate::texture::{CubeTexture, Format, TexturePackages};

/// The largest face edge [`Probe::from_texture`] decodes. The install's probes are 128 pixels
/// (8,304) or 256 (384); the limit keeps a damaged or crafted header from asking for gigabytes of
/// floats (six faces of 12 bytes a texel, and an equirectangular picture eight times that).
pub const MAX_FACE_SIZE: usize = 512;

/// A decoded probe: linear RGB floats per face.
#[derive(Debug, Clone)]
pub struct Probe {
    /// Edge length of a face in texels.
    pub size: usize,
    /// Faces in the order +X, -X, +Y, -Y, +Z, -Z; `size * size` texels each, rows top to bottom.
    pub faces: [Vec<[f32; 3]>; 6],
    /// Mip levels stored in the game files for the faces (the largest is decoded).
    pub mip_levels: usize,
}

pub const FACE_NAMES: [&str; 6] = ["px", "nx", "py", "ny", "pz", "nz"];

/// The direction a face texel looks along (not normalised); `u` right, `v` down, -1..1.
pub fn face_direction(face: usize, u: f32, v: f32) -> [f32; 3] {
    match face {
        0 => [1.0, -v, -u],
        1 => [-1.0, -v, u],
        2 => [u, 1.0, v],
        3 => [u, -1.0, -v],
        4 => [u, -v, 1.0],
        _ => [-u, -v, -1.0],
    }
}

/// The face a direction falls on and its `u`, `v` (-1..1).
pub fn direction_to_face(d: [f32; 3]) -> (usize, f32, f32) {
    let [x, y, z] = d;
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    if ax >= ay && ax >= az {
        if x > 0.0 {
            (0, -z / ax, -y / ax)
        } else {
            (1, z / ax, -y / ax)
        }
    } else if ay >= az {
        if y > 0.0 {
            (2, x / ay, z / ay)
        } else {
            (3, x / ay, -z / ay)
        }
    } else if z > 0.0 {
        (4, x / az, -y / az)
    } else {
        (5, -x / az, -y / az)
    }
}

impl Probe {
    pub fn from_texture(t: &CubeTexture) -> Result<Self> {
        ensure!(
            t.format == Format::Bc6h,
            "reflection probes are BC6H, this one is {:?}",
            t.format
        );
        let size = t.size as usize;
        ensure!(
            (1..=MAX_FACE_SIZE).contains(&size),
            "a probe face of {size} pixels is outside 1..={MAX_FACE_SIZE}"
        );
        ensure!(!t.mips.is_empty(), "the cube map has no mip levels");
        let top = &t.mips[0];
        let mut faces: [Vec<[f32; 3]>; 6] = Default::default();
        for (f, data) in top.iter().enumerate() {
            faces[f] = decode_bc6h(data, size, size, false)?;
        }
        Ok(Probe {
            size,
            faces,
            mip_levels: t.mips.len(),
        })
    }

    /// Loads a probe by its path (`/Lotus/Levels/ClanDojo/ClanObstacleCourseA/0_cross.hdr`).
    pub fn load(game: &Path, path: &str) -> Result<Self> {
        let mut pk = TexturePackages::open_family(game, "LightMap")?;
        Self::from_texture(&pk.load_cube(path)?)
    }

    /// The texel in direction `d`, nearest filtering.
    pub fn sample(&self, d: [f32; 3]) -> [f32; 3] {
        let (face, u, v) = direction_to_face(d);
        let n = self.size as f32;
        let px = (((u + 1.0) * 0.5 * n) as usize).min(self.size - 1);
        let py = (((v + 1.0) * 0.5 * n) as usize).min(self.size - 1);
        self.faces[face][py * self.size + px]
    }

    /// An equirectangular image of `width` x `height` texels, rows top (+Y) to bottom.
    pub fn to_equirect(&self, width: usize, height: usize) -> Vec<[f32; 3]> {
        let mut out = Vec::with_capacity(width * height);
        for y in 0..height {
            let theta = std::f32::consts::PI * (y as f32 + 0.5) / height as f32;
            for x in 0..width {
                let phi = std::f32::consts::TAU * ((x as f32 + 0.5) / width as f32 - 0.5);
                let d = [
                    theta.sin() * phi.sin(),
                    theta.cos(),
                    theta.sin() * phi.cos(),
                ];
                out.push(self.sample(d));
            }
        }
        out
    }

    /// An equirectangular picture in rewrite-bridge's convention (`Environment::Equirectangular`):
    /// for a view direction `d` of the bridge's space (+Y up, right-handed, x the game's x
    /// negated) the texel is at `u = 0.5 + atan2(d.x, -d.z) / 2 pi`, `v = 0.5 - asin(d.y) / pi`,
    /// rows top to bottom. The cube is sampled in the game's space, so each texel's direction has
    /// its x negated first.
    pub fn to_bridge_equirect(&self, width: usize, height: usize) -> Vec<[f32; 3]> {
        let mut out = Vec::with_capacity(width * height);
        for y in 0..height {
            let elevation = std::f32::consts::PI * (0.5 - (y as f32 + 0.5) / height as f32);
            for x in 0..width {
                let azimuth = std::f32::consts::TAU * ((x as f32 + 0.5) / width as f32 - 0.5);
                let c = elevation.cos();
                // The bridge direction is (c sin az, sin el, -c cos az)
                let bridge = [c * azimuth.sin(), elevation.sin(), -c * azimuth.cos()];
                out.push(self.sample([-bridge[0], bridge[1], bridge[2]]));
            }
        }
        out
    }

    /// Mean of all texels (linear) with each channel clamped to `limit`: a few
    /// texels of a lamp or sky are tens of thousands and would swamp a plain mean.
    pub fn mean_clamped(&self, limit: f32) -> [f32; 3] {
        let mut acc = [0.0f64; 3];
        let mut n = 0usize;
        for f in &self.faces {
            for p in f {
                for k in 0..3 {
                    acc[k] += f64::from(p[k].min(limit));
                }
                n += 1;
            }
        }
        acc.map(|a| (a / n.max(1) as f64) as f32)
    }

    /// Largest texel value below 65,000, so that a texel at the half-float maximum does not hide the rest.
    pub fn max_finite(&self) -> f32 {
        self.faces
            .iter()
            .flatten()
            .flatten()
            .copied()
            .filter(|v| *v < 65_000.0)
            .fold(0.0, f32::max)
    }
}

/// A Radiance RGBE (`.hdr`) file for linear RGB floats, run-length encoded as
/// the format's "new" scanlines. Values are clamped to the half-float maximum.
// Radiance stores each channel of a scanline as its own run-length stream
#[allow(clippy::needless_range_loop)]
pub fn radiance_hdr(width: usize, height: usize, pixels: &[[f32; 3]]) -> Result<Vec<u8>> {
    ensure!(
        pixels.len() == width * height,
        "pixel count does not match the size"
    );
    ensure!(
        (8..32768).contains(&width),
        "the run-length format needs a width of 8 to 32767"
    );
    let mut out =
        format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes();
    let mut scan = vec![[0u8; 4]; width];
    for row in pixels.chunks_exact(width) {
        for (dst, p) in scan.iter_mut().zip(row) {
            *dst = rgbe(*p);
        }
        out.extend([2, 2, (width >> 8) as u8, (width & 255) as u8]);
        for ch in 0..4 {
            let mut x = 0;
            while x < width {
                // A run of equal bytes of 4 or more, else a literal stretch
                let mut run = 1;
                while x + run < width && run < 127 && scan[x + run][ch] == scan[x][ch] {
                    run += 1;
                }
                if run >= 4 {
                    out.extend([128 + run as u8, scan[x][ch]]);
                    x += run;
                } else {
                    let mut end = x + 1;
                    while end < width && end - x < 128 {
                        let mut r = 1;
                        while end + r < width && r < 4 && scan[end + r][ch] == scan[end][ch] {
                            r += 1;
                        }
                        if r >= 4 {
                            break;
                        }
                        end += 1;
                    }
                    out.push((end - x) as u8);
                    out.extend((x..end).map(|i| scan[i][ch]));
                    x = end;
                }
            }
        }
    }
    Ok(out)
}

fn rgbe(p: [f32; 3]) -> [u8; 4] {
    let m = p[0].max(p[1]).max(p[2]).min(65_504.0);
    if m < 1e-32 || !m.is_finite() {
        return [0; 4];
    }
    // m = f * 2^e with f in [0.5, 1)
    let e = m.log2().floor() as i32 + 1;
    let scale = 256.0 / 2f32.powi(e);
    let enc = |v: f32| (v.clamp(0.0, 65_504.0) * scale).min(255.0) as u8;
    [enc(p[0]), enc(p[1]), enc(p[2]), (e + 128) as u8]
}

/// Reads a Radiance file written by [`radiance_hdr`] (or any run-length one) back to floats; for tests.
// Radiance stores each channel of a scanline as its own run-length stream
#[allow(clippy::needless_range_loop)]
pub fn read_radiance_hdr(data: &[u8]) -> Result<(usize, usize, Vec<[f32; 3]>)> {
    let end = data
        .windows(2)
        .position(|w| w == b"\n\n")
        .map(|i| i + 2)
        .ok_or_else(|| anyhow::anyhow!("no header end"))?;
    let rest = &data[end..];
    let nl = rest
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| anyhow::anyhow!("no size line"))?;
    let size = std::str::from_utf8(&rest[..nl])?;
    let parts: Vec<&str> = size.split_whitespace().collect();
    ensure!(
        parts.len() == 4 && parts[0] == "-Y" && parts[2] == "+X",
        "unsupported orientation {size}"
    );
    let (h, w): (usize, usize) = (parts[1].parse()?, parts[3].parse()?);
    let mut pos = end + nl + 1;
    let mut out = Vec::with_capacity(w * h);
    for _ in 0..h {
        ensure!(
            data.get(pos..pos + 4) == Some(&[2, 2, (w >> 8) as u8, (w & 255) as u8][..]),
            "not a run-length scanline"
        );
        pos += 4;
        let mut scan = vec![[0u8; 4]; w];
        for ch in 0..4 {
            let mut x = 0;
            while x < w {
                let c = *data.get(pos).ok_or_else(|| anyhow::anyhow!("truncated"))? as usize;
                pos += 1;
                if c > 128 {
                    let v = data[pos];
                    pos += 1;
                    for i in 0..c - 128 {
                        scan[x + i][ch] = v;
                    }
                    x += c - 128;
                } else {
                    for i in 0..c {
                        scan[x + i][ch] = data[pos + i];
                    }
                    pos += c;
                    x += c;
                }
            }
        }
        for p in scan {
            out.push(if p[3] == 0 {
                [0.0; 3]
            } else {
                let f = 2f32.powi(i32::from(p[3]) - 136);
                [
                    f32::from(p[0]) * f,
                    f32::from(p[1]) * f,
                    f32::from(p[2]) * f,
                ]
            });
        }
    }
    Ok((w, h, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directions_and_faces_round_trip() {
        for face in 0..6 {
            for (u, v) in [(0.0, 0.0), (0.5, -0.25), (-0.9, 0.9), (0.99, 0.01)] {
                let d = face_direction(face, u, v);
                let (f2, u2, v2) = direction_to_face(d);
                assert_eq!(f2, face, "face {face} ({u},{v})");
                assert!((u2 - u).abs() < 1e-5 && (v2 - v).abs() < 1e-5);
            }
        }
        // +X looks along +X with +Y at the top of the image
        assert_eq!(direction_to_face([1.0, 0.0, 0.0]).0, 0);
        assert_eq!(face_direction(0, 0.0, -1.0), [1.0, 1.0, 0.0]);
    }

    #[test]
    fn the_bridge_equirect_looks_at_the_game_cube_with_x_negated() {
        // One flat colour per face: the face index in the red channel
        let probe = Probe {
            size: 2,
            faces: std::array::from_fn(|f| vec![[f as f32, 0.0, 0.0]; 4]),
            mip_levels: 1,
        };
        let (w, h) = (8, 4);
        let eq = probe.to_bridge_equirect(w, h);
        let at = |u: f32, v: f32| eq[((v * h as f32) as usize) * w + (u * w as f32) as usize][0];
        // The middle of the picture is the bridge's -Z, the game's -Z (face 5)
        assert_eq!(at(0.5, 0.5), 5.0);
        // Turning right (toward the bridge's +X) is toward the game's -X (face 1)
        assert_eq!(at(0.75, 0.5), 1.0);
        // And left is the game's +X (face 0)
        assert_eq!(at(0.25, 0.5), 0.0);
        // The edges of the picture are the bridge's +Z, the game's +Z (face 4)
        assert_eq!(at(0.0, 0.5), 4.0);
        // The top row is up (+Y, face 2), the bottom row down (-Y, face 3)
        assert_eq!(at(0.5, 0.0), 2.0);
        assert_eq!(at(0.5, 0.99), 3.0);
    }

    #[test]
    fn a_huge_face_size_is_refused_before_anything_is_allocated() {
        use crate::header::tests::build;
        use crate::texture::TextureHeader;
        let mut raw = build(&["/Lotus/p.hdr"], "", 183);
        raw.extend([1, 0, 1, 35]);
        raw.extend(0u32.to_le_bytes());
        raw.extend(8u16.to_le_bytes());
        raw.extend(8u16.to_le_bytes());
        raw.extend([0, 0, 0, 0]);
        let header = TextureHeader::parse(&raw).unwrap();
        let body = vec![0u8; 6 * 64];
        let mut t = CubeTexture::from_parts(header, &body, &[]).unwrap();
        assert!(Probe::from_texture(&t).is_ok());
        t.size = 60_000;
        let e = Probe::from_texture(&t).unwrap_err().to_string();
        assert!(e.contains("outside 1..=512"), "{e}");
        t.size = 0;
        assert!(Probe::from_texture(&t).is_err());
    }

    #[test]
    fn splits_a_probe_into_levels_and_faces() {
        use crate::header::tests::build;
        use crate::texture::TextureHeader;
        // 8x8 BC6H probe, 4 levels (8, 4, 2, 1): face sizes 64, 16, 16, 16
        let mut raw = build(&["/Lotus/p.hdr"], "", 183);
        raw.extend([4, 0, 1, 35]);
        raw.extend(0u32.to_le_bytes());
        raw.extend(8u16.to_le_bytes());
        raw.extend(8u16.to_le_bytes());
        raw.extend([0, 0, 0, 0]);
        let header = TextureHeader::parse(&raw).unwrap();
        // Smallest level first; each level is six faces; face f is filled with the value f+1 plus 10 per level
        let mut body = Vec::new();
        for (lvl, size) in [(3u8, 16usize), (2, 16), (1, 16), (0, 64)] {
            for f in 0..6u8 {
                body.extend(std::iter::repeat_n(lvl * 10 + f + 1, size));
            }
        }
        let t = CubeTexture::from_parts(header, &body, &[]).unwrap();
        assert_eq!((t.size, t.mips.len()), (8, 4));
        assert_eq!(t.mips[0][2], vec![3u8; 64]);
        assert_eq!(t.mips[3][5], vec![36u8; 16]);
    }

    #[test]
    fn radiance_round_trip() {
        let (w, h) = (16, 3);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.push([
                    x as f32 * 0.25,
                    1.5 + y as f32,
                    if x < 8 { 0.0 } else { 100.0 },
                ]);
            }
        }
        let bytes = radiance_hdr(w, h, &px).unwrap();
        assert!(bytes.starts_with(b"#?RADIANCE"));
        let (w2, h2, back) = read_radiance_hdr(&bytes).unwrap();
        assert_eq!((w2, h2), (w, h));
        for (a, b) in px.iter().zip(&back) {
            for k in 0..3 {
                // RGBE keeps about 8 bits of the largest channel
                let tol = a[0].max(a[1]).max(a[2]) / 128.0 + 1e-6;
                assert!((a[k] - b[k]).abs() <= tol, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn equirect_samples_the_faces() {
        let size = 4;
        let mut faces: [Vec<[f32; 3]>; 6] = Default::default();
        for (f, face) in faces.iter_mut().enumerate() {
            *face = vec![[f as f32 + 1.0, 0.0, 0.0]; size * size];
        }
        let p = Probe {
            size,
            faces,
            mip_levels: 1,
        };
        let img = p.to_equirect(8, 4);
        // Top row looks up (+Y, face 2), the middle column of the middle rows looks along +Z (face 4)
        assert_eq!(img[0][0], 3.0);
        assert_eq!(img[8 + 4][0], 5.0);
        assert_eq!(p.sample([0.0, -1.0, 0.1])[0], 4.0);
    }
}
