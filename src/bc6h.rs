//! BC6H block decoder that keeps the half-float range.
//!
//! `texture2ddecoder` decodes BC6H to 8-bit colours and so clamps everything
//! above 1.0, which loses the HDR data of reflection probes. This module is
//! that crate's BC6H decoder (MIT OR Apache-2.0, (c) Rudolf Kolbe) with two
//! changes: pixels come out as `f32` instead of `u8`, and the bit reader and
//! the three BPTC tables it needs are included here. The mode table, the
//! field order per mode and the unquantizing follow the Direct3D 11 BC6H
//! specification; the unit tests check that clamping this decoder's output
//! gives the crate's own result.

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

fn unquantize(_value: u16, _signed: bool, _endpoint_bits: usize) -> u16 {
    let max_value: u16 = 1 << (_endpoint_bits - 1);

    if _signed {
        if _endpoint_bits >= 16 {
            return _value;
        }

        let sign: bool = _value & 0x8000 != 0;
        let _value = _value & 0x7fff;

        let unq: u16 = if 0 == _value {
            0
        } else if _value >= max_value - 1 {
            0x7fff
        } else {
            ((((_value as u32) << 15) + 0x4000) >> (_endpoint_bits - 1)) as u16
        };

        return if sign { u16::MAX - unq + 1 } else { unq };
    }

    if _endpoint_bits >= 15 {
        return _value;
    }

    if 0 == _value {
        return 0;
    }

    if _value == max_value {
        return u16::MAX;
    }

    ((((_value as u32) << 15) + 0x4000) >> (_endpoint_bits - 1)) as u16
}

fn finish_unquantize(_value: u16, _signed: bool) -> u16 {
    if _signed {
        let sign: u16 = _value & 0x8000;
        (((_value & 0x7fff) as u32 * 31) >> 5) as u16 | sign
    } else {
        ((_value as u32 * 31) >> 6) as u16
    }
}

fn sign_extend(_value: u16, _num_bits: usize) -> u16 {
    let mask: u16 = 1 << (_num_bits - 1);
    (_value ^ mask).overflowing_sub(mask).0
}

pub fn decode_bc6h_block(data: &[u8], outbuf: &mut [[f32; 3]; 16], signed: bool) {
    let mut bit: BitReader = BitReader::new(data, 0);

    let mut mode: u8 = bit.read(2) as u8;

    let mut ep_r: [u16; 4] = [0; 4]; //{ /* rw, rx, ry, rz */ };
    let mut ep_g: [u16; 4] = [0; 4]; //{ /* gw, gx, gy, gz */ };
    let mut ep_b: [u16; 4] = [0; 4]; //{ /* bw, bx, by, bz */ };

    if mode & 2 != 0 {
        // 5-bit mode
        mode |= (bit.read(3) << 2) as u8;

        if 0 == S_BC6H_MODE_INFO[mode as usize].endpoint_bits {
            outbuf.fill([0.0; 3]);
            return;
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

    let mi: &Bc6hModeInfo = &S_BC6H_MODE_INFO[mode as usize];

    if signed {
        ep_r[0] = sign_extend(ep_r[0], mi.endpoint_bits);
        ep_g[0] = sign_extend(ep_g[0], mi.endpoint_bits);
        ep_b[0] = sign_extend(ep_b[0], mi.endpoint_bits);
    }

    let num_subsets: usize = if mi.partition_bits != 0 { 2 } else { 1 };

    (1..num_subsets * 2).for_each(|ii| {
        if signed || mi.transformed {
            ep_r[ii] = sign_extend(ep_r[ii], mi.delta_bits[0]);
            ep_g[ii] = sign_extend(ep_g[ii], mi.delta_bits[1]);
            ep_b[ii] = sign_extend(ep_b[ii], mi.delta_bits[2]);
        }

        if mi.transformed {
            let mask = (1 << mi.endpoint_bits) - 1;

            ep_r[ii] = ep_r[ii].overflowing_add(ep_r[0]).0 & mask;
            ep_g[ii] = ep_g[ii].overflowing_add(ep_g[0]).0 & mask;
            ep_b[ii] = ep_b[ii].overflowing_add(ep_b[0]).0 & mask;

            if signed {
                ep_r[ii] = sign_extend(ep_r[ii], mi.endpoint_bits);
                ep_g[ii] = sign_extend(ep_g[ii], mi.endpoint_bits);
                ep_b[ii] = sign_extend(ep_b[ii], mi.endpoint_bits);
            }
        }
    });

    (0..num_subsets * 2).for_each(|ii| {
        ep_r[ii] = unquantize(ep_r[ii], signed, mi.endpoint_bits);
        ep_g[ii] = unquantize(ep_g[ii], signed, mi.endpoint_bits);
        ep_b[ii] = unquantize(ep_b[ii], signed, mi.endpoint_bits);
    });

    let partition_set_idx = if mi.partition_bits != 0 {
        bit.read(5) as usize
    } else {
        0
    };
    let index_bits = if mi.partition_bits != 0 { 3 } else { 4 };
    let factors = BPTC_FACTORS[index_bits - 2];

    (0..4_usize).for_each(|yy| {
        (0..4_usize).for_each(|xx| {
            let idx = yy * 4 + xx;

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

            let fc = factors[index] as u32;
            let fca = 64 - fc;
            let fcb = fc;

            subset_index *= 2;
            let rr = finish_unquantize(
                ((ep_r[subset_index] as u32 * fca + ep_r[subset_index + 1] as u32 * fcb + 32) >> 6)
                    as u16,
                signed,
            );
            let gg = finish_unquantize(
                ((ep_g[subset_index] as u32 * fca + ep_g[subset_index + 1] as u32 * fcb + 32) >> 6)
                    as u16,
                signed,
            );
            let bb = finish_unquantize(
                ((ep_b[subset_index] as u32 * fca + ep_b[subset_index + 1] as u32 * fcb + 32) >> 6)
                    as u16,
                signed,
            );

            outbuf[idx] = [half(rr), half(gg), half(bb)];
        });
    });
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

    #[test]
    fn clamped_output_matches_texture2ddecoder() {
        // 4096 random blocks cover every mode, valid or not
        let data = blocks(4096);
        let (w, h) = (4 * 64, 4 * 64);
        let mut reference = vec![0u32; w * h];
        texture2ddecoder::decode_bc6_unsigned(&data, w, h, &mut reference).unwrap();
        let mine = decode_bc6h(&data, w, h, false).unwrap();
        let mut compared = 0;
        for (a, b) in reference.iter().zip(&mine) {
            let [bl, g, r, _] = a.to_le_bytes();
            let q = |v: f32| (v * 255.0).clamp(0.0, 255.0) as u8;
            assert_eq!([r, g, bl], [q(b[0]), q(b[1]), q(b[2])]);
            compared += 1;
        }
        assert_eq!(compared, w * h);
    }

    #[test]
    fn keeps_values_above_one() {
        // Mode 3 (00011): both endpoints are plain 10-bit values. Endpoint 0 is
        // 0x3FF in all channels (the largest, 65504.0 ... after unquantizing), so
        // the first texel, which uses index 0, is that endpoint.
        let mut block = [0u8; 16];
        // 5 mode bits 00011 (LSB first = 1,1,0,0,0), then rw, gw, bw = 0x3FF
        let mut bits = Vec::new();
        for i in 0..5 {
            bits.push([1, 1, 0, 0, 0][i]);
        }
        bits.extend(std::iter::repeat_n(1, 30));
        for (i, b) in bits.iter().enumerate() {
            block[i / 8] |= b << (i % 8);
        }
        let img = decode_bc6h(&block, 4, 4, false).unwrap();
        assert!(img[0][0] > 60000.0, "{:?}", img[0]);
    }
}
