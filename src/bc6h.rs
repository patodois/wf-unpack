//! BC6H decoding that keeps the half-float range.
//!
//! Decoders that produce 8-bit colours clamp everything above 1.0, which loses the HDR data of
//! reflection probes. The decoding itself is done by `bcn-decode` (our decoder, written from the
//! public specifications: Khronos Data Format Specification section 20.2 and Microsoft's "BC6H
//! Format"); this module is the glue that gives it the shape the rest of wf-unpack uses: linear
//! RGB `f32` texels, one block or a whole image. An earlier version of this file was derived from
//! the BC6H decoders of `texture2ddecoder` and bcdec; none of that code is left.

use anyhow::{ensure, Result};

/// Decodes one 16-byte BC6H block to 16 texels (row by row) of linear RGB. Signed blocks hold
/// two's complement endpoints and decode to values in -65504..65504 (and, for the one endpoint
/// pattern the format reaches by accident, negative infinity). Reserved modes decode to black.
/// `data` must hold at least 16 bytes; a shorter slice gives black.
pub fn decode_bc6h_block(data: &[u8], outbuf: &mut [[f32; 3]; 16], signed: bool) {
    let Some(block) = data.first_chunk::<16>() else {
        outbuf.fill([0.0; 3]);
        return;
    };
    let texels = bcn_decode::bc6h::decode_block_f32(block, signed);
    for (out, t) in outbuf.iter_mut().zip(texels.iter()) {
        *out = [t[0], t[1], t[2]];
    }
}

/// Decodes a BC6H image of `width` x `height` (edge blocks are cropped) to linear RGB floats, row
/// by row.
pub fn decode_bc6h(
    data: &[u8],
    width: usize,
    height: usize,
    signed: bool,
) -> Result<Vec<[f32; 3]>> {
    let format = if signed {
        bcn_decode::Format::Bc6hSf16
    } else {
        bcn_decode::Format::Bc6hUf16
    };
    let (bw, bh) = (width.div_ceil(4), height.div_ceil(4));
    ensure!(
        data.len() >= bw * bh * 16,
        "BC6H data is {} bytes, {} needed",
        data.len(),
        bw * bh * 16
    );
    let rgba = bcn_decode::decode_rgba_f32_to_vec(format, data, width, height)
        .map_err(|e| anyhow::anyhow!("BC6H decoding: {e}"))?;
    Ok(rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|t| [t[0], t[1], t[2]])
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pseudo-random stream of 16-byte blocks (xorshift).
    fn blocks(n: usize) -> Vec<u8> {
        let mut s = 0x9E37_79B9_7F4A_7C15u64;
        (0..n * 16)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 24) as u8
            })
            .collect()
    }

    // The reference below follows the pseudocode of section 20.2 of the Khronos Data Format
    // Specification (unquantizing, Equation 2 with the weights of Table 119, and the final scaling
    // to half-float bits); it is written for the tests only.

    fn extend_sign(v: i32, bits: u32) -> i32 {
        (v << (32 - bits)) >> (32 - bits)
    }

    fn unquantize(x: i32, signed: bool, epb: u32) -> i32 {
        if !signed {
            if epb >= 15 {
                x
            } else if x == 0 {
                0
            } else if x == (1 << epb) - 1 {
                0xFFFF
            } else {
                ((x << 15) + 0x4000) >> (epb - 1)
            }
        } else if epb >= 16 {
            x
        } else {
            let (neg, m) = (x < 0, x.abs());
            let unq = if m == 0 {
                0
            } else if m >= (1 << (epb - 1)) - 1 {
                0x7FFF
            } else {
                ((m << 15) + 0x4000) >> (epb - 1)
            };
            if neg {
                -unq
            } else {
                unq
            }
        }
    }

    const W4: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

    /// The texel for endpoints `e0` and `e1` (already sign-extended, `epb` bits) at 4-bit index `idx`.
    fn spec_texel(e0: [i32; 3], e1: [i32; 3], epb: u32, signed: bool, idx: usize) -> [f32; 3] {
        let w = W4[idx];
        std::array::from_fn(|c| {
            let (a, b) = (
                unquantize(e0[c], signed, epb),
                unquantize(e1[c], signed, epb),
            );
            let i = (a * (64 - w) + b * w + 32) >> 6;
            let bits = if !signed {
                (i * 31) >> 6
            } else if i < 0 {
                (((-i) * 31) >> 5) | 0x8000
            } else {
                (i * 31) >> 5
            };
            crate::bytes::f16_to_f32(bits as u16)
        })
    }

    #[derive(Default)]
    struct Bits {
        bytes: [u8; 16],
        pos: usize,
    }

    impl Bits {
        fn put(&mut self, value: u32, n: usize) {
            for i in 0..n {
                self.bytes[self.pos / 8] |= (((value >> i) & 1) as u8) << (self.pos % 8);
                self.pos += 1;
            }
        }
        /// The 16 texel indices of a one-subset block: 3 bits for the anchor, 4 for the others.
        fn indices(&mut self, idx: [u8; 16]) {
            for (i, &v) in idx.iter().enumerate() {
                self.put(u32::from(v), if i == 0 { 3 } else { 4 });
            }
        }
    }

    /// Mode 3 (00011): two plain 10-bit endpoints.
    fn mode3_block(e0: [u16; 3], e1: [u16; 3], idx: [u8; 16]) -> [u8; 16] {
        let mut b = Bits::default();
        b.put(0b00011, 5);
        for v in e0.into_iter().chain(e1) {
            b.put(u32::from(v), 10);
        }
        b.indices(idx);
        assert_eq!(b.pos, 128);
        b.bytes
    }

    /// Mode 7 (00111): an 11-bit base endpoint and a 9-bit delta, one subset.
    fn mode7_block(base: [u16; 3], delta: [u16; 3], idx: [u8; 16]) -> [u8; 16] {
        let mut b = Bits::default();
        b.put(0b00111, 5);
        for v in base {
            b.put(u32::from(v & 0x3FF), 10);
        }
        // r1, then the top bit of r0; g1 and g0's; b1 and b0's
        for c in 0..3 {
            b.put(u32::from(delta[c]), 9);
            b.put(u32::from(base[c] >> 10), 1);
        }
        b.indices(idx);
        assert_eq!(b.pos, 128);
        b.bytes
    }

    fn all_indices() -> [u8; 16] {
        std::array::from_fn(|i| if i == 0 { 7 } else { i as u8 })
    }

    fn decode(block: &[u8; 16], signed: bool) -> [[f32; 3]; 16] {
        let mut out = [[0.0f32; 3]; 16];
        decode_bc6h_block(block, &mut out, signed);
        out
    }

    /// What `mode3_block(e0, e1, all_indices())` decodes to, per the specification.
    fn spec_mode3(e0: [u16; 3], e1: [u16; 3], signed: bool) -> [[f32; 3]; 16] {
        let ext = |v: u16| {
            if signed {
                extend_sign(i32::from(v), 10)
            } else {
                i32::from(v)
            }
        };
        let (a, b) = (e0.map(ext), e1.map(ext));
        let idx = all_indices();
        std::array::from_fn(|i| spec_texel(a, b, 10, signed, idx[i] as usize))
    }

    #[test]
    fn unsigned_endpoints_around_the_midpoint_decode_smoothly() {
        // 512 is the midpoint of the 10-bit range, not a saturated value (a decoder that treated
        // it as one turned it into 0xFFFF, 65504.0), and 1023, the real maximum, saturates
        let mut texel0 = Vec::new();
        for v in [510u16, 511, 512, 513, 1022, 1023] {
            let out = decode(&mode3_block([v; 3], [v; 3], all_indices()), false);
            assert_eq!(out, spec_mode3([v; 3], [v; 3], false), "endpoint {v}");
            texel0.push(out[0][0]);
        }
        // neighbours differ by a couple of percent (the 10-bit endpoints' steps are 1/1024 of the
        // range, which is about 2% of 1.5)
        for pair in 0..3 {
            let (a, b) = (texel0[pair], texel0[pair + 1]);
            assert!(a < b && (b - a) / a < 0.03, "{a} then {b}");
        }
        // 512 / 1024 of the unsigned range is 0x8000, which scales to the half 0x3E0F
        assert_eq!(texel0[2], 1.0 + 527.0 / 1024.0);
        // 1023 is the maximum half float
        assert_eq!(texel0[5], 65504.0);
        assert!(
            texel0[4] < 65504.0 * 0.999 && texel0[4] > 60000.0,
            "{}",
            texel0[4]
        );
    }

    #[test]
    fn every_ten_bit_endpoint_matches_the_specification() {
        for signed in [false, true] {
            for v in 0..1024u16 {
                let e0 = [v, (v * 7 + 3) % 1024, (v * 13 + 5) % 1024];
                let e1 = [(v * 5 + 1) % 1024, v, (1023 - v) % 1024];
                let block = mode3_block(e0, e1, all_indices());
                assert_eq!(
                    decode(&block, signed),
                    spec_mode3(e0, e1, signed),
                    "signed {signed}, endpoint {v}"
                );
            }
        }
    }

    #[test]
    fn signed_endpoints_are_twos_complement() {
        // 10-bit -1 (0x3FF) is a tiny negative number; read as sign and magnitude it would be the
        // largest negative one
        let out = decode(&mode3_block([0x3FF; 3], [0x3FF; 3], all_indices()), true);
        assert!(out[0][0] < 0.0 && out[0][0] > -0.01, "{}", out[0][0]);
        assert_eq!(out, spec_mode3([0x3FF; 3], [0x3FF; 3], true));
        // -512 and 511 are the extremes
        let out = decode(&mode3_block([0x200; 3], [0x1FF; 3], [0; 16]), true);
        assert_eq!(out[0], [-65504.0; 3]);
        let out = decode(&mode3_block([0x200; 3], [0x1FF; 3], [15; 16]), true);
        assert_eq!(out[5], [65504.0; 3]);
        // halfway between a negative and a positive endpoint is about zero
        let out = decode(&mode3_block([0x3F0; 3], [0x010; 3], [8; 16]), true);
        assert!(out[5][0].abs() < 1e-3, "{}", out[5][0]);
    }

    #[test]
    fn transformed_endpoints_match_the_specification() {
        // Mode 7: an 11-bit base and a 9-bit delta that wraps modulo 2^11
        for signed in [false, true] {
            for base in (0..2048u16).step_by(3).chain([1023, 1024, 1025, 2047]) {
                for delta in [0u16, 1, 255, 256, 257, 511] {
                    let b = [base, (base + 11) % 2048, (2047 - base) % 2048];
                    let d = [delta, (delta + 100) % 512, (511 - delta) % 512];
                    let block = mode7_block(b, d, all_indices());
                    let (mut e0, mut e1) = ([0i32; 3], [0i32; 3]);
                    for c in 0..3 {
                        let a0 = if signed {
                            extend_sign(i32::from(b[c]), 11)
                        } else {
                            i32::from(b[c])
                        };
                        let sum = (a0 + extend_sign(i32::from(d[c]), 9)) & 0x7FF;
                        e0[c] = a0;
                        e1[c] = if signed { extend_sign(sum, 11) } else { sum };
                    }
                    let idx = all_indices();
                    let expected: [[f32; 3]; 16] =
                        std::array::from_fn(|i| spec_texel(e0, e1, 11, signed, idx[i] as usize));
                    assert_eq!(
                        decode(&block, signed),
                        expected,
                        "signed {signed}, {b:?} {d:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn sixteen_bit_transformed_endpoints_keep_their_value() {
        // Mode 15 (01111, 16-bit endpoints) with every other bit set: the base endpoint is 0xFFFF and
        // each 4-bit delta is -1, so the second endpoint is 0xFFFE and the texels that use it (all
        // indices are 15 apart from the anchor) are as bright as the base. A zero wrap mask (the
        // overflow in texture2ddecoder 0.1.2) turns that endpoint into 0.
        let mut block = [0xFFu8; 16];
        block[0] = 0xE0 | 0b01111;
        let out = decode(&block, false);
        let base = out[0][0];
        assert!(base > 1.0, "{out:?}");
        for texel in &out[1..] {
            for v in texel {
                assert!((v - base).abs() / base < 0.01, "{v} vs {base}");
            }
        }
    }

    #[test]
    fn random_blocks_decode_in_debug() {
        // Every mode, valid or not, decodes without overflow and to values that are not NaN
        let data = blocks(4096);
        let (w, h) = (4 * 64, 4 * 64);
        for signed in [false, true] {
            let out = decode_bc6h(&data, w, h, signed).unwrap();
            assert!(out.iter().flatten().all(|v| !v.is_nan()));
            if !signed {
                assert!(out.iter().flatten().all(|v| v.is_finite()));
            }
        }
    }

    #[test]
    fn keeps_values_above_one() {
        // Mode 3 (00011): both endpoints are plain 10-bit values. Endpoint 0 is
        // 0x3FF in all channels (the largest, 65504.0 after unquantizing), so
        // the first texel, which uses index 0, is that endpoint.
        let img = decode_bc6h(&mode3_block([0x3FF; 3], [0; 3], [0; 16]), 4, 4, false).unwrap();
        assert_eq!(img[0][0], 65504.0, "{:?}", img[0]);
    }

    #[test]
    fn images_are_cropped_and_short_data_is_an_error() {
        // 5 x 3 texels = 2 x 1 blocks; the second block is a different colour
        let mut data = Vec::new();
        data.extend_from_slice(&mode3_block([0x3FF; 3], [0; 3], [0; 16]));
        data.extend_from_slice(&mode3_block([0x100; 3], [0; 3], [0; 16]));
        let img = decode_bc6h(&data, 5, 3, false).unwrap();
        assert_eq!(img.len(), 15);
        assert_eq!(img[0][0], 65504.0);
        assert_eq!(img[3][0], 65504.0);
        assert_eq!(img[4], spec_texel([0x100; 3], [0; 3], 10, false, 0));
        assert!(decode_bc6h(&data[..31], 5, 3, false).is_err());
        // a short slice for the block function gives black instead of a panic
        let mut out = [[1.0f32; 3]; 16];
        decode_bc6h_block(&data[..15], &mut out, false);
        assert_eq!(out, [[0.0; 3]; 16]);
    }

    #[test]
    fn reserved_modes_are_black() {
        for mode in [0b10011u8, 0b10111, 0b11011, 0b11111] {
            let mut block = [0xA5u8; 16];
            block[0] = (block[0] & !31) | mode;
            assert_eq!(decode(&block, false), [[0.0; 3]; 16]);
        }
    }
}
