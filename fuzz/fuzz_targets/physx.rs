#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::physx::{parse_convex_mesh, parse_triangle_mesh};

fuzz_target!(|data: &[u8]| {
    let _ = parse_triangle_mesh(data);
    let _ = parse_convex_mesh(data);
});
