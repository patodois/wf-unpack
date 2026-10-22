//! The common header at the start of every entry in the `H.*` packages.
//!
//! An asset is split across up to three packages under the same path:
//! `H.<Kind>` holds a small header, `B.<Kind>` the body and `F.<Kind>` the
//! full-quality data (for textures: the largest mip levels). Every `H.` entry
//! starts with this header (little-endian):
//!
//! ```text
//! [16] hash
//! u32 source_count
//! source_count x { u32 len, [len] path }   source files the asset was built from
//! u32 attr_len
//! [attr_len] attributes                     text, e.g. "EnableStreaming=1\n"
//! [1]  NUL                                  only when attr_len > 0
//! u32 type                                  low 16 bits: asset type (e.g. 163 = diffuse texture)
//! ```
//!
//! The type-specific header follows directly after.

use anyhow::{ensure, Result};

use crate::bytes::Reader;

#[derive(Debug, Clone)]
pub struct CommonHeader {
    pub hash: [u8; 16],
    pub sources: Vec<String>,
    pub attributes: String,
    /// Asset type, the low 16 bits of the type word.
    pub kind: u16,
    /// High 16 bits of the type word (meaning unknown).
    pub kind_high: u16,
    /// Size of the common header; the type-specific header starts here.
    pub len: usize,
}

impl CommonHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let hash = r.bytes(16)?.try_into().unwrap();
        let count = r.u32()?;
        ensure!(count < 4096, "implausible source path count {count}");
        let mut sources = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let len = r.u32()? as usize;
            sources.push(String::from_utf8_lossy(r.bytes(len)?).into_owned());
        }
        let attr_len = r.u32()? as usize;
        let attributes = String::from_utf8_lossy(r.bytes(attr_len)?).into_owned();
        if attr_len > 0 {
            r.skip(1)?;
        }
        let word = r.u32()?;
        Ok(CommonHeader {
            hash,
            sources,
            attributes,
            kind: word as u16,
            kind_high: (word >> 16) as u16,
            len: r.pos(),
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn build(sources: &[&str], attributes: &str, kind: u32) -> Vec<u8> {
        let mut h = vec![0xAB; 16];
        h.extend((sources.len() as u32).to_le_bytes());
        for s in sources {
            h.extend((s.len() as u32).to_le_bytes());
            h.extend(s.as_bytes());
        }
        h.extend((attributes.len() as u32).to_le_bytes());
        h.extend(attributes.as_bytes());
        if !attributes.is_empty() {
            h.push(0);
        }
        h.extend(kind.to_le_bytes());
        h
    }

    #[test]
    fn parses_header() {
        let raw = build(&["/Lotus/a.png"], "EnableStreaming=1\n", 0x0001_00A3);
        let h = CommonHeader::parse(&raw).unwrap();
        assert_eq!(h.sources, ["/Lotus/a.png"]);
        assert_eq!(h.attributes, "EnableStreaming=1\n");
        assert_eq!((h.kind, h.kind_high, h.len), (163, 1, raw.len()));
        // No attributes: no NUL byte
        let raw = build(&[], "", 184);
        assert_eq!(CommonHeader::parse(&raw).unwrap().len, raw.len());
        assert!(CommonHeader::parse(&raw[..raw.len() - 1]).is_err());
    }
}
