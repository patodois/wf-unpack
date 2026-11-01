#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::packages::PackagesBin;

fuzz_target!(|data: &[u8]| {
    let Ok(bin) = PackagesBin::parse(data) else {
        return;
    };
    let paths: Vec<String> = bin.paths().take(64).map(str::to_owned).collect();
    let Ok(mut texts) = bin.texts() else {
        return;
    };
    for p in &paths {
        let _ = bin.lineage(p);
        let _ = bin.parent(p);
        let _ = texts.text(p);
    }
});
