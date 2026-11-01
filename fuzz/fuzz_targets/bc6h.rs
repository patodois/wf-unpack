#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::bc6h::decode_bc6h_block;

fuzz_target!(|data: &[u8]| {
    let mut out = [[0.0f32; 3]; 16];
    decode_bc6h_block(data, &mut out, false);
    decode_bc6h_block(data, &mut out, true);
});
