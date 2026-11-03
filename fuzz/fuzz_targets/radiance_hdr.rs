#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = wf_unpack::probe::read_radiance_hdr(data);
});
