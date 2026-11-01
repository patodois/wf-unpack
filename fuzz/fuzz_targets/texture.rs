#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::texture::{CubeTexture, Texture, TextureHeader};

fuzz_target!(|data: &[u8]| {
    let [head, body, full] = wf_unpack_fuzz::split::<3>(data);
    let Ok(header) = TextureHeader::parse(head) else {
        return;
    };
    let _ = header.level_sizes();
    let _ = header.layout(body.len(), full.len());
    if let Ok(h2) = TextureHeader::parse(head) {
        let _ = CubeTexture::from_parts(h2, body, full);
    }
    if let Ok(t) = Texture::from_parts(header, body, full) {
        let _ = t.to_dds();
        let _ = t.to_rgba();
        let _ = t.to_png();
    }
});
