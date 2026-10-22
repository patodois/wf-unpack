//! Reading entries from `.cache` files.
//!
//! An entry is `comp_len` bytes at `offset` in the `.cache`. When
//! `comp_len == len` it is stored as is. Otherwise it is a run of blocks,
//! each with an 8-byte big-endian header:
//!
//! ```text
//! u32 a     bits 31..24 = 0x80 (marker), bits 25..2 = compressed size
//! u32 b     bits 28..5  = decompressed size, bits 3..0 = 1
//! payload   `compressed size` bytes
//! ```
//!
//! A payload equal in size to its output is stored; one starting with 0x8C
//! is an Oodle stream (Kraken and friends); anything else is LZF. An entry
//! whose first eight bytes are not a block header is a single unframed
//! block spanning the whole entry.
//!
//! Files are never read whole: `.cache` files run to tens of gigabytes, so
//! each entry is fetched with a seek and a read of exactly `comp_len` bytes.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};

use crate::toc::{Entry, Toc};

/// Largest decompressed entry we are willing to hold in memory.
pub const MAX_ENTRY_LEN: u32 = 1 << 30;

const OODLE_MAGIC: u8 = 0x8C;

/// An opened `.toc`/`.cache` pair.
pub struct Package {
    /// File name without extension, e.g. `H.Texture`.
    pub name: String,
    pub toc: Toc,
    cache: File,
    cache_len: u64,
}

impl Package {
    /// Opens `<name>.toc` and its `.cache` next to it.
    pub fn open(toc_path: &Path) -> Result<Self> {
        let raw =
            std::fs::read(toc_path).with_context(|| format!("reading {}", toc_path.display()))?;
        let toc = Toc::parse(&raw).with_context(|| format!("parsing {}", toc_path.display()))?;
        let cache_path = toc_path.with_extension("cache");
        let cache =
            File::open(&cache_path).with_context(|| format!("opening {}", cache_path.display()))?;
        let cache_len = cache.metadata()?.len();
        let name = toc_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        Ok(Package {
            name,
            toc,
            cache,
            cache_len,
        })
    }

    /// Asks the OS not to keep this package's data in the file cache, so a
    /// sweep over tens of gigabytes does not push other programs' memory out.
    /// Only has an effect on macOS.
    pub fn set_nocache(&self) {
        #[cfg(target_os = "macos")]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: plain fcntl on a descriptor we own; failure only loses the hint
            unsafe { libc::fcntl(self.cache.as_raw_fd(), libc::F_NOCACHE, 1) };
        }
    }

    /// Opens a package by name from a `Cache.Windows` folder; `None` if it does not exist.
    pub fn open_in(dir: &Path, name: &str) -> Result<Option<Self>> {
        let path: PathBuf = dir.join(format!("{name}.toc"));
        if !path.exists() {
            return Ok(None);
        }
        Package::open(&path).map(Some)
    }

    /// The bytes of an entry as stored in the `.cache`.
    pub fn read_raw(&mut self, entry: &Entry) -> Result<Vec<u8>> {
        let end = entry.offset + u64::from(entry.comp_len);
        ensure!(
            end <= self.cache_len,
            "{} ends at {end}, past the end of the .cache ({})",
            entry.path,
            self.cache_len
        );
        ensure!(
            entry.comp_len <= MAX_ENTRY_LEN,
            "{} is too large ({} bytes)",
            entry.path,
            entry.comp_len
        );
        let mut data = vec![0u8; entry.comp_len as usize];
        self.cache.seek(SeekFrom::Start(entry.offset))?;
        self.cache.read_exact(&mut data)?;
        Ok(data)
    }

    /// The decompressed bytes of an entry.
    pub fn read(&mut self, entry: &Entry) -> Result<Vec<u8>> {
        ensure!(
            entry.len <= MAX_ENTRY_LEN,
            "{} is too large ({} bytes)",
            entry.path,
            entry.len
        );
        let raw = self.read_raw(entry)?;
        decompress(&raw, entry.len as usize)
    }

    /// Reads and decompresses an entry by path.
    pub fn read_path(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        let Some(entry) = self.toc.get(path).cloned() else {
            return Ok(None);
        };
        self.read(&entry).map(Some)
    }
}

/// Parses an 8-byte block header into (compressed, decompressed) sizes.
pub fn block_header(h: &[u8]) -> Option<(usize, usize)> {
    if h.len() < 8 || h[0] != 0x80 || h[7] & 0x0F != 1 {
        return None;
    }
    let a = u32::from_be_bytes(h[0..4].try_into().unwrap());
    let b = u32::from_be_bytes(h[4..8].try_into().unwrap());
    Some((
        ((a >> 2) & 0xFF_FFFF) as usize,
        ((b >> 5) & 0xFF_FFFF) as usize,
    ))
}

/// Decompresses an entry's data to `len` bytes.
pub fn decompress(data: &[u8], len: usize) -> Result<Vec<u8>> {
    if data.len() == len {
        return Ok(data.to_vec());
    }
    let mut out = vec![0u8; len];
    let mut pos = 0;
    let mut written = 0;
    while written < len {
        let (comp, dec, start) = match block_header(&data[pos..]) {
            Some((comp, dec)) => (comp, dec, pos + 8),
            None if pos == 0 => (data.len(), len, 0),
            None => bail!("bad block header at offset {pos}"),
        };
        let block = data
            .get(start..start + comp)
            .with_context(|| format!("block at offset {pos} is truncated"))?;
        let dest = out.get_mut(written..written + dec).with_context(|| {
            format!("block at offset {pos} decompresses past the entry size {len}")
        })?;
        decompress_block(block, dest)
            .with_context(|| format!("block at offset {pos} ({comp} -> {dec} bytes)"))?;
        written += dec;
        pos = start + comp;
    }
    ensure!(
        pos == data.len(),
        "{} bytes left after the last block",
        data.len() - pos
    );
    Ok(out)
}

fn decompress_block(block: &[u8], out: &mut [u8]) -> Result<()> {
    if block.len() == out.len() {
        out.copy_from_slice(block);
    } else if block.first() == Some(&OODLE_MAGIC) {
        let n = oozextract::Extractor::new()
            .read_from_slice(block, out)
            .map_err(|e| anyhow::anyhow!("Oodle: {e:?}"))?;
        ensure!(n == out.len(), "Oodle produced {n} bytes");
    } else {
        lzf_decompress(block, out)?;
    }
    Ok(())
}

/// LZF (as in liblzf): a control byte below 32 starts a literal run of
/// `ctrl + 1` bytes; otherwise its top three bits are a length (7 means
/// "add the next byte") and the low five bits plus the next byte a distance
/// back into the output, copying `length + 2` bytes.
pub fn lzf_decompress(input: &[u8], out: &mut [u8]) -> Result<()> {
    let (mut i, mut o) = (0, 0);
    while i < input.len() {
        let ctrl = input[i] as usize;
        i += 1;
        if ctrl < 32 {
            let run = ctrl + 1;
            ensure!(i + run <= input.len(), "LZF literal runs past the input");
            ensure!(o + run <= out.len(), "LZF literal runs past the output");
            out[o..o + run].copy_from_slice(&input[i..i + run]);
            i += run;
            o += run;
        } else {
            let mut n = ctrl >> 5;
            if n == 7 {
                n += *input.get(i).context("LZF length byte missing")? as usize;
                i += 1;
            }
            let low = *input.get(i).context("LZF distance byte missing")? as usize;
            i += 1;
            let dist = ((ctrl & 0x1F) << 8) + low + 1;
            ensure!(
                dist <= o,
                "LZF back-reference before the start of the output"
            );
            ensure!(
                o + n + 2 <= out.len(),
                "LZF back-reference runs past the output"
            );
            // Byte by byte: source and destination may overlap
            for k in 0..n + 2 {
                out[o + k] = out[o + k - dist];
            }
            o += n + 2;
        }
    }
    ensure!(o == out.len(), "LZF produced {o} of {} bytes", out.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(comp: usize, dec: usize) -> [u8; 8] {
        let a = 0x8000_0000u32 | ((comp as u32) << 2);
        let b = ((dec as u32) << 5) | 1;
        let mut h = [0; 8];
        h[..4].copy_from_slice(&a.to_be_bytes());
        h[4..].copy_from_slice(&b.to_be_bytes());
        h
    }

    #[test]
    fn block_header_round_trip() {
        assert_eq!(
            block_header(&header(0x3FFFF, 0x40000)),
            Some((0x3FFFF, 0x40000))
        );
        assert_eq!(block_header(&[0x7F, 0, 0, 0, 0, 0, 0, 1]), None);
        assert_eq!(block_header(&[0x80, 0, 0, 0, 0, 0, 0, 2]), None);
    }

    #[test]
    fn stored_entry() {
        assert_eq!(decompress(b"hello", 5).unwrap(), b"hello");
    }

    #[test]
    fn lzf_literals_and_back_references() {
        // "abc" literal, then copy 6 bytes from 3 back => "abcabcabc"
        let lzf = [2, b'a', b'b', b'c', (4 << 5), 2];
        let mut out = [0; 9];
        lzf_decompress(&lzf, &mut out).unwrap();
        assert_eq!(&out, b"abcabcabc");
        // Long match: length code 7 plus an extra byte (7 + 3 + 2 = 12 bytes)
        let lzf = [0, b'x', (7 << 5), 3, 0];
        let mut out = [0; 13];
        lzf_decompress(&lzf, &mut out).unwrap();
        assert_eq!(&out, &[b'x'; 13]);
        // Distance past the start
        assert!(lzf_decompress(&[(1 << 5), 5], &mut [0; 3]).is_err());
    }

    #[test]
    fn framed_blocks() {
        // Block 1 stored (4 -> 4), block 2 LZF (6 -> 9)
        let mut data = Vec::new();
        data.extend(header(4, 4));
        data.extend(b"wxyz");
        let lzf = [2, b'a', b'b', b'c', (4 << 5), 2];
        data.extend(header(lzf.len(), 9));
        data.extend(lzf);
        assert_eq!(decompress(&data, 13).unwrap(), b"wxyzabcabcabc");
        // Declared size larger than the blocks provide
        assert!(decompress(&data, 20).is_err());
        // Truncated block
        assert!(decompress(&data[..data.len() - 1], 13).is_err());
    }

    #[test]
    fn unframed_lzf_entry() {
        let lzf = [2, b'a', b'b', b'c', (4 << 5), 2];
        assert_eq!(decompress(&lzf, 9).unwrap(), b"abcabcabc");
    }

    #[test]
    fn corrupt_oodle_block_is_an_error() {
        let mut data = Vec::new();
        data.extend(header(6, 64));
        data.extend([OODLE_MAGIC, 0x06, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(decompress(&data, 64).is_err());
    }
}
