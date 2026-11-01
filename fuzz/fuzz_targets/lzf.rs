#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::cache::lzf_decompress;

fuzz_target!(|data: &[u8]| {
    let Some((len, input)) = data.split_first_chunk::<2>() else {
        return;
    };
    let mut out = vec![0u8; usize::from(u16::from_le_bytes(*len))];
    let _ = lzf_decompress(input, &mut out);
});
