//! Sky, sun, ambient light, fog and global illumination of a level: what its data says, with the
//! unit and the source of every value, and what it does not say.
//!
//! [`Atmosphere::of`] reads a [`LevelDescription`]; it needs no game files (the textures it names
//! are only named). Nothing is defaulted: a field the level and its types do not set is `None`.
//! The engine's own defaults are compiled into the executable and are not in the data.
//!
//! # What the data defines, and where
//!
//! | what | where | stored as |
//! |---|---|---|
//! | a lighting capture | `LevelInfo.LightingParams.globalCubeMap` (543 levels name one, 4 of 10 pictures are in the install) | a BC6H cube map (type 183), linear radiance, 128 px faces; a capture the baker lights with (weight `giEnvironmentIntensity`), not shown to be the sky the game draws |
//! | a sky light colour | `LightingParams.skyLightColor`, beside the baking options | 0..255, alpha last; the type gives black, only a level's own value is a statement of the level |
//! | a sun | a `Light` with `Directional=1` and `DirectionalLightDir` (566 lights in 434 levels), and `PostProcessInfo.sunPosition` / `sunColor` (635 / 588) | see [`Sun`] |
//! | a sun that moves | `DynamicSky` (44 objects in 28 levels): keyframes of colour and brightness per hour | [`crate::level::DynamicSky`], kept as stored |
//! | ambient light | `ZoneAttribs.AmbientLight{,Mid,Low}`, used when `ZoneUsesAmbient=1` (26 of 11,664 zones) | three 0..255 colours |
//! | fog | `PostProcessInfo` (`fogColor`, `distanceFogDensity`, ...) and a zone's override | see [`Fog`] |
//! | baked indirect light | lightmaps (decoded, see [`crate::lightmap`]), `GIVolume` boxes of probes, `.lpd` light probes, `.epd` environment probes, a zone's `ReflectionSH1` | see [`GlobalIllumination`] |
//!
//! # Conventions
//!
//! Directions are converted to the bridge's space (metres, +Y up, right-handed: the game's x is
//! negated) and normalised, the way `crate::bridge_scene` (with the `bridge` feature) converts instances. Positions stay in
//! the game's left-handed space, like [`LevelDescription`]. Colours keep the units of the data
//! (0..255, alpha last) unless the field says linear.
//!
//! # What is left out, and why
//!
//! [`Atmosphere::left_out`] lists, for the level at hand, each thing the data points at that this
//! reader does not turn into numbers, with the reason and the count. The reasons that hold for
//! every level:
//!
//! - **Sun illuminance.** A directional light has `Lumens` (= 1600 x `Brightness` in every light
//!   that has both) and a colour, but no lux and no exposure; how the engine turns them into
//!   radiance is code. So the sun is a direction and a colour, and its strength stays in the game's
//!   own numbers.
//! - **The sun of a time-of-day sky.** A [`DynamicSky`] gets its sun
//!   position from `Latitude`, `SunDeclination`, the time and the solar offsets in script and
//!   shader code; the keyframes (colour, brightness, fog per hour) and the sky material's constants (`A` to `I`, `Z`:
//!   the coefficients of its radiance formula, in the material's shader, which is not decoded) are kept as stored.
//! - **Voxels of GI volumes, `.lpd` and `.epd`.** The files are read as far as their headers; the
//!   per-probe payload (spherical-harmonic bands packed in 3D textures and half-float records)
//!   is turned into light by shader code that is not decoded here.
//! - **Fog and ambient falloff laws**, and the lightmap directional gain (see
//!   [`crate::pipeline`] for the part that is decoded).

use serde_json::{json, Value as Json};

use crate::level::{DynamicSky, EnvironmentKind, GiVolume, LevelDescription, Light as LevelLight};
use crate::pbr::srgb_to_linear;
use crate::postprocess::num;

/// Negates x: the game's left-handed space to the bridge's right-handed one, for a direction.
pub fn direction_to_bridge(d: [f32; 3]) -> Option<[f32; 3]> {
    let v = [-d[0], d[1], d[2]];
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (n.is_finite() && n > 1e-6).then(|| v.map(|c| c / n))
}

/// The `skyLightColor` that `/EE/Types/Engine/LevelInfo` gives every level.
pub fn is_engine_default(sky_light_color: [f32; 4]) -> bool {
    sky_light_color == [0.0, 0.0, 0.0, 255.0]
}

fn linear(c: [f32; 4]) -> [f32; 3] {
    [0, 1, 2].map(|k| srgb_to_linear((c[k] / 255.0).clamp(0.0, 1.0)))
}

/// Where a [`Sun`] was read from.
#[derive(Debug, Clone, PartialEq)]
pub enum SunSource {
    /// A `Light` with `Directional=1`: the light's name (scope and type).
    DirectionalLight(String),
    /// `PostProcessInfo.sunPosition` / `sunColor` of a `LevelInfo`.
    PostProcessInfo,
}

/// A sun the level states. Which of several suns the engine draws is not in the data; they are
/// all listed.
#[derive(Debug, Clone, PartialEq)]
pub struct Sun {
    pub source: SunSource,
    /// The vector as stored, in the game's space. For a light it is `DirectionalLightDir`, **read
    /// as pointing toward the sun**: that is an inference, the data does not say it in words. Its
    /// support: y is positive in 557 of the 566 directional lights, and
    /// `PostProcessInfo.sunPosition` is a position of the sun (RailjackInteriorV2's is
    /// (0, 28.7, 95.8), 100 m from the origin, like `DynamicSky`'s `SkyboxSunDistance=100`),
    /// which agrees with the light's direction within 8 degrees in 148 of the 348 levels that set
    /// both. In the other 200 the two stored suns disagree (Cetus's by 145 degrees); both are
    /// listed and the data does not say which the game draws. The length differs from 1 by more
    /// than 0.01 in 171 lights.
    pub toward_sun_game: Option<[f32; 3]>,
    /// The same vector, normalised, in the bridge's space (x negated). `None` for a zero vector.
    /// Still the reading "toward the sun" of [`Sun::toward_sun_game`], not a stated fact.
    pub toward_sun: Option<[f32; 3]>,
    /// A light's `Color` as stored: 0..255 sRGB-coded, alpha last.
    pub color: Option<[f32; 4]>,
    /// The same colour decoded to linear RGB (the bridge's rule for light colours).
    pub color_linear: Option<[f32; 3]>,
    /// A light's `Brightness` and `Lumens` as stored: the game's own relative numbers (no lux).
    pub brightness: Option<f32>,
    pub lumens: Option<f32>,
    /// `PostProcessInfo.sunColor` as stored: linear RGB floats that exceed 1 (an HDR colour), and
    /// the fourth value, whose meaning the data does not give.
    pub sun_color: Option<[f32; 4]>,
    /// A light's `On` flag (false: it starts switched off).
    pub on: Option<bool>,
}

/// The sky-related settings of the level (`LevelInfo.LightingParams`, `ZoneAttribs.BackDropId`).
///
/// `LightingParams` holds the baking parameters (beside `bakingOptions:*`): what these are is a
/// lighting input, not shown to be the sky the game draws behind the level. The bridge scene
/// therefore leaves them out of `Scene::environment` unless asked.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sky {
    /// `skyLightColor`, 0..255, alpha last: the level's own value if it sets one, else the
    /// type's (black). See [`Sky::sky_light_color_set_by_level`].
    pub sky_light_color: Option<[f32; 4]>,
    /// The level set `skyLightColor` in its own `LightingParams` (not inherited from a type).
    pub sky_light_color_set_by_level: bool,
    /// The `LevelInfo` type that gives a colour other than the engine default black, when the
    /// level does not set one itself (the Orbiter's `/Lotus/Levels/Proc/PlayerShipLevelInfo`
    /// gives grey 128): a value some content type states, not the engine's.
    pub sky_light_color_from_type: Option<String>,
    /// `globalCubeMap`, `iblCubeMap`, `iblImageFile`: pictures the baker lights with, by path;
    /// empty strings are dropped. Most are editor files that are not in the install. The
    /// `globalCubeMap` is weighted by [`Sky::gi_environment_intensity`] (0 in Duviri's levels).
    pub global_cube_map: Option<String>,
    pub ibl_cube_map: Option<String>,
    pub ibl_image_file: Option<String>,
    pub sky_is_sphere: Option<bool>,
    pub gi_environment_intensity: Option<f32>,
    /// The distinct `BackDropId` of the level's zones (a tag the game's code matches with a
    /// backdrop level at run time), and how many zones are backdrops themselves (`BackDrop=1`).
    pub backdrop_ids: Vec<String>,
    pub backdrop_zones: usize,
}

/// Ambient light of a zone: a hemisphere of three colours (0..255, alpha last).
#[derive(Debug, Clone, PartialEq)]
pub struct Ambient {
    /// `AmbientLight` (up), `AmbientLightMid` (around), `AmbientLightLow` (down). Which colour
    /// is which direction is the field names' reading; the data has no more.
    pub top: Option<[f32; 4]>,
    pub mid: Option<[f32; 4]>,
    pub low: Option<[f32; 4]>,
    /// `ZoneUsesAmbient`. False in 11,638 of 11,664 zones: then these colours are present in the
    /// data but the zone is lit by its lightmaps and probes.
    pub used: Option<bool>,
    pub rim_light: Option<[f32; 4]>,
}

/// Fog settings, as stored. The densities have no unit in the data and the law that turns them
/// into attenuation is shader code, so they are numbers of the game, not an extinction in 1/m.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fog {
    /// 0..255, alpha last.
    pub color: Option<[f32; 4]>,
    pub distance_density: Option<f32>,
    pub height_density: Option<f32>,
    pub height_falloff: Option<f32>,
    pub height_y: Option<f32>,
    pub linear_density: Option<f32>,
    pub horizon_fog: Option<f32>,
    pub extinction: Option<f32>,
    pub texture: Option<String>,
}

impl Fog {
    fn is_empty(&self) -> bool {
        *self == Fog::default()
    }
}

/// What one zone (`ZoneAttribs`) says about its light.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneAtmosphere {
    pub object: usize,
    pub scope: String,
    /// The zone object's position in the game's space.
    pub position: [f32; 3],
    pub ambient: Option<Ambient>,
    /// The zone's own fog, which replaces the level's when `override_level_fog` is true.
    pub fog: Option<Fog>,
    pub override_level_fog: Option<bool>,
    /// `ReflectionMap`: the zone's probe (a type 183 cube map, see [`crate::probe`]).
    pub reflection_map: Option<String>,
    /// `ReflectionSH1`: four numbers, as stored (their meaning is not in the data).
    pub reflection_sh1: Option<[f32; 4]>,
    pub use_light_map: Option<bool>,
    pub light_probe_radius: Option<f32>,
}

/// Baked indirect light.
#[derive(Debug, Clone, Default)]
pub struct GlobalIllumination {
    pub volumes: Vec<GiVolume>,
    /// `LevelInfo.EnableGIVolumes`.
    pub enabled: Option<bool>,
    /// `LevelInfo.LightProbeData` (`.lpd`) and `EnvProbesData` (`.epd`) of the level, absolute paths.
    pub light_probe_data: Vec<String>,
    pub env_probes_data: Vec<String>,
    pub global_probe_position: Option<[f32; 3]>,
    /// `PostProcessInfo.irradianceVolumeGain`.
    pub irradiance_volume_gain: Option<f32>,
    /// Instances with lightmap pages (see [`crate::lightmap`]).
    pub lightmapped_instances: usize,
}

/// Something the data points at that is not turned into numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftOut {
    pub what: &'static str,
    pub count: usize,
    pub why: &'static str,
}

/// Everything a level says about its sky and light. See the module documentation.
#[derive(Debug, Clone, Default)]
pub struct Atmosphere {
    pub sky: Sky,
    pub suns: Vec<Sun>,
    pub dynamic_skies: Vec<DynamicSky>,
    /// The level's own fog (`LevelInfo.PostProcessInfo`).
    pub fog: Option<Fog>,
    pub zones: Vec<ZoneAtmosphere>,
    pub gi: GlobalIllumination,
}

fn non_empty(s: &Option<String>) -> Option<String> {
    s.clone().filter(|s| !s.trim().is_empty())
}

fn light_name(l: &LevelLight) -> String {
    format!(
        "{}{}",
        l.scope,
        l.type_path.rsplit('/').next().unwrap_or(&l.type_path)
    )
}

impl Atmosphere {
    /// Reads the sky, suns, ambient light, fog and GI of a level description.
    pub fn of(d: &LevelDescription) -> Atmosphere {
        let mut a = Atmosphere {
            dynamic_skies: d.dynamic_skies.clone(),
            gi: GlobalIllumination {
                volumes: d.gi_volumes.clone(),
                lightmapped_instances: d.instances.iter().filter(|i| i.lightmap.is_some()).count(),
                ..GlobalIllumination::default()
            },
            ..Atmosphere::default()
        };

        for e in &d.environments {
            match e.kind {
                EnvironmentKind::Level => {
                    if let Some(lp) = &e.lighting.lighting_params {
                        let s = &mut a.sky;
                        // A colour a level sets in its own table wins over the type's
                        match e.sky_light_color_own {
                            Some(own) if !s.sky_light_color_set_by_level => {
                                s.sky_light_color = Some(own);
                                s.sky_light_color_set_by_level = true;
                                s.sky_light_color_from_type = None;
                            }
                            _ if s.sky_light_color.is_none() => {
                                s.sky_light_color = lp.sky_light_color;
                                if lp.sky_light_color.is_some_and(|c| !is_engine_default(c)) {
                                    s.sky_light_color_from_type = Some(e.type_path.clone());
                                }
                            }
                            _ => {}
                        }
                        s.global_cube_map =
                            s.global_cube_map.take().or(non_empty(&lp.global_cube_map));
                        s.ibl_cube_map = s.ibl_cube_map.take().or(non_empty(&lp.ibl_cube_map));
                        s.ibl_image_file =
                            s.ibl_image_file.take().or(non_empty(&lp.ibl_image_file));
                        s.sky_is_sphere = s.sky_is_sphere.or(lp.sky_is_sphere);
                        s.gi_environment_intensity =
                            s.gi_environment_intensity.or(lp.gi_environment_intensity);
                    }
                    if let Some(p) = &e.lighting.post_process {
                        if a.fog.is_none() {
                            let f = Fog {
                                color: p.fog_color,
                                distance_density: p.distance_fog_density,
                                height_density: p.height_fog_density,
                                height_falloff: p.height_fog_falloff,
                                height_y: p.height_fog_y,
                                linear_density: p.linear_fog_density,
                                horizon_fog: p.horizon_fog,
                                extinction: p.extinction,
                                texture: non_empty(&p.fog_texture),
                            };
                            if !f.is_empty() {
                                a.fog = Some(f);
                            }
                        }
                        if let Some(pos) = p.sun_position {
                            a.suns.push(Sun {
                                source: SunSource::PostProcessInfo,
                                toward_sun_game: Some(pos),
                                toward_sun: direction_to_bridge(pos),
                                color: None,
                                color_linear: None,
                                brightness: None,
                                lumens: None,
                                sun_color: p.sun_color,
                                on: None,
                            });
                        }
                        if a.gi.irradiance_volume_gain.is_none() {
                            a.gi.irradiance_volume_gain = p.irradiance_volume_gain;
                        }
                    }
                    if let Some(p) = &e.light_probe_data {
                        a.gi.light_probe_data.push(p.clone());
                    }
                    if let Some(p) = &e.env_probes_data {
                        a.gi.env_probes_data.push(p.clone());
                    }
                    a.gi.enabled = a.gi.enabled.or(e.enable_gi_volumes);
                    a.gi.global_probe_position =
                        a.gi.global_probe_position.or(e.global_probe_position);
                }
                EnvironmentKind::Zone => {
                    if let Some(id) = e.backdrop.as_deref().filter(|s| !s.is_empty()) {
                        if !a.sky.backdrop_ids.iter().any(|b| b == id) {
                            a.sky.backdrop_ids.push(id.to_string());
                        }
                    }
                    if e.attributes
                        .iter()
                        .any(|(k, v)| k == "BackDrop" && v.trim() == "1")
                    {
                        a.sky.backdrop_zones += 1;
                    }
                    let z = e.lighting.zone.as_ref();
                    let ambient = z.and_then(|z| {
                        let amb = Ambient {
                            top: z.ambient_light,
                            mid: z.ambient_light_mid,
                            low: z.ambient_light_low,
                            used: z.zone_uses_ambient,
                            rim_light: z.rim_light,
                        };
                        (amb.top.is_some() || amb.mid.is_some() || amb.low.is_some()).then_some(amb)
                    });
                    let fog = z.and_then(|z| {
                        let f = Fog {
                            color: z.fog_color,
                            distance_density: z.distance_fog_density,
                            height_density: z.height_fog_density,
                            height_falloff: z.height_fog_falloff,
                            height_y: z.height_fog_y,
                            ..Fog::default()
                        };
                        (!f.is_empty()).then_some(f)
                    });
                    a.zones.push(ZoneAtmosphere {
                        object: e.object,
                        scope: e.scope.clone(),
                        position: e.position,
                        ambient,
                        fog,
                        override_level_fog: z.and_then(|z| z.override_level_fog),
                        reflection_map: e.reflection_map.clone(),
                        reflection_sh1: z.and_then(|z| z.reflection_sh1),
                        use_light_map: z.and_then(|z| z.use_light_map),
                        light_probe_radius: z.and_then(|z| z.light_probe_radius),
                    });
                }
            }
        }

        for l in d.lights.iter().filter(|l| l.directional == Some(true)) {
            a.suns.push(Sun {
                source: SunSource::DirectionalLight(light_name(l)),
                toward_sun_game: l.direction,
                toward_sun: l.direction.and_then(direction_to_bridge),
                color: l.color,
                color_linear: l.color.map(linear),
                brightness: l.brightness,
                lumens: l.lumens,
                sun_color: None,
                on: l.on,
            });
        }
        a
    }

    /// What this level's data points at and this reader does not turn into numbers.
    pub fn left_out(&self) -> Vec<LeftOut> {
        let mut out = Vec::new();
        let lights = self
            .suns
            .iter()
            .filter(|s| matches!(s.source, SunSource::DirectionalLight(_)))
            .count();
        if lights > 0 {
            out.push(LeftOut {
                what: "directional light strength",
                count: lights,
                why: "the data gives Lumens (1600 x Brightness) and a colour, not lux or an exposure; the bridge's directional light is in lux",
            });
        }
        if !self.dynamic_skies.is_empty() {
            out.push(LeftOut {
                what: "sun position of a dynamic sky",
                count: self.dynamic_skies.len(),
                why: "the sun follows TimeOfDay through Latitude, SunDeclination and the solar offsets in script and shader code; an ExplicitSunDirection is stored by some but its relation to FlipSunDirection and SunDirectionDecoBone is code too (Cetus's points the other way from its directional light), and the `SunDir` among its material constants is a stored snapshot of a value the game's code overwrites (7 distinct vectors in 44 sky objects, the same one at different hours), so the sky is kept as stored in Atmosphere::dynamic_skies (keyframes, and the sky material's constants, whose formula is in a shader that is not decoded) and no sun is made from it",
            });
        }
        if !self.gi.volumes.is_empty() {
            out.push(LeftOut {
                what: "GI volume probes",
                count: self.gi.volumes.len(),
                why: "the probes are 3D textures (type 175) of packed spherical-harmonic bands; the shader that reconstructs light from them is not decoded",
            });
        }
        let probe_files = self.gi.light_probe_data.len() + self.gi.env_probes_data.len();
        if probe_files > 0 {
            out.push(LeftOut {
                what: ".lpd light probes and .epd environment probes",
                count: probe_files,
                why: "the per-probe payload of these files is not decoded, and the bridge has no probe type",
            });
        }
        let fogged = self.fog.is_some() || self.zones.iter().any(|z| z.fog.is_some());
        if fogged {
            out.push(LeftOut {
                what: "fog",
                count: usize::from(self.fog.is_some())
                    + self.zones.iter().filter(|z| z.fog.is_some()).count(),
                why: "the bridge's Scene has no fog, and the law that turns the densities into attenuation is shader code; the settings are in Atmosphere::fog and ZoneAtmosphere::fog as stored",
            });
        }
        let ambient = self
            .zones
            .iter()
            .filter(|z| z.ambient.as_ref().is_some_and(|a| a.used == Some(true)))
            .count();
        if ambient > 0 {
            out.push(LeftOut {
                what: "zone ambient light",
                count: ambient,
                why: "a scene has one environment and zones are regions; the gradients are in ZoneAtmosphere::ambient as stored, and how the engine blends the three colours is shader code",
            });
        }
        out
    }

    /// The same data as JSON, for the glTF export's `extras` (values as stored, colours 0..255).
    pub fn to_json(&self) -> Json {
        let arr = |v: &[f32]| Json::Array(v.iter().map(|x| num(*x)).collect());
        let opt3 = |v: Option<[f32; 3]>| v.map_or(Json::Null, |v| arr(&v));
        let opt4 = |v: Option<[f32; 4]>| v.map_or(Json::Null, |v| arr(&v));
        let fog = |f: &Fog| {
            json!({
                "color": opt4(f.color), "distanceDensity": f.distance_density.map(num),
                "heightDensity": f.height_density.map(num), "heightFalloff": f.height_falloff.map(num),
                "heightY": f.height_y.map(num), "linearDensity": f.linear_density.map(num),
                "horizonFog": f.horizon_fog.map(num), "extinction": f.extinction.map(num),
                "texture": f.texture,
            })
        };
        json!({
            "units": "colours 0..255 sRGB-coded, alpha last, unless named linear; directions: towardSunGame is the vector as stored and towardSun the same, unit length, in the bridge's space (x negated); both read DirectionalLightDir as pointing toward the sun, which is an inference (see Sun::toward_sun_game), not stated by the data; densities and Brightness/Lumens are the game's own numbers",
            "sky": {
                "skyLightColor": opt4(self.sky.sky_light_color),
                "skyLightColorSetByLevel": self.sky.sky_light_color_set_by_level,
                "skyLightColorFromType": self.sky.sky_light_color_from_type,
                "giEnvironmentIntensity": self.sky.gi_environment_intensity.map(num),
                "globalCubeMap": self.sky.global_cube_map, "iblCubeMap": self.sky.ibl_cube_map,
                "iblImageFile": self.sky.ibl_image_file, "skyIsSphere": self.sky.sky_is_sphere,
                "backdropIds": self.sky.backdrop_ids, "backdropZones": self.sky.backdrop_zones,
            },
            "suns": self.suns.iter().map(|s| json!({
                "source": match &s.source {
                    SunSource::DirectionalLight(n) => format!("light {n}"),
                    SunSource::PostProcessInfo => "PostProcessInfo.sunPosition".to_string(),
                        },
                "towardSunGame": opt3(s.toward_sun_game), "towardSun": opt3(s.toward_sun),
                "color": opt4(s.color), "colorLinear": opt3(s.color_linear),
                "brightness": s.brightness.map(num), "lumens": s.lumens.map(num),
                "sunColor": opt4(s.sun_color), "on": s.on,
            })).collect::<Vec<_>>(),
            "dynamicSkies": self.dynamic_skies.iter().map(|s| json!({
                "timeOfDay": s.time_of_day.map(num), "timeRate": s.time_rate.map(num),
                "latitude": s.latitude.map(num), "sunDeclination": s.sun_declination.map(num),
                "useExplicitSunDirection": s.use_explicit_sun_direction,
                "explicitSunDirection": opt3(s.explicit_sun_direction),
                "keys": s.keys.iter().map(|k| json!({
                    "time": k.time.map(num), "sunColor": opt4(k.sun_color),
                    "sunBrightness": k.sun_brightness.map(num), "skySunBrightness": k.sky_sun_brightness.map(num),
                    "exposure": k.exposure.map(num), "fogColor": opt4(k.fog_color),
                    "skyLight": k.sky_light, "debugName": k.debug_name,
                })).collect::<Vec<_>>(),
                "skyMaterial": s.sky_material,
                "materialParams": s.material_params.iter().map(|(n, v)| json!({"name": n, "values": arr(v)})).collect::<Vec<_>>(),
                "illuminationKeys": s.illumination_keys.iter().map(|(t, i)| json!([num(*t), num(*i)])).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "fog": self.fog.as_ref().map(fog),
            "zones": self.zones.iter().map(|z| json!({
                "object": z.object, "scope": z.scope, "position": arr(&z.position),
                "ambient": z.ambient.as_ref().map(|a| json!({
                    "top": opt4(a.top), "mid": opt4(a.mid), "low": opt4(a.low),
                    "used": a.used, "rimLight": opt4(a.rim_light),
                })),
                "fog": z.fog.as_ref().map(fog), "overrideLevelFog": z.override_level_fog,
                "reflectionMap": z.reflection_map, "reflectionSH1": opt4(z.reflection_sh1),
            })).collect::<Vec<_>>(),
            "gi": {
                "enabled": self.gi.enabled, "lightProbeData": self.gi.light_probe_data,
                "envProbesData": self.gi.env_probes_data,
                "globalProbePosition": opt3(self.gi.global_probe_position),
                "irradianceVolumeGain": self.gi.irradiance_volume_gain.map(num),
                "lightmappedInstances": self.gi.lightmapped_instances,
                "volumes": self.gi.volumes.iter().map(|v| json!({
                    "position": arr(&v.position), "rotation": arr(&v.rotation),
                    "dimensions": opt3(v.dimensions), "cellSize": v.cell_size.map(num),
                    "textures": v.textures, "indirection": v.indirection,
                    "indirectionSize": v.indirection_size, "fallbackBand0": v.fallback_band0,
                    "band1Scale": v.band1_scale.iter().map(|r| arr(r)).collect::<Vec<_>>(),
                    "band1Offset": v.band1_offset.iter().map(|r| arr(r)).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            },
            "leftOut": self.left_out().iter().map(|l| json!({"what": l.what, "count": l.count, "why": l.why})).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::{Environment, Light};
    use crate::postprocess::{EnvironmentLighting, LightingParams, PostProcess, ZoneLighting};

    fn level_env(lighting: EnvironmentLighting, attrs: &[(&str, &str)]) -> Environment {
        Environment {
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
            lighting,
            sky_light_color_own: None,
            light_probe_data: Some("/L/0.lpd".into()),
            env_probes_data: Some("/L/0.epd".into()),
            global_probe_position: Some([1.0, 2.0, 3.0]),
            enable_gi_volumes: Some(true),
            initial_time_of_day: None,
            attributes: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn sun_light() -> Light {
        Light {
            type_path: "/Lotus/Levels/X/Sun".into(),
            scope: "/Layer1/".into(),
            color: Some([255.0, 128.0, 0.0, 255.0]),
            brightness: Some(2.0),
            lumens: Some(3200.0),
            directional: Some(true),
            direction: Some([1.0, 2.0, 2.0]),
            ..Light::default()
        }
    }

    #[test]
    fn a_directional_light_is_a_sun_toward_the_light_in_bridge_space() {
        let mut d = LevelDescription::default();
        d.lights.push(sun_light());
        let a = Atmosphere::of(&d);
        assert_eq!(a.suns.len(), 1);
        let s = &a.suns[0];
        assert_eq!(s.source, SunSource::DirectionalLight("/Layer1/Sun".into()));
        assert_eq!(s.toward_sun_game, Some([1.0, 2.0, 2.0]));
        // Unit length, x negated
        let t = s.toward_sun.unwrap();
        assert!((t[0] + 1.0 / 3.0).abs() < 1e-6 && (t[1] - 2.0 / 3.0).abs() < 1e-6);
        assert!((t.iter().map(|c| c * c).sum::<f32>() - 1.0).abs() < 1e-6);
        // sRGB decoded: 128 is darker in linear light, 255 stays 1
        let c = s.color_linear.unwrap();
        assert_eq!(c[0], 1.0);
        assert!((c[1] - 0.2158).abs() < 1e-3);
        assert_eq!((s.brightness, s.lumens), (Some(2.0), Some(3200.0)));
        // Its strength is not converted: the reason is listed
        let left = a.left_out();
        assert_eq!(left[0].what, "directional light strength");
        assert_eq!(left[0].count, 1);
    }

    #[test]
    fn a_zero_direction_has_no_unit_vector() {
        assert_eq!(direction_to_bridge([0.0; 3]), None);
        assert_eq!(direction_to_bridge([f32::NAN, 0.0, 1.0]), None);
        assert_eq!(direction_to_bridge([-3.0, 0.0, 0.0]), Some([1.0, 0.0, 0.0]));
    }

    #[test]
    fn sky_fog_sun_and_gi_come_from_the_level_info() {
        let lp = LightingParams {
            sky_light_color: Some([1.0, 2.0, 3.0, 255.0]),
            global_cube_map: Some("/Lotus/Materials/Cubemaps/G_cross.hdr".into()),
            ibl_image_file: Some(String::new()),
            ..LightingParams::default()
        };
        let pp = PostProcess {
            fog_color: Some([102.0, 110.0, 126.0, 255.0]),
            distance_fog_density: Some(0.17),
            sun_position: Some([0.0, 1.0, 0.0]),
            sun_color: Some([1.1, 1.0, 0.9, 0.0]),
            irradiance_volume_gain: Some(0.5),
            ..PostProcess::default()
        };
        let lighting = EnvironmentLighting {
            lighting_params: Some(lp),
            post_process: Some(pp),
            ..EnvironmentLighting::default()
        };
        let mut d = LevelDescription::default();
        let mut e = level_env(
            lighting,
            &[("LightingParams", "{skyLightColor={1,2,3,255}}")],
        );
        e.sky_light_color_own = Some([1.0, 2.0, 3.0, 255.0]);
        d.environments.push(e);
        let a = Atmosphere::of(&d);
        assert_eq!(a.sky.sky_light_color, Some([1.0, 2.0, 3.0, 255.0]));
        assert!(a.sky.sky_light_color_set_by_level);
        assert_eq!(
            a.sky.global_cube_map.as_deref(),
            Some("/Lotus/Materials/Cubemaps/G_cross.hdr")
        );
        // An empty path is no path
        assert_eq!(a.sky.ibl_image_file, None);
        let f = a.fog.as_ref().unwrap();
        assert_eq!(f.color, Some([102.0, 110.0, 126.0, 255.0]));
        assert_eq!(f.distance_density, Some(0.17));
        assert_eq!(a.suns.len(), 1);
        assert_eq!(a.suns[0].source, SunSource::PostProcessInfo);
        assert_eq!(a.suns[0].toward_sun, Some([-0.0, 1.0, 0.0]));
        assert_eq!(a.suns[0].sun_color, Some([1.1, 1.0, 0.9, 0.0]));
        assert_eq!(a.gi.light_probe_data, vec!["/L/0.lpd".to_string()]);
        assert_eq!(a.gi.env_probes_data, vec!["/L/0.epd".to_string()]);
        assert_eq!(a.gi.global_probe_position, Some([1.0, 2.0, 3.0]));
        assert_eq!(a.gi.enabled, Some(true));
        assert_eq!(a.gi.irradiance_volume_gain, Some(0.5));
        let left: Vec<_> = a.left_out().iter().map(|l| l.what).collect();
        assert!(left.contains(&"fog"));
        assert!(left.contains(&".lpd light probes and .epd environment probes"));
    }

    #[test]
    fn a_sky_colour_from_the_type_is_not_set_by_the_level() {
        let lp = LightingParams {
            sky_light_color: Some([0.0, 0.0, 0.0, 255.0]),
            ..LightingParams::default()
        };
        let lighting = EnvironmentLighting {
            lighting_params: Some(lp),
            ..EnvironmentLighting::default()
        };
        let mut d = LevelDescription::default();
        // The merged text of the object carries the type's `skyLightColor` too: that must not
        // count as the level's own
        d.environments.push(level_env(
            lighting.clone(),
            &[("LightingParams", "{skyLightColor={0,0,0,255}}")],
        ));
        let a = Atmosphere::of(&d);
        assert_eq!(a.sky.sky_light_color, Some([0.0, 0.0, 0.0, 255.0]));
        assert!(!a.sky.sky_light_color_set_by_level);
        // The same colour in the level's own table is a statement of the level
        let mut own = level_env(lighting, &[]);
        own.sky_light_color_own = Some([0.0, 0.0, 0.0, 255.0]);
        let mut d = LevelDescription::default();
        d.environments.push(own);
        assert!(Atmosphere::of(&d).sky.sky_light_color_set_by_level);
    }

    #[test]
    fn a_colour_a_content_type_gives_is_named_but_is_not_the_levels_own() {
        let lighting = EnvironmentLighting {
            lighting_params: Some(LightingParams {
                sky_light_color: Some([128.0, 128.0, 128.0, 255.0]),
                ..LightingParams::default()
            }),
            ..EnvironmentLighting::default()
        };
        let mut e = level_env(lighting, &[]);
        e.type_path = "/Lotus/Levels/Proc/PlayerShipLevelInfo".into();
        let mut d = LevelDescription::default();
        d.environments.push(e);
        let a = Atmosphere::of(&d);
        assert_eq!(
            a.sky.sky_light_color,
            Some(
                [128.0; 3]
                    .into_iter()
                    .chain([255.0])
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap()
            )
        );
        assert!(!a.sky.sky_light_color_set_by_level);
        assert_eq!(
            a.sky.sky_light_color_from_type.as_deref(),
            Some("/Lotus/Levels/Proc/PlayerShipLevelInfo")
        );
    }

    #[test]
    fn a_colour_the_level_sets_wins_over_an_earlier_level_info_that_does_not() {
        let typed = |c: [f32; 4]| EnvironmentLighting {
            lighting_params: Some(LightingParams {
                sky_light_color: Some(c),
                ..LightingParams::default()
            }),
            ..EnvironmentLighting::default()
        };
        let mut d = LevelDescription::default();
        d.environments
            .push(level_env(typed([0.0, 0.0, 0.0, 255.0]), &[]));
        let mut second = level_env(typed([69.0, 84.0, 86.0, 255.0]), &[]);
        second.sky_light_color_own = Some([69.0, 84.0, 86.0, 255.0]);
        d.environments.push(second);
        let a = Atmosphere::of(&d);
        assert_eq!(a.sky.sky_light_color, Some([69.0, 84.0, 86.0, 255.0]));
        assert!(a.sky.sky_light_color_set_by_level);
    }

    #[test]
    fn zones_carry_their_ambient_fog_and_backdrop() {
        let z = ZoneLighting {
            ambient_light: Some([50.0; 4]),
            ambient_light_mid: Some([60.0; 4]),
            ambient_light_low: Some([70.0; 4]),
            zone_uses_ambient: Some(true),
            override_level_fog: Some(true),
            fog_color: Some([53.0, 88.0, 105.0, 60.0]),
            distance_fog_density: Some(0.435),
            reflection_sh1: Some([0.03, 0.0, 0.0, 0.0]),
            ..ZoneLighting::default()
        };
        let mut e = level_env(
            EnvironmentLighting {
                zone: Some(z),
                ..EnvironmentLighting::default()
            },
            &[("BackDrop", "1")],
        );
        e.kind = EnvironmentKind::Zone;
        e.backdrop = Some("BackDropSpace".into());
        e.reflection_map = Some("/L/0_cross.hdr".into());
        let mut d = LevelDescription::default();
        d.environments.push(e);
        let a = Atmosphere::of(&d);
        assert_eq!(a.sky.backdrop_ids, vec!["BackDropSpace".to_string()]);
        assert_eq!(a.sky.backdrop_zones, 1);
        let zone = &a.zones[0];
        let amb = zone.ambient.as_ref().unwrap();
        assert_eq!(
            (amb.top, amb.mid, amb.low, amb.used),
            (
                Some([50.0; 4]),
                Some([60.0; 4]),
                Some([70.0; 4]),
                Some(true)
            )
        );
        assert_eq!(zone.fog.as_ref().unwrap().distance_density, Some(0.435));
        assert_eq!(zone.override_level_fog, Some(true));
        assert_eq!(zone.reflection_sh1, Some([0.03, 0.0, 0.0, 0.0]));
        let left: Vec<_> = a.left_out().iter().map(|l| l.what).collect();
        assert!(left.contains(&"zone ambient light"));
    }

    #[test]
    fn a_dynamic_sky_is_kept_as_stored_and_makes_no_sun() {
        let mut d = LevelDescription::default();
        d.dynamic_skies.push(DynamicSky {
            use_explicit_sun_direction: Some(true),
            explicit_sun_direction: Some([-0.2, 0.6, 1.0]),
            ..DynamicSky::default()
        });
        let a = Atmosphere::of(&d);
        assert!(a.suns.is_empty());
        assert_eq!(a.dynamic_skies.len(), 1);
        let left = a.left_out();
        assert_eq!(left[0].what, "sun position of a dynamic sky");
        assert_eq!(left[0].count, 1);
    }

    #[test]
    fn json_holds_the_units_and_what_is_left_out() {
        let mut d = LevelDescription::default();
        d.lights.push(sun_light());
        let j = Atmosphere::of(&d).to_json();
        assert!(j["units"].as_str().unwrap().contains("0..255"));
        assert_eq!(j["suns"][0]["lumens"], 3200.0);
        assert_eq!(j["leftOut"][0]["what"], "directional light strength");
    }
}
