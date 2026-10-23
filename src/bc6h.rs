//! BC6H block decoder that keeps the half-float range.
//!
//! `texture2ddecoder` decodes BC6H to 8-bit colours and so clamps everything
//! above 1.0, which loses the HDR data of reflection probes. This module is
//! that crate's BC6H decoder (MIT OR Apache-2.0, (c) Rudolf Kolbe) with some
//! changes: pixels come out as `f32` instead of `u8`, the bit reader and the
//! three BPTC tables it needs are included here, and the unquantizing,
//! interpolation and signed handling follow bcdec (iOrange/bcdec, MIT or
//! Unlicense) instead. The crate's unquantizing treats the half-way value
//! (`1 << (bits - 1)`) as the saturated endpoint instead of the all-ones one,
//! and its signed path mixes sign-magnitude and two's complement; this module
//! keeps endpoints as signed integers throughout. The mode table and the
//! field order per mode follow the Direct3D 11 BC6H specification. The unit
//! tests compare the unquantizing and the blending against a port of bcdec's
//! and, where the two decoders agree, the output against texture2ddecoder.

struct Bc6hModeInfo {
    transformed: bool,
    partition_bits: usize,
    endpoint_bits: usize,
    delta_bits: [usize; 3],
}

static S_BC6H_MODE_INFO: [Bc6hModeInfo; 32] = [
    //  +--------------------------- transformed
    //  |  +------------------------ partition bits
    //  |  |  +--------------------- endpoint bits
    //  |  |  |      +-------------- delta bits
    // { 1, 5, 10, {  5,  5,  5 } }, // 00    2-bits
    // { 1, 5,  7, {  6,  6,  6 } }, // 01
    // { 1, 5, 11, {  5,  4,  4 } }, // 00010 5-bits
    // { 0, 0, 10, { 10, 10, 10 } }, // 00011
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 1, 5, 11, {  4,  5,  4 } }, // 00110
    // { 1, 0, 11, {  9,  9,  9 } }, // 00010
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 1, 5, 11, {  4,  4,  5 } }, // 00010
    // { 1, 0, 12, {  8,  8,  8 } }, // 00010
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 1, 5,  9, {  5,  5,  5 } }, // 00010
    // { 1, 0, 16, {  4,  4,  4 } }, // 00010
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 1, 5,  8, {  6,  5,  5 } }, // 00010
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 1, 5,  8, {  5,  6,  5 } }, // 00010
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 1, 5,  8, {  5,  5,  6 } }, // 00010
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // { 0, 5,  6, {  6,  6,  6 } }, // 00010
    // { 0, 0,  0, {  0,  0,  0 } }, // -
    // 00    2-bits
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 5,
        endpoint_bits: 10,
        delta_bits: [5, 5, 5],
    },
    // 01
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 5,
        endpoint_bits: 7,
        delta_bits: [6, 6, 6],
    },
    // 00010 5-bits
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 5,
        endpoint_bits: 11,
        delta_bits: [5, 4, 4],
    },
    // 00011
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 10,
        delta_bits: [10, 10, 10],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // 00110
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 5,
        endpoint_bits: 11,
        delta_bits: [4, 5, 4],
    },
    // 00010
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 0,
        endpoint_bits: 11,
        delta_bits: [9, 9, 9],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // 00010
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 5,
        endpoint_bits: 11,
        delta_bits: [4, 4, 5],
    },
    // 00010
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 0,
        endpoint_bits: 12,
        delta_bits: [8, 8, 8],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // 00010
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 5,
        endpoint_bits: 9,
        delta_bits: [5, 5, 5],
    },
    // 00010
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 0,
        endpoint_bits: 16,
        delta_bits: [4, 4, 4],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // 00010
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 5,
        endpoint_bits: 8,
        delta_bits: [6, 5, 5],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // 00010
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 5,
        endpoint_bits: 8,
        delta_bits: [5, 6, 5],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // 00010
    Bc6hModeInfo {
        transformed: true,
        partition_bits: 5,
        endpoint_bits: 8,
        delta_bits: [5, 5, 6],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
    // 00010
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 5,
        endpoint_bits: 6,
        delta_bits: [6, 6, 6],
    },
    // -
    Bc6hModeInfo {
        transformed: false,
        partition_bits: 0,
        endpoint_bits: 0,
        delta_bits: [0, 0, 0],
    },
];

/// Sign-extends the low `bits` bits of `val`.
fn extend_sign(val: i32, bits: usize) -> i32 {
    (val << (32 - bits)) >> (32 - bits)
}

/// Undoes the delta transform: a delta is added to the base endpoint modulo `2^bits`.
fn transform_inverse(val: i32, base: i32, bits: usize, signed: bool) -> i32 {
    // Mode 15 has 16-bit endpoints: the mask is 0xFFFF there (a `1u16 << 16` would be a zero mask,
    // which texture2ddecoder 0.1.2 still has)
    let val = (val + base) & ((1 << bits) - 1);
    if signed {
        extend_sign(val, bits)
    } else {
        val
    }
}

/// Expands an endpoint of `bits` bits to 16 bits: 0..=0xFFFF unsigned, -0x7FFF..=0x7FFF signed
/// (`val` already sign-extended). The all-ones endpoint is the maximum, and only that one.
fn unquantize(val: i32, signed: bool, bits: usize) -> i32 {
    if !signed {
        if bits >= 15 {
            val
        } else if val == 0 {
            0
        } else if val == (1 << bits) - 1 {
            0xFFFF
        } else {
            ((val << 16) + 0x8000) >> bits
        }
    } else if bits >= 16 {
        val
    } else {
        let magnitude = val.abs();
        let unq = if magnitude == 0 {
            0
        } else if magnitude >= (1 << (bits - 1)) - 1 {
            0x7FFF
        } else {
            ((magnitude << 15) + 0x4000) >> (bits - 1)
        };
        if val < 0 {
            -unq
        } else {
            unq
        }
    }
}

/// Blends two unquantized endpoints with a BPTC weight (0..=64).
fn interpolate(a: i32, b: i32, weight: i32) -> i32 {
    (a * (64 - weight) + b * weight + 32) >> 6
}

/// Scales an interpolated value into the half-float range and returns the half's bits (signed
/// values as sign and magnitude).
fn finish_unquantize(val: i32, signed: bool) -> u16 {
    if !signed {
        ((val * 31) >> 6) as u16
    } else {
        let magnitude = (val.abs() * 31) >> 5;
        if val < 0 {
            (0x8000 | magnitude) as u16
        } else {
            magnitude as u16
        }
    }
}

/// What is left of a block's header once the endpoints are read.
struct Endpoints<'a> {
    mi: &'static Bc6hModeInfo,
    /// Per channel (r, g, b) the endpoints of the subsets, sign-extended and with the delta
    /// transform undone, but still quantized.
    ep: [[i32; 4]; 3],
    /// Positioned at the partition number (two-subset modes) or the indices.
    bit: BitReader<'a>,
}

/// Reads the mode and the endpoints of a block; `None` for the reserved modes.
fn read_endpoints(data: &[u8], signed: bool) -> Option<Endpoints<'_>> {
    let mut bit: BitReader = BitReader::new(data, 0);

    let mut mode: u8 = bit.read(2) as u8;

    let mut ep_r: [u16; 4] = [0; 4]; //{ /* rw, rx, ry, rz */ };
    let mut ep_g: [u16; 4] = [0; 4]; //{ /* gw, gx, gy, gz */ };
    let mut ep_b: [u16; 4] = [0; 4]; //{ /* bw, bx, by, bz */ };

    if mode & 2 != 0 {
        // 5-bit mode
        mode |= (bit.read(3) << 2) as u8;

        if 0 == S_BC6H_MODE_INFO[mode as usize].endpoint_bits {
            return None;
        }

        match mode {
            2 => {
                ep_r[0] |= bit.read(10);
                ep_g[0] |= bit.read(10);
                ep_b[0] |= bit.read(10);
                ep_r[1] |= bit.read(5);
                ep_r[0] |= bit.read(1) << 10;
                ep_g[2] |= bit.read(4);
                ep_g[1] |= bit.read(4);
                ep_g[0] |= bit.read(1) << 10;
                ep_b[3] |= bit.read(1);
                ep_g[3] |= bit.read(4);
                ep_b[1] |= bit.read(4);
                ep_b[0] |= bit.read(1) << 10;
                ep_b[3] |= bit.read(1) << 1;
                ep_b[2] |= bit.read(4);
                ep_r[2] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 2;
                ep_r[3] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 3;
            }

            3 => {
                ep_r[0] |= bit.read(10);
                ep_g[0] |= bit.read(10);
                ep_b[0] |= bit.read(10);
                ep_r[1] |= bit.read(10);
                ep_g[1] |= bit.read(10);
                ep_b[1] |= bit.read(10);
            }

            6 => {
                ep_r[0] |= bit.read(10);
                ep_g[0] |= bit.read(10);
                ep_b[0] |= bit.read(10);
                ep_r[1] |= bit.read(4);
                ep_r[0] |= bit.read(1) << 10;
                ep_g[3] |= bit.read(1) << 4;
                ep_g[2] |= bit.read(4);
                ep_g[1] |= bit.read(5);
                ep_g[0] |= bit.read(1) << 10;
                ep_g[3] |= bit.read(4);
                ep_b[1] |= bit.read(4);
                ep_b[0] |= bit.read(1) << 10;
                ep_b[3] |= bit.read(1) << 1;
                ep_b[2] |= bit.read(4);
                ep_r[2] |= bit.read(4);
                ep_b[3] |= bit.read(1);
                ep_b[3] |= bit.read(1) << 2;
                ep_r[3] |= bit.read(4);
                ep_g[2] |= bit.read(1) << 4;
                ep_b[3] |= bit.read(1) << 3;
            }

            7 => {
                ep_r[0] |= bit.read(10);
                ep_g[0] |= bit.read(10);
                ep_b[0] |= bit.read(10);
                ep_r[1] |= bit.read(9);
                ep_r[0] |= bit.read(1) << 10;
                ep_g[1] |= bit.read(9);
                ep_g[0] |= bit.read(1) << 10;
                ep_b[1] |= bit.read(9);
                ep_b[0] |= bit.read(1) << 10;
            }

            10 => {
                ep_r[0] |= bit.read(10);
                ep_g[0] |= bit.read(10);
                ep_b[0] |= bit.read(10);
                ep_r[1] |= bit.read(4);
                ep_r[0] |= bit.read(1) << 10;
                ep_b[2] |= bit.read(1) << 4;
                ep_g[2] |= bit.read(4);
                ep_g[1] |= bit.read(4);
                ep_g[0] |= bit.read(1) << 10;
                ep_b[3] |= bit.read(1);
                ep_g[3] |= bit.read(4);
                ep_b[1] |= bit.read(5);
                ep_b[0] |= bit.read(1) << 10;
                ep_b[2] |= bit.read(4);
                ep_r[2] |= bit.read(4);
                ep_b[3] |= bit.read(1) << 1;
                ep_b[3] |= bit.read(1) << 2;
                ep_r[3] |= bit.read(4);
                ep_b[3] |= bit.read(1) << 4;
                ep_b[3] |= bit.read(1) << 3;
            }

            11 => {
                ep_r[0] |= bit.read(10);
                ep_g[0] |= bit.read(10);
                ep_b[0] |= bit.read(10);
                ep_r[1] |= bit.read(8);
                ep_r[0] |= bit.read(1) << 11;
                ep_r[0] |= bit.read(1) << 10;
                ep_g[1] |= bit.read(8);
                ep_g[0] |= bit.read(1) << 11;
                ep_g[0] |= bit.read(1) << 10;
                ep_b[1] |= bit.read(8);
                ep_b[0] |= bit.read(1) << 11;
                ep_b[0] |= bit.read(1) << 10;
            }

            14 => {
                ep_r[0] |= bit.read(9);
                ep_b[2] |= bit.read(1) << 4;
                ep_g[0] |= bit.read(9);
                ep_g[2] |= bit.read(1) << 4;
                ep_b[0] |= bit.read(9);
                ep_b[3] |= bit.read(1) << 4;
                ep_r[1] |= bit.read(5);
                ep_g[3] |= bit.read(1) << 4;
                ep_g[2] |= bit.read(4);
                ep_g[1] |= bit.read(5);
                ep_b[3] |= bit.read(1);
                ep_g[3] |= bit.read(4);
                ep_b[1] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 1;
                ep_b[2] |= bit.read(4);
                ep_r[2] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 2;
                ep_r[3] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 3;
            }

            15 => {
                ep_r[0] |= bit.read(10);
                ep_g[0] |= bit.read(10);
                ep_b[0] |= bit.read(10);
                ep_r[1] |= bit.read(4);
                ep_r[0] |= bit.read(1) << 15;
                ep_r[0] |= bit.read(1) << 14;
                ep_r[0] |= bit.read(1) << 13;
                ep_r[0] |= bit.read(1) << 12;
                ep_r[0] |= bit.read(1) << 11;
                ep_r[0] |= bit.read(1) << 10;
                ep_g[1] |= bit.read(4);
                ep_g[0] |= bit.read(1) << 15;
                ep_g[0] |= bit.read(1) << 14;
                ep_g[0] |= bit.read(1) << 13;
                ep_g[0] |= bit.read(1) << 12;
                ep_g[0] |= bit.read(1) << 11;
                ep_g[0] |= bit.read(1) << 10;
                ep_b[1] |= bit.read(4);
                ep_b[0] |= bit.read(1) << 15;
                ep_b[0] |= bit.read(1) << 14;
                ep_b[0] |= bit.read(1) << 13;
                ep_b[0] |= bit.read(1) << 12;
                ep_b[0] |= bit.read(1) << 11;
                ep_b[0] |= bit.read(1) << 10;
            }

            18 => {
                ep_r[0] |= bit.read(8);
                ep_g[3] |= bit.read(1) << 4;
                ep_b[2] |= bit.read(1) << 4;
                ep_g[0] |= bit.read(8);
                ep_b[3] |= bit.read(1) << 2;
                ep_g[2] |= bit.read(1) << 4;
                ep_b[0] |= bit.read(8);
                ep_b[3] |= bit.read(1) << 3;
                ep_b[3] |= bit.read(1) << 4;
                ep_r[1] |= bit.read(6);
                ep_g[2] |= bit.read(4);
                ep_g[1] |= bit.read(5);
                ep_b[3] |= bit.read(1);
                ep_g[3] |= bit.read(4);
                ep_b[1] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 1;
                ep_b[2] |= bit.read(4);
                ep_r[2] |= bit.read(6);
                ep_r[3] |= bit.read(6);
            }

            22 => {
                ep_r[0] |= bit.read(8);
                ep_b[3] |= bit.read(1);
                ep_b[2] |= bit.read(1) << 4;
                ep_g[0] |= bit.read(8);
                ep_g[2] |= bit.read(1) << 5;
                ep_g[2] |= bit.read(1) << 4;
                ep_b[0] |= bit.read(8);
                ep_g[3] |= bit.read(1) << 5;
                ep_b[3] |= bit.read(1) << 4;
                ep_r[1] |= bit.read(5);
                ep_g[3] |= bit.read(1) << 4;
                ep_g[2] |= bit.read(4);
                ep_g[1] |= bit.read(6);
                ep_g[3] |= bit.read(4);
                ep_b[1] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 1;
                ep_b[2] |= bit.read(4);
                ep_r[2] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 2;
                ep_r[3] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 3;
            }

            26 => {
                ep_r[0] |= bit.read(8);
                ep_b[3] |= bit.read(1) << 1;
                ep_b[2] |= bit.read(1) << 4;
                ep_g[0] |= bit.read(8);
                ep_b[2] |= bit.read(1) << 5;
                ep_g[2] |= bit.read(1) << 4;
                ep_b[0] |= bit.read(8);
                ep_b[3] |= bit.read(1) << 5;
                ep_b[3] |= bit.read(1) << 4;
                ep_r[1] |= bit.read(5);
                ep_g[3] |= bit.read(1) << 4;
                ep_g[2] |= bit.read(4);
                ep_g[1] |= bit.read(5);
                ep_b[3] |= bit.read(1);
                ep_g[3] |= bit.read(4);
                ep_b[1] |= bit.read(6);
                ep_b[2] |= bit.read(4);
                ep_r[2] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 2;
                ep_r[3] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 3;
            }

            30 => {
                ep_r[0] |= bit.read(6);
                ep_g[3] |= bit.read(1) << 4;
                ep_b[3] |= bit.read(1);
                ep_b[3] |= bit.read(1) << 1;
                ep_b[2] |= bit.read(1) << 4;
                ep_g[0] |= bit.read(6);
                ep_g[2] |= bit.read(1) << 5;
                ep_b[2] |= bit.read(1) << 5;
                ep_b[3] |= bit.read(1) << 2;
                ep_g[2] |= bit.read(1) << 4;
                ep_b[0] |= bit.read(6);
                ep_g[3] |= bit.read(1) << 5;
                ep_b[3] |= bit.read(1) << 3;
                ep_b[3] |= bit.read(1) << 5;
                ep_b[3] |= bit.read(1) << 4;
                ep_r[1] |= bit.read(6);
                ep_g[2] |= bit.read(4);
                ep_g[1] |= bit.read(6);
                ep_g[3] |= bit.read(4);
                ep_b[1] |= bit.read(6);
                ep_b[2] |= bit.read(4);
                ep_r[2] |= bit.read(6);
                ep_r[3] |= bit.read(6);
            }
            _ => {}
        }
    } else {
        match mode {
            0 => {
                ep_g[2] |= bit.read(1) << 4;
                ep_b[2] |= bit.read(1) << 4;
                ep_b[3] |= bit.read(1) << 4;
                ep_r[0] |= bit.read(10);
                ep_g[0] |= bit.read(10);
                ep_b[0] |= bit.read(10);
                ep_r[1] |= bit.read(5);
                ep_g[3] |= bit.read(1) << 4;
                ep_g[2] |= bit.read(4);
                ep_g[1] |= bit.read(5);
                ep_b[3] |= bit.read(1);
                ep_g[3] |= bit.read(4);
                ep_b[1] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 1;
                ep_b[2] |= bit.read(4);
                ep_r[2] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 2;
                ep_r[3] |= bit.read(5);
                ep_b[3] |= bit.read(1) << 3;
            }

            1 => {
                ep_g[2] |= bit.read(1) << 5;
                ep_g[3] |= bit.read(1) << 4;
                ep_g[3] |= bit.read(1) << 5;
                ep_r[0] |= bit.read(7);
                ep_b[3] |= bit.read(1);
                ep_b[3] |= bit.read(1) << 1;
                ep_b[2] |= bit.read(1) << 4;
                ep_g[0] |= bit.read(7);
                ep_b[2] |= bit.read(1) << 5;
                ep_b[3] |= bit.read(1) << 2;
                ep_g[2] |= bit.read(1) << 4;
                ep_b[0] |= bit.read(7);
                ep_b[3] |= bit.read(1) << 3;
                ep_b[3] |= bit.read(1) << 5;
                ep_b[3] |= bit.read(1) << 4;
                ep_r[1] |= bit.read(6);
                ep_g[2] |= bit.read(4);
                ep_g[1] |= bit.read(6);
                ep_g[3] |= bit.read(4);
                ep_b[1] |= bit.read(6);
                ep_b[2] |= bit.read(4);
                ep_r[2] |= bit.read(6);
                ep_r[3] |= bit.read(6);
            }
            _ => {}
        }
    }

    let mi: &'static Bc6hModeInfo = &S_BC6H_MODE_INFO[mode as usize];
    let eb = mi.endpoint_bits;
    let num_subsets: usize = if mi.partition_bits != 0 { 2 } else { 1 };

    let raw = [ep_r, ep_g, ep_b];
    let mut ep = [[0i32; 4]; 3];
    for (c, channel) in ep.iter_mut().enumerate() {
        // The base endpoint has `endpoint_bits` bits, the others are deltas of `delta_bits[c]`
        // bits when the block is transformed (and always signed when the format is)
        channel[0] = i32::from(raw[c][0]);
        if signed {
            channel[0] = extend_sign(channel[0], eb);
        }
        for ii in 1..num_subsets * 2 {
            let mut v = i32::from(raw[c][ii]);
            if signed || mi.transformed {
                v = extend_sign(v, mi.delta_bits[c]);
            }
            if mi.transformed {
                v = transform_inverse(v, channel[0], eb, signed);
            }
            channel[ii] = v;
        }
    }

    Some(Endpoints { mi, ep, bit })
}

/// Decodes one 16-byte BC6H block to 16 texels (row by row) of linear RGB. Signed blocks hold
/// two's complement endpoints and decode to values in -65504..65504.
pub fn decode_bc6h_block(data: &[u8], outbuf: &mut [[f32; 3]; 16], signed: bool) {
    let Some(Endpoints {
        mi,
        mut ep,
        mut bit,
    }) = read_endpoints(data, signed)
    else {
        outbuf.fill([0.0; 3]);
        return;
    };
    let num_subsets: usize = if mi.partition_bits != 0 { 2 } else { 1 };

    for channel in &mut ep {
        for v in &mut channel[..num_subsets * 2] {
            *v = unquantize(*v, signed, mi.endpoint_bits);
        }
    }

    let partition_set_idx = if mi.partition_bits != 0 {
        bit.read(5) as usize
    } else {
        0
    };
    let index_bits = if mi.partition_bits != 0 { 3 } else { 4 };
    let factors = BPTC_FACTORS[index_bits - 2];

    for idx in 0..16_usize {
        let mut subset_index = 0;
        let mut index_anchor = 0;

        if 0 != mi.partition_bits {
            subset_index = (BPTC_P2[partition_set_idx] >> idx) & 1;
            index_anchor = if subset_index != 0 {
                BPTC_A2[partition_set_idx]
            } else {
                0
            };
        }

        let anchor = idx == index_anchor;
        let num = index_bits - anchor as usize;
        let index = bit.read(num) as usize;
        let weight = i32::from(factors[index]);

        let s = subset_index * 2;
        let texel = |c: usize| {
            half(finish_unquantize(
                interpolate(ep[c][s], ep[c][s + 1], weight),
                signed,
            ))
        };
        outbuf[idx] = [texel(0), texel(1), texel(2)];
    }
}

static BPTC_A2: [usize; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2, 8, 2, 2, 8, 8, 15, 2, 8,
    2, 2, 8, 8, 2, 2, 15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2, 2, 15, 15, 6, 6, 2, 6, 8, 15, 15, 2,
    2, 15, 15, 15, 15, 15, 2, 2, 15,
];

static BPTC_FACTORS: [[u8; 16]; 3] = [
    [0, 21, 43, 64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 9, 18, 27, 37, 46, 55, 64, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64],
];

static BPTC_P2: [usize; 64] = [
    //  3210     0000000000   1111111111   2222222222   3333333333
    0xcccc, // 0, 0, 1, 1,  0, 0, 1, 1,  0, 0, 1, 1,  0, 0, 1, 1
    0x8888, // 0, 0, 0, 1,  0, 0, 0, 1,  0, 0, 0, 1,  0, 0, 0, 1
    0xeeee, // 0, 1, 1, 1,  0, 1, 1, 1,  0, 1, 1, 1,  0, 1, 1, 1
    0xecc8, // 0, 0, 0, 1,  0, 0, 1, 1,  0, 0, 1, 1,  0, 1, 1, 1
    0xc880, // 0, 0, 0, 0,  0, 0, 0, 1,  0, 0, 0, 1,  0, 0, 1, 1
    0xfeec, // 0, 0, 1, 1,  0, 1, 1, 1,  0, 1, 1, 1,  1, 1, 1, 1
    0xfec8, // 0, 0, 0, 1,  0, 0, 1, 1,  0, 1, 1, 1,  1, 1, 1, 1
    0xec80, // 0, 0, 0, 0,  0, 0, 0, 1,  0, 0, 1, 1,  0, 1, 1, 1
    0xc800, // 0, 0, 0, 0,  0, 0, 0, 0,  0, 0, 0, 1,  0, 0, 1, 1
    0xffec, // 0, 0, 1, 1,  0, 1, 1, 1,  1, 1, 1, 1,  1, 1, 1, 1
    0xfe80, // 0, 0, 0, 0,  0, 0, 0, 1,  0, 1, 1, 1,  1, 1, 1, 1
    0xe800, // 0, 0, 0, 0,  0, 0, 0, 0,  0, 0, 0, 1,  0, 1, 1, 1
    0xffe8, // 0, 0, 0, 1,  0, 1, 1, 1,  1, 1, 1, 1,  1, 1, 1, 1
    0xff00, // 0, 0, 0, 0,  0, 0, 0, 0,  1, 1, 1, 1,  1, 1, 1, 1
    0xfff0, // 0, 0, 0, 0,  1, 1, 1, 1,  1, 1, 1, 1,  1, 1, 1, 1
    0xf000, // 0, 0, 0, 0,  0, 0, 0, 0,  0, 0, 0, 0,  1, 1, 1, 1
    0xf710, // 0, 0, 0, 0,  1, 0, 0, 0,  1, 1, 1, 0,  1, 1, 1, 1
    0x008e, // 0, 1, 1, 1,  0, 0, 0, 1,  0, 0, 0, 0,  0, 0, 0, 0
    0x7100, // 0, 0, 0, 0,  0, 0, 0, 0,  1, 0, 0, 0,  1, 1, 1, 0
    0x08ce, // 0, 1, 1, 1,  0, 0, 1, 1,  0, 0, 0, 1,  0, 0, 0, 0
    0x008c, // 0, 0, 1, 1,  0, 0, 0, 1,  0, 0, 0, 0,  0, 0, 0, 0
    0x7310, // 0, 0, 0, 0,  1, 0, 0, 0,  1, 1, 0, 0,  1, 1, 1, 0
    0x3100, // 0, 0, 0, 0,  0, 0, 0, 0,  1, 0, 0, 0,  1, 1, 0, 0
    0x8cce, // 0, 1, 1, 1,  0, 0, 1, 1,  0, 0, 1, 1,  0, 0, 0, 1
    0x088c, // 0, 0, 1, 1,  0, 0, 0, 1,  0, 0, 0, 1,  0, 0, 0, 0
    0x3110, // 0, 0, 0, 0,  1, 0, 0, 0,  1, 0, 0, 0,  1, 1, 0, 0
    0x6666, // 0, 1, 1, 0,  0, 1, 1, 0,  0, 1, 1, 0,  0, 1, 1, 0
    0x366c, // 0, 0, 1, 1,  0, 1, 1, 0,  0, 1, 1, 0,  1, 1, 0, 0
    0x17e8, // 0, 0, 0, 1,  0, 1, 1, 1,  1, 1, 1, 0,  1, 0, 0, 0
    0x0ff0, // 0, 0, 0, 0,  1, 1, 1, 1,  1, 1, 1, 1,  0, 0, 0, 0
    0x718e, // 0, 1, 1, 1,  0, 0, 0, 1,  1, 0, 0, 0,  1, 1, 1, 0
    0x399c, // 0, 0, 1, 1,  1, 0, 0, 1,  1, 0, 0, 1,  1, 1, 0, 0
    0xaaaa, // 0, 1, 0, 1,  0, 1, 0, 1,  0, 1, 0, 1,  0, 1, 0, 1
    0xf0f0, // 0, 0, 0, 0,  1, 1, 1, 1,  0, 0, 0, 0,  1, 1, 1, 1
    0x5a5a, // 0, 1, 0, 1,  1, 0, 1, 0,  0, 1, 0, 1,  1, 0, 1, 0
    0x33cc, // 0, 0, 1, 1,  0, 0, 1, 1,  1, 1, 0, 0,  1, 1, 0, 0
    0x3c3c, // 0, 0, 1, 1,  1, 1, 0, 0,  0, 0, 1, 1,  1, 1, 0, 0
    0x55aa, // 0, 1, 0, 1,  0, 1, 0, 1,  1, 0, 1, 0,  1, 0, 1, 0
    0x9696, // 0, 1, 1, 0,  1, 0, 0, 1,  0, 1, 1, 0,  1, 0, 0, 1
    0xa55a, // 0, 1, 0, 1,  1, 0, 1, 0,  1, 0, 1, 0,  0, 1, 0, 1
    0x73ce, // 0, 1, 1, 1,  0, 0, 1, 1,  1, 1, 0, 0,  1, 1, 1, 0
    0x13c8, // 0, 0, 0, 1,  0, 0, 1, 1,  1, 1, 0, 0,  1, 0, 0, 0
    0x324c, // 0, 0, 1, 1,  0, 0, 1, 0,  0, 1, 0, 0,  1, 1, 0, 0
    0x3bdc, // 0, 0, 1, 1,  1, 0, 1, 1,  1, 1, 0, 1,  1, 1, 0, 0
    0x6996, // 0, 1, 1, 0,  1, 0, 0, 1,  1, 0, 0, 1,  0, 1, 1, 0
    0xc33c, // 0, 0, 1, 1,  1, 1, 0, 0,  1, 1, 0, 0,  0, 0, 1, 1
    0x9966, // 0, 1, 1, 0,  0, 1, 1, 0,  1, 0, 0, 1,  1, 0, 0, 1
    0x0660, // 0, 0, 0, 0,  0, 1, 1, 0,  0, 1, 1, 0,  0, 0, 0, 0
    0x0272, // 0, 1, 0, 0,  1, 1, 1, 0,  0, 1, 0, 0,  0, 0, 0, 0
    0x04e4, // 0, 0, 1, 0,  0, 1, 1, 1,  0, 0, 1, 0,  0, 0, 0, 0
    0x4e40, // 0, 0, 0, 0,  0, 0, 1, 0,  0, 1, 1, 1,  0, 0, 1, 0
    0x2720, // 0, 0, 0, 0,  0, 1, 0, 0,  1, 1, 1, 0,  0, 1, 0, 0
    0xc936, // 0, 1, 1, 0,  1, 1, 0, 0,  1, 0, 0, 1,  0, 0, 1, 1
    0x936c, // 0, 0, 1, 1,  0, 1, 1, 0,  1, 1, 0, 0,  1, 0, 0, 1
    0x39c6, // 0, 1, 1, 0,  0, 0, 1, 1,  1, 0, 0, 1,  1, 1, 0, 0
    0x639c, // 0, 0, 1, 1,  1, 0, 0, 1,  1, 1, 0, 0,  0, 1, 1, 0
    0x9336, // 0, 1, 1, 0,  1, 1, 0, 0,  1, 1, 0, 0,  1, 0, 0, 1
    0x9cc6, // 0, 1, 1, 0,  0, 0, 1, 1,  0, 0, 1, 1,  1, 0, 0, 1
    0x817e, // 0, 1, 1, 1,  1, 1, 1, 0,  1, 0, 0, 0,  0, 0, 0, 1
    0xe718, // 0, 0, 0, 1,  1, 0, 0, 0,  1, 1, 1, 0,  0, 1, 1, 1
    0xccf0, // 0, 0, 0, 0,  1, 1, 1, 1,  0, 0, 1, 1,  0, 0, 1, 1
    0x0fcc, // 0, 0, 1, 1,  0, 0, 1, 1,  1, 1, 1, 1,  0, 0, 0, 0
    0x7744, // 0, 0, 1, 0,  0, 0, 1, 0,  1, 1, 1, 0,  1, 1, 1, 0
    0xee22, // 0, 1, 0, 0,  0, 1, 0, 0,  0, 1, 1, 1,  0, 1, 1, 1
];

/// Half-float bits to `f32`; BC6H signed blocks store two's complement-ish
/// sign-magnitude halves, which is what IEEE half is.
fn half(h: u16) -> f32 {
    crate::bytes::f16_to_f32(h)
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8], bit_pos: usize) -> Self {
        BitReader { data, bit_pos }
    }

    fn read(&mut self, num_bits: usize) -> u16 {
        let shift = self.bit_pos & 7;
        let first = self.bit_pos / 8;
        let last = (self.bit_pos + num_bits).div_ceil(8);
        let mut raw = [0u8; 4];
        raw[..last - first].copy_from_slice(&self.data[first..last]);
        self.bit_pos += num_bits;
        ((u32::from_le_bytes(raw) >> shift) as u16) & ((1u32 << num_bits) - 1) as u16
    }
}

/// Decodes a BC6H image of `width` x `height` (multiples of 4 are padded up)
/// to linear RGB floats, row by row.
pub fn decode_bc6h(
    data: &[u8],
    width: usize,
    height: usize,
    signed: bool,
) -> anyhow::Result<Vec<[f32; 3]>> {
    let (bw, bh) = (width.div_ceil(4), height.div_ceil(4));
    anyhow::ensure!(
        data.len() >= bw * bh * 16,
        "BC6H data is {} bytes, {} needed",
        data.len(),
        bw * bh * 16
    );
    let mut out = vec![[0.0f32; 3]; width * height];
    let mut block = [[0.0f32; 3]; 16];
    for by in 0..bh {
        for bx in 0..bw {
            let at = (by * bw + bx) * 16;
            decode_bc6h_block(&data[at..at + 16], &mut block, signed);
            for (i, px) in block.iter().enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < width && y < height {
                    out[y * width + x] = *px;
                }
            }
        }
    }
    Ok(out)
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

    // The reference below is a port of the BC6H helpers of bcdec
    // (https://github.com/iOrange/bcdec, bcdec__unquantize, bcdec__interpolate and
    // bcdec__finish_unquantize; MIT or Unlicense, (c) Sergii "iOrange" Kudlai), written for the
    // tests only and kept in bcdec's shape (a sign flag and a negation) on purpose, so that it
    // does not share the structure of the code under test.

    fn ref_unquantize(mut val: i32, bits: i32, is_signed: bool) -> i32 {
        let mut s = false;
        let unq;
        if !is_signed {
            if bits >= 15 {
                unq = val;
            } else if val == 0 {
                unq = 0;
            } else if val == (1 << bits) - 1 {
                unq = 0xFFFF;
            } else {
                unq = ((val << 16) + 0x8000) >> bits;
            }
            unq
        } else if bits >= 16 {
            val
        } else {
            if val < 0 {
                s = true;
                val = -val;
            }
            let mut unq = if val == 0 {
                0
            } else if val >= (1 << (bits - 1)) - 1 {
                0x7FFF
            } else {
                ((val << 15) + 0x4000) >> (bits - 1)
            };
            if s {
                unq = -unq;
            }
            unq
        }
    }

    fn ref_interpolate(a: i32, b: i32, weights: &[i32], index: usize) -> i32 {
        (a * (64 - weights[index]) + b * weights[index] + 32) >> 6
    }

    fn ref_finish_unquantize(mut val: i32, is_signed: bool) -> u16 {
        if !is_signed {
            ((val * 31) >> 6) as u16
        } else {
            val = if val < 0 {
                -(((-val) * 31) >> 5)
            } else {
                (val * 31) >> 5
            };
            let mut s = 0;
            if val < 0 {
                s = 0x8000;
                val = -val;
            }
            (s | val) as u16
        }
    }

    /// The texel a block decodes to given its unquantized endpoint integers, per bcdec.
    fn ref_texel(
        e0: [i32; 3],
        e1: [i32; 3],
        bits: i32,
        is_signed: bool,
        weights: &[i32],
        index: usize,
    ) -> [f32; 3] {
        let mut out = [0.0; 3];
        for c in 0..3 {
            let a = ref_unquantize(e0[c], bits, is_signed);
            let b = ref_unquantize(e1[c], bits, is_signed);
            let v = ref_interpolate(a, b, weights, index);
            out[c] = half(ref_finish_unquantize(v, is_signed));
        }
        out
    }

    const W4: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

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

    /// Texel `i` of a block made by `mode3_block(e0, e1, all_indices())`, per the reference. The
    /// anchor texel (0) has the 3-bit index 7 and so a 4-bit weight of its own, `W4[7]`.
    fn ref_mode3(e0: [u16; 3], e1: [u16; 3], signed: bool) -> [[f32; 3]; 16] {
        let ext = |v: u16| {
            if signed {
                extend_sign(i32::from(v), 10)
            } else {
                i32::from(v)
            }
        };
        let (a, b) = (e0.map(ext), e1.map(ext));
        let idx = all_indices();
        std::array::from_fn(|i| ref_texel(a, b, 10, signed, &W4, idx[i] as usize))
    }

    #[test]
    fn unsigned_endpoints_around_the_midpoint_decode_smoothly() {
        // 512 is the midpoint of the 10-bit range, not a saturated value: the old code turned it
        // into 0xFFFF (65504.0), and 1023, the real maximum, into an ordinary value
        let mut texel0 = Vec::new();
        for v in [510u16, 511, 512, 513, 1022, 1023] {
            let out = decode(&mode3_block([v; 3], [v; 3], all_indices()), false);
            assert_eq!(out, ref_mode3([v; 3], [v; 3], false), "endpoint {v}");
            texel0.push(out[0][0]);
        }
        // neighbours differ by a couple of percent (a half float's steps at 1.5 are 0.1%, the 10-bit
        // endpoints' steps are 1/1024 of the range, which is about 2% of 1.5)
        for (pair, name) in [(0, "510-511"), (1, "511-512"), (2, "512-513")] {
            let (a, b) = (texel0[pair], texel0[pair + 1]);
            assert!(a < b && (b - a) / a < 0.03, "{name}: {a} then {b}");
        }
        // 512 / 1024 of the unsigned range is 0x8000, which is the half 1.47
        assert!(texel0[2] > 1.4 && texel0[2] < 1.6, "{}", texel0[2]);
        // 1023 is the maximum half float
        assert_eq!(texel0[5], 65504.0);
        assert!(
            texel0[4] < 65504.0 * 0.999 && texel0[4] > 60000.0,
            "{}",
            texel0[4]
        );
    }

    #[test]
    fn every_ten_bit_endpoint_matches_the_reference() {
        for signed in [false, true] {
            for v in 0..1024u16 {
                let e0 = [v, (v * 7 + 3) % 1024, (v * 13 + 5) % 1024];
                let e1 = [(v * 5 + 1) % 1024, v, (1023 - v) % 1024];
                let block = mode3_block(e0, e1, all_indices());
                assert_eq!(
                    decode(&block, signed),
                    ref_mode3(e0, e1, signed),
                    "signed {signed}, endpoint {v}"
                );
            }
        }
    }

    #[test]
    fn signed_endpoints_are_twos_complement() {
        // 10-bit -1 (0x3FF) is a tiny negative number; read as sign and magnitude it was the
        // largest negative one
        let out = decode(&mode3_block([0x3FF; 3], [0x3FF; 3], all_indices()), true);
        assert!(out[0][0] < 0.0 && out[0][0] > -0.01, "{}", out[0][0]);
        assert_eq!(out, ref_mode3([0x3FF; 3], [0x3FF; 3], true));
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
    fn transformed_endpoints_match_the_reference() {
        // Mode 7: an 11-bit base and a 9-bit delta that wraps modulo 2^11
        for signed in [false, true] {
            for base in (0..2048u16).step_by(3).chain([1023, 1024, 1025, 2047]) {
                for delta in [0u16, 1, 255, 256, 257, 511] {
                    let b = [base, (base + 11) % 2048, (2047 - base) % 2048];
                    let d = [delta, (delta + 100) % 512, (511 - delta) % 512];
                    let block = mode7_block(b, d, all_indices());
                    // reference: sign-extend, add, wrap
                    let mut e1 = [0i32; 3];
                    let mut e0 = [0i32; 3];
                    for c in 0..3 {
                        let a0 = if signed {
                            extend_sign(i32::from(b[c]), 11)
                        } else {
                            i32::from(b[c])
                        };
                        let delta = extend_sign(i32::from(d[c]), 9);
                        let sum = (a0 + delta) & 0x7FF;
                        e0[c] = a0;
                        e1[c] = if signed { extend_sign(sum, 11) } else { sum };
                    }
                    let idx = all_indices();
                    let expected: [[f32; 3]; 16] = std::array::from_fn(|i| {
                        ref_texel(e0, e1, 11, signed, &W4, idx[i] as usize)
                    });
                    assert_eq!(
                        decode(&block, signed),
                        expected,
                        "signed {signed}, {b:?} {d:?}"
                    );
                }
            }
        }
    }

    /// The independent reconstruction of a whole block: the endpoints the code under test reads,
    /// unquantized and blended by the reference.
    fn ref_block(data: &[u8], signed: bool) -> [[f32; 3]; 16] {
        let Some(Endpoints { mi, ep, mut bit }) = read_endpoints(data, signed) else {
            return [[0.0; 3]; 16];
        };
        let two = mi.partition_bits != 0;
        let part = if two { bit.read(5) as usize } else { 0 };
        let (index_bits, weights): (usize, &[i32]) = if two {
            (3, &[0, 9, 18, 27, 37, 46, 55, 64])
        } else {
            (4, &W4)
        };
        std::array::from_fn(|i| {
            let (subset, anchor) = if two {
                let s = (BPTC_P2[part] >> i) & 1;
                (s, i == if s != 0 { BPTC_A2[part] } else { 0 })
            } else {
                (0, i == 0)
            };
            let index = bit.read(index_bits - usize::from(anchor)) as usize;
            let pick = |c: usize, k: usize| ep[c][subset * 2 + k];
            ref_texel(
                [pick(0, 0), pick(1, 0), pick(2, 0)],
                [pick(0, 1), pick(1, 1), pick(2, 1)],
                mi.endpoint_bits as i32,
                signed,
                weights,
                index,
            )
        })
    }

    #[test]
    fn random_blocks_match_the_reference() {
        let data = blocks(8192);
        for signed in [false, true] {
            for block in data.chunks(16) {
                assert_eq!(
                    decode(block.try_into().unwrap(), signed),
                    ref_block(block, signed),
                    "signed {signed}, block {block:02x?}"
                );
            }
        }
    }

    #[test]
    // texture2ddecoder overflows on some invalid random blocks in debug builds, so the comparison
    // runs in release (`cargo test --release`); `random_blocks_decode_in_debug` covers this crate
    #[cfg_attr(
        debug_assertions,
        ignore = "the reference decoder overflows in debug builds"
    )]
    fn clamped_output_matches_texture2ddecoder_where_it_is_right() {
        // texture2ddecoder 0.1.2 differs from the format in two ways: it masks the endpoints of
        // the 16-bit transformed mode to zero, and it saturates an unquantized endpoint at the
        // half-way value instead of the all-ones one. Blocks with the first are switched to a
        // two-bit mode here and blocks with an endpoint at either of the two values are left out
        let mut data = blocks(4096);
        for block in data.chunks_mut(16) {
            let mode = if block[0] & 2 != 0 {
                block[0] & 0x1F
            } else {
                block[0] & 3
            };
            let mi = &S_BC6H_MODE_INFO[mode as usize];
            if mi.transformed && mi.endpoint_bits == 16 {
                block[0] &= !2;
            }
        }
        let (w, h) = (4 * 64, 4 * 64);
        let mut reference = vec![0u32; w * h];
        texture2ddecoder::decode_bc6_unsigned(&data, w, h, &mut reference).unwrap();
        let mine = decode_bc6h(&data, w, h, false).unwrap();
        let mut compared_blocks = 0;
        for (n, block) in data.chunks(16).enumerate() {
            if let Some(e) = read_endpoints(block, false) {
                let bits = e.mi.endpoint_bits;
                let n_ep = if e.mi.partition_bits != 0 { 4 } else { 2 };
                let disputed = bits < 15
                    && e.ep.iter().any(|ch| {
                        ch[..n_ep]
                            .iter()
                            .any(|&v| v == 1 << (bits - 1) || v == (1 << bits) - 1)
                    });
                if disputed {
                    continue;
                }
            }
            compared_blocks += 1;
            let (bx, by) = (n % (w / 4), n / (w / 4));
            for i in 0..16 {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                let [bl, g, r, _] = reference[y * w + x].to_le_bytes();
                let b = mine[y * w + x];
                let q = |v: f32| (v * 255.0).clamp(0.0, 255.0) as u8;
                assert_eq!([r, g, bl], [q(b[0]), q(b[1]), q(b[2])], "block {n}");
            }
        }
        assert!(compared_blocks > 3800, "{compared_blocks} of 4096 compared");
    }

    #[test]
    fn sixteen_bit_transformed_endpoints_keep_their_value() {
        // The 16-bit transformed mode with every other bit set: the base endpoint is 0xFFFF and
        // each 4-bit delta is -1, so the second endpoint is 0xFFFE and the texels that use it (all
        // indices are 15 apart from the anchor) are as bright as the base. A zero mask (the old
        // overflow, still in texture2ddecoder 0.1.2) turns that endpoint into 0.
        let mode = (0..32u8)
            .find(|&m| {
                let mi = &S_BC6H_MODE_INFO[m as usize];
                m & 2 != 0 && mi.transformed && mi.endpoint_bits == 16
            })
            .unwrap();
        let mut block = [0xFFu8; 16];
        block[0] = 0xE0 | mode;
        let mut out = [[0.0f32; 3]; 16];
        decode_bc6h_block(&block, &mut out, false);
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
        // Every mode, valid or not, decodes without overflow and to finite values
        let data = blocks(4096);
        let (w, h) = (4 * 64, 4 * 64);
        for signed in [false, true] {
            let out = decode_bc6h(&data, w, h, signed).unwrap();
            assert!(out.iter().flatten().all(|v| v.is_finite()));
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
}
