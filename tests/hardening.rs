//! Regression tests for inputs found by the fuzz targets in `fuzz/`: malformed data must give an
//! error, never a panic, and the memory a reader asks for must follow the input, not the sizes the
//! input claims. All inputs are synthetic.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// Records the largest single allocation made by the current thread.
struct Tracking;

thread_local! {
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

// SAFETY: forwards every call to the system allocator; the bookkeeping only touches a const-initialised
// thread-local cell, which does not allocate.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = LARGEST.try_with(|l| l.set(l.get().max(layout.size())));
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let _ = LARGEST.try_with(|l| l.set(l.get().max(layout.size())));
        System.alloc_zeroed(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = LARGEST.try_with(|l| l.set(l.get().max(new_size)));
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOC: Tracking = Tracking;

/// Runs `f` and returns its result with the largest single allocation it made.
fn largest_allocation<T>(f: impl FnOnce() -> T) -> (T, usize) {
    LARGEST.with(|l| l.set(0));
    let out = f();
    (out, LARGEST.with(Cell::get))
}

/// Allocations of a reader over a few bytes of input must stay far below this.
const SMALL: usize = 1 << 20;

mod cache {
    use super::*;
    use wf_unpack::cache::{decompress, MAX_ENTRY_LEN};

    const MAX_ENTRY: usize = MAX_ENTRY_LEN as usize;

    /// Block header: 0x80 marker, compressed size, decompressed size, type 1.
    fn block_header(comp: u32, dec: u32) -> [u8; 8] {
        let mut h = [0; 8];
        h[..4].copy_from_slice(&(0x8000_0000 | comp << 2).to_be_bytes());
        h[4..].copy_from_slice(&(dec << 5 | 1).to_be_bytes());
        h
    }

    /// An unframed Oodle stream of fill blocks: 6 bytes (block header, quantum header, fill byte) make
    /// 262,144 output bytes, and block `i` is filled with the byte `i`.
    fn oodle_fill_blocks(n: usize) -> Vec<u8> {
        let mut data = Vec::new();
        for i in 0..n {
            // first byte 0x8C (restart, as every stream we dispatch on starts), codec 0x06 (Kraken)
            data.extend([0x8C, 0x06, 0x07, 0xFF, 0xFF, i as u8]);
        }
        data
    }

    /// Found by fuzzing: a `.toc` entry may declare any size, and the output buffer was allocated up
    /// front from it (3 GB for a 5-byte entry).
    #[test]
    fn declared_size_does_not_drive_the_allocation() {
        let declared = 3_372_932_899;
        // Unframed LZF: three literal bytes
        let (r, big) = largest_allocation(|| decompress(&[2, b'a', b'b', b'c'], declared));
        assert!(r.is_err());
        assert!(big < SMALL, "allocated {big} bytes");
        // Framed: a header promising 16 MB from a 1-byte payload
        let mut data = block_header(1, 0xFF_FFFF).to_vec();
        data.push(0);
        let (r, big) = largest_allocation(|| decompress(&data, declared));
        assert!(r.is_err());
        assert!(big < 1 << 25, "allocated {big} bytes");
        // Unframed Oodle, entry claiming the largest size: first a stub, then a real stream of three blocks
        let claimed = MAX_ENTRY;
        let (r, big) = largest_allocation(|| decompress(&[0x8C, 0, 0, 0, 0], claimed));
        assert!(r.is_err());
        assert!(big < SMALL, "allocated {big} bytes");
        let three = oodle_fill_blocks(3);
        let (r, big) = largest_allocation(|| decompress(&three, claimed));
        assert!(r.is_err());
        assert!(big <= 2 * 3 * 262_144, "allocated {big} bytes");
    }

    /// An unframed Oodle stream is not limited by the 24-bit size of a block header, which an
    /// unframed stream does not have: 70 blocks are 18,350,080 bytes, more than the header can say.
    #[test]
    fn unframed_oodle_may_be_larger_than_a_block_header_can_say() {
        let blocks = 70;
        let len = blocks * 262_144;
        assert!(len > 0xFF_FFFF);
        let out = decompress(&oodle_fill_blocks(blocks), len).unwrap();
        assert_eq!(out.len(), len);
        for (i, chunk) in out.chunks(262_144).enumerate() {
            assert!(chunk.iter().all(|&b| b == i as u8), "block {i}");
        }
        // and an entry that ends inside a block
        let out = decompress(&oodle_fill_blocks(3), 2 * 262_144 + 7).unwrap();
        assert_eq!((out.len(), out[2 * 262_144 + 6]), (2 * 262_144 + 7, 2));
    }

    /// `decompress` is public and `Package::read` is not the only way in: the largest entry applies to it too.
    #[test]
    fn declared_size_over_the_largest_entry_is_refused() {
        // 4097 blocks make 1 GiB + 262,144 bytes, which the stream could really produce
        let data = oodle_fill_blocks(MAX_ENTRY / 262_144 + 1);
        let (r, big) = largest_allocation(|| decompress(&data, MAX_ENTRY + 1));
        let e = r.unwrap_err().to_string();
        assert!(e.contains("largest entry"), "{e}");
        assert!(big < SMALL, "allocated {big} bytes");
        // the largest entry itself is let through to the decoder, which refuses this stub
        let e = decompress(&[0x8C, 0, 0, 0, 0], MAX_ENTRY)
            .unwrap_err()
            .to_string();
        assert!(e.contains("Oodle"), "{e}");
    }

    /// An LZF stream of one literal and `copies` back-references of 264 bytes (three input bytes each).
    fn lzf_best_ratio(copies: usize) -> (Vec<u8>, usize) {
        let mut lzf = vec![0, b'x'];
        for _ in 0..copies {
            lzf.extend([7 << 5, 255, 0]);
        }
        (lzf, 1 + 264 * copies)
    }

    #[test]
    fn lzf_at_its_best_ratio_is_still_accepted() {
        // A literal, then a copy of 7 + 255 + 2 bytes from three input bytes
        let lzf = [0, b'x', 7 << 5, 255, 0];
        assert_eq!(decompress(&lzf, 1 + 264).unwrap(), vec![b'x'; 265]);
        // With 100 copies the output is over 87 bytes per input byte
        let (lzf, len) = lzf_best_ratio(100);
        assert!(len > 87 * lzf.len());
        assert_eq!(decompress(&lzf, len).unwrap(), vec![b'x'; len]);
    }

    /// The unframed LZF limit is exactly 88 output bytes per input byte, no looser.
    #[test]
    fn unframed_lzf_limit_is_88_per_input_byte() {
        let (lzf, _) = lzf_best_ratio(100);
        let refused = decompress(&lzf, lzf.len() * 88 + 1)
            .unwrap_err()
            .to_string();
        assert!(refused.contains("cannot come out"), "{refused}");
        // at the limit the size check passes; the stream then simply makes fewer bytes
        let short = decompress(&lzf, lzf.len() * 88).unwrap_err().to_string();
        assert!(!short.contains("cannot come out"), "{short}");
    }

    /// Two blocks whose sizes add up to more than the entry: the second must not push the output past it.
    #[test]
    fn blocks_may_not_add_up_to_more_than_the_entry() {
        let mut data = Vec::new();
        for _ in 0..2 {
            data.extend(block_header(4, 4));
            data.extend(b"abcd");
        }
        assert_eq!(decompress(&data, 8).unwrap(), b"abcdabcd");
        let e = decompress(&data, 6).unwrap_err().to_string();
        assert!(e.contains("past the entry size"), "{e}");
    }
}

mod glob {
    use wf_unpack::toc::{clip_filter_matches, glob_match};

    /// The matcher as it was before the fix (backtracking), kept to check that the new one agrees.
    fn backtracking(p: &[u8], s: &[u8]) -> bool {
        match p {
            [] => s.is_empty(),
            [b'*', b'*', rest @ ..] => (0..=s.len()).any(|i| backtracking(rest, &s[i..])),
            [b'*', rest @ ..] => {
                let segment = s.iter().position(|&c| c == b'/').unwrap_or(s.len());
                (0..=segment).any(|i| backtracking(rest, &s[i..]))
            }
            [b'?', rest @ ..] => !s.is_empty() && s[0] != b'/' && backtracking(rest, &s[1..]),
            [c, rest @ ..] => s.first() == Some(c) && backtracking(rest, &s[1..]),
        }
    }

    fn all_strings(alphabet: &[u8], max_len: usize) -> Vec<String> {
        let mut out = vec![String::new()];
        let mut layer = vec![String::new()];
        for _ in 0..max_len {
            layer = layer
                .iter()
                .flat_map(|s| alphabet.iter().map(move |&c| format!("{s}{}", c as char)))
                .collect();
            out.extend(layer.iter().cloned());
        }
        out
    }

    #[test]
    fn agrees_with_the_backtracking_matcher_on_small_inputs() {
        let patterns = all_strings(b"a/*?", 5);
        let paths = all_strings(b"a/b", 5);
        for p in &patterns {
            for s in &paths {
                assert_eq!(
                    glob_match(p, s),
                    backtracking(p.as_bytes(), s.as_bytes()),
                    "pattern {p:?} path {s:?}"
                );
            }
        }
    }

    /// Found by fuzzing (a timeout): every `**` multiplied the work, so a pattern of a few dozen
    /// wildcards that cannot match took longer than the age of the universe.
    #[test]
    fn many_wildcards_do_not_take_exponential_time() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let pattern = format!("{}b", "**a".repeat(40));
            let path = "a".repeat(200);
            let star = format!("{}b", "*a".repeat(40));
            tx.send((
                glob_match(&pattern, &path),
                glob_match(&star, &path),
                clip_filter_matches(&pattern, &path),
            ))
            .ok();
        });
        let done = rx.recv_timeout(std::time::Duration::from_secs(20));
        assert_eq!(done, Ok((false, false, false)));
    }
}

mod toc {
    use wf_unpack::toc::Toc;

    fn entry(offset: i64, comp_len: i32, len: i32, parent: i32, name: &str) -> Vec<u8> {
        let mut e = Vec::new();
        e.extend(offset.to_le_bytes());
        e.extend(0i64.to_le_bytes());
        e.extend(comp_len.to_le_bytes());
        e.extend(len.to_le_bytes());
        e.extend(0i32.to_le_bytes());
        e.extend(parent.to_le_bytes());
        e.extend(name.as_bytes());
        e.resize(96, 0);
        e
    }

    /// Each directory is named after its full path, so a chain of N directories with 60-byte names
    /// held N * 60 * N / 2 bytes of paths: a 400 KB file asked for almost 500 MB.
    #[test]
    fn a_chain_of_nested_directories_is_an_error_not_quadratic_memory() {
        let mut raw = Vec::new();
        raw.extend(0x1867_C64Eu32.to_le_bytes());
        raw.extend(20u32.to_le_bytes());
        let name = "d".repeat(60);
        for i in 0..4000 {
            raw.extend(entry(-1, 0, 0, i, &name));
        }
        assert!(Toc::parse(&raw).is_err());
    }

    /// A `.toc` of 63 directories, each named with 63 bytes (64 with its slash, so 4032 for the chain),
    /// and a last entry whose name makes the full path `total` bytes long. The name field holds 64 bytes.
    fn toc_with_path_of(total: usize, is_dir: bool) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend(0x1867_C64Eu32.to_le_bytes());
        raw.extend(20u32.to_le_bytes());
        let dir_name = "d".repeat(63);
        for i in 0..63 {
            raw.extend(entry(-1, 0, 0, i, &dir_name));
        }
        let name = "f".repeat(total - 63 * 64 - 1);
        raw.extend(if is_dir {
            entry(-1, 0, 0, 63, &name)
        } else {
            entry(0, 4, 4, 63, &name)
        });
        raw
    }

    /// The limit is 4096 bytes for the whole path, and files count as well as directories.
    #[test]
    fn the_path_limit_is_4096_bytes_for_files_and_directories() {
        let toc = Toc::parse(&toc_with_path_of(4096, false)).unwrap();
        assert_eq!(toc.entries[0].path.len(), 4096);
        let toc = Toc::parse(&toc_with_path_of(4096, true)).unwrap();
        assert_eq!(toc.dir_count, 64);
        for is_dir in [false, true] {
            let e = Toc::parse(&toc_with_path_of(4097, is_dir)).err().unwrap();
            assert!(e.to_string().contains("over 4096 bytes"), "{e}");
        }
    }

    #[test]
    fn a_deep_but_plausible_path_is_accepted() {
        let mut raw = Vec::new();
        raw.extend(0x1867_C64Eu32.to_le_bytes());
        raw.extend(20u32.to_le_bytes());
        for i in 0..20 {
            raw.extend(entry(-1, 0, 0, i, "folder"));
        }
        raw.extend(entry(0, 4, 4, 20, "file.bin"));
        let toc = Toc::parse(&raw).unwrap();
        assert_eq!(toc.entries[0].path.matches('/').count(), 21);
    }
}

mod probe {
    use super::*;
    use wf_unpack::probe::read_radiance_hdr;

    /// Found by fuzzing (out of memory): the size line asked for 122 million pixels per scanline.
    #[test]
    fn radiance_size_line_does_not_drive_the_allocation() {
        let data = b"\n\n-Y 5 +X 122777777\n-Y 5 +X 122777777\x0e\\\n\x80$\x7f\x0e\\\n\x80$\x7f";
        let (r, big) = largest_allocation(|| read_radiance_hdr(data));
        assert!(r.is_err());
        assert!(big < SMALL, "allocated {big} bytes");
        // A large height with no data behind it
        let (r, big) = largest_allocation(|| read_radiance_hdr(b"\n\n-Y 99999999999 +X 8\n"));
        assert!(r.is_err());
        assert!(big < SMALL, "allocated {big} bytes");
    }

    /// The width is a `u32` in the size line but 16 bits in the marker, which is compared after truncating:
    /// 4294967304 is 0x1_0000_0008 and matches the marker `02 02 00 08`, so only the width check keeps
    /// the 17 GB scanline from being asked for.
    #[test]
    fn radiance_width_must_fit_the_marker() {
        let mut data = b"\n\n-Y 1 +X 4294967304\n\x02\x02\x00\x08".to_vec();
        data.extend([128 + 8, 1, 128 + 8, 2, 128 + 8, 3, 128 + 8, 4]);
        let (r, big) = largest_allocation(|| read_radiance_hdr(&data));
        let e = r.unwrap_err().to_string();
        assert!(e.contains("does not fit"), "{e}");
        assert!(big < SMALL, "allocated {big} bytes");
        // 0xFFFF is the widest the marker holds
        let (r, _) =
            largest_allocation(|| read_radiance_hdr(b"\n\n-Y 1 +X 65535\n\x02\x02\xFF\xFF"));
        assert!(!r.unwrap_err().to_string().contains("does not fit"));
        let (r, _) =
            largest_allocation(|| read_radiance_hdr(b"\n\n-Y 1 +X 65536\n\x02\x02\x00\x00"));
        assert!(r.unwrap_err().to_string().contains("does not fit"));
    }

    /// A literal run longer than the rest of the scanline, with enough data behind it: it must be refused
    /// before it is copied into the scanline.
    #[test]
    fn radiance_literal_longer_than_the_scanline_is_an_error() {
        let mut data = b"\n\n-Y 1 +X 4\n\x02\x02\x00\x04".to_vec();
        data.push(100);
        data.extend([7u8; 100]);
        let e = read_radiance_hdr(&data).unwrap_err().to_string();
        assert!(e.contains("past the end of the scanline"), "{e}");
    }

    /// A literal run cut short by the end of the data, in the last channel of the last scanline: reading
    /// past the data (as if it were padded) would finish the image, so the cut must be an error.
    #[test]
    fn radiance_literal_cut_short_at_the_end_is_an_error() {
        let mut data = b"\n\n-Y 1 +X 4\n\x02\x02\x00\x04".to_vec();
        for v in [128u8, 64, 32] {
            data.extend([128 + 4, v]);
        }
        data.extend([4, 1, 2]);
        let e = read_radiance_hdr(&data).unwrap_err().to_string();
        assert!(e.contains("truncated"), "{e}");
        // the same literal with its four bytes is the valid image
        data.extend([3, 4]);
        assert!(read_radiance_hdr(&data).is_ok());
    }

    /// Found by fuzzing (panics): a run longer than the rest of the scanline, and runs cut short.
    #[test]
    fn radiance_malformed_runs_are_errors() {
        let head = b"\n\n-Y 1 +X 8\n\x02\x02\x00\x08";
        for tail in [
            &[200u8, 1][..],     // a run of 72 pixels in a scanline of 8
            &[100u8][..],        // a literal of 100 pixels
            &[9u8, 1, 2, 3][..], // a literal of 9 pixels in a scanline of 8
            &[130u8][..],        // a run without its value
            &[4u8, 1, 2][..],    // a literal cut short
        ] {
            let mut data = head.to_vec();
            data.extend(tail);
            assert!(read_radiance_hdr(&data).is_err(), "{tail:?}");
        }
    }

    #[test]
    fn radiance_valid_scanline_still_reads() {
        // 8 pixels, each channel one run of 8 (a count byte of 128 + 8); the channels are 128, 64, 32 and exponent 129
        let mut data = b"\n\n-Y 1 +X 8\n\x02\x02\x00\x08".to_vec();
        for v in [128u8, 64, 32, 129] {
            data.extend([128 + 8, v]);
        }
        let (w, h, px) = read_radiance_hdr(&data).unwrap();
        assert_eq!((w, h, px.len()), (8, 1, 8));
        assert_eq!(
            px[0],
            [
                128.0 * 2f32.powi(-7),
                64.0 * 2f32.powi(-7),
                32.0 * 2f32.powi(-7)
            ]
        );
    }
}
