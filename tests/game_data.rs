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
    for px in page.clr.rgba.chunks_exact(4) {
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
