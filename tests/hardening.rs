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
    use wf_unpack::cache::decompress;

    /// Block header: 0x80 marker, compressed size, decompressed size, type 1.
    fn block_header(comp: u32, dec: u32) -> [u8; 8] {
        let mut h = [0; 8];
        h[..4].copy_from_slice(&(0x8000_0000 | comp << 2).to_be_bytes());
        h[4..].copy_from_slice(&(dec << 5 | 1).to_be_bytes());
        h
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
        // Unframed Oodle is limited to what a block header can express
        let (r, big) = largest_allocation(|| decompress(&[0x8C, 0, 0, 0, 0], declared));
        assert!(r.is_err());
        assert!(big < SMALL, "allocated {big} bytes");
    }

    #[test]
    fn lzf_at_its_best_ratio_is_still_accepted() {
        // A literal, then a copy of 7 + 255 + 2 bytes from three input bytes
        let lzf = [0, b'x', 7 << 5, 255, 0];
        assert_eq!(decompress(&lzf, 1 + 264).unwrap(), vec![b'x'; 265]);
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
        // 8 pixels, each channel one run of 8 (130 - 128 = 2 would be too short): value 128 / exponent 129
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
