//! Helpers shared by the fuzz targets.

/// Splits `data` into `N` parts. The first `2 * (N - 1)` bytes are the little-endian `u16`
/// lengths of the first `N - 1` parts (clamped to what is left); the last part is the rest.
pub fn split<const N: usize>(data: &[u8]) -> [&[u8]; N] {
    let head = (2 * (N - 1)).min(data.len());
    let (lens, mut rest) = data.split_at(head);
    let mut out: [&[u8]; N] = [&[]; N];
    for (i, slot) in out.iter_mut().enumerate().take(N - 1) {
        let len = lens
            .get(2 * i..2 * i + 2)
            .map_or(0, |b| usize::from(u16::from_le_bytes([b[0], b[1]])));
        let (a, b) = rest.split_at(len.min(rest.len()));
        *slot = a;
        rest = b;
    }
    out[N - 1] = rest;
    out
}

/// The input as text (invalid UTF-8 is replaced).
pub fn text(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

/// Builds a Packages.bin of the layout in `src/packages.rs` (version 46, texts stored uncompressed):
/// one folder `/F/` holding the entities `A`, `B` (child of `A`) and `C` (child of `B`), each with the
/// given text. Lets the fuzz targets drive the property-text readers through the real file reader.
pub fn packages_bin(texts: [&[u8]; 3]) -> Vec<u8> {
    // The junk blocks between sections have lengths drawn from a generator seeded by the header
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
    fn str32(out: &mut Vec<u8>, s: &str) {
        out.extend((s.len() as u32).to_le_bytes());
        out.extend(s.as_bytes());
    }
    fn leb(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
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
    out.extend(1u32.to_le_bytes()); // folders

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
    for t in texts {
        push_bit(&mut flags, true);
        leb(&mut sizes, t.len() as u64);
        push_bit(&mut flags, false);
        frames.extend(t);
    }
    for buf in [&flags, &sizes, &frames] {
        out.resize(out.len() + junk.next(0x3FF), 0xBB);
        out.extend((buf.len() as u32).to_le_bytes());
        out.extend(buf.iter());
    }
    out.extend(3u32.to_le_bytes());
    out.resize(out.len() + junk.next(0xF), 0xCC);
    str32(&mut out, "/F/");
    out.push(1);
    out.extend(3u32.to_le_bytes());
    for (name, parent) in [("A", ""), ("B", "A"), ("C", "B")] {
        str32(&mut out, name);
        out.extend([0x81, 0, 1]);
        str32(&mut out, parent);
    }
    out
}
