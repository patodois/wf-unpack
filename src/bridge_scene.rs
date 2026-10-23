//! Levels as rewrite-bridge [`Scene`]s.
//!
//! [`Provider::load_scene`](rewrite_bridge::Provider::load_scene) on a `.level` path (the ids
//! `list` returns as `AssetKind::Level`) reads the level with [`crate::level`] and builds:
//!
//! - **Models.** Every model the level places, the CSG zones' own geometry (types 105/109) and
//!   the props (types 102/281), is a [`SceneModel::Inline`] model, loaded once however many
//!   instances use it, with its materials baked as for a single model. The props of `.static`
//!   levels (hubs and missions) are included. An instance with an `OverrideMaterial` list gets
//!   its own variant of the model with those materials; models sharing a material bake it once.
//!   Each model's `name` is its asset path (variants add `#n`).
//! - **Instances.** One per placed object, in the level's order. The transform is the object's
//!   position, rotation and `MeshScale`, turned right-handed like the models: x of the
//!   translation negated, rotation `(x, -y, -z, w)`. Skinned models stand in their bind pose.
//!   Objects with `StartHidden=1` (a script shows them later) are left out; so are lights with
//!   `On=0`. `SceneOptions::include_hidden` keeps both.
//! - **Lights.** See [`convert_light`].
//! - **Environment.** See [`environment`].
//! - **Collision.** As a scene of its own, see below.
//!
//! The scene is in metres, +Y up, right-handed (the game's left-handed space with x negated,
//! which keeps what faces +z facing +z).
//!
//! # Collision
//!
//! The level's PhysX shapes are not mixed into the visible scene, so a renderer never draws
//! them. `load_scene("<level path>?collision")` (see [`crate::bridge::collision_id`]) returns a
//! second scene of the same level whose models are the collision shapes:
//!
//! - one `SceneModel::Inline` model per placed model that has shapes, named like the visual
//!   model (its asset path), with one mesh per shape. The mesh name says what the shape is:
//!   `triangles`, `convex`, `box half_extents=[..]`, `sphere radius=..` or
//!   `capsule radius=.. half_height=..`; the triangles are the cooked meshes, or a tessellation
//!   of the primitive (see [`crate::collision`]), so an engine can rebuild exact primitives from the
//!   names if it wants;
//! - the instances are the visible scene's, one per placed object with shapes, in the same
//!   order and with the same transforms, so a physics world is built by walking
//!   `instances` and creating a static body per instance from its model's meshes;
//! - no lights, no environment, and one plain material (collision has none).
//!
//! The CSG zones' own triangle meshes are shapes of the zone models, so the level's architecture
//! is included. `BlockingVolume` objects (`Dimensions` or `Length`/`Width`/`Height`) are **not**
//! in the collision scene: their axes and whether the numbers are full or half extents are not
//! verified. They are in [`WarframeScene::description`] with the raw values. Per-triangle PhysX
//! materials are not carried either.
//!
//! [`Warframe::scene`] returns both scenes, the level description (spawn points, portals,
//! volumes, every environment setting, the objects that were not turned into anything) and a
//! report of what was left out.

use std::collections::{BTreeMap, HashMap};

use anyhow::{ensure, Context, Result};
use rewrite_bridge::{
    Environment, Instance, Light, LightKind, Material, Mesh, Model, Node, Scene, SceneModel,
    Transform,
};

use crate::bridge::{Warframe, COLLISION_SUFFIX};
use crate::collision::shape_mesh;
use crate::ee;
use crate::level::{self, EnvironmentKind, LevelDescription};
use crate::pbr::srgb_to_linear;

/// What to build.
#[derive(Debug, Clone)]
pub struct SceneOptions {
    /// Also place objects with `StartHidden=1` and lights with `On=0`.
    pub include_hidden: bool,
    /// Build the visible scene (models, instances, lights, environment).
    pub visuals: bool,
    /// Build the collision scene.
    pub collision: bool,
}

impl Default for SceneOptions {
    fn default() -> Self {
        SceneOptions {
            include_hidden: false,
            visuals: true,
            collision: true,
        }
    }
}

/// What was built and what was left out.
#[derive(Debug, Clone, Default)]
pub struct SceneReport {
    /// Instances in the visible scene, and the distinct models they use (material variants
    /// count separately).
    pub instances: usize,
    pub models: usize,
    /// Triangles stored once, and triangles the instances add up to.
    pub triangles_stored: u64,
    pub triangles_placed: u64,
    /// Instances left out because they start hidden.
    pub hidden_skipped: usize,
    /// Models that could not be loaded, with the reason, and the instances lost with them.
    pub model_failures: Vec<(String, String)>,
    pub instances_lost: usize,
    /// Models without triangles (collision or trigger shapes only), and their instances.
    pub no_geometry: Vec<String>,
    pub instances_without_geometry: usize,
    /// Instances whose transform the bridge cannot hold: a scale of zero (the data scales some
    /// objects of `.static` levels to nothing, which makes them invisible), a rotation of length
    /// zero or a non-finite value.
    pub invalid_transforms: usize,
    /// Lights in the scene, and the reasons for the ones left out, with their number.
    pub lights: usize,
    pub lights_left_out: BTreeMap<&'static str, usize>,
    /// Collision: models, instances and shapes, and models whose shapes could not be read.
    pub collision_models: usize,
    pub collision_instances: usize,
    pub collision_shapes: usize,
    pub collision_failures: Vec<(String, String)>,
}

/// The result of [`Warframe::scene`].
pub struct WarframeScene {
    /// The visible scene; empty when `SceneOptions::visuals` is off.
    pub scene: Scene,
    /// The collision scene; empty when `SceneOptions::collision` is off.
    pub collision: Scene,
    /// Everything the level places, in the game's own left-handed space: spawn points, portals,
    /// blocking volumes, environment settings, and the objects that became nothing (`skipped`).
    pub description: LevelDescription,
    pub report: SceneReport,
}

const FOUR_PI: f32 = 4.0 * std::f32::consts::PI;

/// A quaternion product `a * b` (x, y, z, w): `b` first, then `a`.
fn mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

fn unit(q: [f32; 4]) -> Option<[f32; 4]> {
    let n = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    (n.is_finite() && n > 1e-6).then(|| q.map(|c| c / n))
}

/// The placement of an object as a bridge transform, or `None` when the bridge cannot hold it.
pub fn to_bridge(t: &level::Transform) -> Option<Transform> {
    let rh = t.to_right_handed();
    let rotation = unit(rh.rotation)?;
    let ok = rh.translation.iter().all(|c| c.is_finite())
        && rh.scale.iter().all(|c| c.is_finite() && *c != 0.0);
    ok.then_some(Transform {
        translation: rh.translation,
        rotation,
        scale: rh.scale,
    })
}

/// Why a level light is not a bridge light.
pub type LeftOut = &'static str;

/// Turns a level light into a bridge light.
///
/// | level | bridge | how |
/// |---|---|---|
/// | `Lumens` | `intensity` in candela | divided by 4 pi, the bridge's rule for a source given in lumens (point and spot alike) |
/// | `Color` (0..255, alpha last) | linear `color` | sRGB decoded. The colours match the usual sRGB values of the stored `Temperature` (2800 K is 255, 174, 94; 10000 K is 204, 220, 255), which is why they are read as sRGB; the data does not say |
/// | `Radius` | `range` in metres | as is |
/// | `Angle` (not `OMNI` types) | a spot with outer cone `Angle / 2` radians, capped at pi/2, inner cone 0 | `Angle` is read as the full cone angle in degrees (it is at most 170 in the data); the data has no inner cone |
/// | no `Angle`, or `Angle` 0 | a point light | an `Angle` of 0 is read as "no cone" (about 1,000 lights) |
///
/// The light's position is the object's, turned right-handed. **Axis (unverified):** the game's
/// light is read as shining along its local +z axis, so the bridge transform is the object's
/// rotation turned right-handed and then a half turn about y (the bridge shines along -z).
/// The data does not state the axis; it is chosen because, of the spot lights, the local +z
/// axis is the one that points downward in half of them (the others in at most a tenth),
/// as ceiling lights do.
///
/// Left out, with the reason as the returned `Err`:
///
/// - lights without `Lumens` (the data then gives only `Radius` or nothing; `Brightness` is
///   `Lumens / 1600` in every light that has both, but a light with neither is not guessed);
/// - lights without `Color`;
/// - directional lights (`Directional=1`): the data has lumens, not the illuminance in lux that
///   the bridge's directional light needs;
/// - lights with `On=0` unless `include_hidden`.
///
/// Not represented: the box of an area light (`Length`, `Width`, `Height`), the projection
/// `Texture` and `.ies` profile, `Falloff` and `FalloffExponent`, shadow flags and the light
/// type's shadow behaviour. They stay in [`LevelDescription::lights`].
pub fn convert_light(l: &level::Light, include_hidden: bool) -> Result<Light, LeftOut> {
    if l.on == Some(false) && !include_hidden {
        return Err("starts switched off (On=0)");
    }
    if l.directional == Some(true) {
        return Err("directional (lumens, not lux)");
    }
    let lumens = l.lumens.ok_or("no Lumens")?;
    if !(lumens.is_finite() && lumens > 0.0) {
        return Err("Lumens not positive");
    }
    let color = l.color.ok_or("no Color")?;
    let rgb = [color[0], color[1], color[2]];
    if rgb
        .iter()
        .any(|c| !c.is_finite() || !(0.0..=255.0).contains(c))
    {
        return Err("Color outside 0..255");
    }
    let range = l.radius.filter(|r| r.is_finite() && *r > 0.0);
    let omni = l.light_type.as_deref().is_some_and(|t| t.contains("OMNI"));
    let kind = match l.angle.filter(|_| !omni) {
        Some(a) if a.is_finite() && a > 0.0 => LightKind::Spot {
            range,
            inner_cone_angle: 0.0,
            outer_cone_angle: (a.to_radians() / 2.0).min(std::f32::consts::FRAC_PI_2),
        },
        // An `Angle` of 0 (about 1,000 lights) is read as "no cone": a point light
        Some(0.0) => LightKind::Point { range },
        Some(_) => return Err("Angle negative or not finite"),
        None => LightKind::Point { range },
    };
    let place = level::Transform {
        translation: l.position,
        rotation: l.rotation,
        scale: [1.0; 3],
    };
    let rh = place.to_right_handed();
    // The bridge shines along -z, the game's light along +z (unverified): a half turn about y
    let rotation = unit(mul(rh.rotation, [0.0, 1.0, 0.0, 0.0])).ok_or("rotation of length zero")?;
    if rh.translation.iter().any(|c| !c.is_finite()) {
        return Err("position not finite");
    }
    Ok(Light {
        name: format!(
            "{}{}",
            l.scope,
            l.type_path.rsplit('/').next().unwrap_or(&l.type_path)
        ),
        kind,
        color: rgb.map(|c| srgb_to_linear(c / 255.0)),
        intensity: lumens / FOUR_PI,
        transform: Transform {
            translation: rh.translation,
            rotation,
            scale: [1.0; 3],
        },
    })
}

/// The scene's environment: the level's `LevelInfo` `LightingParams.skyLightColor`, a flat
/// colour.
///
/// The colour is read as sRGB (0..255, alpha last) and decoded to linear. It is the only
/// sky-like value in the data that fits the bridge's flat colour: the pictures a level names
/// (`ReflectionMap`, `BackDropId`) are not decoded, and fog and ambient light are not a sky. The
/// dojo's is black. The level's fog (`PostProcessInfo.fogColor`, `distanceFogDensity`), each
/// zone's `AmbientLight` and the rest stay in [`LevelDescription::environments`], in the game's
/// units. `None` when the level has no `LightingParams.skyLightColor` (most levels do not: 10 of
/// the first 150 set it).
pub fn environment(d: &LevelDescription) -> Option<Environment> {
    let info = d
        .environments
        .iter()
        .find(|e| e.kind == EnvironmentKind::Level)?;
    let text = &info
        .attributes
        .iter()
        .find(|(k, _)| k == "LightingParams")?
        .1;
    let params = ee::parse_value(text).ok()?;
    let c = params.get("skyLightColor")?.floats()?;
    (c.len() == 4 && c.iter().all(|v| v.is_finite() && (0.0..=255.0).contains(v)))
        .then(|| Environment::Color([0, 1, 2].map(|k| srgb_to_linear(c[k] / 255.0))))
}

/// How a model fared when it was loaded.
#[derive(Clone, Copy)]
enum State {
    Loaded,
    NoGeometry,
    Failed,
}

fn stem(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.trim_end_matches(".level").trim_end_matches(".fbx")
}

/// The visible part: models, instances, lights, environment.
fn build_visuals(
    wf: &Warframe,
    d: &LevelDescription,
    opts: &SceneOptions,
    report: &mut SceneReport,
) -> Result<Scene> {
    let mut scene = Scene {
        name: stem(&d.path).to_string(),
        ..Scene::default()
    };
    let mut cache: HashMap<String, Material> = HashMap::new();
    let mut state: HashMap<usize, State> = HashMap::new();
    let mut variants: HashMap<(usize, Vec<String>), usize> = HashMap::new();
    let mut variant_count: HashMap<usize, usize> = HashMap::new();
    let mut triangles: Vec<u64> = Vec::new();
    for inst in &d.instances {
        if inst.hidden && !opts.include_hidden {
            report.hidden_skipped += 1;
            continue;
        }
        let path = &d.models[inst.model];
        let mut overrides = inst.materials.clone();
        while overrides.last().is_some_and(String::is_empty) {
            overrides.pop();
        }
        let key = (inst.model, overrides);
        let index = match variants.get(&key) {
            Some(&i) => Some(i),
            None => {
                if matches!(
                    state.get(&inst.model),
                    Some(State::Failed) | Some(State::NoGeometry)
                ) {
                    None
                } else {
                    let n = variant_count.entry(inst.model).or_default();
                    let name = if *n == 0 {
                        path.clone()
                    } else {
                        format!("{path}#{n}")
                    };
                    match wf.load_model_as(path, &name, &key.1, Some(&mut cache)) {
                        Ok(m) if !m.meshes.is_empty() => {
                            *n += 1;
                            state.insert(inst.model, State::Loaded);
                            let tri: u64 =
                                m.meshes.iter().map(|x| x.indices.len() as u64 / 3).sum();
                            triangles.push(tri);
                            report.triangles_stored += tri;
                            scene.models.push(SceneModel::Inline(m));
                            variants.insert(key.clone(), scene.models.len() - 1);
                            Some(scene.models.len() - 1)
                        }
                        Ok(_) => {
                            state.insert(inst.model, State::NoGeometry);
                            report.no_geometry.push(path.clone());
                            None
                        }
                        Err(e) => {
                            state.insert(inst.model, State::Failed);
                            // The error chain starts with the path, which the report has already
                            let text = format!("{e:#}");
                            let text = text.strip_prefix(&format!("{path}: ")).unwrap_or(&text);
                            report.model_failures.push((path.clone(), text.to_string()));
                            None
                        }
                    }
                }
            }
        };
        let Some(model) = index else {
            match state.get(&inst.model) {
                Some(State::NoGeometry) => report.instances_without_geometry += 1,
                _ => report.instances_lost += 1,
            }
            continue;
        };
        let Some(transform) = to_bridge(&inst.transform) else {
            report.invalid_transforms += 1;
            continue;
        };
        scene.instances.push(Instance::new(model, transform));
        report.triangles_placed += triangles[model];
    }
    report.instances = scene.instances.len();
    report.models = scene.models.len();

    for l in &d.lights {
        match convert_light(l, opts.include_hidden) {
            Ok(light) => scene.lights.push(light),
            Err(why) => *report.lights_left_out.entry(why).or_default() += 1,
        }
    }
    report.lights = scene.lights.len();
    scene.environment = environment(d);
    Ok(scene)
}

/// The collision scene; reads the shapes of the models the level places.
fn build_collision(
    wf: &Warframe,
    d: &mut LevelDescription,
    opts: &SceneOptions,
    report: &mut SceneReport,
) -> Result<Scene> {
    // `load_collision` skips hidden instances; with `include_hidden` they count
    let hidden: Vec<bool> = d.instances.iter().map(|i| i.hidden).collect();
    if opts.include_hidden {
        d.instances.iter_mut().for_each(|i| i.hidden = false);
    }
    let failures = d.load_collision(wf.game_dir());
    for (i, h) in d.instances.iter_mut().zip(hidden) {
        i.hidden = h;
    }
    report.collision_failures = failures?;

    let mut scene = Scene {
        name: format!("{} collision", stem(&d.path)),
        ..Scene::default()
    };
    let mut index: HashMap<usize, Option<usize>> = HashMap::new();
    for inst in &d.instances {
        if inst.hidden && !opts.include_hidden {
            continue;
        }
        let model = match index.get(&inst.model) {
            Some(&m) => m,
            None => {
                let built = d.collision_shapes(inst.model).and_then(|shapes| {
                    let meshes: Vec<Mesh> = shapes
                        .iter()
                        .filter_map(shape_mesh)
                        .filter(|m| !m.indices.is_empty())
                        .map(|m| Mesh {
                            name: m.kind,
                            node: 0,
                            material: 0,
                            // Mirror x, and swap two corners of each triangle to keep the winding
                            positions: m.vertices.iter().map(|p| [-p[0], p[1], p[2]]).collect(),
                            indices: m
                                .indices
                                .as_chunks::<3>()
                                .0
                                .iter()
                                .flat_map(|t| [t[0], t[2], t[1]])
                                .collect(),
                            ..Mesh::default()
                        })
                        .collect();
                    (!meshes.is_empty()).then(|| {
                        report.collision_shapes += meshes.len();
                        Model {
                            name: d.models[inst.model].clone(),
                            nodes: vec![Node::new(stem(&d.models[inst.model]), None)],
                            materials: vec![Material {
                                name: "collision".into(),
                                ..Material::default()
                            }],
                            meshes,
                            skins: Vec::new(),
                        }
                    })
                });
                let m = built.map(|model| {
                    scene.models.push(SceneModel::Inline(model));
                    scene.models.len() - 1
                });
                index.insert(inst.model, m);
                m
            }
        };
        let Some(model) = model else { continue };
        if let Some(t) = to_bridge(&inst.transform) {
            scene.instances.push(Instance::new(model, t));
        }
    }
    report.collision_models = scene.models.len();
    report.collision_instances = scene.instances.len();
    Ok(scene)
}

impl Warframe {
    /// Builds both scenes of a `.level` path, with the level's description and a report.
    pub fn scene(&self, path: &str, opts: &SceneOptions) -> Result<WarframeScene> {
        ensure!(
            crate::bridge::is_level(path),
            "{path} is not a level (a .level path)"
        );
        let d = level::load_description(self.game_dir(), path)
            .with_context(|| format!("reading {path}"))?;
        self.scene_of(d, opts)
    }

    /// Builds both scenes from a level description the caller read (for sweeps that share one
    /// Packages.bin between levels).
    pub fn scene_of(&self, mut d: LevelDescription, opts: &SceneOptions) -> Result<WarframeScene> {
        ensure!(
            crate::bridge::is_level(&d.path),
            "{} is not a level (a .level path)",
            d.path
        );
        let mut report = SceneReport::default();
        let scene = if opts.visuals {
            build_visuals(self, &d, opts, &mut report)?
        } else {
            Scene::default()
        };
        let collision = if opts.collision {
            build_collision(self, &mut d, opts, &mut report)?
        } else {
            Scene::default()
        };
        if opts.visuals {
            scene.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        if opts.collision {
            collision.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        Ok(WarframeScene {
            scene,
            collision,
            description: d,
            report,
        })
    }
}

/// `Provider::load_scene`: the visible scene of a level path, or, with the `?collision`
/// suffix, its collision scene.
pub fn load_scene(wf: &Warframe, id: &str) -> Result<Scene> {
    let (path, collision) = match id.strip_suffix(COLLISION_SUFFIX) {
        Some(path) => (path, true),
        None => (id, false),
    };
    let opts = SceneOptions {
        visuals: !collision,
        collision,
        ..SceneOptions::default()
    };
    let built = wf.scene(path, &opts)?;
    Ok(if collision {
        built.collision
    } else {
        built.scene
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light() -> level::Light {
        level::Light {
            type_path: "/Lotus/Levels/X/DojoLight".into(),
            scope: "/Layer1/".into(),
            position: [10.0, 4.0, 5.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            color: Some([255.0, 255.0, 255.0, 255.0]),
            lumens: Some(10000.0),
            radius: Some(30.0),
            ..level::Light::default()
        }
    }

    #[test]
    fn a_point_light_in_candela_and_linear_colour() {
        let l = convert_light(&light(), false).unwrap();
        assert!((l.intensity - 10000.0 / (4.0 * std::f32::consts::PI)).abs() < 1e-3);
        assert_eq!(l.kind, LightKind::Point { range: Some(30.0) });
        assert_eq!(l.color, [1.0; 3]);
        // x is negated by the handedness conversion
        assert_eq!(l.transform.translation, [-10.0, 4.0, 5.0]);
        assert_eq!(l.name, "/Layer1/DojoLight");
        // A mid grey in sRGB is darker in linear light
        let mut g = light();
        g.color = Some([128.0, 128.0, 128.0, 255.0]);
        let c = convert_light(&g, false).unwrap().color;
        assert!((c[0] - 0.2158).abs() < 1e-3);
    }

    #[test]
    fn a_spot_has_half_the_angle_and_shines_along_game_z() {
        let mut s = light();
        s.angle = Some(100.0);
        let l = convert_light(&s, false).unwrap();
        let LightKind::Spot {
            outer_cone_angle,
            inner_cone_angle,
            ..
        } = l.kind
        else {
            panic!("not a spot");
        };
        assert!((outer_cone_angle - 50f32.to_radians()).abs() < 1e-6);
        assert_eq!(inner_cone_angle, 0.0);
        // With no rotation the game's +z becomes the bridge's +z: the bridge light's -z axis
        // is turned by a half turn about y to point there
        let q = l.transform.rotation;
        let conj = [-q[0], -q[1], -q[2], q[3]];
        let v = mul(mul(q, [0.0, 0.0, -1.0, 0.0]), conj);
        assert!(v[2] > 0.999, "{v:?}");
        // Wide angles cap at a right angle; OMNI types stay points
        s.angle = Some(180.0);
        s.light_type = Some("OMNI_SHADOWMAP".into());
        assert!(matches!(
            convert_light(&s, false).unwrap().kind,
            LightKind::Point { .. }
        ));
        s.light_type = None;
        let LightKind::Spot {
            outer_cone_angle, ..
        } = convert_light(&s, false).unwrap().kind
        else {
            panic!()
        };
        assert!((outer_cone_angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn lights_the_data_cannot_convert_are_left_out_with_a_reason() {
        let mut l = light();
        l.lumens = None;
        assert_eq!(convert_light(&l, false).unwrap_err(), "no Lumens");
        let mut l = light();
        l.color = None;
        assert_eq!(convert_light(&l, false).unwrap_err(), "no Color");
        let mut l = light();
        l.directional = Some(true);
        assert!(convert_light(&l, false)
            .unwrap_err()
            .starts_with("directional"));
        let mut l = light();
        l.on = Some(false);
        assert!(convert_light(&l, false).is_err());
        assert!(convert_light(&l, true).is_ok());
        // An angle of 0 is no cone; a negative one is nonsense
        let mut l = light();
        l.angle = Some(0.0);
        assert!(matches!(
            convert_light(&l, false).unwrap().kind,
            LightKind::Point { .. }
        ));
        l.angle = Some(-5.0);
        assert!(convert_light(&l, false).unwrap_err().starts_with("Angle"));
        let mut l = light();
        l.radius = None;
        assert_eq!(
            convert_light(&l, false).unwrap().kind,
            LightKind::Point { range: None }
        );
    }

    #[test]
    fn transforms_are_right_handed_and_checked() {
        let t = level::Transform {
            translation: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 2.0],
            scale: [2.0, 2.0, 2.0],
        };
        let b = to_bridge(&t).unwrap();
        assert_eq!(b.translation, [-1.0, 2.0, 3.0]);
        assert_eq!(b.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(b.scale, [2.0; 3]);
        // A quarter turn about +y in the game turns +z to +x; mirrored it turns +z to -x
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let turned = level::Transform {
            rotation: [0.0, q, 0.0, q],
            ..t
        };
        let r = to_bridge(&turned).unwrap().rotation;
        assert!(r
            .iter()
            .zip([0.0, -q, 0.0, q])
            .all(|(a, b)| (a - b).abs() < 1e-6));
        assert!(to_bridge(&level::Transform {
            scale: [1.0, 0.0, 1.0],
            ..t
        })
        .is_none());
        assert!(to_bridge(&level::Transform {
            rotation: [0.0; 4],
            ..t
        })
        .is_none());
    }

    #[test]
    fn the_environment_is_the_sky_light_colour() {
        let mut d = LevelDescription::default();
        assert!(environment(&d).is_none());
        d.environments.push(level::Environment {
            object: 0,
            kind: EnvironmentKind::Level,
            type_path: "/EE/Types/Engine/LevelInfo".into(),
            scope: "/".into(),
            position: [0.0; 3],
            ambient_light: None,
            fog_color: None,
            fog_density: None,
            override_level_fog: None,
            reflection_map: None,
            backdrop: None,
            attributes: vec![(
                "LightingParams".into(),
                "{skyLightColor={255,128,0,255}}".into(),
            )],
            lighting: Default::default(),
        });
        let Some(Environment::Color(c)) = environment(&d) else {
            panic!("no colour")
        };
        assert_eq!(c[0], 1.0);
        assert!((c[1] - 0.2158).abs() < 1e-3);
        assert_eq!(c[2], 0.0);
    }

    #[test]
    fn quaternion_product_composes_rotations() {
        // A half turn about y twice is the identity (up to sign)
        let q = mul([0.0, 1.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(q, [0.0, 0.0, 0.0, -1.0]);
        assert_eq!(unit([0.0; 4]), None);
    }
}
