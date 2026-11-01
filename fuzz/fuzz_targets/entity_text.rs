#![no_main]
use libfuzzer_sys::fuzz_target;
use wf_unpack::clipspec::clip_spec;
use wf_unpack::ee::Attributes;
use wf_unpack::level::{Resolved, TypeResolver};
use wf_unpack::material::Plan;
use wf_unpack::packages::PackagesBin;
use wf_unpack::postprocess::{overlay_tables, PostFxMaterial};

// The property text of three chained Packages.bin entities (and a fourth text, an object's own
// attributes), read through the type, material, clip and post-process readers
fuzz_target!(|data: &[u8]| {
    let [a, b, c, own] = wf_unpack_fuzz::split::<4>(data);
    let bin = PackagesBin::parse(&wf_unpack_fuzz::packages_bin([a, b, c]))
        .expect("the builder makes a valid Packages.bin");
    let own_text = wf_unpack_fuzz::text(own);
    let Ok(mut types) = TypeResolver::new(&bin) else {
        return;
    };
    for path in ["/F/A", "/F/B", "/F/C"] {
        if let Ok(mut spec) = clip_spec(&mut types, path) {
            let _ = spec.apply_header(&own_text);
        }
        if let Ok(Some(m)) = types.material(path) {
            let _ = Plan::new(&m, |_| true);
            let _ = Plan::new(&m, |_| false);
            let _ = PostFxMaterial::from_properties(path, &m.props);
        }
        if let Ok(Some(ty)) = types.resolve(path) {
            for layers in ty.layers.values() {
                let _ = overlay_tables(layers.iter()).map(|p| p.to_json());
            }
            if let Ok(own) = Attributes::parse(&own_text) {
                let r = Resolved {
                    own: &own,
                    ty: Some(&ty),
                    level_base: "/Lotus/Levels/X/Level/",
                };
                for (k, _) in r.all_text() {
                    let _ = (r.get(&k), r.text(&k), r.float(&k), r.floats(&k));
                    let _ = (r.flag(&k), r.path(&k), r.paths(&k));
                }
            }
        }
    }
});
