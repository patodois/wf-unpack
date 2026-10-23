//! Baked lightmaps: what the files hold and how the pixel shaders decode them.
//!
//! # Where the data is
//!
//! A level object with `UseLightMap=1` (the CSG zone, most placed models)
//! carries
//!
//! ```text
//! LightMap={<N>_clr.png,<N>_dir.png}       the atlas page it is baked into (H.LightMap / B.LightMap / F.LightMap)
//! LightMapCoords={ox,oy,sx,sy}             its rectangle in the page, in 0..1
//! LightMapNormalize={nr,ng,nb,nw}          colour and luma scale
//! LightMapNormalize1={0,0,0,w1}            directional gain scale
//! IndexedLightMaps={{lights={...} lightMap=<N>_I_l1.png lightMapCoords={...} bakedFalloffMask=0},...}
//! ```
//!
//! The mesh's second UV set (`uv1`, [`crate::model::Vertex::uv1`], 0..1 over
//! the object) addresses the rectangle: `a = uv1 * (sx, sy) + (ox, oy)`, with
//! no flip of v (rows run top to bottom in the files, like the game's UVs).
//! The zone `CsgZone` of the dojo uses (0.00195, 0.00195, 0.496, 0.496) of a
//! 1024 x 1024 page, a trim 4 m long (0.00195, 0.377, 0.0586, 0.0586), i.e. 60
//! x 60 texels with a 2-texel margin.
//!
//! - `_clr` is BC3, 8 bits per channel: RGB is a chroma that is 1.0 in its
//!   largest channel almost everywhere, A is a luma that is stored squared
//!   (see below) and normalised so the brightest texel of an object is 1.0.
//! - `_dir` is BC5 (two channels) holding two pages side by side, each as tall
//!   as the whole image: the left page's R and G and the right page's R give a
//!   direction (x, y, z) as `2v - 1`, the right page's G is a directional gain
//!   (squared). In the game's files this texture is stored at half the
//!   resolution of `_clr` (`bakingOptions:DirectionMapMipBias=1`), and the
//!   top mip level named by its header is often not shipped, so a 2048 x 1024
//!   `_dir` is 1024 x 512 here (two 512 x 512 pages).
//! - Textures are clamped, not wrapped (`/EE/Types/GraphicsRes/LightMapColor`
//!   has `Clamp=CLAMP`).
//!
//! # The decode
//!
//! Recovered from the game's own pixel shaders (`ArchitecturalFillDeferred`
//! and `PBRFillDeferred`, option `_LIGHT_MAP`; the DXBC inside
//! `B.ShaderPermutationDx11` was decompressed and executed in an emulator, which
//! reproduced the formula below to 1e-7). For one pixel with (perturbed) world
//! normal `n`:
//!
//! ```text
//! a      = uv1 * LightMapCoords.zw + LightMapCoords.xy
//! clr    = B-spline(_clr, a)                          // filtered first
//! dL     = B-spline(_dir, (0.5 a.x,       a.y)).rg    // left page
//! dR     = B-spline(_dir, (0.5 a.x + 0.5, a.y)).rg    // right page
//! chroma = clr.rgb * LightMapNormalize.rgb
//! luma   = clr.a^2 * LightMapNormalize.w
//! d      = (2 dL.x - 1, 2 dL.y - 1, 2 dR.x - 1)       // |d| ~ 1 where lit
//! gain   = dR.y^2 * S                                 // S: see below
//! dw     = gain * (M d)
//! light  = (luma + max(dot(n, dw), 0)) * chroma       // linear radiance
//! diffuse output = light * albedo * (1 - metalness)
//! ```
//!
//! The B-spline is the cubic one built from four bilinear taps: per axis
//! `x = u * size - 0.5`, `f = frac(x)`, weights of the texels `floor(x) - 1 ..
//! floor(x) + 2` are `((1-f)^3, 3f^3 - 6f^2 + 4, -3f^3 + 3f^2 + 3f + 1, f^3) / 6`.
//! There is no sRGB or gamma curve on `chroma` in the shader (whether the
//! hardware view of the texture is sRGB is not visible in the bytecode; the
//! texture type `LightMapColor` has no `ColorSpace=CS_SRGB`, `LightMapLuma`
//! does), and the zone's `LightMapTint` and `PostProcessInfo.lightMapBoost`
//! scale the result outside this pixel shader.
//!
//! The directional part depends on the surface normal, so a texture cannot
//! hold it; the non-directional part, `luma * chroma`, can: it is
//! [`LightmapSample::ambient`].
//!
//! ## What is not known
//!
//! - `S` (the scalar the shader reads next to the matrix `M`) is **not**
//!   identified. It is not `LightMapNormalize1.w`: the shader constant table
//!   puts `LightMapNormalize1` in another slot, which the static-lightmap
//!   code only reads in its indexed-lightmap paths. `S` sits in the same
//!   constant block as `M`, which the table names `ProcRotation`; `M` is
//!   therefore the procedural-tile rotation (the identity in a fixed level
//!   like the dojo) and the directions are in level space, not object space.
//!   So [`LightmapSample::directional`] and [`LightmapSample::gain`] expose
//!   the raw `d` and `dR.y^2`, and [`LightmapSample::light`] takes the scale as
//!   an argument; the exports and renders apply the ambient term only.
//! - The indexed light maps (`_I_l1`) hold a bent normal in xyz and a
//!   visibility in w; they feed a sky-light spherical-harmonic term and are
//!   not decoded here.

use anyhow::{ensure, Result};

use crate::texture::{Format, Texture, TexturePackages};

/// One 8-bit RGBA image (a decoded lightmap texture).
#[derive(Debug, Clone)]
pub struct Atlas {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Atlas {
    pub fn from_texture(t: &Texture) -> Result<Self> {
        Ok(Atlas {
            width: t.width as usize,
            height: t.height as usize,
            rgba: t.to_rgba()?,
        })
    }

    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let x = x.clamp(0, self.width as i64 - 1) as usize;
        let y = y.clamp(0, self.height as i64 - 1) as usize;
        let i = (y * self.width + x) * 4;
        std::array::from_fn(|c| f32::from(self.rgba[i + c]) / 255.0)
    }

    /// Cubic B-spline filtering at `(u, v)` in 0..1 of this image, clamped at
    /// the edges (the filter the game's shaders apply to lightmaps).
    pub fn bspline(&self, u: f32, v: f32) -> [f32; 4] {
        let weights = |t: f32| {
            let f = t - t.floor();
            [
                (1.0 - f).powi(3) / 6.0,
                (3.0 * f.powi(3) - 6.0 * f * f + 4.0) / 6.0,
                (-3.0 * f.powi(3) + 3.0 * f * f + 3.0 * f + 1.0) / 6.0,
                f.powi(3) / 6.0,
            ]
        };
        let (x, y) = (u * self.width as f32 - 0.5, v * self.height as f32 - 0.5);
        let (wx, wy) = (weights(x), weights(y));
        let (ix, iy) = (x.floor() as i64, y.floor() as i64);
        let mut out = [0.0f32; 4];
        for (b, wb) in wy.iter().enumerate() {
            for (a, wa) in wx.iter().enumerate() {
                let t = self.texel(ix - 1 + a as i64, iy - 1 + b as i64);
                for c in 0..4 {
                    out[c] += wa * wb * t[c];
                }
            }
        }
        out
    }
}

/// The two textures of a lightmap page.
#[derive(Debug, Clone)]
pub struct LightmapPage {
    pub clr_path: String,
    pub dir_path: Option<String>,
    pub clr: Atlas,
    /// Two pages side by side; `None` when the level names no `_dir` texture.
    pub dir: Option<Atlas>,
}

impl LightmapPage {
    /// Loads `clr` (BC3) and `dir` (BC5) from `H.LightMap` and friends.
    pub fn load(pk: &mut TexturePackages, clr: &str, dir: Option<&str>) -> Result<Self> {
        let t = pk.load(clr)?;
        ensure!(t.format == Format::Bc3, "{clr} is {:?}, not BC3", t.format);
        let clr_atlas = Atlas::from_texture(&t)?;
        let dir_atlas = match dir {
            Some(p) => {
                let t = pk.load(p)?;
                ensure!(t.format == Format::Bc5, "{p} is {:?}, not BC5", t.format);
                Some(Atlas::from_texture(&t)?)
            }
            None => None,
        };
        Ok(LightmapPage {
            clr_path: clr.to_string(),
            dir_path: dir.map(str::to_string),
            clr: clr_atlas,
            dir: dir_atlas,
        })
    }
}

/// The decoded values at one point of an object, before the surface normal is known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightmapSample {
    /// `clr.rgb * LightMapNormalize.rgb`.
    pub chroma: [f32; 3],
    /// `clr.a^2 * LightMapNormalize.w`.
    pub luma: f32,
    /// `d = (2 dL.x - 1, 2 dL.y - 1, 2 dR.x - 1)`: the dominant light's direction
    /// (about unit length where there is one), before `M`. Zero without a `_dir` page.
    pub directional: [f32; 3],
    /// `dR.y^2`: the directional intensity before the unidentified scale `S`.
    pub gain: f32,
}

/// The constants one object feeds the shader.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightmapParams {
    pub coords: [f32; 4],
    pub normalize: [f32; 4],
    /// Kept as the level gives it; the static-lightmap shader code does not read it (see the module docs).
    pub normalize1: [f32; 4],
}

impl LightmapPage {
    /// Decodes the lightmap at mesh UV set 1 position `uv1`.
    pub fn sample(&self, p: &LightmapParams, uv1: [f32; 2]) -> LightmapSample {
        let a = [
            uv1[0] * p.coords[2] + p.coords[0],
            uv1[1] * p.coords[3] + p.coords[1],
        ];
        let clr = self.clr.bspline(a[0], a[1]);
        let chroma = [
            clr[0] * p.normalize[0],
            clr[1] * p.normalize[1],
            clr[2] * p.normalize[2],
        ];
        let luma = clr[3] * clr[3] * p.normalize[3];
        let (directional, gain) = match &self.dir {
            Some(dir) => {
                let l = dir.bspline(0.5 * a[0], a[1]);
                let r = dir.bspline(0.5 * a[0] + 0.5, a[1]);
                (
                    [2.0 * l[0] - 1.0, 2.0 * l[1] - 1.0, 2.0 * r[0] - 1.0],
                    r[1] * r[1],
                )
            }
            None => ([0.0; 3], 0.0),
        };
        LightmapSample {
            chroma,
            luma,
            directional,
            gain,
        }
    }
}

impl LightmapSample {
    /// The part of the light that does not depend on the surface normal.
    pub fn ambient(&self) -> [f32; 3] {
        self.chroma.map(|c| c * self.luma)
    }

    /// Linear radiance arriving at a surface with world normal `normal`, with
    /// the directional term scaled by `scale` (the unidentified `S`; 0 keeps
    /// only the ambient term) and the directions turned by `m` (row-major 3x3,
    /// the identity in a fixed level).
    pub fn light(&self, normal: [f32; 3], scale: f32, m: &[[f32; 3]; 3]) -> [f32; 3] {
        let d = self.directional;
        let g = self.gain * scale;
        let dw: [f32; 3] = std::array::from_fn(|i| g * (0..3).map(|j| m[i][j] * d[j]).sum::<f32>());
        let lambert = (0..3).map(|k| normal[k] * dw[k]).sum::<f32>().max(0.0);
        self.chroma.map(|c| c * (self.luma + lambert))
    }
}

/// Maps `a` to the page rectangle: where a uv1 coordinate lands in the atlas (0..1).
pub fn atlas_uv(coords: [f32; 4], uv1: [f32; 2]) -> [f32; 2] {
    [
        uv1[0] * coords[2] + coords[0],
        uv1[1] * coords[3] + coords[1],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 4]) -> Atlas {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend(f(x, y));
            }
        }
        Atlas {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn bspline_weights_sum_to_one_and_keep_constants() {
        let a = atlas(8, 8, |_, _| [100, 150, 200, 255]);
        for (u, v) in [(0.0, 0.0), (0.31, 0.77), (1.0, 1.0), (0.5, 0.5)] {
            let t = a.bspline(u, v);
            assert!((t[0] - 100.0 / 255.0).abs() < 1e-5);
            assert!((t[1] - 150.0 / 255.0).abs() < 1e-5);
            assert!((t[3] - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn bspline_is_smooth_between_texels() {
        // A step: left half 0, right half 255
        let a = atlas(8, 1, |x, _| if x < 4 { [0; 4] } else { [255; 4] });
        let at = |x: f32| a.bspline(x / 8.0, 0.5)[0];
        // Cubic B-spline of a step: 0.5 exactly on the edge between texels 3 and 4
        assert!((at(4.0) - 0.5).abs() < 1e-5);
        assert!(at(3.5) < 0.5 && at(3.5) > 0.0 && at(4.5) > 0.5 && at(4.5) < 1.0);
        // Never overshoots
        assert!((0..80).all(|i| {
            let v = at(i as f32 / 10.0);
            (-1e-6..=1.0 + 1e-6).contains(&v)
        }));
    }

    #[test]
    fn decode_follows_the_shader_formula() {
        // clr: uniform (0.5, 1.0, 0.25, 0.5); dir: left page (x=1, y=0.5), right page (z=0.5, gain 1)
        let clr = atlas(4, 4, |_, _| [128, 255, 64, 128]);
        let dir = atlas(8, 4, |x, _| {
            if x < 4 {
                [255, 128, 0, 255]
            } else {
                [128, 255, 0, 255]
            }
        });
        let page = LightmapPage {
            clr_path: "c".into(),
            dir_path: Some("d".into()),
            clr,
            dir: Some(dir),
        };
        let p = LightmapParams {
            coords: [0.0, 0.0, 1.0, 1.0],
            normalize: [2.0, 1.0, 4.0, 8.0],
            normalize1: [0.0, 0.0, 0.0, 3.0],
        };
        let s = page.sample(&p, [0.5, 0.5]);
        let q = |v: u8| f32::from(v) / 255.0;
        assert!((s.chroma[0] - q(128) * 2.0).abs() < 1e-4);
        assert!((s.chroma[2] - q(64) * 4.0).abs() < 1e-4);
        assert!((s.luma - q(128) * q(128) * 8.0).abs() < 1e-4);
        // d = (2*1-1, 2*0.502-1, 2*0.502-1), gain = 1 * 3
        let d = [1.0, 2.0 * q(128) - 1.0, 2.0 * q(128) - 1.0];
        for k in 0..3 {
            assert!((s.directional[k] - d[k]).abs() < 1e-3, "{k}");
        }
        assert!((s.gain - 1.0).abs() < 1e-6);
        // Facing the light: the directional term adds; facing away it does not
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let toward = s.light([1.0, 0.0, 0.0], 3.0, &id);
        let away = s.light([-1.0, 0.0, 0.0], 3.0, &id);
        assert!((toward[0] - s.chroma[0] * (s.luma + 3.0)).abs() < 1e-3);
        assert!((away[0] - s.chroma[0] * s.luma).abs() < 1e-5);
        assert_eq!(s.ambient()[1], s.chroma[1] * s.luma);
        // Rotating the object by 180 degrees about y turns x into -x
        let rot = [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]];
        let rotated = s.light([-1.0, 0.0, 0.0], 3.0, &rot);
        // Scale 0 leaves the ambient term
        assert!((s.light([1.0, 0.0, 0.0], 0.0, &id)[0] - s.ambient()[0]).abs() < 1e-6);
        assert!((rotated[0] - toward[0]).abs() < 1e-3);
    }

    #[test]
    fn atlas_rectangle() {
        assert_eq!(
            atlas_uv([0.25, 0.5, 0.125, 0.25], [1.0, 0.5]),
            [0.375, 0.625]
        );
    }
}
