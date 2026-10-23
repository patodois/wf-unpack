//! `/Packages.bin` (an entry of `H.Misc`): the type tree of every asset and
//! its property text, e.g. the texture paths of a material.
//!
//! Layout of version 46 (little-endian):
//!
//! ```text
//! [16] hash
//! u32 20 (header size), u32 version, u32 1
//! u32 seed                      drives the junk lengths below
//! [junk(0xFF)]
//! u32 ref_count, ref_count x { u32 len, [len] type path, [2] }
//! u32 folder_count
//! [junk(0x3FF)]  u32 len, [len] flag bits
//! [junk(0x3FF)]  u32 len, [len] sizes
//! [junk(0x3FF)]  u32 len, [len] frames
//! u32 entity_count
//! [junk(0xF)]
//! folder_count x {
//!   u32 len, [len] folder        e.g. "/Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/"
//!   u8  unknown
//!   u32 count, count x {
//!     u32 len, [len] name
//!     [3] unknown
//!     u32 len, [len] parent      absolute, or relative to the folder; may be empty
//!   }
//! }
//! ```
//!
//! Junk lengths: a 64-bit LCG with Knuth's MMIX constants, seeded with the
//! u32 seed; each step is `state = state * 6364136223846793005 +
//! 1442695040888963407` and the length is `(state >> 32) & mask`.
//!
//! Flag bits are read LSB first: per entity one "has text" bit, followed by
//! an "is compressed" bit when it has text. The sizes buffer starts with the
//! u32 size of a zstd dictionary, which also starts the frames buffer; then
//! one LEB128 size per text, the texts following each other in the frames
//! buffer after the dictionary. A compressed text is a LEB128 decompressed
//! size and a zstd frame without its magic number, made with the dictionary.
//!
//! Learned from LotusLib (the `dev` branch has the version 45+ layout and the
//! junk generator); no code was copied.

use std::collections::HashMap;

use anyhow::{bail, ensure, Context, Result};
use zstd::dict::DecoderDictionary;
use zstd::zstd_safe::{DParameter, FrameFormat};

use crate::bytes::Reader;

pub struct PackagesBin {
    pub version: u32,
    /// The frames buffer (dictionary first).
    frames: Vec<u8>,
    dict: DecoderDictionary<'static>,
    entities: HashMap<String, Entity>,
}

#[derive(Debug, Clone)]
struct Entity {
    parent: String,
    text: Option<Text>,
}

#[derive(Debug, Clone, Copy)]
struct Text {
    start: usize,
    len: usize,
    compressed: bool,
}

/// Lengths of the junk blocks between sections.
struct Junk(u64);

impl Junk {
    fn next(&mut self, mask: u32) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 32) as u32 & mask) as usize
    }
}

/// Unsigned LEB128.
fn leb(data: &[u8], pos: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *data.get(*pos).context("LEB128 runs past the end")?;
        *pos += 1;
        v |= u64::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    bail!("LEB128 too long")
}

/// LSB-first bit reader.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn next(&mut self) -> Result<bool> {
        let byte = *self.data.get(self.pos / 8).context("flag bits run out")?;
        let bit = byte >> (self.pos % 8) & 1 == 1;
        self.pos += 1;
        Ok(bit)
    }
}

impl PackagesBin {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::at(data, 16)?;
        ensure!(r.u32()? == 20, "unexpected Packages.bin header size");
        let version = r.u32()?;
        ensure!(
            version >= 46,
            "Packages.bin version {version} is not supported (46 and later only)"
        );
        ensure!(r.u32()? == 1, "unexpected Packages.bin flags");
        let mut junk = Junk(u64::from(r.u32()?));

        r.skip(junk.next(0xFF))?;
        let refs = r.u32()?;
        ensure!(refs < 10_000, "implausible reference count {refs}");
        for _ in 0..refs {
            r.str32(4096)?;
            r.skip(2)?;
        }
        let folder_count = r.u32()?;

        fn buffer<'a>(r: &mut Reader<'a>, junk: &mut Junk) -> Result<&'a [u8]> {
            r.skip(junk.next(0x3FF))?;
            let len = r.u32()? as usize;
            r.bytes(len)
        }
        let flags = buffer(&mut r, &mut junk).context("flag buffer")?;
        let sizes = buffer(&mut r, &mut junk).context("size buffer")?;
        let frames = buffer(&mut r, &mut junk).context("frame buffer")?.to_vec();
        let entity_count = r.u32()? as usize;
        r.skip(junk.next(0xF))?;
        // An entity takes 11 bytes at the least: two empty strings (4 bytes each) and 3 more.
        // The map is sized from the count, so a corrupt one must not ask for gigabytes
        ensure!(
            entity_count <= r.peek().len() / 11,
            "{entity_count} entities do not fit in the {} bytes left",
            r.peek().len()
        );

        let dict_len = u32::from_le_bytes(
            sizes
                .get(..4)
                .context("size buffer too small")?
                .try_into()
                .unwrap(),
        ) as usize;
        ensure!(
            dict_len <= frames.len(),
            "dictionary larger than the frame buffer"
        );
        let dict = DecoderDictionary::copy(&frames[..dict_len]);

        let mut bits = Bits {
            data: flags,
            pos: 0,
        };
        let (mut size_pos, mut frame_pos) = (4usize, dict_len);
        let mut entities = HashMap::with_capacity(entity_count);
        let mut read = 0usize;
        for _ in 0..folder_count {
            let folder = r.str32(4096).context("folder name")?;
            r.skip(1)?;
            let count = r.u32()? as usize;
            ensure!(
                count <= entity_count - read,
                "folders hold more than {entity_count} entities"
            );
            for _ in 0..count {
                let name = r.str32(4096).context("entity name")?;
                r.skip(3)?;
                let parent = r.str32(4096).context("parent name")?;
                let text = if bits.next()? {
                    let len = usize::try_from(leb(sizes, &mut size_pos)?)
                        .ok()
                        .with_context(|| format!("text of {folder}{name} is too long"))?;
                    let compressed = bits.next()?;
                    // frame_pos never passes the frames: the dictionary fits in them and every
                    // text is checked here
                    ensure!(
                        len <= frames.len() - frame_pos,
                        "text of {folder}{name} runs past the frames"
                    );
                    let text = Text {
                        start: frame_pos,
                        len,
                        compressed,
                    };
                    frame_pos += len;
                    Some(text)
                } else {
                    None
                };
                let parent = if parent.is_empty() || parent.starts_with('/') {
                    parent
                } else {
                    format!("{folder}{parent}")
                };
                entities.insert(format!("{folder}{name}"), Entity { parent, text });
            }
            read += count;
        }
        ensure!(
            read == entity_count,
            "read {read} entities, the header says {entity_count}"
        );
        ensure!(r.rest().is_empty(), "bytes left after the entity list");
        ensure!(
            frame_pos == frames.len(),
            "{} frame bytes not claimed by any entity",
            frames.len() - frame_pos
        );
        Ok(PackagesBin {
            version,
            frames,
            dict,
            entities,
        })
    }

    /// Reads `/Packages.bin` from the `H.Misc` package of a `Cache.Windows` folder.
    pub fn load(game: &std::path::Path) -> Result<Self> {
        let mut pkg = crate::cache::Package::open(&game.join("H.Misc.toc"))?;
        let raw = pkg
            .read_path("/Packages.bin")?
            .context("/Packages.bin is not in H.Misc")?;
        PackagesBin::parse(&raw).context("parsing /Packages.bin")
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entities.contains_key(path)
    }

    /// All entity paths (in no particular order).
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entities.keys().map(String::as_str)
    }

    /// The entity this one derives from, if any.
    pub fn parent(&self, path: &str) -> Option<&str> {
        self.entities
            .get(path)
            .map(|e| e.parent.as_str())
            .filter(|p| !p.is_empty())
    }

    /// A reader for property texts, reusing one zstd context.
    pub fn texts(&self) -> Result<TextReader<'_>> {
        let mut dctx = zstd::bulk::Decompressor::with_prepared_dictionary(&self.dict)?;
        dctx.set_parameter(DParameter::Format(FrameFormat::Magicless))?;
        Ok(TextReader { bin: self, dctx })
    }
}

pub struct TextReader<'a> {
    bin: &'a PackagesBin,
    dctx: zstd::bulk::Decompressor<'a>,
}

impl TextReader<'_> {
    /// The property text of an entity, without what it inherits. `None` when
    /// the entity does not exist; empty when it has no text of its own.
    pub fn text(&mut self, path: &str) -> Result<Option<String>> {
        let Some(entity) = self.bin.entities.get(path) else {
            return Ok(None);
        };
        let Some(t) = entity.text else {
            return Ok(Some(String::new()));
        };
        let frame = &self.bin.frames[t.start..t.start + t.len];
        if !t.compressed {
            return Ok(Some(String::from_utf8_lossy(frame).into_owned()));
        }
        let mut pos = 0;
        let len = leb(frame, &mut pos)? as usize;
        ensure!(len <= 64 << 20, "implausible text size {len}");
        let out = self
            .dctx
            .decompress(&frame[pos..], len)
            .with_context(|| format!("zstd frame of {path}"))?;
        ensure!(
            out.len() == len,
            "text of {path} is {} bytes, expected {len}",
            out.len()
        );
        Ok(Some(String::from_utf8_lossy(&out).into_owned()))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn leb128() {
        let data = [0x05, 0xE5, 0x8E, 0x26, 0x80];
        let mut pos = 0;
        assert_eq!(leb(&data, &mut pos).unwrap(), 5);
        assert_eq!(leb(&data, &mut pos).unwrap(), 624_485);
        assert!(leb(&data, &mut pos).is_err());
    }

    #[test]
    fn junk_sequence() {
        // First junk length of the game's file (seed 0x1EAB77BA) is 197
        assert_eq!(Junk(0x1EAB_77BA).next(0xFF), 197);
        let mut j = Junk(1);
        let a: Vec<usize> = (0..4).map(|_| j.next(0xFFFF_FFFF)).collect();
        let mut k = Junk(1);
        assert_eq!(a, (0..4).map(|_| k.next(0xFFFF_FFFF)).collect::<Vec<_>>());
        assert!(a.windows(2).all(|w| w[0] != w[1]));
    }

    fn str32(out: &mut Vec<u8>, s: &str) {
        out.extend((s.len() as u32).to_le_bytes());
        out.extend(s.as_bytes());
    }

    /// A small Packages.bin with uncompressed texts, built field by field as documented.
    pub fn build(entities: &[(&str, &str, &str, Option<&str>)]) -> Vec<u8> {
        let seed = 12345u32;
        let mut junk = Junk(u64::from(seed));
        let mut out = vec![0u8; 16];
        for v in [20u32, 46, 1, seed] {
            out.extend(v.to_le_bytes());
        }
        out.resize(out.len() + junk.next(0xFF), 0xAA);
        out.extend(1u32.to_le_bytes());
        str32(&mut out, "/EE/Types/Base");
        out.extend([0, 0]);

        // Group by folder, keeping order
        type Folder<'a> = (&'a str, Vec<(&'a str, &'a str, Option<&'a str>)>);
        let mut folders: Vec<Folder> = Vec::new();
        for &(folder, name, parent, text) in entities {
            match folders.iter_mut().find(|f| f.0 == folder) {
                Some(f) => f.1.push((name, parent, text)),
                None => folders.push((folder, vec![(name, parent, text)])),
            }
        }
        out.extend((folders.len() as u32).to_le_bytes());

        let dict = b"not a real dictionary";
        let (mut flags, mut nbits) = (Vec::new(), 0usize);
        let mut push_bit = |flags: &mut Vec<u8>, b: bool| {
            if nbits % 8 == 0 {
                flags.push(0);
            }
            if b {
                *flags.last_mut().unwrap() |= 1 << (nbits % 8);
            }
            nbits += 1;
        };
        let mut sizes = (dict.len() as u32).to_le_bytes().to_vec();
        let mut frames = dict.to_vec();
        for (_, list) in &folders {
            for (_, _, text) in list {
                push_bit(&mut flags, text.is_some());
                if let Some(t) = text {
                    assert!(t.len() < 128);
                    sizes.push(t.len() as u8);
                    push_bit(&mut flags, false);
                    frames.extend(t.as_bytes());
                }
            }
        }
        for buf in [&flags, &sizes, &frames] {
            out.resize(out.len() + junk.next(0x3FF), 0xBB);
            out.extend((buf.len() as u32).to_le_bytes());
            out.extend(buf.iter());
        }
        out.extend((entities.len() as u32).to_le_bytes());
        out.resize(out.len() + junk.next(0xF), 0xCC);
        for (folder, list) in &folders {
            str32(&mut out, folder);
            out.push(1);
            out.extend((list.len() as u32).to_le_bytes());
            for (name, parent, _) in list {
                str32(&mut out, name);
                out.extend([0x81, 0, 1]);
                str32(&mut out, parent);
            }
        }
        out
    }

    #[test]
    fn parses_synthetic_file() {
        let raw = build(&[
            (
                "/Lotus/A/",
                "Mat",
                "Base",
                Some("DiffuseTexture=Mat_d.png\n"),
            ),
            ("/Lotus/A/", "Base", "/EE/Types/Base", None),
            ("/Lotus/B/", "Other", "", Some("X=1\n")),
        ]);
        let bin = PackagesBin::parse(&raw).unwrap();
        assert_eq!(bin.len(), 3);
        assert_eq!(bin.parent("/Lotus/A/Mat"), Some("/Lotus/A/Base"));
        assert_eq!(bin.parent("/Lotus/A/Base"), Some("/EE/Types/Base"));
        assert_eq!(bin.parent("/Lotus/B/Other"), None);
        let mut texts = bin.texts().unwrap();
        assert_eq!(
            texts.text("/Lotus/A/Mat").unwrap().unwrap(),
            "DiffuseTexture=Mat_d.png\n"
        );
        assert_eq!(texts.text("/Lotus/A/Base").unwrap().unwrap(), "");
        assert_eq!(texts.text("/Lotus/B/Other").unwrap().unwrap(), "X=1\n");
        assert!(texts.text("/Nope").unwrap().is_none());

        // A truncated file fails cleanly
        assert!(PackagesBin::parse(&raw[..raw.len() - 3]).is_err());
    }

    /// A file of three entities named A, B and C whose buffers are given as they are.
    fn build_with_buffers(flags: &[u8], sizes: &[u8], frames: &[u8], entity_count: u32) -> Vec<u8> {
        let seed = 12345u32;
        let mut junk = Junk(u64::from(seed));
        let mut out = vec![0u8; 16];
        for v in [20u32, 46, 1, seed] {
            out.extend(v.to_le_bytes());
        }
        out.resize(out.len() + junk.next(0xFF), 0xAA);
        out.extend(1u32.to_le_bytes());
        str32(&mut out, "/EE/Types/Base");
        out.extend([0, 0]);
        out.extend(1u32.to_le_bytes()); // one folder
        for buf in [flags, sizes, frames] {
            out.resize(out.len() + junk.next(0x3FF), 0xBB);
            out.extend((buf.len() as u32).to_le_bytes());
            out.extend(buf);
        }
        out.extend(entity_count.to_le_bytes());
        out.resize(out.len() + junk.next(0xF), 0xCC);
        str32(&mut out, "/F/");
        out.push(1);
        out.extend(3u32.to_le_bytes());
        for n in ["A", "B", "C"] {
            str32(&mut out, n);
            out.extend([0x81, 0, 1]);
            str32(&mut out, "");
        }
        out
    }

    #[test]
    fn a_text_length_that_wraps_is_an_error() {
        // Three uncompressed texts of 5, 2^64 - 3 and 8 bytes after a 5 byte dictionary: the
        // second one added to the position wrapped around to a small number
        let mut sizes = 5u32.to_le_bytes().to_vec();
        sizes.push(5);
        sizes.extend([0xFD, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
        sizes.push(8);
        let mut frames = b"dict0".to_vec();
        frames.extend([b'x'; 10]);
        let raw = build_with_buffers(&[0b0001_0101, 0], &sizes, &frames, 3);
        let result = std::panic::catch_unwind(|| PackagesBin::parse(&raw).map(|b| b.len()));
        let err = result.expect("parse panicked").err().expect("accepted");
        assert!(err.to_string().contains("runs past the frames"), "{err:#}");
    }

    #[test]
    fn an_entity_count_the_file_cannot_hold_is_an_error() {
        // The count sized a hash map before anything checked it against the file
        let mut sizes = 5u32.to_le_bytes().to_vec();
        sizes.extend([1, 1, 1]);
        let frames = b"dict0abc".to_vec();
        let raw = build_with_buffers(&[0b0001_0101, 0], &sizes, &frames, u32::MAX);
        let result = std::panic::catch_unwind(|| PackagesBin::parse(&raw).map(|b| b.len()));
        let err = result.expect("parse panicked").err().expect("accepted");
        assert!(err.to_string().contains("do not fit"), "{err:#}");
        // The same file with the right count is fine
        let ok = build_with_buffers(&[0b0001_0101, 0], &sizes, &frames, 3);
        assert_eq!(PackagesBin::parse(&ok).unwrap().len(), 3);
    }
}
