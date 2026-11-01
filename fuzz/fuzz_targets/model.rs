#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::model::{Model, ModelHeader};

fuzz_target!(|data: &[u8]| {
    let [head, body, full] = wf_unpack_fuzz::split::<3>(data);
    let Ok(header) = ModelHeader::parse(head) else {
        return;
    };
    let _ = (header.scale(), header.materials());
    if let Ok(m) = Model::from_parts(header, body, full) {
        let _ = m.check_bounds();
        let _ = m.bounds();
        for lod in 0..4 {
            let _ = m.usable_lod(lod);
            for mesh in 0..m.header.meshes.len().min(8) {
                let _ = m.lod_indices(mesh, lod);
            }
        }
    }
});
