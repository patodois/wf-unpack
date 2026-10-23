//! Bakes a material [`Plan`] into glTF metallic-roughness images.
//!
//! - base colour = albedo x layered tint. The albedo is the diffuse map, the
//!   grey albedo channel of a pack map, or `PS:DiffuseColor`. The tint is what
//!   the game's fill shaders compute (decoded from the DXBC): a neutral 0.5
//!   that each mask channel (R, then G, B, A) lerps towards its
//!   `PS:TintColor0..3`, then doubled, so a tint colour of 0.5 means x1. Tint,
//!   diffuse and emissive colour constants are linear values used as they are;
//!   textures are sRGB.
//! - metallic-roughness: glTF wants roughness in G and metalness in B.
//! - normal: the game's maps point green down (DirectX); glTF wants green up,
//!   so green is inverted and blue rebuilt from red and green.
//! - emissive: as the game computes it, the map (its red channel for
//!   single-channel maps) is squared, then multiplied by the emissive colour
//!   (`PS:EmissiveTintColor`, or a lerp from `Lo` to `Hi` by the squared map
//!   for multi-tint materials) and by `PS:EmissiveMapAtten`; the first two
//!   are baked into the texture (sRGB encoded) and the last is
//!   `KHR_materials_emissive_strength`.

use anyhow::{ensure, Result};

use std::path::Path;

use crate::cache::Package;
use crate::header::CommonHeader;
use crate::material::{self, MaskLayout, Material, Plan, Source};
use crate::packages::{PackagesBin, TextReader};
use crate::texture::{Texture, TexturePackages};

/// Resolves material `name` of the model at `model_path` and picks its
/// textures. The material's own properties may sit in its `H.Misc` entry's
/// attribute text.
pub fn plan_material(
    bin: &PackagesBin,
    texts: &mut TextReader,
    misc: &mut Package,
    texture_exists: impl Fn(&str) -> bool,
    model_path: &str,
    name: &str,
) -> Result<Plan> {
    let path = material::resolve_path(material::folder(model_path), name);
    let own = match misc.read_path(&path)? {
        Some(raw) => CommonHeader::parse(&raw)
            .map(|h| h.attributes)
            .unwrap_or_default(),
        None => String::new(),
    };
    let m = Material::resolve(bin, texts, &path, &own)?;
    Ok(Plan::new(&m, texture_exists))
}

/// Everything needed to plan and bake materials, opened once.
pub struct MaterialBaker {
    pub bin: PackagesBin,
    pub misc: Package,
    pub textures: TexturePackages,
}

impl MaterialBaker {
    pub fn open(game: &Path) -> Result<Self> {
        Ok(MaterialBaker {
            bin: PackagesBin::load(game)?,
            misc: Package::open(&game.join("H.Misc.toc"))?,
            textures: TexturePackages::open(game)?,
        })
    }

    pub fn plan(&mut self, model_path: &str, name: &str) -> Result<Plan> {
        let mut texts = self.bin.texts()?;
        let textures = &self.textures;
        plan_material(
            &self.bin,
            &mut texts,
            &mut self.misc,
            |t| textures.contains(t),
            model_path,
            name,
        )
    }

    pub fn bake(&mut self, plan: &Plan) -> Result<Baked> {
        bake(plan, &mut self.textures)
    }
}

/// An 8-bit RGBA image.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn from_texture(t: &Texture) -> Result<Self> {
        let rgba = t.to_rgba()?;
        ensure!(
            rgba.len() == t.width as usize * t.height as usize * 4,
            "decoded texture has the wrong size"
        );
        Ok(Image {
            width: t.width,
            height: t.height,
            rgba,
        })
    }

    pub fn new(width: u32, height: u32) -> Self {
        Image {
            width,
            height,
            rgba: vec![0; width as usize * height as usize * 4],
        }
    }

    /// Nearest-neighbour sample at pixel (x, y) of a `w` x `h` grid.
    fn sample(&self, x: u32, y: u32, w: u32, h: u32) -> [u8; 4] {
        let sx = (u64::from(x) * u64::from(self.width) / u64::from(w)) as usize;
        let sy = (u64::from(y) * u64::from(self.height) / u64::from(h)) as usize;
        let i = (sy * self.width as usize + sx) * 4;
        self.rgba[i..i + 4].try_into().unwrap()
    }

    fn put(&mut self, x: u32, y: u32, px: [u8; 4]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.rgba[i..i + 4].copy_from_slice(&px);
    }

    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(&self.rgba)?;
        writer.finish()?;
        Ok(out)
    }
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The tint multiplier the game's fill shaders build from the four mask
/// weights and `PS:TintColor0..3` (linear): start at 0.5, lerp towards each
/// colour by its mask in turn (0, 1, 2, 3), double, and keep the vector's
/// length at 0.04 or more. A tint colour of 0.5 therefore means "unchanged".
pub fn layered_tint(mask: [u8; 4], tints: &[[f32; 3]; 4]) -> [f32; 3] {
    let mut t = [0.5f32; 3];
    for (i, tint) in tints.iter().enumerate() {
        let w = f32::from(mask[i]) / 255.0;
        for k in 0..3 {
            t[k] += (tint[k] - t[k]) * w;
        }
    }
    let v = t.map(|c| 2.0 * c + 1e-4);
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|c| c / len * len.max(0.04))
}

/// The four tint layer weights at pixel (x, y) of a `w` x `h` output.
pub fn mask_at(mask: &Image, layout: MaskLayout, x: u32, y: u32, w: u32, h: u32) -> [u8; 4] {
    match layout {
        MaskLayout::Channels => mask.sample(x, y, w, h),
        MaskLayout::PackAlphaQuadrants => {
            let (hw, hh) = ((mask.width / 2).max(1), (mask.height / 2).max(1));
            let qx = (u64::from(x) * u64::from(hw) / u64::from(w)) as u32;
            let qy = (u64::from(y) * u64::from(hh) / u64::from(h)) as u32;
            std::array::from_fn(|c| {
                let (ox, oy) = ((c as u32 % 2) * hw, (c as u32 / 2) * hh);
                let (sx, sy) = (
                    (ox + qx).min(mask.width - 1),
                    (oy + qy).min(mask.height - 1),
                );
                mask.rgba[(sy as usize * mask.width as usize + sx as usize) * 4 + 3]
            })
        }
    }
}

/// The game's emission fade: `smoothstep(sat(10 * (1.1 * cutoff - v)))`; 1 for every map value when `cutoff` is 1.
pub fn emissive_fade(v: f32, cutoff: f32) -> f32 {
    let t = (10.0 * (1.1 * cutoff - v)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Metalness from a specular level (sRGB 0..1): dielectrics sit near 4 % reflectance.
pub fn metal_from_specular(spec: f32) -> f32 {
    ((srgb_to_linear(spec) - 0.04) / 0.96).clamp(0.0, 1.0)
}

/// Converts a DirectX-style normal map pixel (green down) to glTF's convention.
pub fn convert_normal(px: [u8; 4]) -> [u8; 4] {
    let x = f32::from(px[0]) / 255.0 * 2.0 - 1.0;
    let y = -(f32::from(px[1]) / 255.0 * 2.0 - 1.0);
    let z = (1.0 - x * x - y * y).max(0.0).sqrt();
    [px[0], 255 - px[1], to_u8(z * 0.5 + 0.5), 255]
}

/// glTF material inputs: images plus the factors they are multiplied by.
pub struct Baked {
    pub base_color: Option<Image>,
    /// Linear RGBA, as glTF's baseColorFactor.
    pub base_color_factor: [f32; 4],
    pub metallic_roughness: Option<Image>,
    pub metallic_factor: f32,
    pub roughness_factor: f32,
    pub normal: Option<Image>,
    pub emissive: Option<Image>,
    pub emissive_factor: [f32; 3],
    /// `KHR_materials_emissive_strength`: `PS:EmissiveMapAtten`.
    pub emissive_strength: f32,
}

/// Loads each texture once.
struct Loader<'a> {
    packages: &'a mut TexturePackages,
    cache: Vec<(String, Image)>,
}

impl Loader<'_> {
    fn get(&mut self, path: &str) -> Result<Image> {
        if let Some((_, img)) = self.cache.iter().find(|(p, _)| p == path) {
            return Ok(img.clone());
        }
        let img = Image::from_texture(&self.packages.load(path)?)?;
        self.cache.push((path.to_string(), img.clone()));
        Ok(img)
    }

    fn source(&mut self, s: &Source) -> Result<Option<Image>> {
        match s {
            Source::Channel(p, _) | Source::Rgb(p) => self.get(p).map(Some),
            Source::Constant(_) => Ok(None),
        }
    }
}

fn read(src: &Source, img: Option<&Image>, x: u32, y: u32, w: u32, h: u32) -> f32 {
    match (src, img) {
        (Source::Channel(_, c), Some(i)) => f32::from(i.sample(x, y, w, h)[*c]) / 255.0,
        (Source::Rgb(_), Some(i)) => f32::from(i.sample(x, y, w, h)[0]) / 255.0,
        (Source::Constant(v), _) => *v,
        _ => 0.0,
    }
}

fn largest<'a>(images: impl IntoIterator<Item = Option<&'a Image>>) -> Option<(u32, u32)> {
    images
        .into_iter()
        .flatten()
        .map(|i| (i.width, i.height))
        .max_by_key(|&(w, h)| u64::from(w) * u64::from(h))
}

pub fn bake(plan: &Plan, packages: &mut TexturePackages) -> Result<Baked> {
    let mut loader = Loader {
        packages,
        cache: Vec::new(),
    };

    // Base colour and metallic-roughness
    let albedo = loader.source(&plan.albedo)?;
    let tint = match &plan.tint {
        Some(t) => Some((
            loader.get(&t.mask)?,
            t.layout,
            t.colors.map(|c| [c[0], c[1], c[2]]),
        )),
        None => None,
    };
    let rough_img = loader.source(&plan.roughness)?;
    let metal_img = loader.source(&plan.metalness)?;
    let metal_at = |x: u32, y: u32, w: u32, h: u32| -> f32 {
        let v = read(&plan.metalness, metal_img.as_ref(), x, y, w, h);
        if plan.specular_workflow && !matches!(plan.metalness, Source::Constant(_)) {
            metal_from_specular(v)
        } else {
            v
        }
    };
    let diffuse_lin = [
        plan.diffuse_color[0],
        plan.diffuse_color[1],
        plan.diffuse_color[2],
    ];
    // In the specular workflow metals take their colour from the specular map
    let spec_tints_base = plan.specular_workflow && metal_img.is_some();
    let base_inputs = [
        albedo.as_ref(),
        tint.as_ref().map(|t| &t.0),
        if spec_tints_base {
            metal_img.as_ref()
        } else {
            None
        },
    ];
    let (base_color, base_color_factor) = match largest(base_inputs) {
        // The colour constants are linear, like glTF's factor
        None => (None, [diffuse_lin[0], diffuse_lin[1], diffuse_lin[2], 1.0]),
        Some((w, h)) => {
            let mut out = Image::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    let a_lin = match (&plan.albedo, &albedo) {
                        (Source::Rgb(_), Some(i)) => {
                            let p = i.sample(x, y, w, h);
                            [0, 1, 2].map(|k| srgb_to_linear(f32::from(p[k]) / 255.0))
                        }
                        (Source::Channel(_, c), Some(i)) => {
                            [srgb_to_linear(f32::from(i.sample(x, y, w, h)[*c]) / 255.0); 3]
                        }
                        _ => diffuse_lin,
                    };
                    let t = tint.as_ref().map_or([1.0; 3], |(mask, layout, colors)| {
                        layered_tint(mask_at(mask, *layout, x, y, w, h), colors)
                    });
                    let mut c_lin = [0, 1, 2].map(|k| a_lin[k] * t[k]);
                    if spec_tints_base {
                        let s =
                            srgb_to_linear(read(&plan.metalness, metal_img.as_ref(), x, y, w, h));
                        let m = metal_at(x, y, w, h);
                        for k in 0..3 {
                            c_lin[k] += (s * t[k] - c_lin[k]) * m;
                        }
                    }
                    let c = c_lin.map(|v| to_u8(linear_to_srgb(v)));
                    out.put(x, y, [c[0], c[1], c[2], 255]);
                }
            }
            (Some(out), [1.0; 4])
        }
    };

    let (metallic_roughness, metallic_factor, roughness_factor) =
        match largest([rough_img.as_ref(), metal_img.as_ref()]) {
            None => (
                None,
                read(&plan.metalness, None, 0, 0, 1, 1),
                read(&plan.roughness, None, 0, 0, 1, 1),
            ),
            Some((w, h)) => {
                let mut out = Image::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        let r = read(&plan.roughness, rough_img.as_ref(), x, y, w, h);
                        out.put(x, y, [255, to_u8(r), to_u8(metal_at(x, y, w, h)), 255]);
                    }
                }
                (Some(out), 1.0, 1.0)
            }
        };

    // Normal
    let normal = match &plan.normal {
        Some(p) => {
            let mut img = loader.get(p)?;
            for px in img.rgba.as_chunks_mut::<4>().0 {
                *px = convert_normal(*px);
            }
            Some(img)
        }
        None => None,
    };

    // Emissive
    let (emissive, emissive_factor, emissive_strength) = match &plan.emissive {
        Some((p, single, hi)) => {
            let mut img = loader.get(p)?;
            let (lo, cutoff) = (plan.emissive_lo, plan.emissive_cutoff);
            for px in img.rgba.as_chunks_mut::<4>().0 {
                let e = f32::from(px[0]) / 255.0;
                let out = if *single {
                    let s = e * e;
                    if plan.multi_tint_emission {
                        // lerp(Lo, Hi, s) times s
                        [0, 1, 2].map(|k| s * (lo[k] + (hi[k] - lo[k]) * s))
                    } else {
                        let g = emissive_fade(s, cutoff);
                        [0, 1, 2].map(|k| s * hi[k] * g)
                    }
                } else {
                    let g = emissive_fade(f32::from(px[1]) / 255.0, cutoff);
                    [0, 1, 2].map(|k| f32::from(px[k]) / 255.0 * hi[k] * g)
                };
                *px = [
                    to_u8(linear_to_srgb(out[0].clamp(0.0, 1.0))),
                    to_u8(linear_to_srgb(out[1].clamp(0.0, 1.0))),
                    to_u8(linear_to_srgb(out[2].clamp(0.0, 1.0))),
                    255,
                ];
            }
            (Some(img), [1.0; 3], plan.emissive_atten)
        }
        None => (None, [0.0; 3], 1.0),
    };

    Ok(Baked {
        base_color,
        base_color_factor,
        metallic_roughness,
        metallic_factor,
        roughness_factor,
        normal,
        emissive,
        emissive_factor,
        emissive_strength,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trip() {
        for v in [0.0, 0.01, 0.2, 0.5, 0.9, 1.0] {
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5);
        }
    }

    #[test]
    fn tint_layers() {
        let tints = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.5, 0.5, 0.5],
        ];
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
        // No mask: 0.5 doubled is 1, the material's own colour
        assert!(near(layered_tint([0, 0, 0, 0], &tints), [1.0, 1.0, 1.0]));
        // A tint of 0.5 means "unchanged", 1.0 doubles
        assert!(near(layered_tint([255, 0, 0, 0], &tints), [2.0, 0.0, 0.0]));
        // Green over red, alpha over everything
        assert!(near(
            layered_tint([255, 255, 0, 0], &tints),
            [0.0, 2.0, 0.0]
        ));
        assert!(near(
            layered_tint([255, 255, 255, 255], &tints),
            [1.0, 1.0, 1.0]
        ));
        let half = layered_tint([255, 128, 0, 0], &tints);
        assert!((half[0] - 0.996).abs() < 0.01 && (half[1] - 1.004).abs() < 0.01);
        // Near black keeps at least a length of 0.04
        let dark = layered_tint([255, 0, 0, 0], &[[0.0; 3]; 4]);
        assert!((dark.iter().map(|c| c * c).sum::<f32>().sqrt() - 0.04).abs() < 1e-3);
    }

    #[test]
    fn emission_fade() {
        assert_eq!(emissive_fade(1.0, 1.0), 1.0);
        assert_eq!(emissive_fade(0.0, 0.5), 1.0);
        assert_eq!(emissive_fade(1.0, 0.5), 0.0);
        let mid = emissive_fade(0.5, 0.5);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn normal_conversion() {
        // Flat: (128, 128) -> blue near 255
        let n = convert_normal([128, 128, 0, 0]);
        assert_eq!((n[0], n[1]), (128, 127));
        assert!(n[2] >= 254);
        // DirectX green down (0) becomes glTF green up (255)
        assert_eq!(convert_normal([128, 0, 0, 0])[1], 255);
    }

    #[test]
    fn tint_mask_from_pack_alpha_quadrants() {
        // 4x4 pack map: alpha quadrants (2x2 each) hold R=255 (top-left), G=0 (top-right),
        // B=255 only in the bottom-right pixel of its quadrant, A=128 (bottom-right)
        let mut pack = Image::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                let a = match (x / 2, y / 2) {
                    (0, 0) => 255,
                    (1, 0) => 0,
                    (0, 1) => {
                        if (x, y) == (1, 3) {
                            255
                        } else {
                            0
                        }
                    }
                    _ => 128,
                };
                pack.put(x, y, [9, 9, 9, a]);
            }
        }
        // A 2x2 output maps each output pixel to one pixel per quadrant
        assert_eq!(
            mask_at(&pack, MaskLayout::PackAlphaQuadrants, 0, 0, 2, 2),
            [255, 0, 0, 128]
        );
        assert_eq!(
            mask_at(&pack, MaskLayout::PackAlphaQuadrants, 1, 1, 2, 2),
            [255, 0, 255, 128]
        );
        // The plain layout just reads the pixel
        assert_eq!(
            mask_at(&pack, MaskLayout::Channels, 3, 3, 4, 4),
            [9, 9, 9, 128]
        );
    }

    #[test]
    fn specular_to_metal() {
        assert_eq!(metal_from_specular(10.0 / 255.0), 0.0);
        assert!(metal_from_specular(1.0) > 0.99);
        let mid = metal_from_specular(0.7);
        assert!(mid > 0.4 && mid < 0.5);
    }

    #[test]
    fn nearest_sampling_scales() {
        let mut small = Image::new(2, 1);
        small.put(1, 0, [9, 9, 9, 9]);
        assert_eq!(small.sample(3, 0, 4, 1), [9, 9, 9, 9]);
        assert_eq!(small.sample(1, 0, 4, 1), [0, 0, 0, 0]);
    }
}
