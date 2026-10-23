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

#[cfg(feature = "bridge")]
mod bridge {
    use super::game_dir;
    use rewrite_bridge::{
        AssetKind, Environment, Interpolation, LightKind, Provider, SceneModel, Target,
    };
    use wf_unpack::bridge::{animated_id, collision_id, Warframe};
    use wf_unpack::bridge_scene::SceneOptions;

    const EXCALIBUR: &str = "/Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx";
    const SWORD: &str = "/Lotus/Animations/Tenno/Movement/MeleeWeapon/Sword/";
    const DOJO: &str = "/Lotus/Levels/ClanDojo/ClanObstacleCourseA.level";

    #[test]
    fn excalibur_with_sword_clips_validates_and_has_root_motion() {
        let Some(dir) = game_dir() else { return };
        let wf = Warframe::open(&dir).unwrap().with_textures(false);
        let id = animated_id(EXCALIBUR, SWORD);
        let animated = wf.load_animated(&id).unwrap();
        animated.validate().unwrap();
        // The Sword folder holds 38 clips that fit Excalibur's bones
        assert_eq!(animated.animations.len(), 38);
        let jog_name = format!("{SWORD}JogForward_anim.fbx");
        let jog = animated.animation(&jog_name).expect("JogForward");
        assert!((jog.duration_s - 25.0 / 30.0).abs() < 1e-6);
        // Every bone has a translation and a rotation channel, on the model's own nodes
        assert_eq!(jog.channels.len(), 162);
        let root = animated
            .model
            .nodes
            .iter()
            .position(|n| n.name == "GAME_C1_ROOT")
            .unwrap();
        assert_eq!(animated.model.nodes[root].parent, Some(0));
        let t = jog
            .channels
            .iter()
            .find(|c| c.node == root && c.target == Target::Translation)
            .unwrap();
        // Root motion is the root bone's translation track: 26 keys, 3.52 m forward (z is kept)
        assert_eq!(t.times.len(), 26);
        assert_eq!(t.interpolation, Interpolation::Linear);
        assert!((t.values[3 * 25 + 2] - 3.519).abs() < 1e-3);
        assert!(t.values[2].abs() < 1e-3);
        // The hip's x is mirrored like the model's bones
        let hip = animated
            .model
            .nodes
            .iter()
            .position(|n| n.name == "GAME_C1_HIP1")
            .unwrap();
        let rest = animated.model.nodes[hip].translation;
        let hip_t = jog
            .channels
            .iter()
            .find(|c| c.node == hip && c.target == Target::Translation)
            .unwrap();
        assert!(
            (hip_t.values[0] - rest[0]).abs() < 0.2,
            "hip x {} against its rest pose {}",
            hip_t.values[0],
            rest[0]
        );

        // The side table: events with their times, and the root motion of the whole clip
        let info = wf.clip_info(&jog_name).unwrap();
        assert!(info.events_from_header);
        let (l, r) = (
            info.event_time("StepL").unwrap(),
            info.event_time("StepR").unwrap(),
        );
        assert!(
            (l - 0.05).abs() < 1e-3 && (r - 0.425).abs() < 1e-3,
            "{l} {r}"
        );
        assert!((info.root_translation[2] - 3.519).abs() < 1e-3);
        assert!((info.duration_s - jog.duration_s).abs() < 1e-6);
        let start = wf.clip_info(&format!("{SWORD}JumpStart_anim.fbx")).unwrap();
        assert!(
            (start.event_time("JumpStartImpulse").unwrap() - 0.4 * start.duration_s).abs() < 1e-5
        );

        // A clip with no events in its header takes them from its Packages.bin entity
        let bullet = "/Lotus/Animations/Player/Movement/JumpRollForwardB_anim.fbx";
        let b = wf.load_animated(&animated_id(EXCALIBUR, bullet)).unwrap();
        assert_eq!(b.animations.len(), 1);
        let info = wf.clip_info(bullet).unwrap();
        assert!(!info.events_from_header);
        assert!(info.event_time("BulletJumpStart").is_some());
        assert!(info.event_time("JumpStartImpulse").is_some());
        assert_eq!(info.rate, Some(0.9));
    }

    #[test]
    fn animated_ids_without_a_filter_or_with_a_wide_one() {
        let Some(dir) = game_dir() else { return };
        let wf = Warframe::open(&dir)
            .unwrap()
            .with_textures(false)
            .with_max_clips(100);
        // No suffix: the model alone, equal to `load`
        let plain = wf.load_animated(EXCALIBUR).unwrap();
        assert!(plain.animations.is_empty());
        assert_eq!(plain.model, wf.load(EXCALIBUR).unwrap());
        // `load` ignores the filter of an animated id
        assert_eq!(
            wf.load(&animated_id(EXCALIBUR, SWORD)).unwrap(),
            plain.model
        );
        // Several filters, one per `|`
        let two = wf
            .load_animated(&animated_id(
                EXCALIBUR,
                &format!("{SWORD}Idle_anim.fbx|{SWORD}Sprint_anim.fbx"),
            ))
            .unwrap();
        assert_eq!(two.animations.len(), 2);
        // A filter that matches too much is an error that says so
        let err = wf
            .load_animated(&animated_id(EXCALIBUR, "/Lotus/Animations/"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("narrow it"), "{err}");
        // A static model has no skeleton for clips to fit
        let sword = "/Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx";
        assert!(wf.load_animated(&animated_id(sword, SWORD)).is_err());
        assert!(wf.load_animated(sword).unwrap().animations.is_empty());
    }

    #[test]
    fn the_dojo_is_a_valid_scene_with_collision() {
        let Some(dir) = game_dir() else { return };
        let wf = Warframe::open(&dir).unwrap().with_textures(false);
        let listed = wf.list("ClanObstacleCourseA.level").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, AssetKind::Level);
        assert_eq!(listed[0].id, DOJO);
        assert_eq!(listed[0].name, "ClanObstacleCourseA");

        let scene = wf.load_scene(DOJO).unwrap();
        scene.validate().unwrap();
        // 1 zone and 498 props: 499 instances of 39 models, so the models are shared
        assert_eq!(scene.instances.len(), 499);
        assert_eq!(scene.models.len(), 39);
        assert!(scene
            .models
            .iter()
            .all(|m| matches!(m, SceneModel::Inline(_))));
        let used: std::collections::HashSet<_> = scene.instances.iter().map(|i| i.model).collect();
        assert_eq!(used.len(), 39);
        // The most used prop is placed 176 times
        let mut count = vec![0usize; scene.models.len()];
        for i in &scene.instances {
            count[i.model] += 1;
        }
        assert_eq!(count.iter().max(), Some(&176));

        // Placement is mirrored like the models: x negated against the level's own numbers
        let d = wf_unpack::level::load_description(&dir, DOJO).unwrap();
        assert_eq!(d.instances.len(), scene.instances.len());
        for (i, inst) in scene.instances.iter().enumerate() {
            let g = d.instances[i].transform.translation;
            assert_eq!(inst.transform.translation, [-g[0], g[1], g[2]]);
            let SceneModel::Inline(m) = &scene.models[inst.model] else {
                panic!()
            };
            assert_eq!(m.name, d.models[d.instances[i].model]);
        }

        // Eight lights in candela: 10000 lm point lights of radius 30 m and one 100 degree spot
        assert_eq!(scene.lights.len(), 8);
        let l = &scene.lights[0];
        assert!((l.intensity - 10000.0 / (4.0 * std::f32::consts::PI)).abs() < 0.01);
        assert_eq!(l.kind, LightKind::Point { range: Some(30.0) });
        assert!((l.color[0] - 0.604).abs() < 0.01 && l.color[2] == 1.0);
        let spot = scene
            .lights
            .iter()
            .find(|l| matches!(l.kind, LightKind::Spot { .. }))
            .unwrap();
        let LightKind::Spot {
            outer_cone_angle, ..
        } = spot.kind
        else {
            unreachable!()
        };
        assert!((outer_cone_angle - 50f32.to_radians()).abs() < 1e-5);
        assert!((spot.intensity - 5000.0 / (4.0 * std::f32::consts::PI)).abs() < 0.01);
        // The level's sky light colour is black
        assert_eq!(scene.environment, Some(Environment::Color([0.0; 3])));

        // The collision scene: the zone's 5766-triangle mesh and the props' shapes, as models
        let c = wf.load_scene(&collision_id(DOJO)).unwrap();
        c.validate().unwrap();
        assert_eq!(c.instances.len(), 498);
        assert_eq!(c.models.len(), 38);
        assert!(c.lights.is_empty() && c.environment.is_none());
        let zone = c
            .models
            .iter()
            .find_map(|m| match m {
                SceneModel::Inline(m) if m.name.ends_with("/ClanObstacleCourseA/0_c.fbx") => {
                    Some(m)
                }
                _ => None,
            })
            .expect("the zone's collision");
        assert_eq!(zone.meshes.len(), 1);
        assert_eq!(zone.meshes[0].name, "triangles");
        assert_eq!(zone.meshes[0].indices.len() / 3, 5766);

        // The full result carries what the bridge has no place for
        let full = wf.scene(DOJO, &SceneOptions::default()).unwrap();
        assert_eq!(full.description.volumes.len(), 4);
        assert_eq!(full.description.portals.len(), 1);
        assert_eq!(full.report.instances, 499);
        assert_eq!(full.report.collision_instances, 498);
        assert!(full.report.model_failures.is_empty());
        assert!(full.report.collision_failures.is_empty());
        assert_eq!(full.scene, scene);
        assert_eq!(full.collision, c);
    }
}

#[test]
fn the_tenno_movement_values_match_the_known_ones() {
    use wf_unpack::player::Player;
    let Some(dir) = game_dir() else { return };
    let p = Player::load(&dir).unwrap();
    assert_eq!(
        p.mesh.value,
        "/Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx"
    );
    assert_eq!(p.ground.max_speed.value, 7.5);
    assert_eq!(
        p.ground.max_speed.cite(),
        "/Lotus/Types/Player/TennoWalkAttribs: MaxSpeed"
    );
    assert_eq!(
        (p.ground.walk_fraction.value, p.ground.jog_fraction.value),
        (0.4, 0.8)
    );
    assert_eq!(p.jump.height.value, 5.0);
    assert_eq!(p.jump.num_jumps.value, 2.0);
    assert_eq!(p.jump.air_jump_height_multiplier.value, 0.75);
    assert_eq!(p.jump.num_jumps.entity, "/Lotus/Types/Player/TennoAvatar");
    assert_eq!(p.gravity.world.value, [0.0, -15.0, 0.0]);
    assert_eq!(p.gravity.additional.value, -9.0);
    assert_eq!((p.rise_gravity(), p.fall_gravity()), (15.0, 24.0));
    assert_eq!(
        (p.capsule.height.value, p.capsule.radius.value),
        (1.5, 0.45)
    );
    assert_eq!(p.capsule.slope_limit.value, 65.0);
    assert_eq!(
        p.capsule.slope_limit.entity,
        "/Lotus/Types/Player/TennoWalkAttribs"
    );
    assert_eq!(
        (
            p.ground.acceleration.value,
            p.ground.accel_drag.value,
            p.ground.decel_drag.value
        ),
        (12.0, 8.5, 10.0)
    );
    assert_eq!(
        (
            p.air.acceleration.value,
            p.air.accel_drag.value,
            p.air.decel_drag.value
        ),
        (10.0, 0.05, 0.2)
    );
    assert_eq!((p.slide.accel.value, p.slide.drag.value), (6.0, 0.7));
    assert_eq!(p.slide.end_threshold.value, 1.66);
    assert_eq!(p.slide.jump_speed.entity, "/EE/Types/Game/SlideAction");
    assert_eq!(p.bullet_jump.speed.value, 2.5);
    assert_eq!(
        p.bullet_jump.speed.entity,
        "/Lotus/Types/Player/TennoClimbController"
    );
    assert_eq!(p.camera.view_offset.value, [0.5, -0.1, -1.5]);
    assert_eq!(p.camera.base_fov.value, 65.0);
    assert_eq!(p.camera.pitch_clamp.value, [-89.0, 89.0]);
    assert_eq!(p.camera.collision_radius.value, 0.25);
    // The crouch button: a tap rolls or slides, a hold crouches, jump and melee cancel the tap
    assert_eq!(p.crouch.input_code, "PRE_CROUCH");
    assert_eq!(p.crouch.tap_codes, ["ACTION"]);
    assert_eq!(p.crouch.hold_codes, ["HOLD_CROUCH"]);
    assert_eq!(p.crouch.bypass_tap_codes, ["JUMP", "MELEE"]);
    assert!((p.crouch.required_hold_time.value - 0.15).abs() < 1e-6);
    assert!(p.crouch.force_hold_when_directionless && !p.crouch.send_hold_immediately);
    // The roll covers 8 m through the entity its clip derives from
    assert_eq!(p.roll.clip.root_override, Some([0.0, 0.0, 8.0]));
    assert_eq!(
        p.roll.clip.root_conditioning.as_deref(),
        Some("ROOTC_ADJUST_LINEAR")
    );
    assert_eq!(
        p.roll.clip.end_events,
        ["DodgeRollFinished", "EnableLookIK"]
    );
    assert!(p
        .bullet_jump
        .clip
        .events
        .iter()
        .any(|e| e.name == "BulletJumpStart"));
}
