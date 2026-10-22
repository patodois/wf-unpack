//! Opens Warframe models straight from the game files in a Bevy window.
//!
//! Usage: `wf-viewer <model path>... [--game <Cache.Windows>] [--lod N] [--screenshot <file.png>] [--pose-demo]`
//!
//! Drag with the mouse to orbit, scroll to zoom, N / P for the next or previous model. With
//! `--screenshot` the window saves one frame and closes. `--pose-demo` swings the left upper arm of
//! a skinned model (bone `GAME_L1_ARM1`) to show the skin following its joints.

use std::path::PathBuf;

use anyhow::Result;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use wf_unpack::model::Model;
use wf_unpack::pbr::{self, Baked, MaterialBaker};
use wf_unpack::scene::{self, Scene};

const DEFAULT_GAME: &str = "~/Games/Warframe/Cache.Windows";
const ARM_BONE: &str = "GAME_L1_ARM1";

/// Model paths given on the command line and which one is shown
#[derive(Resource)]
struct Playlist {
    paths: Vec<String>,
    current: usize,
    lod: usize,
    /// Set when the shown model must be (re)loaded
    dirty: bool,
}

/// Opened packages, Packages.bin and texture packages, kept between models
struct Game {
    dir: PathBuf,
    baker: MaterialBaker,
}

/// Where to save a single frame before exiting
#[derive(Resource)]
struct ScreenshotTo(PathBuf);

/// Current pose-demo angle in degrees, logged with screenshots
#[derive(Resource, Default)]
struct PoseDemo(f32);

/// Root entity of the shown model
#[derive(Component)]
struct ModelRoot;

/// The arm bone the pose demo swings, with its rest rotation
#[derive(Component)]
struct SwingBone(Quat);

#[derive(Component)]
struct InfoText;

/// Orbit camera state: angles in radians, distance in metres
#[derive(Resource)]
struct Orbit {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    min_distance: f32,
}

fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(p),
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let option = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let game = expand_home(&option("--game").unwrap_or_else(|| DEFAULT_GAME.to_string()));
    let screenshot = option("--screenshot").map(PathBuf::from);
    let lod = option("--lod")
        .map_or(Ok(0), |l| l.parse())
        .map_err(|e| anyhow::anyhow!("--lod: {e}"))?;
    let pose_demo = args.iter().any(|a| a == "--pose-demo");
    // Positional arguments are everything that is neither a flag nor a flag's value
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--pose-demo" => i += 1,
            a if a.starts_with("--") => i += 2,
            a => {
                paths.push(a.to_string());
                i += 1;
            }
        }
    }
    if paths.is_empty() {
        paths.push("/Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx".into());
    }

    eprintln!("opening the packages in {}", game.display());
    let baker = MaterialBaker::open(&game)?;

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "wf-viewer".into(),
            ..default()
        }),
        ..default()
    }));
    if let Some(path) = screenshot {
        app.insert_resource(ScreenshotTo(path))
            .add_systems(Update, screenshot_and_exit);
    }
    if pose_demo {
        app.insert_resource(PoseDemo::default())
            .add_systems(Update, swing_arm);
    }
    app.insert_non_send(Game { dir: game, baker })
        .insert_resource(Playlist {
            paths,
            current: 0,
            lod,
            dirty: true,
        })
        .insert_resource(ClearColor(Color::srgb(0.11, 0.12, 0.14)))
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 150.0,
            affects_lightmapped_meshes: true,
        })
        .insert_resource(Orbit {
            target: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.25,
            distance: 3.0,
            min_distance: 0.05,
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (navigate, load_model, orbit_camera).chain())
        .run();
    Ok(())
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    // A soft sky-to-ground gradient for reflections: without one, metals render black
    let environment = EnvironmentMapLight::hemispherical_gradient(
        &mut images,
        Color::srgb(0.85, 0.88, 0.95),
        Color::srgb(0.5, 0.5, 0.5),
        Color::srgb(0.18, 0.16, 0.14),
    )
    .with_intensity(900.0);
    commands.spawn((Camera3d::default(), Transform::default(), environment));
    commands.spawn((
        DirectionalLight {
            illuminance: 9000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Text::new("loading"),
        TextFont::from_font_size(15.0),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(10.0),
            ..default()
        },
        InfoText,
    ));
}

/// N / P switch to the next or previous model of the list
fn navigate(keys: Res<ButtonInput<KeyCode>>, mut playlist: ResMut<Playlist>) {
    let n = playlist.paths.len();
    if keys.just_pressed(KeyCode::KeyN) {
        playlist.current = (playlist.current + 1) % n;
        playlist.dirty = true;
    }
    if keys.just_pressed(KeyCode::KeyP) {
        playlist.current = (playlist.current + n - 1) % n;
        playlist.dirty = true;
    }
}

fn to_bevy_image(img: &pbr::Image, srgb: bool) -> Image {
    let format = if srgb {
        TextureFormat::Rgba8UnormSrgb
    } else {
        TextureFormat::Rgba8Unorm
    };
    let mut image = Image::new(
        Extent3d {
            width: img.width,
            height: img.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        img.rgba.clone(),
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    // UVs tile beyond 0..1 (mirrored halves sit at u + 1)
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        anisotropy_clamp: 8,
        ..ImageSamplerDescriptor::linear()
    });
    image
}

/// The same inputs as the glTF export, as a StandardMaterial
fn to_material(b: &Baked, images: &mut Assets<Image>) -> StandardMaterial {
    let [r, g, bl, a] = b.base_color_factor;
    StandardMaterial {
        base_color: Color::linear_rgba(r, g, bl, a),
        base_color_texture: b
            .base_color
            .as_ref()
            .map(|i| images.add(to_bevy_image(i, true))),
        metallic: b.metallic_factor,
        perceptual_roughness: b.roughness_factor,
        metallic_roughness_texture: b
            .metallic_roughness
            .as_ref()
            .map(|i| images.add(to_bevy_image(i, false))),
        normal_map_texture: b
            .normal
            .as_ref()
            .map(|i| images.add(to_bevy_image(i, false))),
        emissive: if b.emissive.is_some() {
            LinearRgba::rgb(
                b.emissive_factor[0],
                b.emissive_factor[1],
                b.emissive_factor[2],
            ) * 4.0
        } else {
            LinearRgba::BLACK
        },
        emissive_texture: b
            .emissive
            .as_ref()
            .map(|i| images.add(to_bevy_image(i, true))),
        ..default()
    }
}

struct LoadedModel {
    scene: Scene,
    materials: Vec<Option<Baked>>,
}

fn load(game: &mut Game, path: &str, lod: usize) -> Result<LoadedModel> {
    let model = Model::load(&game.dir, path)?;
    let scene = scene::prepare(&model, lod, true)?;
    let names = model.header.materials();
    let materials = (0..model.header.meshes.len())
        .map(|i| {
            let name = names.get(i)?;
            let plan = game
                .baker
                .plan(path, name)
                .map_err(|e| warn!("material {name}: {e:#}"))
                .ok()?;
            game.baker
                .bake(&plan)
                .map_err(|e| warn!("material {name}: {e:#}"))
                .ok()
        })
        .collect();
    Ok(LoadedModel { scene, materials })
}

#[allow(clippy::too_many_arguments)]
fn load_model(
    mut commands: Commands,
    mut playlist: ResMut<Playlist>,
    mut game: NonSendMut<Game>,
    old: Query<Entity, With<ModelRoot>>,
    mut info: Query<&mut Text, With<InfoText>>,
    mut orbit: ResMut<Orbit>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    if !playlist.dirty {
        return;
    }
    playlist.dirty = false;
    for e in &old {
        commands.entity(e).despawn();
    }
    let path = playlist.paths[playlist.current].clone();
    let header = format!("[{}/{}] {path}", playlist.current + 1, playlist.paths.len());
    let loaded = match load(&mut game, &path, playlist.lod) {
        Ok(l) => l,
        Err(e) => {
            error!("{path}: {e:#}");
            if let Ok(mut t) = info.single_mut() {
                *t = Text::new(format!("{header}\nerror: {e:#}\nN / P: next / previous"));
            }
            return;
        }
    };
    let scene = &loaded.scene;
    let root = commands
        .spawn((ModelRoot, Transform::default(), Visibility::default()))
        .id();

    // Joints first, so the meshes can point at them
    let mut skin = None;
    if let Some(sk) = &scene.skin {
        let bone_entities: Vec<Entity> = sk
            .bones
            .iter()
            .map(|b| {
                let transform = Transform {
                    translation: Vec3::from(b.translation),
                    rotation: Quat::from_array(b.rotation),
                    scale: Vec3::from(b.scale),
                };
                let mut e =
                    commands.spawn((Name::new(b.name.clone()), transform, Visibility::default()));
                if b.name == ARM_BONE {
                    e.insert(SwingBone(transform.rotation));
                }
                e.id()
            })
            .collect();
        for (i, b) in sk.bones.iter().enumerate() {
            let parent = b.parent.map_or(root, |p| bone_entities[p]);
            commands.entity(bone_entities[i]).insert(ChildOf(parent));
        }
        // Row-major in the scene; Bevy matrices are column-major
        let ibm: Vec<Mat4> = sk
            .inverse_binds
            .iter()
            .map(|m| Mat4::from_cols_array_2d(&m.map(|r| r)).transpose())
            .collect();
        let joints: Vec<Entity> = sk.joints.iter().map(|&b| bone_entities[b]).collect();
        skin = Some(SkinnedMesh {
            inverse_bindposes: bindposes.add(SkinnedMeshInverseBindposes::from(ibm)),
            joints,
        });
    }

    let mats: Vec<Handle<StandardMaterial>> = loaded
        .materials
        .iter()
        .map(|m| match m {
            Some(b) => materials.add(to_material(b, &mut images)),
            None => materials.add(StandardMaterial {
                base_color: Color::srgb(0.6, 0.6, 0.6),
                ..default()
            }),
        })
        .collect();
    for p in &scene.primitives {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, scene.positions.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, scene.normals.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, scene.uv0.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, scene.tangents.clone())
        .with_inserted_indices(Indices::U32(p.indices.clone()));
        if skin.is_some() {
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_JOINT_INDEX,
                VertexAttributeValues::Uint16x4(scene.joints.clone()),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, scene.weights.clone());
        }
        let material = mats
            .get(p.mesh)
            .cloned()
            .unwrap_or_else(|| materials.add(StandardMaterial::default()));
        let mut e = commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            ChildOf(root),
        ));
        if let Some(s) = &skin {
            // The bounding box of the bind pose does not follow the skin
            e.insert((s.clone(), NoFrustumCulling));
        }
    }

    let (lo, hi) = (Vec3::from(scene.bounds.0), Vec3::from(scene.bounds.1));
    let size = (hi - lo).max_element().max(0.01);
    // Fit the bounding sphere in the default 45 degree field of view
    let radius = (hi - lo).length() / 2.0;
    let distance = radius / (22.5f32.to_radians()).sin() * 1.05;
    *orbit = Orbit {
        target: (lo + hi) / 2.0,
        yaw: 0.6,
        pitch: 0.25,
        distance,
        min_distance: size * 0.05,
    };
    // Ground plane under the model
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(size * 4.0, size * 4.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.23, 0.24, 0.22),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz((lo.x + hi.x) / 2.0, lo.y, (lo.z + hi.z) / 2.0),
        ChildOf(root),
    ));
    let bones = scene
        .skin
        .as_ref()
        .map_or(String::new(), |s| format!(", {} bones", s.bones.len()));
    let text = format!(
        "{header}\n{} triangles (LOD {}), {} vertices{bones}\ndrag: orbit   scroll: zoom   N / P: next / previous",
        scene.triangles(),
        playlist.lod,
        scene.positions.len()
    );
    info!("{}", text.replace('\n', " | "));
    if let Ok(mut t) = info.single_mut() {
        *t = Text::new(text);
    }
}

/// Swings the left upper arm up and down about its local z axis, 45 degrees each way
fn swing_arm(
    time: Res<Time>,
    mut demo: ResMut<PoseDemo>,
    mut bones: Query<(&SwingBone, &mut Transform)>,
) {
    demo.0 = (time.elapsed_secs() * 1.5).sin() * 45.0;
    for (rest, mut t) in &mut bones {
        t.rotation = rest.0 * Quat::from_rotation_z(demo.0.to_radians());
    }
}

fn orbit_camera(
    mut orbit: ResMut<Orbit>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    mut camera: Query<&mut Transform, With<Camera3d>>,
) {
    if buttons.pressed(MouseButton::Left) {
        orbit.yaw -= motion.delta.x * 0.005;
        orbit.pitch = (orbit.pitch + motion.delta.y * 0.005).clamp(-1.4, 1.4);
    }
    let min = orbit.min_distance;
    orbit.distance = (orbit.distance * (1.0 - scroll.delta.y * 0.1)).clamp(min, 200.0);
    let offset = Vec3::new(
        orbit.yaw.sin() * orbit.pitch.cos(),
        orbit.pitch.sin(),
        orbit.yaw.cos() * orbit.pitch.cos(),
    ) * orbit.distance;
    for mut t in &mut camera {
        *t = Transform::from_translation(orbit.target + offset).looking_at(orbit.target, Vec3::Y);
    }
}

/// Waits for textures to upload, saves one frame, then exits. With several
/// paths it saves one frame per model (`out-1.png`, `out-2.png`, ...), going
/// through the list the way N does.
#[allow(clippy::too_many_arguments)]
fn screenshot_and_exit(
    mut commands: Commands,
    target: Res<ScreenshotTo>,
    mut playlist: ResMut<Playlist>,
    mut frame: Local<u32>,
    mut taken: Local<Option<u32>>,
    mut exit: MessageWriter<AppExit>,
    demo: Option<Res<PoseDemo>>,
) {
    *frame += 1;
    // With the pose demo, wait for the arm to be near the end of its swing
    let ready = demo.as_ref().is_none_or(|d| d.0.abs() > 40.0);
    if taken.is_none() && *frame >= 90 && ready {
        if let Some(d) = demo {
            info!("pose demo: arm at {:.1} degrees", d.0);
        }
        let path = if playlist.paths.len() == 1 {
            target.0.clone()
        } else {
            let stem = target.0.file_stem().unwrap_or_default().to_string_lossy();
            target
                .0
                .with_file_name(format!("{stem}-{}.png", playlist.current + 1))
        };
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        *taken = Some(*frame);
    }
    if taken.is_some_and(|t| *frame == t + 60) {
        if playlist.current + 1 < playlist.paths.len() {
            playlist.current += 1;
            playlist.dirty = true;
            *frame = 0;
            *taken = None;
        } else {
            exit.write(AppExit::Success);
        }
    }
}
