//! Tests against a real install. Skipped unless `WF_GAME_DIR` points at a
//! `Cache.Windows` folder, e.g. `WF_GAME_DIR=~/Games/Warframe/Cache.Windows`.

use std::path::PathBuf;

use wf_unpack::cache::Package;
use wf_unpack::texture::{Format, Texture};

fn game_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("WF_GAME_DIR")?);
    assert!(
        dir.join("H.Texture.toc").exists(),
        "WF_GAME_DIR has no H.Texture.toc: {}",
        dir.display()
    );
    Some(dir)
}

#[test]
fn first_entries_of_every_package_decompress() {
    let Some(dir) = game_dir() else { return };
    let mut tocs: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toc"))
        .collect();
    tocs.sort();
    assert!(!tocs.is_empty());
    for toc in tocs {
        let mut pkg = Package::open(&toc).unwrap();
        assert!(pkg.toc.entries.iter().all(|e| e.path.starts_with('/')));
        let entries: Vec<_> = pkg.toc.entries.iter().take(50).cloned().collect();
        for e in &entries {
            let data = pkg
                .read(e)
                .unwrap_or_else(|err| panic!("{}:{}: {err:#}", pkg.name, e.path));
            assert_eq!(data.len(), e.len as usize);
        }
    }
}

#[test]
fn texture_with_body_and_full_parts() {
    let Some(dir) = game_dir() else { return };
    let tex = Texture::load(&dir, "/Lotus/Interface/Icons/Miscellaneous/Kuva.png").unwrap();
    assert_eq!(tex.format, Format::Bc3);
    assert_eq!((tex.width, tex.height, tex.mips.len()), (512, 512, 10));
    let dds = tex.to_dds();
    assert_eq!(&dds[..4], b"DDS ");
    let rgba = tex.to_rgba().unwrap();
    assert_eq!(rgba.len(), 512 * 512 * 4);
    // An icon on a transparent background: the corner is see-through, the centre is not
    assert_eq!(rgba[3], 0);
    assert!(rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
}

#[test]
fn static_model_with_body_and_full_parts() {
    use wf_unpack::model::Model;
    let Some(dir) = game_dir() else { return };
    let model = Model::load(
        &dir,
        "/Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx",
    )
    .unwrap();
    assert_eq!(model.vertices.len(), 4138);
    assert_eq!(model.indices.len(), 36393);
    assert_eq!(model.header.meshes.len(), 1);
    let lod_triangles: Vec<usize> = (0..5)
        .map(|l| model.lod_indices(0, l).unwrap().len() / 3)
        .collect();
    assert_eq!(lod_triangles, [4112, 3084, 2262, 1645, 1028]);
    assert_eq!(model.header.materials(), ["PrimeGalatinePBR"]);
    model.check_bounds().unwrap();
    // Unit normals
    assert!(model
        .vertices
        .iter()
        .all(|v| (v.normal.iter().map(|c| c * c).sum::<f32>() - 1.0).abs() < 1e-3));
}

#[test]
fn galatine_material_resolves_to_its_textures() {
    use wf_unpack::cache::Package;
    use wf_unpack::material::{Material, Plan, Source};
    use wf_unpack::packages::PackagesBin;
    let Some(dir) = game_dir() else { return };
    let mut misc = Package::open(&dir.join("H.Misc.toc")).unwrap();
    let bin = PackagesBin::parse(&misc.read_path("/Packages.bin").unwrap().unwrap()).unwrap();
    let mut texts = bin.texts().unwrap();
    let folder = "/Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/";
    let m = Material::resolve(&bin, &mut texts, &format!("{folder}PrimeGalatinePBR"), "").unwrap();
    assert_eq!(bin.parent(&m.path), Some("/EE/Materials/Defaults/PBR"));
    let plan = Plan::new(&m, |_| true);
    let pack = format!("{folder}PrimeGalatinePackMap");
    assert_eq!(plan.albedo, Source::Channel(pack.clone(), 1));
    assert_eq!(plan.roughness, Source::Channel(pack.clone(), 2));
    assert_eq!(plan.metalness, Source::Channel(pack, 0));
    assert_eq!(
        plan.normal.as_deref(),
        Some(&*format!("{folder}PrimeGalatine_n.png"))
    );
    assert_eq!(
        plan.tint.as_ref().unwrap().mask,
        format!("{folder}PrimeGalatine_t.png")
    );
    assert_eq!(
        plan.emissive.as_ref().unwrap().0,
        format!("{folder}PrimeGalatine_e.png")
    );
    assert_eq!(
        plan.tint.as_ref().unwrap().colors[3],
        [0.84, 0.56, 0.1, 0.5]
    );
}

#[test]
fn tint_mask_pack_map_material_resolves_its_mask() {
    use wf_unpack::cache::Package;
    use wf_unpack::header::CommonHeader;
    use wf_unpack::material::{MaskLayout, Material, Plan};
    use wf_unpack::packages::PackagesBin;
    let Some(dir) = game_dir() else { return };
    let mut misc = Package::open(&dir.join("H.Misc.toc")).unwrap();
    let bin = PackagesBin::parse(&misc.read_path("/Packages.bin").unwrap().unwrap()).unwrap();
    let mut texts = bin.texts().unwrap();
    let path = "/Lotus/Weapons/Tenno/Pistols/PrimeSicarus/PrimeSicarusPBR";
    // This material keeps its properties in its H.Misc attribute text
    let own = CommonHeader::parse(&misc.read_path(path).unwrap().unwrap())
        .unwrap()
        .attributes;
    let m = Material::resolve(&bin, &mut texts, path, &own).unwrap();
    assert_eq!(m.get("DETAILING"), Some("TINT_MASK_PACK_MAP"));
    let plan = Plan::new(&m, |_| true);
    let tint = plan.tint.expect("tint mask");
    assert_eq!(
        tint.mask,
        "/Lotus/Weapons/Tenno/Pistols/PrimeSicarus/PrimeSicarusPackMap"
    );
    assert_eq!(tint.layout, MaskLayout::PackAlphaQuadrants);
    assert!(!tint.default_colors);
    assert_eq!(tint.colors[3], [0.84, 0.56, 0.1, 0.5]);
    assert!(plan.unsupported.is_empty());
}

#[test]
fn excalibur_body_with_skeleton() {
    use wf_unpack::model::Model;
    let Some(dir) = game_dir() else { return };
    let model = Model::load(&dir, "/Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx").unwrap();
    assert_eq!(model.vertices.len(), 3882);
    assert_eq!(model.indices.len(), 57348);
    let sk = model.skeleton.as_ref().unwrap();
    assert_eq!(sk.bones.len(), 83);
    assert_eq!(sk.joints.len(), 81);
    assert_eq!(sk.bones[0].name, "GAME_C1_ROOT");
    assert_eq!(
        sk.bones[sk
            .bones
            .iter()
            .position(|b| b.name == "GAME_L1_ARM1")
            .unwrap()]
        .parent
        .map(|p| sk.bones[p].name.as_str()),
        Some("GAME_L1_CLAV1")
    );
    assert!(sk.bind_error() < 1e-5);
    for v in &model.vertices {
        assert!((v.weights.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        assert!(v.joints.iter().all(|&j| (j as usize) < sk.joints.len()));
    }
}

#[cfg(feature = "bridge")]
#[test]
fn bridge_provider_loads_valid_models_in_metres() {
    use rewrite_bridge::{AssetKind, Provider};
    use wf_unpack::bridge::Warframe;
    let Some(dir) = game_dir() else { return };
    let wf = Warframe::open(&dir).unwrap();
    let listed = wf
        .list("/Lotus/Characters/Tenno/Excalibur/Excalibur_skel")
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, AssetKind::Character);
    assert_eq!(
        wf.list("PrimeGalatine/PrimeGalatine.fbx").unwrap()[0].kind,
        AssetKind::Weapon
    );

    let exc = wf.load(&listed[0].id).unwrap();
    exc.validate().unwrap();
    assert_eq!(exc.skins.len(), 1);
    assert_eq!(exc.skins[0].joints.len(), 81);
    assert!(exc.materials.iter().all(|m| m.base_color.is_some()));
    // Warframe's units are metres: Excalibur is a bit under 2 m tall, feet at 0
    let (lo, hi) = exc.bounds().unwrap();
    let height = hi[1] - lo[1];
    assert!(
        (1.7..2.0).contains(&height),
        "Excalibur is {height} units tall"
    );
    assert!(lo[1].abs() < 0.05);

    let sword = wf
        .load("/Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx")
        .unwrap();
    sword.validate().unwrap();
    assert!(sword.skins.is_empty());
    assert_eq!(
        sword
            .meshes
            .iter()
            .map(|m| m.indices.len() / 3)
            .sum::<usize>(),
        4112
    );
    assert!(sword.materials[0].normal.is_some() && sword.materials[0].metallic_roughness.is_some());
}

#[test]
fn excalibur_jog_animation() {
    use wf_unpack::animation::{AnimationPackages, Target};
    use wf_unpack::model::Model;
    let Some(dir) = game_dir() else { return };
    let path = "/Lotus/Animations/Tenno/Movement/MeleeWeapon/Sword/JogForward_anim.fbx";
    let clips = AnimationPackages::open(&dir).unwrap().load(path).unwrap();
    assert_eq!(clips.len(), 1);
    let a = &clips[0];
    assert_eq!(
        (a.skeleton.as_str(), a.name.as_str()),
        ("ExcaliburBody1", "Take 001")
    );
    assert_eq!(a.bones.len(), 81);
    assert_eq!(a.times.len(), 26);
    // 25 intervals of 1/30 s
    assert!((a.duration - 25.0 / 30.0).abs() < 1e-6);
    assert!((a.times[1] - 1.0 / 30.0).abs() < 1e-6);
    assert_eq!(a.times.last().copied(), Some(a.duration));

    // The root travels 3.52 m forward; the header says so too
    assert!((a.root_translation[2] - 3.519).abs() < 1e-3);
    let moved = a.root_displacement().unwrap();
    assert!((moved[2] - a.root_translation[2]).abs() < 1e-3);
    assert!(moved[0].abs() < 1e-4 && moved[1].abs() < 1e-4);

    // Footsteps: left then right, half a cycle apart
    let steps: Vec<_> = a
        .events
        .iter()
        .filter(|e| e.name.starts_with("Step"))
        .collect();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        (steps[0].name.as_str(), steps[1].name.as_str()),
        ("StepL", "StepR")
    );
    assert!((steps[1].phase - steps[0].phase - 0.45).abs() < 1e-3);

    // Every bone is in the Excalibur model with the same parent
    let model = Model::load(&dir, "/Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx").unwrap();
    let sk = model.skeleton.unwrap();
    for b in &a.bones {
        let m = sk
            .bones
            .iter()
            .find(|m| m.name == b.name)
            .unwrap_or_else(|| panic!("{} missing", b.name));
        let parent_name = m.parent.map(|p| sk.bones[p].name.as_str());
        assert_eq!(
            parent_name,
            b.parent.map(|p| a.bones[p].name.as_str()),
            "{}",
            b.name
        );
    }

    // Decoded rotations are unit quaternions, and the first key is the stored initial transform
    for t in &a.tracks {
        for q in t.rotation.iter().flatten() {
            let n: f32 = q.iter().map(|v| v * v).sum();
            assert!((n - 1.0).abs() < 1e-3);
        }
        if let Some(r) = &t.rotation {
            let d: f32 = (0..4)
                .map(|i| r[0][i] * t.initial_rotation[i])
                .sum::<f32>()
                .abs();
            assert!(d > 0.9999, "first key differs from the initial rotation");
        }
    }

    // glTF-style channels: translation and rotation for every bone, mirrored to right-handed
    let ch = a.channels(true);
    assert_eq!(ch.len(), 162);
    let root = ch
        .iter()
        .find(|c| c.node == 0 && c.target == Target::Translation)
        .unwrap();
    assert_eq!(root.times.len(), 26);
    assert!((root.values[3 * 25 + 2] - 3.519).abs() < 1e-3);
    let hip = ch
        .iter()
        .find(|c| c.node == 1 && c.target == Target::Translation)
        .unwrap();
    // The hip bone's x is negated by the mirror
    let game_x = a.tracks[1].translation.as_ref().unwrap()[0][0];
    assert_eq!(hip.values[0], -game_x);
}
