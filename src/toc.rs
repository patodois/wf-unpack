//! Reader for `.toc` package indexes.
//!
//! Every package is a pair of files: `<Name>.toc` lists the entries and
//! `<Name>.cache` holds their data. The `.toc` layout (all little-endian):
//!
//! ```text
//! u32 magic        0x1867C64E
//! u32 version      20
//! entries          96 bytes each, until the end of the file:
//!   i64 offset       offset of the data in the .cache; -1 marks a directory
//!   i64 time         modification time (Windows FILETIME)
//!   i32 comp_len     size of the data in the .cache
//!   i32 len          size once decompressed (equal to comp_len when stored)
//!   i32 reserved
//!   i32 parent       index of the parent directory (0 = root)
//!   [u8; 64] name    NUL-padded
//! ```
//!
//! Directories are numbered in the order they appear, starting at 1 (0 is
//! the implicit root), so a full path is built by following `parent` back to
//! the root. Paths use `/` and start with one, e.g. `/Lotus/Interface/Foo.png`.

use std::collections::HashMap;

use anyhow::{ensure, Context, Result};

pub const MAGIC: u32 = 0x1867_C64E;
pub const VERSION: u32 = 20;
const HEADER_SIZE: usize = 8;
const ENTRY_SIZE: usize = 96;

pub struct Toc {
    /// Files in .toc order (directories are folded into the paths).
    pub entries: Vec<Entry>,
    /// Number of directories, not counting the root.
    pub dir_count: usize,
    by_path: HashMap<String, usize>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: String,
    pub offset: u64,
    pub time: i64,
    pub comp_len: u32,
    pub len: u32,
}

impl Entry {
    pub fn is_compressed(&self) -> bool {
        self.comp_len != self.len
    }
}

impl Toc {
    pub fn parse(raw: &[u8]) -> Result<Self> {
        ensure!(raw.len() >= HEADER_SIZE, "file too small for a .toc header");
        let magic = u32::from_le_bytes(raw[0..4].try_into().unwrap());
        ensure!(magic == MAGIC, "not a .toc file (magic {magic:#010x})");
        let version = u32::from_le_bytes(raw[4..8].try_into().unwrap());
        ensure!(
            version == VERSION,
            "unsupported .toc version {version} (expected {VERSION})"
        );
        let body = &raw[HEADER_SIZE..];
        ensure!(
            body.len().is_multiple_of(ENTRY_SIZE),
            "trailing {} bytes after the last entry",
            body.len() % ENTRY_SIZE
        );

        // dirs[i] is the full path of directory i; the root is the empty string
        let mut dirs = vec![String::new()];
        let mut entries = Vec::new();
        for (i, e) in body.as_chunks::<ENTRY_SIZE>().0.iter().enumerate() {
            let i64_at = |o: usize| i64::from_le_bytes(e[o..o + 8].try_into().unwrap());
            let i32_at = |o: usize| i32::from_le_bytes(e[o..o + 4].try_into().unwrap());
            let (offset, time, comp_len, len, parent) =
                (i64_at(0), i64_at(8), i32_at(16), i32_at(20), i32_at(28));
            let name_raw = &e[32..];
            let name_len = name_raw
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(name_raw.len());
            let name = String::from_utf8_lossy(&name_raw[..name_len]);
            let parent_path = usize::try_from(parent)
                .ok()
                .and_then(|p| dirs.get(p))
                .with_context(|| {
                    format!("entry {i} ({name}) has unknown parent directory {parent}")
                })?;
            let path = format!("{parent_path}/{name}");
            if offset < 0 {
                dirs.push(path);
                continue;
            }
            ensure!(
                comp_len >= 0 && len >= 0,
                "entry {i} ({path}) has a negative size"
            );
            entries.push(Entry {
                path,
                offset: offset as u64,
                time,
                comp_len: comp_len as u32,
                len: len as u32,
            });
        }
        let by_path = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.path.clone(), i))
            .collect();
        Ok(Toc {
            entries,
            dir_count: dirs.len() - 1,
            by_path,
        })
    }

    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.by_path.get(path).map(|&i| &self.entries[i])
    }
}

/// Matches a path against a glob: `?` is one character, `*` any run of
/// characters within one path segment and `**` any run including `/`.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match p {
            [] => s.is_empty(),
            [b'*', b'*', rest @ ..] => (0..=s.len()).any(|i| go(rest, &s[i..])),
            [b'*', rest @ ..] => {
                let segment = s.iter().position(|&c| c == b'/').unwrap_or(s.len());
                (0..=segment).any(|i| go(rest, &s[i..]))
            }
            [b'?', rest @ ..] => !s.is_empty() && s[0] != b'/' && go(rest, &s[1..]),
            [c, rest @ ..] => s.first() == Some(c) && go(rest, &s[1..]),
        }
    }
    go(pattern.as_bytes(), path.as_bytes())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One 96-byte .toc entry.
    pub fn entry(offset: i64, comp_len: i32, len: i32, parent: i32, name: &str) -> Vec<u8> {
        let mut e = Vec::with_capacity(ENTRY_SIZE);
        e.extend(offset.to_le_bytes());
        e.extend(1234i64.to_le_bytes());
        e.extend(comp_len.to_le_bytes());
        e.extend(len.to_le_bytes());
        e.extend(0i32.to_le_bytes());
        e.extend(parent.to_le_bytes());
        e.extend(name.as_bytes());
        e.resize(ENTRY_SIZE, 0);
        e
    }

    pub fn toc(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend(MAGIC.to_le_bytes());
        raw.extend(VERSION.to_le_bytes());
        for e in entries {
            raw.extend(e);
        }
        raw
    }

    #[test]
    fn builds_full_paths() {
        let raw = toc(&[
            entry(-1, 0, 0, 0, "Lotus"),
            entry(0, 10, 10, 1, "a.txt"),
            entry(-1, 0, 0, 1, "Interface"),
            entry(10, 5, 20, 2, "b.png"),
            entry(15, 3, 3, 0, "top"),
        ]);
        let toc = Toc::parse(&raw).unwrap();
        assert_eq!(toc.dir_count, 2);
        let paths: Vec<_> = toc.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["/Lotus/a.txt", "/Lotus/Interface/b.png", "/top"]);
        let b = toc.get("/Lotus/Interface/b.png").unwrap();
        assert_eq!((b.offset, b.comp_len, b.len, b.time), (10, 5, 20, 1234));
        assert!(b.is_compressed());
        assert!(!toc.get("/top").unwrap().is_compressed());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Toc::parse(&[0; 4]).is_err());
        let mut raw = toc(&[]);
        raw[0] ^= 1;
        assert!(Toc::parse(&raw).is_err());
        // parent 5 does not exist
        assert!(Toc::parse(&toc(&[entry(0, 1, 1, 5, "x")])).is_err());
        // truncated entry
        let mut raw = toc(&[entry(0, 1, 1, 0, "x")]);
        raw.pop();
        assert!(Toc::parse(&raw).is_err());
    }

    #[test]
    fn globs() {
        assert!(glob_match("/Lotus/*.png", "/Lotus/a.png"));
        assert!(!glob_match("/Lotus/*.png", "/Lotus/x/a.png"));
        assert!(glob_match("/Lotus/**.png", "/Lotus/x/a.png"));
        assert!(glob_match("/Lotus/**/a.png", "/Lotus/x/y/a.png"));
        assert!(glob_match("/a?c", "/abc"));
        assert!(!glob_match("/a?c", "/a/c"));
        assert!(glob_match("/exact", "/exact"));
        assert!(!glob_match("/exact", "/exactly"));
    }
}
