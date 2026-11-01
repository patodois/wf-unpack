#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::cache::{block_header, decompress};

// The first four bytes are the declared decompressed size (what a .toc entry says), the rest the entry
fuzz_target!(|data: &[u8]| {
    let Some((len, entry)) = data.split_first_chunk::<4>() else {
        return;
    };
    let len = u32::from_le_bytes(*len) as usize;
    let _ = block_header(entry);
    let _ = decompress(entry, len);
});
