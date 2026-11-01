#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::toc::Toc;

fuzz_target!(|data: &[u8]| {
    if let Ok(toc) = Toc::parse(data) {
        for e in &toc.entries {
            let _ = toc.get(&e.path);
            let _ = e.is_compressed();
        }
    }
});
