#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::header::CommonHeader;

fuzz_target!(|data: &[u8]| {
    let _ = CommonHeader::parse(data);
});
