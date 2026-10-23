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
    // Every transition has a blend time, and none is instant except by data: all are positive
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
