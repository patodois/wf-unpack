//! Textures, split across `H.Texture`, `B.Texture` and `F.Texture`.
//!
//! The `H.` entry is a [`CommonHeader`] followed by (little-endian):
//!
//! ```text
//! u8  body_mips       number of mip levels stored in B.
//! u8  full_mips       number of mip levels stored in F.
//! u8  unknown
//! u8  format          see `Format::from_code`
//! u32 count
//! u32 unknown[count]  increasing values, one per level or so (offsets?)
//! u16 width, height             size of the top mip level
//! u16 body_width, body_height   size of the largest level in B. (0 if none)
//! ...                           more fields, not needed to read the pixels
//! ```
//!
//! The pixels are a mip chain split in two: `F.` holds the `full_mips`
//! largest levels and `B.` the `body_mips` levels after them. Each part
//! stores its levels smallest first. The chain does not always go down to
//! 1x1 (interface art often has a single level), and does not always start
//! at the size in the header: many textures ship without their top one or
//! two levels, in which case the largest level present is used.
//!
//! Arrays (type 166) and cubemaps (type 177) store several images and are
//! not handled yet.

use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

use crate::bytes::Reader;
use crate::cache::Package;
use crate::header::CommonHeader;

pub const KIND_ARRAY: u16 = 166;
pub const KIND_CUBEMAP: u16 = 177;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc6h,
    Bc7,
    Bgra8,
    A8,
    R16,
}

impl Format {
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 | 1 => Format::Bc1,
            2 => Format::Bc2,
            3 => Format::Bc3,
            6 => Format::Bc4,
            7 => Format::Bc5,
            10 => Format::Bgra8,
            12 => Format::A8,
            30 => Format::R16,
            34 => Format::Bc7,
            35 => Format::Bc6h,
            _ => return None,
        })
    }

    pub fn is_block_compressed(self) -> bool {
        !matches!(self, Format::Bgra8 | Format::A8 | Format::R16)
    }

    /// Bytes per 4x4 block, or per pixel for uncompressed formats.
    fn unit_size(self) -> usize {
        match self {
            Format::Bc1 | Format::Bc4 => 8,
            Format::Bc2 | Format::Bc3 | Format::Bc5 | Format::Bc6h | Format::Bc7 => 16,
            Format::Bgra8 => 4,
            Format::A8 => 1,
            Format::R16 => 2,
        }
    }

    /// Size in bytes of one image of the given dimensions.
    pub fn image_size(self, width: u32, height: u32) -> usize {
        let (w, h) = (width.max(1) as usize, height.max(1) as usize);
        if self.is_block_compressed() {
            w.div_ceil(4) * h.div_ceil(4) * self.unit_size()
        } else {
            w * h * self.unit_size()
        }
    }

    /// DXGI_FORMAT value for the DDS DX10 header.
    fn dxgi(self) -> u32 {
        match self {
            Format::Bc1 => 71,
            Format::Bc2 => 74,
            Format::Bc3 => 77,
            Format::Bc4 => 80,
            Format::Bc5 => 83,
            Format::Bc6h => 95,
            Format::Bc7 => 98,
            Format::Bgra8 => 87,
            Format::A8 => 65,
            Format::R16 => 56,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextureHeader {
    pub common: CommonHeader,
    pub body_mips: u8,
    pub full_mips: u8,
    pub unknown: u8,
    pub format_code: u8,
    pub format: Option<Format>,
    pub extra: Vec<u32>,
    pub width: u16,
    pub height: u16,
    pub body_width: u16,
    pub body_height: u16,
    /// Bytes after the fields above.
    pub rest_len: usize,
}

impl TextureHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let common = CommonHeader::parse(data).context("common header")?;
        let mut r = Reader::at(data, common.len)?;
        let body_mips = r.u8()?;
        let full_mips = r.u8()?;
        let unknown = r.u8()?;
        let format_code = r.u8()?;
        let count = r.u32()?;
        ensure!(
            count < 1024,
            "implausible texture header array size {count}"
        );
        let extra = (0..count).map(|_| r.u32()).collect::<Result<_>>()?;
        let width = r.u16()?;
        let height = r.u16()?;
        let body_width = r.u16()?;
        let body_height = r.u16()?;
        let rest_len = r.rest().len();
        let format = Format::from_code(format_code);
        Ok(TextureHeader {
            common,
            body_mips,
            full_mips,
            unknown,
            format_code,
            format,
            extra,
            width,
            height,
            body_width,
            body_height,
            rest_len,
        })
    }

    /// Byte sizes of the full mip chain, top level first.
    pub fn level_sizes(&self) -> Result<Vec<usize>> {
        let format = self
            .format
            .with_context(|| format!("unknown texture format code {}", self.format_code))?;
        let (w, h) = (u32::from(self.width), u32::from(self.height));
        ensure!(w > 0 && h > 0, "texture has zero size");
        let levels = 32 - w.max(h).leading_zeros();
        Ok((0..levels)
            .map(|l| format.image_size(w >> l, h >> l))
            .collect())
    }
}

pub struct Texture {
    pub header: TextureHeader,
    pub format: Format,
    /// Size of `mips[0]`; smaller than the header's when top levels are missing.
    pub width: u32,
    pub height: u32,
    /// Mip levels, largest first.
    pub mips: Vec<Vec<u8>>,
}

/// Where the levels of a texture are: `F.` holds `full` levels starting at
/// level `first` of the header's chain, `B.` the `body` levels after them.
#[derive(Debug, PartialEq, Eq)]
pub struct Layout {
    pub first: usize,
    pub full: usize,
    pub body: usize,
}

impl TextureHeader {
    /// Matches the header against the sizes of the `B.` and `F.` parts.
    pub fn layout(&self, body_len: usize, full_len: usize) -> Result<Layout> {
        let sizes = self.level_sizes()?;
        let (body, full) = (usize::from(self.body_mips), usize::from(self.full_mips));
        ensure!(body + full > 0, "texture has no mip levels");
        let first = (0..sizes.len())
            .find(|&k| {
                let fits = |from: usize, count: usize, len: usize| sizes.get(from..from + count).is_some_and(|s| s.iter().sum::<usize>() == len);
                fits(k, full, full_len) && fits(k + full, body, body_len)
            })
            .with_context(|| {
                let (w, h) = (self.width, self.height);
                format!("{full} F. levels ({full_len} bytes) + {body} B. levels ({body_len} bytes) fit no part of the {w}x{h} chain {sizes:?}")
            })?;
        Ok(Layout { first, full, body })
    }
}

/// Splits the `F.` and `B.` parts into levels, largest first.
fn split_levels(sizes: &[usize], layout: &Layout, body: &[u8], full: &[u8]) -> Vec<Vec<u8>> {
    let mut mips = Vec::with_capacity(layout.full + layout.body);
    for (part, levels) in [
        (full, &sizes[layout.first..layout.first + layout.full]),
        (body, &sizes[layout.first + layout.full..][..layout.body]),
    ] {
        // Stored smallest first: the largest level is at the end
        let mut end = part.len();
        for &size in levels {
            mips.push(part[end - size..end].to_vec());
            end -= size;
        }
    }
    mips
}

/// The three texture packages, opened once for loading many textures.
pub struct TexturePackages {
    head: Package,
    body: Option<Package>,
    full: Option<Package>,
}

impl TexturePackages {
    pub fn open(dir: &Path) -> Result<Self> {
        Self::open_family(dir, "Texture")
    }

    /// Opens `H.<family>`, `B.<family>` and `F.<family>`: `Texture`, or
    /// `LightMap` for the baked lighting of levels.
    pub fn open_family(dir: &Path, family: &str) -> Result<Self> {
        let head = Package::open_in(dir, &format!("H.{family}"))?
            .with_context(|| format!("no H.{family} package"))?;
        Ok(TexturePackages {
            head,
            body: Package::open_in(dir, &format!("B.{family}"))?,
            full: Package::open_in(dir, &format!("F.{family}"))?,
        })
    }

    pub fn contains(&self, path: &str) -> bool {
        self.head.toc.get(path).is_some()
    }

    pub fn load(&mut self, path: &str) -> Result<Texture> {
        let h = self
            .head
            .read_path(path)?
            .with_context(|| format!("{path} is not in H.Texture"))?;
        let header = TextureHeader::parse(&h)?;
        let part = |pkg: &mut Option<Package>| -> Result<Vec<u8>> {
            Ok(match pkg {
                Some(p) => p.read_path(path)?.unwrap_or_default(),
                None => Vec::new(),
            })
        };
        let body = part(&mut self.body)?;
        let full = part(&mut self.full)?;
        Texture::from_parts(header, &body, &full).with_context(|| path.to_string())
    }
}

impl Texture {
    pub fn from_parts(header: TextureHeader, body: &[u8], full: &[u8]) -> Result<Self> {
        if header.common.kind == KIND_ARRAY || header.common.kind == KIND_CUBEMAP {
            bail!(
                "texture arrays and cubemaps (type {}) are not supported yet",
                header.common.kind
            );
        }
        let layout = header.layout(body.len(), full.len())?;
        let sizes = header.level_sizes()?;
        let format = header.format.unwrap();
        let mips = split_levels(&sizes, &layout, body, full);
        let width = (u32::from(header.width) >> layout.first).max(1);
        let height = (u32::from(header.height) >> layout.first).max(1);
        Ok(Texture {
            header,
            format,
            width,
            height,
            mips,
        })
    }

    /// Loads a texture by path from a `Cache.Windows` folder.
    pub fn load(dir: &Path, path: &str) -> Result<Self> {
        TexturePackages::open(dir)?.load(path)
    }

    /// A DDS file with every mip level. Block-compressed formats up to BC5
    /// use the classic FourCC header; the rest use the DX10 extension.
    pub fn to_dds(&self) -> Vec<u8> {
        const DDSD_CAPS: u32 = 0x1;
        const DDSD_HEIGHT: u32 = 0x2;
        const DDSD_WIDTH: u32 = 0x4;
        const DDSD_PIXELFORMAT: u32 = 0x1000;
        const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
        const DDSD_LINEARSIZE: u32 = 0x8_0000;
        const DDPF_FOURCC: u32 = 0x4;
        const DDSCAPS_COMPLEX: u32 = 0x8;
        const DDSCAPS_TEXTURE: u32 = 0x1000;
        const DDSCAPS_MIPMAP: u32 = 0x40_0000;

        let fourcc = |s: &[u8; 4]| u32::from_le_bytes(*s);
        let four_cc = match self.format {
            Format::Bc1 => fourcc(b"DXT1"),
            Format::Bc2 => fourcc(b"DXT3"),
            Format::Bc3 => fourcc(b"DXT5"),
            Format::Bc4 => fourcc(b"ATI1"),
            Format::Bc5 => fourcc(b"ATI2"),
            _ => fourcc(b"DX10"),
        };
        let mut caps = DDSCAPS_TEXTURE;
        if self.mips.len() > 1 {
            caps |= DDSCAPS_COMPLEX | DDSCAPS_MIPMAP;
        }
        let data_len: usize = self.mips.iter().map(Vec::len).sum();
        let mut out = Vec::with_capacity(4 + 124 + 20 + data_len);
        let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());
        put(fourcc(b"DDS "));
        put(124);
        put(DDSD_CAPS
            | DDSD_HEIGHT
            | DDSD_WIDTH
            | DDSD_PIXELFORMAT
            | DDSD_MIPMAPCOUNT
            | DDSD_LINEARSIZE);
        put(self.height);
        put(self.width);
        put(self.mips[0].len() as u32);
        put(0); // depth
        put(self.mips.len() as u32);
        for _ in 0..11 {
            put(0);
        }
        // Pixel format: size, flags, FourCC, then bit count and masks (unused with FourCC)
        put(32);
        put(DDPF_FOURCC);
        put(four_cc);
        for _ in 0..5 {
            put(0);
        }
        put(caps);
        for _ in 0..4 {
            put(0);
        }
        if four_cc == fourcc(b"DX10") {
            put(self.format.dxgi());
            put(3); // D3D10_RESOURCE_DIMENSION_TEXTURE2D
            put(0);
            put(1); // array size
            put(0);
        }
        for mip in &self.mips {
            out.extend_from_slice(mip);
        }
        out
    }

    /// Decodes the top mip level to 8-bit RGBA (BC6H is clamped to [0, 1]).
    pub fn to_rgba(&self) -> Result<Vec<u8>> {
        let (w, h) = (self.width as usize, self.height as usize);
        let top = &self.mips[0];
        type Decoder = fn(&[u8], usize, usize, &mut [u32]) -> Result<(), &'static str>;
        let decoder: Option<Decoder> = match self.format {
            Format::Bc1 => Some(texture2ddecoder::decode_bc1),
            Format::Bc2 => Some(texture2ddecoder::decode_bc2),
            Format::Bc3 => Some(texture2ddecoder::decode_bc3),
            Format::Bc4 => Some(texture2ddecoder::decode_bc4),
            Format::Bc5 => Some(texture2ddecoder::decode_bc5),
            Format::Bc6h => Some(texture2ddecoder::decode_bc6_unsigned),
            Format::Bc7 => Some(texture2ddecoder::decode_bc7),
            _ => None,
        };
        let mut rgba = Vec::with_capacity(w * h * 4);
        if let Some(decode) = decoder {
            // The decoder works on whole blocks; pad to a multiple of 4 and crop
            let (bw, bh) = (w.div_ceil(4) * 4, h.div_ceil(4) * 4);
            let mut pixels = vec![0u32; bw * bh];
            decode(top, bw, bh, &mut pixels)
                .map_err(|e| anyhow::anyhow!("texture decoding: {e}"))?;
            for row in pixels.chunks_exact(bw).take(h) {
                for p in &row[..w] {
                    // The decoder yields BGRA in little-endian order
                    let [b, g, r, a] = p.to_le_bytes();
                    if self.format == Format::Bc4 {
                        // One channel: show it as grey
                        rgba.extend_from_slice(&[r, r, r, 255]);
                    } else {
                        rgba.extend_from_slice(&[r, g, b, a]);
                    }
                }
            }
            return Ok(rgba);
        }
        match self.format {
            Format::Bgra8 => {
                for px in top.as_chunks::<4>().0 {
                    rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            }
            Format::A8 => {
                for &a in top {
                    rgba.extend_from_slice(&[0xFF, 0xFF, 0xFF, a]);
                }
            }
            Format::R16 => {
                for c in top.as_chunks::<2>().0 {
                    let v = c[1]; // high byte of a 16-bit value
                    rgba.extend_from_slice(&[v, v, v, 0xFF]);
                }
            }
            _ => unreachable!(),
        }
        Ok(rgba)
    }

    pub fn to_png(&self) -> Result<Vec<u8>> {
        let rgba = self.to_rgba()?;
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(&rgba)?;
        writer.finish()?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::tests::build;

    fn header(format: u8, width: u16, height: u16, kind: u32) -> TextureHeader {
        let levels = (16 - width.max(height).leading_zeros()) as u8;
        header_mips(format, width, height, kind, levels - 1, 1)
    }

    fn header_mips(
        format: u8,
        width: u16,
        height: u16,
        kind: u32,
        body_mips: u8,
        full_mips: u8,
    ) -> TextureHeader {
        let mut raw = build(&["/Lotus/t.png"], "EnableStreaming=1\n", kind);
        raw.extend([body_mips, full_mips, 1, format]);
        raw.extend(1u32.to_le_bytes());
        raw.extend(0x2F89u32.to_le_bytes());
        raw.extend(width.to_le_bytes());
        raw.extend(height.to_le_bytes());
        raw.extend([0x80, 0, 0x80, 0]);
        TextureHeader::parse(&raw).unwrap()
    }

    #[test]
    fn parses_texture_header() {
        let h = header(3, 512, 256, 163);
        assert_eq!(h.format, Some(Format::Bc3));
        assert_eq!(
            (h.width, h.height, h.body_width, h.body_height, h.rest_len),
            (512, 256, 128, 128, 0)
        );
        assert_eq!((h.body_mips, h.full_mips), (9, 1));
        assert_eq!(h.extra, [0x2F89]);
        assert_eq!(h.common.kind, 163);
    }

    #[test]
    fn level_sizes() {
        // BC3 8x4: 2x1 blocks, 1x1 (4x2 rounds up), 1x1, 1x1
        assert_eq!(
            header(3, 8, 4, 163).level_sizes().unwrap(),
            [32, 16, 16, 16]
        );
        // BC1 non-square
        assert_eq!(
            header(1, 16, 4, 163).level_sizes().unwrap(),
            [32, 16, 8, 8, 8]
        );
        // Uncompressed
        assert_eq!(header(10, 4, 4, 163).level_sizes().unwrap(), [64, 16, 4]);
    }

    #[test]
    fn splits_body_and_full_into_levels() {
        // Uncompressed 4x4: levels of 64, 16 and 4 bytes, stored smallest first
        let small = [1u8; 4];
        let mid = [2u8; 16];
        let top = [3u8; 64];
        let body: Vec<u8> = small.iter().chain(&mid).copied().collect();
        let tex = Texture::from_parts(header(10, 4, 4, 163), &body, &top).unwrap();
        assert_eq!(tex.mips, [top.to_vec(), mid.to_vec(), small.to_vec()]);
        assert_eq!((tex.width, tex.height), (4, 4));
        // Everything in F.
        let full: Vec<u8> = body.iter().chain(&top).copied().collect();
        let tex = Texture::from_parts(header_mips(10, 4, 4, 163, 0, 3), &[], &full).unwrap();
        assert_eq!(tex.mips, [top.to_vec(), mid.to_vec(), small.to_vec()]);
        // A single level, no mips
        let tex = Texture::from_parts(header_mips(10, 4, 4, 163, 1, 0), &top, &[]).unwrap();
        assert_eq!((tex.width, tex.height, tex.mips.len()), (4, 4, 1));
        // Top level not shipped: the chain starts one level down
        let tex = Texture::from_parts(header_mips(10, 4, 4, 163, 2, 0), &body, &[]).unwrap();
        assert_eq!((tex.width, tex.height), (2, 2));
        assert_eq!(tex.mips, [mid.to_vec(), small.to_vec()]);
        // Sizes that fit no chain
        assert!(Texture::from_parts(header(10, 4, 4, 163), &[0; 5], &[]).is_err());
        assert!(
            Texture::from_parts(header(10, 4, 4, u32::from(KIND_CUBEMAP)), &body, &top).is_err()
        );
    }

    #[test]
    fn dds_header() {
        let tex = Texture::from_parts(header(34, 4, 4, 163), &[0; 32], &[0; 16]).unwrap();
        let dds = tex.to_dds();
        assert_eq!(&dds[..4], b"DDS ");
        assert_eq!(u32::from_le_bytes(dds[12..16].try_into().unwrap()), 4); // height
        assert_eq!(u32::from_le_bytes(dds[28..32].try_into().unwrap()), 3); // mip count
        assert_eq!(&dds[84..88], b"DX10");
        assert_eq!(u32::from_le_bytes(dds[128..132].try_into().unwrap()), 98); // BC7
        assert_eq!(dds.len(), 148 + 48);
    }

    #[test]
    fn rgba_from_bgra() {
        let mut top = Vec::new();
        for _ in 0..16 {
            top.extend([10, 20, 30, 40]);
        }
        let tex = Texture::from_parts(header(10, 4, 4, 163), &[0; 20], &top).unwrap();
        assert_eq!(&tex.to_rgba().unwrap()[..4], &[30, 20, 10, 40]);
    }
}
