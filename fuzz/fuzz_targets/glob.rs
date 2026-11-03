#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::toc::{clip_filter_matches, glob_match};

fuzz_target!(|data: &[u8]| {
    let [pattern, path] = wf_unpack_fuzz::split::<2>(data);
    let (pattern, path) = (wf_unpack_fuzz::text(pattern), wf_unpack_fuzz::text(path));
    let _ = glob_match(&pattern, &path);
    let _ = clip_filter_matches(&pattern, &path);
});
