#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::model::{decode_float_vertex, decode_skinned_vertex, decode_vertex};

// The decoders take one record of exactly the documented size (callers chunk the vertex data)
fuzz_target!(|data: &[u8]| {
    let mut raw = data.to_vec();
    raw.resize(32, 0);
    let _ = decode_vertex(&raw[..24], [1.0, 1.0, 1.0]);
    let _ = decode_float_vertex(&raw);
    let _ = decode_skinned_vertex(&raw, [1.0, 1.0, 1.0]);
});
