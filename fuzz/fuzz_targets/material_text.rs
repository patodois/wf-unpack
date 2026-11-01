#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::material::{folder, parse_floats, parse_properties, resolve_path};

fuzz_target!(|data: &[u8]| {
    let [a, b] = wf_unpack_fuzz::split::<2>(data);
    let (a, b) = (wf_unpack_fuzz::text(a), wf_unpack_fuzz::text(b));
    for (_, v) in parse_properties(&a) {
        let _ = parse_floats(&v);
    }
    let _ = parse_floats(&a);
    let _ = folder(&a);
    let _ = resolve_path(&a, &b);
});
