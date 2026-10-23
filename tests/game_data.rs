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

/// Only whole characters are `Character`; the folder also holds what is worn on them.
#[cfg(feature = "bridge")]
#[test]
fn only_whole_characters_are_listed_as_characters() {
    use rewrite_bridge::{AssetKind, Provider};
    use wf_unpack::bridge::Warframe;
    let Some(dir) = game_dir() else { return };
    let wf = Warframe::open(&dir).unwrap();
    let kind = |id: &str| {
        let l = wf.list(id).unwrap();
        l.iter()
            .find(|a| a.id == id)
            .unwrap_or_else(|| panic!("{id} is not listed"))
            .kind
    };
    for whole in [
        "/Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx",
        "/Lotus/Characters/Tenno/Excalibur/ExcaliburBody_skel.fbx",
        "/Lotus/Characters/Tenno/Rhino/Rhino_skel.fbx",
    ] {
        assert_eq!(kind(whole), AssetKind::Character, "{whole}");
    }
    for part in [
        // helmets weight a few bones of the body's rig
        "/Lotus/Characters/Tenno/Excalibur/ExcaliburHelmet_skel.fbx",
        // an armour plate and a cloth reference mesh are static models
        "/Lotus/Characters/Tenno/Antimatter/NovaDeluxeUpArmLB.fbx",
        "/Lotus/Characters/Zariman/Holdfasts/Yonta/Cloth/YontaCapeRefMesh.fbx",
        // cloth has a rig of its own
        "/Lotus/Characters/Solaris/SolarisYouthOne/Cloth/YouthOneApron_skel.fbx",
    ] {
        assert_eq!(kind(part), AssetKind::Prop, "{part}");
    }
    // The folder holds characters and the props worn on them, nothing else
    assert!(wf
        .list("/Lotus/Characters/")
        .unwrap()
        .iter()
        .all(|a| matches!(a.kind, AssetKind::Character | AssetKind::Prop)));
}

/// The provider bakes a material the way the glTF export does: the same plan and bake, so the
/// same layered tint (0.5 lerped towards each tint colour by its mask, doubled), the same pixels.
#[cfg(feature = "bridge")]
#[test]
fn provider_base_colour_is_the_export_base_colour() {
    use rewrite_bridge::Provider;
    use wf_unpack::bridge::Warframe;
    use wf_unpack::pbr::{srgb_to_linear, MaterialBaker};
    let Some(dir) = game_dir() else { return };
    let id = "/Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx";
    let model = Warframe::open(&dir).unwrap().load(id).unwrap();
    let names = wf_unpack::model::Model::load(&dir, id)
        .unwrap()
        .header
        .materials();
    let mut baker = MaterialBaker::open(&dir).unwrap();
    for (name, material) in names.iter().zip(&model.materials) {
        let plan = baker.plan(id, name).unwrap();
        let baked = baker.bake(&plan).unwrap();
        let (export, provider) = (
            baked.base_color.unwrap(),
            material.base_color.as_ref().unwrap(),
        );
        assert_eq!(
            (export.width, export.height, &export.rgba),
            (provider.width, provider.height, &provider.rgba),
            "{name}"
        );
        assert_eq!(baked.base_color_factor, material.base_color_factor);
        // Excalibur is dark steel, not black: the untinted pipeline gave a mean linear albedo
        // of 0.027, the layered tint 0.051 (body) and 0.073 (helmet)
        let texels = provider.rgba.as_chunks::<4>().0;
        let n = texels.len() as f64;
        let mean = texels
            .iter()
            .map(|p| {
                (0..3)
                    .map(|k| f64::from(srgb_to_linear(f32::from(p[k]) / 255.0)))
                    .sum::<f64>()
                    / 3.0
            })
            .sum::<f64>()
            / n;
        assert!(mean > 0.04, "{name}: mean linear albedo {mean:.4}");
    }
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

fn excalibur_animtree() -> (
    wf_unpack::animtree::SuitAnimation,
    wf_unpack::animtree::Tree,
    Vec<Option<wf_unpack::animtree::Fsm>>,
) {
    use wf_unpack::animtree::{suit_animation, GraphPackages};
    use wf_unpack::level::TypeResolver;
    use wf_unpack::material::{folder, resolve_path};
    use wf_unpack::packages::PackagesBin;
    let dir = game_dir().unwrap();
    let bin = PackagesBin::load(&dir).unwrap();
    let mut types = TypeResolver::new(&bin).unwrap();
    let suit = suit_animation(&mut types, "/Lotus/Powersuits/Excalibur/ExcaliburBaseSuit").unwrap();
    let mut pk = GraphPackages::open(&dir).unwrap();
    let tree = pk.tree(&suit.anim_tree).unwrap().unwrap();
    let base = folder(&suit.anim_tree).to_string();
    let fsms = tree
        .fsms
        .iter()
        .map(|p| pk.fsm(&resolve_path(&base, p)).unwrap())
        .collect();
    (suit, tree, fsms)
}

#[test]
fn excalibur_controller_and_tree() {
    use wf_unpack::animtree::Node;
    if game_dir().is_none() {
        return;
    }
    let (suit, tree, fsms) = excalibur_animtree();
    assert_eq!(
        suit.mesh.as_deref(),
        Some("/Lotus/Characters/Tenno/Excalibur/ExcaliburBody_skel.fbx")
    );
    assert_eq!(
        suit.animation_set.as_deref(),
        Some("/Lotus/Upgrades/Skins/Excalibur/ExcaliburNobleAnims")
    );
    assert_eq!(suit.subtree_overrides.len(), 1);
    assert_eq!(suit.subtree_overrides[0].0, "PRN_IDLE_OVERRIDE");
    assert!(suit.controller_is_default);
    assert_eq!(suit.controller, "/Lotus/Types/Player/TennoAnimController");
    assert_eq!(
        suit.anim_tree,
        "/Lotus/Types/Player/Animation/SimpleTennoAnimTree.animtree"
    );
    // The IV_ names the controller assigns
    assert!(suit
        .controller_variables
        .iter()
        .any(|(v, x)| v == "IV_JUMP" && x == "JS_JUMPING"));

    assert_eq!(tree.nodes.len(), 1936);
    assert_eq!(
        (tree.clips.len(), tree.fsms.len(), tree.subtrees.len()),
        (910, 45, 10)
    );
    assert!(tree.errors.is_empty());
    // Every state machine the tree names decodes
    assert!(fsms.iter().all(Option::is_some));
    // Every node decodes and every edge points at a node
    let n = tree.nodes.len() as i32;
    assert!(tree
        .graph
        .edges
        .iter()
        .flatten()
        .all(|&c| (-1..n).contains(&c)));
    // The root of the tree is the one node nothing points to
    assert_eq!(tree.root(), Some(0));
    // Every clip index in the nodes is in the pool
    for node in &tree.nodes {
        for c in node.clip_refs() {
            assert!((c as usize) < tree.clips.len());
        }
    }
    // Clips are named by path, without a folder prefix problem
    assert!(tree.clips.iter().all(|c| c.starts_with('/')));
    // The direction blend: five clips by the angle between movement and facing
    assert!(tree.nodes.iter().any(|n| matches!(n,
        Node::SingleAxis { var: 65, names, points, .. }
            if names == &["BackLeft", "Left", "Forward", "Right", "BackRight"]
                && points.iter().map(|p| p.pos).collect::<Vec<_>>() == [-180.0, -90.0, 0.0, 90.0, 180.0])));
}

#[test]
fn excalibur_locomotion_state_machine() {
    use wf_unpack::animtree::{find_locomotion, Node};
    if game_dir().is_none() {
        return;
    }
    let (_, tree, fsms) = excalibur_animtree();
    let loco = find_locomotion(&tree, &fsms).unwrap();
    let Node::Fsm { fsm } = &tree.nodes[loco.fsm_node] else {
        panic!()
    };
    assert!(tree.fsms[*fsm as usize].ends_with("FsmTestLocomotion.animfsm"));
    let f = fsms[*fsm as usize].as_ref().unwrap();
    let names: Vec<_> = f.exposed.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Idle",
            "Moving",
            "Sprinting",
            "SprintStart",
            "SprintStop",
            "FullBodyAction",
            "Roll",
            "Slide",
            "ZipLine",
            "WallRun",
            "GrapplingHook"
        ]
    );
    assert_eq!(f.start, 0);
    let by_name = |n: &str| f.states.iter().position(|s| s.name == n).unwrap();
    let (idle, moving, sprinting) = (by_name("Idle"), by_name("Moving"), by_name("Sprinting"));
    // The state machine plays the children of its node by exposed state
    let children = &tree.graph.edges[loco.fsm_node];
    assert_eq!(children.len(), names.len());
    assert_eq!(f.child_name(&f.states[moving]), Some("Moving"));
    assert!(children[1] >= 0 && children[3] < 0);

    // Moving -> Sprinting on MoveState == SPRINT, 0.3 s; v108 is the move state
    let t = &f.states[moving].transitions[0];
    assert_eq!(t.target as usize, sprinting);
    assert_eq!(
        (t.blend, t.conditions[0].var, t.conditions[0].op_symbol()),
        (0.3, 108, "==".into())
    );
    assert_eq!(
        (t.conditions[0].text.as_str(), t.conditions[0].value),
        ("SPRINT", 2.0)
    );
    // Moving -> Slide 0.3 s, Moving -> Idle when the speed (v126) drops under 0.05
    let slide = by_name("Slide");
    let t = f.states[moving]
        .transitions
        .iter()
        .find(|t| t.target as usize == slide)
        .unwrap();
    assert_eq!(t.blend, 0.3);
    let t = f.states[moving]
        .transitions
        .iter()
        .find(|t| t.target as usize == idle)
        .unwrap();
    assert_eq!(
        (
            t.conditions[0].var,
            t.conditions[0].op_symbol().as_str(),
            t.conditions[0].text.as_str()
        ),
        (126, "<", "0.05")
    );
    assert_eq!(t.blend, 0.3);
    // Sprinting back to Moving: 0.3 s when the move state leaves SPRINT with the weapon firing, 0.05 s otherwise
    let blends: Vec<f32> = f.states[sprinting]
        .transitions
        .iter()
        .filter(|t| t.target as usize == moving)
        .map(|t| t.blend)
        .collect();
    assert_eq!(blends, [0.3, 0.05]);
    // 38 transitions in the machine; every one has a blend time
    assert_eq!(
        f.states.iter().map(|s| s.transitions.len()).sum::<usize>(),
        38
    );
    assert!(f
        .states
        .iter()
        .flat_map(|s| &s.transitions)
        .all(|t| t.blend > 0.0));
    assert!(f
        .states
        .iter()
        .flat_map(|s| &s.transitions)
        .any(|t| t.blend == 0.2));
    // The sprint events name the footfall cues
    assert_eq!(f.states[sprinting].events[0].name, "SprintStart");
    assert_eq!(f.states[sprinting].events[1].name, "SprintStopped");
}

#[test]
fn speed_blend_points_are_the_root_speeds_of_their_clips() {
    use wf_unpack::animation::AnimationPackages;
    use wf_unpack::animtree::Node;
    if game_dir().is_none() {
        return;
    }
    let (_, tree, _) = excalibur_animtree();
    let mut anim = AnimationPackages::open(&game_dir().unwrap()).unwrap();
    // The Sword's walk/jog blend: idle, walk and jog clips with their speeds and rates
    let sword =
        |c: &str| format!("/Lotus/Animations/Tenno/Movement/MeleeWeapon/Sword/{c}_anim.fbx");
    let mut seen = 0;
    for node in &tree.nodes {
        let Node::SpeedBlend { var, points, .. } = node else {
            continue;
        };
        assert_eq!(*var, 126);
        let paths: Vec<&str> = points
            .iter()
            .map(|p| tree.clips[p.clip as usize].as_str())
            .collect();
        if paths != [sword("Idle"), sword("WalkForward"), sword("JogForward")] {
            continue;
        }
        seen += 1;
        for p in points.iter().skip(1) {
            // The speed stored next to a clip is the speed its root bone moves at
            let a = &anim.load(&tree.clips[p.clip as usize]).unwrap()[0];
            let speed = a.root_translation[2] / a.duration;
            assert!(
                (speed - p.speed).abs() < 0.01,
                "{}: {speed} vs {}",
                a.path,
                p.speed
            );
        }
        assert_eq!(points[0].speed, 0.0);
        assert!((points[1].rate - 0.909).abs() < 1e-3 && (points[2].rate - 1.2).abs() < 1e-6);
    }
    assert!(seen >= 1);
}

#[test]
fn every_state_machine_and_most_trees_decode() {
    use wf_unpack::animtree::GraphPackages;
    let Some(dir) = game_dir() else { return };
    let mut pk = GraphPackages::open(&dir).unwrap();
    let fsms = pk.fsm_paths();
    assert!(fsms.len() >= 419);
    for p in &fsms {
        pk.fsm(p).unwrap_or_else(|e| panic!("{p}: {e:#}"));
    }
    let trees = pk.tree_paths();
    let ok = trees.iter().filter(|p| pk.tree(p).is_ok()).count();
    assert!(ok >= 570 && ok < trees.len(), "{ok} of {}", trees.len());
}

#[test]
fn every_clip_the_excalibur_tree_names_exists() {
    use wf_unpack::animation::AnimationPackages;
    use wf_unpack::animtree::Node;
    if game_dir().is_none() {
        return;
    }
    let (_, tree, _) = excalibur_animtree();
    let anim = AnimationPackages::open(&game_dir().unwrap()).unwrap();
    let have: std::collections::HashSet<&str> =
        anim.entries().iter().map(|e| e.path.as_str()).collect();
    let mut names: Vec<&str> = tree.clips.iter().map(String::as_str).collect();
    names.extend(tree.nodes.iter().filter_map(|n| match n {
        Node::Parametric { path } => Some(path.as_str()),
        _ => None,
    }));
    assert!(names.len() >= 1163);
    let missing: Vec<_> = names.iter().filter(|n| !have.contains(*n)).collect();
    assert!(missing.is_empty(), "{missing:?}");
}

#[test]
fn moving_state_with_a_sword() {
    use wf_unpack::animtree::{bone_mask, find_locomotion};
    use wf_unpack::level::TypeResolver;
    use wf_unpack::packages::PackagesBin;
    let Some(dir) = game_dir() else { return };
    let (_, tree, fsms) = excalibur_animtree();
    let loco = find_locomotion(&tree, &fsms).unwrap();
    // The child of the locomotion machine that plays "Moving" (exposed state 1)
    let moving = usize::try_from(tree.graph.edges[loco.fsm_node][1]).unwrap();
    let leaves = tree.leaves(moving, Some("Sword"));
    assert!(!leaves.is_empty());
    let walk = leaves
        .iter()
        .find(|l| l.kind == "SpeedBlendNode" && l.clips.len() == 3)
        .expect("a speed blend with the Sword's idle, walk and jog");
    let names: Vec<&str> = walk
        .clips
        .iter()
        .map(|c| c.clip.rsplit('/').next().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Idle_anim.fbx",
            "WalkForward_anim.fbx",
            "JogForward_anim.fbx"
        ]
    );
    let speeds: Vec<f32> = walk.clips.iter().map(|c| c.speed.unwrap()).collect();
    assert_eq!(speeds[0], 0.0);
    assert!((speeds[1] - 1.81).abs() < 0.01 && (speeds[2] - 4.22).abs() < 0.01);
    // The way there goes through the stance and the weapon class variables
    let vars: Vec<u32> = walk.path.iter().filter_map(|s| s.var).collect();
    assert!(vars.contains(&158) && vars.contains(&100) && vars.contains(&44));
    // No clip of another stance
    assert!(leaves
        .iter()
        .flat_map(|l| &l.clips)
        .all(|c| c.clip.contains("/Sword/")));

    // Layering: the bone weights of a blend mask are in Packages.bin
    let bin = PackagesBin::load(&dir).unwrap();
    let mut types = TypeResolver::new(&bin).unwrap();
    let mask = bone_mask(
        &mut types,
        "/Lotus/Types/Player/Animation/TennoUpperBodyRequestMask",
    )
    .unwrap()
    .unwrap();
    let weight = |b: &str| mask.iter().find(|m| m.bone == b).unwrap().weight;
    assert_eq!(weight("GAME_C1_HIP1"), 0.0);
    assert_eq!(weight("GAME_C1_SPINE1"), 0.75);
    assert_eq!(weight("GAME_C1_HEAD1"), 1.0);
    assert!(bone_mask(&mut types, "/Lotus/Nothing").unwrap().is_none());
}

#[test]
fn movement_and_aim_pose_sets() {
    use wf_unpack::animtree::{GraphPackages, KIND_AIM_POSE_SET, KIND_MOVEMENT_SET};
    let Some(dir) = game_dir() else { return };
    let mut pk = GraphPackages::open(&dir).unwrap();
    // The Sword's sprint: a jog, a sprint and two 90 degree turns
    let set = pk
        .movement_set("/Lotus/Animations/Tenno/Movement/MeleeWeapon/Sword/SwordSprinting")
        .unwrap()
        .unwrap();
    assert_eq!(set.kind, KIND_MOVEMENT_SET);
    assert_eq!(
        set.names,
        [
            "JogForward_anim.fbx",
            "Sprint_anim.fbx",
            "Sprint90Left_anim.fbx",
            "Sprint90Right_anim.fbx"
        ]
    );
    assert!(set
        .sources
        .iter()
        .all(|s| s.starts_with("/Lotus/Animations/Tenno/")));
    let p = set.params.unwrap();
    assert_eq!(
        (p.a, p.b, p.angle, p.turn, p.d),
        (0, 12.0, 180.0, 720.0, 10.0)
    );
    // The Bow's aim-walk set: idle, forward, back, left and right
    let aim = pk
        .movement_set("/Lotus/Animations/Tenno/Movement/Bow/BowMovement")
        .unwrap()
        .unwrap();
    assert_eq!(aim.names.len(), 5);
    assert_eq!(aim.params.unwrap().a, 1);
    let poses = pk
        .movement_set("/Lotus/Animations/Tenno/Movement/Bow/AimPoses")
        .unwrap()
        .unwrap();
    assert_eq!(poses.kind, KIND_AIM_POSE_SET);
    assert_eq!(poses.bone.as_deref(), Some("GAME_L1_WEAPON1"));
    assert!(pk.movement_set("/Lotus/Nothing").unwrap().is_none());
    // A level is not a set
    assert!(pk
        .movement_set("/Lotus/Animations/Tenno/Movement/MeleeWeapon/Sword/Idle_anim.fbx")
        .is_err());
}

const DOJO: &str = "/Lotus/Levels/ClanDojo/ClanObstacleCourseA.level";

#[test]
fn dojo_reflection_probe() {
    use wf_unpack::probe::{direction_to_face, face_direction, Probe};
    let Some(dir) = game_dir() else { return };
    let p = Probe::load(
        &dir,
        "/Lotus/Levels/ClanDojo/ClanObstacleCourseA/0_cross.hdr",
    )
    .unwrap();
    // BC6H, 128 x 128 faces, 8 mip levels
    assert_eq!((p.size, p.mip_levels), (128, 8));
    assert!(p.faces.iter().all(|f| f.len() == 128 * 128));
    // The probe holds light above 1.0 (the cyan emissive strips)
    assert!(p.max_finite() > 10.0);
    // Faces in Direct3D's order and orientation: the colours on both sides of every
    // cube edge agree. Step just outside each face edge, find the neighbouring texel
    // by direction, and compare log luminance.
    let lum = |t: [f32; 3]| (1.0 + 50.0 * (t[0] + t[1] + t[2]).min(60000.0) / 3.0).ln();
    let n = p.size;
    let (mut total, mut count) = (0.0f32, 0usize);
    for face in 0..6 {
        for side in 0..4 {
            for i in 0..n {
                let t = (i as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                let out = 1.0 + 1.0 / n as f32;
                let ((u, v), (px, py)) = match side {
                    0 => ((-out, t), (0, i)),
                    1 => ((out, t), (n - 1, i)),
                    2 => ((t, -out), (i, 0)),
                    _ => ((t, out), (i, n - 1)),
                };
                let (f2, u2, v2) = direction_to_face(face_direction(face, u, v));
                let qx = (((u2 + 1.0) / 2.0 * n as f32) as usize).min(n - 1);
                let qy = (((v2 + 1.0) / 2.0 * n as f32) as usize).min(n - 1);
                total += (lum(p.faces[face][py * n + px]) - lum(p.faces[f2][qy * n + qx])).abs();
                count += 1;
            }
        }
    }
    let mean = total / count as f32;
    assert!(mean < 0.15, "mean edge difference {mean}");
    // Swapping two faces breaks it
    let mut swapped = p.clone();
    swapped.faces.swap(0, 1);
    let mut total2 = 0.0f32;
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32 * 2.0 - 1.0;
        let (f2, u2, v2) = direction_to_face(face_direction(4, 1.0 + 1.0 / n as f32, t));
        let qx = (((u2 + 1.0) / 2.0 * n as f32) as usize).min(n - 1);
        let qy = (((v2 + 1.0) / 2.0 * n as f32) as usize).min(n - 1);
        total2 +=
            (lum(swapped.faces[4][i * n + n - 1]) - lum(swapped.faces[f2][qy * n + qx])).abs();
    }
    assert!(total2 / n as f32 > 0.0);
    // The equirect has the requested size and finite texels
    let eq = p.to_equirect(64, 32);
    assert_eq!(eq.len(), 64 * 32);
    assert!(eq
        .iter()
        .all(|t| t.iter().all(|c| c.is_finite() && *c >= 0.0)));
}

#[test]
fn dojo_lightmaps_decode() {
    use wf_unpack::level::{load_description, InstanceRole};
    use wf_unpack::lightmap::{LightmapPage, LightmapParams};
    use wf_unpack::texture::TexturePackages;
    let Some(dir) = game_dir() else { return };
    let d = load_description(&dir, DOJO).unwrap();
    let with: Vec<_> = d
        .instances
        .iter()
        .filter(|i| i.lightmap.is_some())
        .collect();
    assert_eq!(with.len(), 316);
    // The zone sits in the top left quarter of page 0, half a texel margin of 1/512 around
    let zone = d
        .instances
        .iter()
        .find(|i| i.role == InstanceRole::Zone)
        .unwrap();
    let lm = zone.lightmap.as_ref().unwrap();
    assert_eq!(
        lm.coords,
        [0.001953125, 0.001953125, 0.49609375, 0.49609375]
    );
    assert_eq!(
        lm.pages(),
        (
            Some("/Lotus/Levels/ClanDojo/ClanObstacleCourseA/0_clr.png"),
            Some("/Lotus/Levels/ClanDojo/ClanObstacleCourseA/0_dir.png"),
        )
    );
    // Two indexed light maps of four lights each; all eight names match the level's lights
    assert_eq!(lm.indexed.len(), 2);
    let mut matched: Vec<usize> = lm
        .indexed
        .iter()
        .flat_map(|ix| ix.light_indices.iter().flatten().copied())
        .collect();
    matched.sort();
    assert_eq!(matched, (0..8).collect::<Vec<_>>());
    assert_eq!(
        lm.indexed[0].texture,
        "/Lotus/Levels/ClanDojo/ClanObstacleCourseA/0_I_l1.png"
    );

    let mut pk = TexturePackages::open_family(&dir, "LightMap").unwrap();
    let (clr, dir_tex) = lm.pages();
    let page = LightmapPage::load(&mut pk, clr.unwrap(), dir_tex).unwrap();
    // clr 1024 x 1024; dir is two 512 x 512 pages at half resolution (its 2048 x 1024 top level is not shipped)
    assert_eq!((page.clr.width, page.clr.height), (1024, 1024));
    let dir_atlas = page.dir.as_ref().unwrap();
    assert_eq!((dir_atlas.width, dir_atlas.height), (1024, 512));
    // Alpha (luma) peaks at 1.0 in an object's rectangle; the colour channels peak at 1.0 too (chroma)
    let (mut amax, mut rgbmax) = (0u8, 0u8);
    for px in page.clr.rgba.as_chunks::<4>().0 {
        amax = amax.max(px[3]);
        rgbmax = rgbmax.max(px[0].max(px[1]).max(px[2]));
    }
    assert_eq!((amax, rgbmax), (255, 255));
    // The right page's G (the directional gain) reaches 1.0 and the directions are unit length where lit
    let mut gmax = 0u8;
    let mut lens = Vec::new();
    for y in 0..512usize {
        for x in 0..512usize {
            let l = &dir_atlas.rgba[(y * 1024 + x) * 4..][..2];
            let r = &dir_atlas.rgba[(y * 1024 + 512 + x) * 4..][..2];
            gmax = gmax.max(r[1]);
            if r[1] > 128 {
                let v = [
                    2.0 * f32::from(l[0]) / 255.0 - 1.0,
                    2.0 * f32::from(l[1]) / 255.0 - 1.0,
                    2.0 * f32::from(r[0]) / 255.0 - 1.0,
                ];
                lens.push((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt());
            }
        }
    }
    assert!(gmax >= 250);
    lens.sort_by(|a, b| a.total_cmp(b));
    let median = lens[lens.len() / 2];
    assert!(
        (0.9..1.1).contains(&median),
        "median direction length {median}"
    );

    // Decoding: chroma follows LightMapNormalize.rgb, luma is alpha squared times its w
    let p = LightmapParams {
        coords: lm.coords,
        normalize: lm.normalize,
        normalize1: lm.normalize1,
    };
    let s = page.sample(&p, [0.5, 0.5]);
    assert!(s.chroma.iter().all(|c| (0.0..=1.0).contains(c)));
    assert!(s.luma >= 0.0 && s.luma <= lm.normalize[3]);
    assert!(s.ambient().iter().all(|c| *c <= 1.0));
}

#[test]
fn dojo_post_process_and_zone() {
    use wf_unpack::level::{load_description, EnvironmentKind};
    use wf_unpack::postprocess::ToneMapType;
    let Some(dir) = game_dir() else { return };
    let d = load_description(&dir, DOJO).unwrap();
    let level = d
        .environments
        .iter()
        .find(|e| e.kind == EnvironmentKind::Level)
        .unwrap();
    let pp = level.lighting.post_process.as_ref().unwrap();
    // The level sets fog and the final-pass material; the type gives the exposure
    assert_eq!(pp.fog_color, Some([102.0, 110.0, 126.0, 255.0]));
    assert_eq!(pp.distance_fog_density, Some(0.17));
    assert_eq!(
        pp.post_fx_material.as_deref(),
        Some("/Lotus/Types/ACESPost")
    );
    assert!((pp.target_exposure.unwrap() - 0.07).abs() < 1e-6);
    assert_eq!(pp.adaptation_rate_up, Some(3.0));
    assert_eq!(pp.tone_map_type, None::<ToneMapType>);
    let fx = level.lighting.post_fx.as_ref().unwrap();
    assert_eq!(
        fx.shader.as_deref(),
        Some("/EE/Shaders/PostFX/FinalScenePass.hlsl")
    );
    assert_eq!(
        fx.switches.get("SIMPLE_VIGNETTE").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        fx.switches.get("BLOOM_ENABLED").map(String::as_str),
        Some("0")
    );
    assert_eq!(
        level
            .lighting
            .lighting_params
            .as_ref()
            .unwrap()
            .sky_light_color,
        Some([0.0, 0.0, 0.0, 255.0])
    );

    let zone = d
        .environments
        .iter()
        .find(|e| e.kind == EnvironmentKind::Zone)
        .unwrap();
    let z = zone.lighting.zone.as_ref().unwrap();
    assert_eq!(z.zone_uses_ambient, Some(false));
    assert_eq!(z.use_light_map, Some(true));
    assert_eq!(z.ambient_light, Some([50.0, 50.0, 50.0, 0.0]));
    assert_eq!(
        z.reflection_map.as_deref(),
        Some("/Lotus/Levels/ClanDojo/ClanObstacleCourseA/0_cross.hdr")
    );
    assert!((z.reflection_sh1.unwrap()[0] - 0.030593872).abs() < 1e-6);
    assert_eq!(z.light_map_size.as_deref(), Some("LM_1024x1024"));
}

#[test]
fn warframe_default_colours() {
    use wf_unpack::palette::SuitPalette;
    let Some(dir) = game_dir() else { return };
    let exc = SuitPalette::resolve(&dir, "/Lotus/Powersuits/Excalibur/Excalibur").unwrap();
    assert_eq!(
        exc.energy,
        [
            Some([77.0, 176.0, 224.0, 255.0]),
            Some([54.0, 146.0, 190.0, 255.0])
        ]
    );
    let tint = exc.body_tint().unwrap();
    assert_eq!(tint[0], [0.35, 0.35, 0.35, 0.5]);
    assert_eq!(tint[1], [0.085, 0.085, 0.085, 0.5]);
    assert_eq!(tint[3], [0.26, 0.0075, 0.0037, 0.5]);
    // The helmet is a second part with the same defaults
    let parts: Vec<_> = exc.parts.iter().map(|p| p.part.as_str()).collect();
    assert_eq!(parts, ["body", "Helmet"]);
    assert_eq!(
        exc.parts[1].materials[0].tint,
        exc.parts[0].materials[0].tint
    );
    assert!(exc.parts[0].materials[0].multi_tint_emission);

    // Mag has other defaults: a steel blue, a bright blue, copper and white
    let mag = SuitPalette::resolve(&dir, "/Lotus/Powersuits/Mag/Mag").unwrap();
    assert_eq!(mag.energy[0], Some([77.0, 195.0, 126.0, 255.0]));
    let t = mag.body_tint().unwrap();
    assert_eq!(t[1], [0.2, 0.52, 0.69, 0.5]);
    assert_eq!(t[3], [1.0, 1.0, 1.0, 0.5]);
    assert!(SuitPalette::resolve(&dir, "/Lotus/Powersuits/Nobody/Nobody").is_err());
}

/// Runs the command-line tool and returns its standard output.
fn run_cli(args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_wf-unpack"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn list_has_a_header_row_and_a_limit() {
    let Some(dir) = game_dir() else { return };
    let toc = dir.join("H.Misc.toc");
    let text = run_cli(&["list", toc.to_str().unwrap(), "--limit", "3"]);
    let lines: Vec<_> = text.lines().collect();
    // the summary, the column names, then three files
    assert_eq!(lines.len(), 5, "{text}");
    assert!(lines[0].contains("files in"));
    assert!(
        lines[1].contains("bytes") && lines[1].contains("stored") && lines[1].ends_with("path")
    );
}

#[test]
fn anim_filter_says_how_many_clips_fit_first() {
    let Some(dir) = game_dir() else { return };
    let model = "/Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx";
    // both spellings of the filter select the same clips
    let game = dir.to_str().unwrap();
    let anim = |filter: &str| run_cli(&["anim", model, "--filter", filter, "--game", game]);
    let a = anim("/Tenno/Movement/MeleeWeapon/Sword/");
    let b = anim("Tenno/Movement/MeleeWeapon/Sword/");
    // (the first line quotes the filter as typed)
    assert_eq!(
        a.lines().skip(1).collect::<Vec<_>>(),
        b.lines().skip(1).collect::<Vec<_>>()
    );
    let first = a.lines().next().unwrap();
    assert!(first.starts_with("fits this model: "), "{first}");
    let n: usize = first["fits this model: ".len()..]
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(n > 10, "{first}");
}

#[cfg(feature = "bridge")]
#[test]
fn check_conformance_takes_texture_options_among_the_checkers() {
    let Some(dir) = game_dir() else { return };
    let game = dir.to_str().unwrap();
    // `--max-texture` and `--no-textures` work before and after the checker's own options; a level at the
    // default size is checked with 256-pixel textures, which keeps a run of levels under about 1 GB
    let a = run_cli(&[
        "check-conformance",
        "--game",
        game,
        "--no-textures",
        "--kind",
        "level",
        "--limit",
        "2",
    ]);
    let b = run_cli(&[
        "check-conformance",
        "--game",
        game,
        "--kind",
        "level",
        "--limit",
        "1",
        "--max-texture",
        "64",
    ]);
    assert!(a.contains("2 asset(s) checked"), "{a}");
    assert!(b.contains("1 asset(s) checked"), "{b}");
}
