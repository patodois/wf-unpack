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
fn dojo_obstacle_course_level() {
    use wf_unpack::level::{load_description, InstanceRole};
    use wf_unpack::model::Model;
    use wf_unpack::physx::{read_shapes, Collider};
    let Some(dir) = game_dir() else { return };
    let path = "/Lotus/Levels/ClanDojo/ClanObstacleCourseA.level";
    let mut d = load_description(&dir, path).unwrap();
    // One CSG zone and 498 placed models of 38 kinds; nothing starts hidden
    assert_eq!((d.instances.len(), d.models.len()), (499, 39));
    assert_eq!(
        d.instances
            .iter()
            .filter(|i| i.role == InstanceRole::Zone)
            .count(),
        1
    );
    assert_eq!(d.instances.iter().filter(|i| i.hidden).count(), 0);
    assert_eq!((d.lights.len(), d.volumes.len(), d.spawns.len()), (8, 4, 0));
    assert!(d.static_levels.is_empty() && d.sheets.is_empty());
    let l = &d.lights[0];
    assert_eq!(l.color, Some([204.0, 220.0, 255.0, 255.0]));
    assert_eq!(
        (l.brightness, l.lumens, l.radius),
        (Some(6.25), Some(10000.0), Some(30.0))
    );
    assert_eq!(l.temperature, Some(10000.0));
    let zone = d
        .instances
        .iter()
        .find(|i| i.role == InstanceRole::Zone)
        .unwrap()
        .clone();
    assert_eq!(
        d.models[zone.model],
        "/Lotus/Levels/ClanDojo/ClanObstacleCourseA/0_c.fbx"
    );
    assert_eq!(zone.lightmap.as_ref().unwrap().textures.len(), 2);

    // The zone's geometry: 9005 vertices, 8 materials, and a PhysX triangle mesh of it
    let geometry = Model::load(&dir, &d.models[zone.model]).unwrap();
    assert_eq!(
        (geometry.vertices.len(), geometry.indices.len()),
        (9005, 18498)
    );
    assert_eq!(geometry.header.materials().len(), 8);
    geometry.check_bounds().unwrap();
    let mut body = Package::open_in(&dir, "B.Misc").unwrap().unwrap();
    let raw = body.read_path(&d.models[zone.model]).unwrap().unwrap();
    let shapes = read_shapes(&geometry.header.physx_shapes, &raw).unwrap();
    let Collider::Triangles(mesh) = &shapes[0].collider else {
        panic!("expected a triangle mesh")
    };
    assert_eq!((mesh.vertices.len(), mesh.indices.len() / 3), (2774, 5766));

    // The placed models stand inside the zone's box, and every one has readable collision
    let (lo, hi) = geometry.bounds();
    for i in &d.instances {
        let p = i.transform.translation;
        for k in 0..3 {
            assert!(
                p[k] >= lo[k] - 1.0 && p[k] <= hi[k] + 1.0,
                "{p:?} outside {lo:?}..{hi:?}"
            );
        }
    }
    let failures = d.load_collision(&dir).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(d.collision.len(), d.models.len());
    assert!(d.collision.iter().flatten().any(|s| !s.is_empty()));
    assert_eq!(d.collision_shapes(zone.model).unwrap().len(), 1);
}

#[test]
fn static_level_of_a_hub() {
    use wf_unpack::level::StaticLevel;
    let Some(dir) = game_dir() else { return };
    let path = "/Lotus/Levels/Hub/TennoProtoRelayHUBSummer/0.static";
    let read = |name: &str| {
        Package::open_in(&dir, name)
            .unwrap()
            .unwrap()
            .read_path(path)
            .unwrap()
            .unwrap()
    };
    let s = StaticLevel::parse(&read("H.AnimRetarget"), &read("B.AnimRetarget")).unwrap();
    assert_eq!((s.objects.len(), s.models.len()), (9335, 969));
    assert!(s.models[0].starts_with("/Lotus/Objects/Lasria/"));
    // Every object is listed under at least one zone group
    let mut listed = vec![false; s.objects.len()];
    for &i in &s.group_indices {
        listed[i as usize] = true;
    }
    assert!(listed.iter().all(|&b| b));
}
