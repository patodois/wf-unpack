//! Post-processing, exposure and baked-lighting settings of levels and zones.
//!
//! Where they live:
//!
//! - `LevelInfo.PostProcessInfo`: a table with the level's exposure, tone
//!   mapping, colour grading, bloom, fog and light-map settings. The 100-odd
//!   field names were taken from the engine executable's property table and
//!   checked against every `PostProcessInfo` in Packages.bin (210 of them) and
//!   in the levels. A level that sets a field overrides the value its type
//!   gives; fields it leaves out keep the type's, so a type chain and the
//!   object's own text are overlaid field by field (root first). Defaults
//!   compiled into the executable are not in the data: a field neither sets is
//!   `None` here. `/EE/Types/Engine/LevelInfo` itself only sets
//!   `targetExposure=0.07` and `adaptationRateUp=3`.
//! - `LevelInfo.PostProcessInfoAlt`: the same fields for an alternative look
//!   (28 levels).
//! - `LevelInfo.LightingParams`: the baking parameters (sky light colour,
//!   global cube map, `diffuseBoost`, `emissiveScale`, `bakingOptions:*`).
//! - `ZoneAttribs`: per-zone ambient colours, light-map tint, reflection probe
//!   and optional exposure and fog overrides ([`ZoneLighting`]).
//! - `PostProcessInfo.postFXMaterial` names the final-pass material
//!   (`/Lotus/Types/ACESPost` in most levels): a pixel shader
//!   (`FinalScenePass.hlsl`) with compile-time switches ([`PostFxMaterial`]).
//!
//! `toneMapType` is one of `TM_LINEAR`, `TM_UNCHARTED`, `TM_ACES_REF`,
//! `TM_ACES_OPT`, `TM_FILMIC`, `TM_CUSTOM`, `TM_ACES_1000`, `TM_DISPLAY_RANGE`
//! (names from the executable; only `TM_ACES_REF` and `TM_CUSTOM` occur in
//! the data). Colours keep the units the data uses: `fogColor`, `lightMapTint`
//! and the zone's colours are 0..255 with alpha last, `bloomTint` and
//! `colorCorrectTint` are 0..1. What the engine does with them (the tone
//! curve, the exposure adaptation) is code, not data, and is not described.

use std::collections::BTreeMap;

use serde_json::{json, Value as Json};

use crate::ee::Value;

/// A value type a typed table can hold.
pub trait Field: Sized {
    fn parse(v: &Value) -> Option<Self>;
    fn to_json(&self) -> Json;
}

/// An `f32` as the JSON number with its shortest decimal form (0.07, not 0.07000000029802322).
pub fn num(v: f32) -> Json {
    format!("{v}")
        .parse::<f64>()
        .ok()
        .and_then(serde_json::Number::from_f64)
        .map_or(Json::Null, Json::Number)
}

impl Field for f32 {
    fn parse(v: &Value) -> Option<Self> {
        v.text()?.trim().parse().ok()
    }
    fn to_json(&self) -> Json {
        num(*self)
    }
}

impl Field for bool {
    fn parse(v: &Value) -> Option<Self> {
        match v.text()?.trim() {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        }
    }
    fn to_json(&self) -> Json {
        json!(self)
    }
}

impl Field for String {
    fn parse(v: &Value) -> Option<Self> {
        v.text().map(|t| t.trim().to_string())
    }
    fn to_json(&self) -> Json {
        json!(self)
    }
}

macro_rules! float_array {
    ($n:literal) => {
        impl Field for [f32; $n] {
            fn parse(v: &Value) -> Option<Self> {
                v.floats()?.try_into().ok()
            }
            fn to_json(&self) -> Json {
                Json::Array(self.iter().map(|v| num(*v)).collect())
            }
        }
    };
}
float_array!(2);
float_array!(3);
float_array!(4);

/// One of the engine's tone mapping operators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToneMapType {
    Linear,
    Uncharted,
    AcesRef,
    AcesOpt,
    Filmic,
    Custom,
    Aces1000,
    DisplayRange,
    /// A name this reader does not know.
    Other(String),
}

impl ToneMapType {
    pub fn name(&self) -> &str {
        match self {
            ToneMapType::Linear => "TM_LINEAR",
            ToneMapType::Uncharted => "TM_UNCHARTED",
            ToneMapType::AcesRef => "TM_ACES_REF",
            ToneMapType::AcesOpt => "TM_ACES_OPT",
            ToneMapType::Filmic => "TM_FILMIC",
            ToneMapType::Custom => "TM_CUSTOM",
            ToneMapType::Aces1000 => "TM_ACES_1000",
            ToneMapType::DisplayRange => "TM_DISPLAY_RANGE",
            ToneMapType::Other(s) => s,
        }
    }
}

impl Field for ToneMapType {
    fn parse(v: &Value) -> Option<Self> {
        let t = v.text()?.trim();
        Some(match t {
            "TM_LINEAR" => ToneMapType::Linear,
            "TM_UNCHARTED" => ToneMapType::Uncharted,
            "TM_ACES_REF" => ToneMapType::AcesRef,
            "TM_ACES_OPT" => ToneMapType::AcesOpt,
            "TM_FILMIC" => ToneMapType::Filmic,
            "TM_CUSTOM" => ToneMapType::Custom,
            "TM_ACES_1000" => ToneMapType::Aces1000,
            "TM_DISPLAY_RANGE" => ToneMapType::DisplayRange,
            other => ToneMapType::Other(other.to_string()),
        })
    }
    fn to_json(&self) -> Json {
        json!(self.name())
    }
}

/// A point of the exposure curve: `{sceneEV=-5 adjustmentEV=-3.5}`. Either may be absent.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ExposurePoint {
    pub scene_ev: Option<f32>,
    pub adjustment_ev: Option<f32>,
}

impl Field for ExposurePoint {
    fn parse(v: &Value) -> Option<Self> {
        matches!(v, Value::Table(_)).then(|| ExposurePoint {
            scene_ev: v.get("sceneEV").and_then(f32::parse),
            adjustment_ev: v.get("adjustmentEV").and_then(f32::parse),
        })
    }
    fn to_json(&self) -> Json {
        json!({"sceneEV": self.scene_ev.map(num), "adjustmentEV": self.adjustment_ev.map(num)})
    }
}

/// `colorGradingData`: regions (`Global`, `Shadows`, `MidTones`, `Highlights`)
/// of settings (`Contrast`, `Gain`, `Gamma`, `Saturation`, `Offset`), each
/// one to four numbers (an RGB multiplier plus a master value, usually).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColorGradingData {
    pub regions: BTreeMap<String, BTreeMap<String, Vec<f32>>>,
    /// Anything that is not a number list, in the notation it came in.
    pub other: BTreeMap<String, String>,
}

impl Field for ColorGradingData {
    fn parse(v: &Value) -> Option<Self> {
        let Value::Table(items) = v else { return None };
        let mut out = ColorGradingData::default();
        for (region, rv) in items {
            match rv {
                Value::Table(settings) => {
                    for (k, sv) in settings {
                        match sv.floats() {
                            Some(f) => {
                                out.regions
                                    .entry(region.clone())
                                    .or_default()
                                    .insert(k.clone(), f);
                            }
                            None => {
                                out.other.insert(format!("{region}.{k}"), sv.to_string());
                            }
                        }
                    }
                }
                other => {
                    out.other.insert(region.clone(), other.to_string());
                }
            }
        }
        Some(out)
    }
    fn to_json(&self) -> Json {
        let regions: BTreeMap<&String, BTreeMap<&String, Json>> = self
            .regions
            .iter()
            .map(|(r, m)| {
                (
                    r,
                    m.iter()
                        .map(|(k, v)| (k, Json::Array(v.iter().map(|x| num(*x)).collect())))
                        .collect(),
                )
            })
            .collect();
        json!({"regions": regions, "other": self.other})
    }
}

/// Defines a struct of optional typed fields read from a keyed table, with
/// every key it does not know kept as text in `other`. Keys are compared with
/// their first letter lower-cased (`AmbientLight` and `ambientLight` are the same).
macro_rules! typed_table {
    ($(#[$meta:meta])* pub struct $name:ident { $( $(#[$fmeta:meta])* $field:ident : $ty:ty = $key:literal ),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default, PartialEq)]
        pub struct $name {
            $( $(#[$fmeta])* pub $field: Option<$ty>, )*
            /// Keys not listed above (nested tables, rarely used fields), as text.
            pub other: BTreeMap<String, String>,
        }

        impl $name {
            /// The keys this struct knows (first letter lower-cased).
            pub const KEYS: &'static [&'static str] = &[$($key),*];

            /// Reads a table; anything that is not a table gives an empty struct.
            pub fn from_value(v: &Value) -> Self {
                let mut out = Self::default();
                let Value::Table(items) = v else { return out };
                for (k, val) in items {
                    out.set(k, val);
                }
                out
            }

            /// Sets one key (later values win).
            pub fn set(&mut self, key: &str, val: &Value) {
                let mut k = key.to_string();
                if let Some(c) = k.get_mut(0..1) {
                    c.make_ascii_lowercase();
                }
                match k.as_str() {
                    $( $key => {
                        match <$ty as Field>::parse(val) {
                            Some(x) => self.$field = Some(x),
                            None => { self.other.insert(key.to_string(), val.to_string()); }
                        }
                    } )*
                    _ => { self.other.insert(key.to_string(), val.to_string()); }
                }
            }

            /// Lets every field `over` sets replace this one's.
            pub fn overlay(&mut self, over: &Self) {
                $( if over.$field.is_some() { self.$field = over.$field.clone(); } )*
                for (k, v) in &over.other {
                    self.other.insert(k.clone(), v.clone());
                }
            }

            /// The fields that are set, plus `other`, as a JSON object (original key spellings).
            pub fn to_json(&self) -> Json {
                let mut m = serde_json::Map::new();
                $( if let Some(x) = &self.$field { m.insert($key.to_string(), Field::to_json(x)); } )*
                if !self.other.is_empty() {
                    m.insert("other".to_string(), json!(self.other));
                }
                Json::Object(m)
            }

            /// How many typed fields are set.
            pub fn set_count(&self) -> usize {
                0 $( + usize::from(self.$field.is_some()) )*
            }
        }
    };
}

typed_table! {
    /// `LevelInfo.PostProcessInfo` (and `PostProcessInfoAlt`).
    pub struct PostProcess {
        // Exposure
        /// `{min, max}` of the automatic exposure.
        dynamic_exposure: [f32; 2] = "dynamicExposure",
        target_exposure: f32 = "targetExposure",
        adaptation_rate_up: f32 = "adaptationRateUp",
        adaptation_rate_down: f32 = "adaptationRateDown",
        exposure_converge_speed: f32 = "exposureConvergeSpeed",
        use_exposure_ev_curve: bool = "useExposureEVCurve",
        exposure_curve_min: ExposurePoint = "exposureCurveMin",
        exposure_curve_med: ExposurePoint = "exposureCurveMed",
        exposure_curve_max: ExposurePoint = "exposureCurveMax",
        exposure_curve_adjust_clamp: [f32; 2] = "exposureCurveAdjustClamp",
        exposure_curve_bias: f32 = "exposureCurveBias",
        // Tone mapping
        tone_map_type: ToneMapType = "toneMapType",
        tone_map_toe_strength: f32 = "toneMapToeStrength",
        tone_map_toe_length: f32 = "toneMapToeLength",
        tone_map_shoulder_strength: f32 = "toneMapShoulderStrength",
        tone_map_shoulder_length: f32 = "toneMapShoulderLength",
        tone_map_shoulder_angle: f32 = "toneMapShoulderAngle",
        /// A `.cube` colour lookup table.
        tone_map_lut: String = "toneMapLUT",
        tone_map_lut_blend: f32 = "toneMapLUTBlend",
        // Colour
        color_correct_tint: [f32; 4] = "colorCorrectTint",
        color_correct_curve_lift: [f32; 4] = "colorCorrectCurveLift",
        color_correction_converge: f32 = "colorCorrectionConverge",
        /// A `ColorGrading` asset (`/Lotus/Fx/ColorGradingData/...`); may be empty.
        color_grading: String = "colorGrading",
        color_grading_data: ColorGradingData = "colorGradingData",
        saturation: f32 = "saturation",
        /// 0..255, alpha last.
        desaturate_color: [f32; 4] = "desaturateColor",
        // Blur, bloom, depth of field
        blur: f32 = "blur",
        motion_blur_strength: f32 = "motionBlurStrength",
        bloom: f32 = "bloom",
        bloom_tint: [f32; 4] = "bloomTint",
        sky_bloom: f32 = "skyBloom",
        radial_blur_strength: f32 = "radialBlurStrength",
        radial_blur_scale: [f32; 2] = "radialBlurScale",
        focal_blur: f32 = "focalBlur",
        focal_tint: [f32; 4] = "focalTint",
        focal_near_plane: f32 = "focalNearPlane",
        focal_near_depth: f32 = "focalNearDepth",
        focal_far_plane: f32 = "focalFarPlane",
        focal_far_depth: f32 = "focalFarDepth",
        horizon_dof: f32 = "horizonDOF",
        horizon_radius: f32 = "horizonRadius",
        grain_bias: f32 = "grainBias",
        // Fog
        /// 0..255, alpha last.
        fog_color: [f32; 4] = "fogColor",
        distance_fog_density: f32 = "distanceFogDensity",
        linear_fog_density: f32 = "linearFogDensity",
        linear_fog_blend: f32 = "linearFogBlend",
        extinction: f32 = "extinction",
        height_fog_falloff: f32 = "heightFogFalloff",
        height_fog_density: f32 = "heightFogDensity",
        height_fog_y: f32 = "heightFogY",
        horizon_fog: f32 = "horizonFog",
        horizon_fog_radius: f32 = "horizonFogRadius",
        horizon_fog_elevation: f32 = "horizonFogElevation",
        fog_texture: String = "fogTexture",
        // Baked lighting and tints
        /// Multiplies every light map's colour; 0..255, alpha last.
        light_map_tint: [f32; 4] = "lightMapTint",
        /// Multiplies every light map's brightness.
        light_map_boost: f32 = "lightMapBoost",
        zone_map_tint: [f32; 4] = "zoneMapTint",
        ao_tint_color: [f32; 4] = "aoTintColor",
        irradiance_volume_gain: f32 = "irradianceVolumeGain",
        ssgi_attenuation: f32 = "ssgiAttenuation",
        vegetation_tint_lut: String = "vegetationTintLUT",
        tint_converge_speed: f32 = "tintConvergeSpeed",
        // Sun and shafts
        sun_position: [f32; 3] = "sunPosition",
        sun_color: [f32; 4] = "sunColor",
        sun_shafts: bool = "sunShafts",
        sun_shafts_density: f32 = "sunShaftsDensity",
        sun_shafts_decay: f32 = "sunShaftsDecay",
        sun_shafts_multiplier: f32 = "sunShaftsMultiplier",
        sun_shafts_tint_color: [f32; 4] = "sunShaftsTintColor",
        // The final pass
        /// The final-pass material, e.g. `/Lotus/Types/ACESPost`.
        post_fx_material: String = "postFXMaterial",
        post_fx_aux: f32 = "postFXAux",
    }
}

typed_table! {
    /// `LevelInfo.LightingParams`: what the lightmap baker used.
    pub struct LightingParams {
        /// 0..255, alpha last.
        sky_light_color: [f32; 4] = "skyLightColor",
        global_cube_map: String = "globalCubeMap",
        glocal_cube_map_atten: [f32; 4] = "glocalCubeMapAtten",
        ibl_cube_map: String = "iblCubeMap",
        ibl_image_file: String = "iblImageFile",
        diffuse_boost: f32 = "diffuseBoost",
        diffuse_gamma: f32 = "diffuseGamma",
        emissive_scale: f32 = "emissiveScale",
        gi_environment_intensity: f32 = "giEnvironmentIntensity",
        gi_environment_use_ibl: bool = "giEnvironmentUseIBL",
        indirect_radius_boost: f32 = "indirectRadiusBoost",
        light_boost: f32 = "lightBoost",
        ao_max_distance: f32 = "aoMaxDistance",
        ao_power: f32 = "aoPower",
        sky_is_sphere: bool = "skyIsSphere",
    }
}

typed_table! {
    /// `ZoneAttribs`: what a zone adds to the level's settings.
    pub struct ZoneLighting {
        /// The three ambient colours (top, middle, bottom of a hemisphere gradient, 0..255, alpha last).
        ambient_light: [f32; 4] = "ambientLight",
        ambient_light_mid: [f32; 4] = "ambientLightMid",
        ambient_light_low: [f32; 4] = "ambientLightLow",
        light_mapped_ambient_light_mid: [f32; 4] = "lightMappedAmbientLightMid",
        zone_uses_ambient: bool = "zoneUsesAmbient",
        rim_light: [f32; 4] = "rimLight",
        use_light_map: bool = "useLightMap",
        /// `LM_1024x1024` and the like.
        light_map_size: String = "lightMapSize",
        override_light_map_tint: bool = "overrideLightMapTint",
        light_map_tint: [f32; 4] = "lightMapTint",
        override_zone_map_tint: bool = "overrideZoneMapTint",
        zone_map_tint: [f32; 4] = "zoneMapTint",
        light_probe_radius: f32 = "lightProbeRadius",
        /// The zone's reflection probe (type 183, see [`crate::probe`]).
        reflection_map: String = "reflectionMap",
        reflection_sh1: [f32; 4] = "reflectionSH1",
        reflection_map_attenuation: f32 = "reflectionMapAttenuation",
        override_level_fog: bool = "overrideLevelFog",
        fog_color: [f32; 4] = "fogColor",
        distance_fog_density: f32 = "distanceFogDensity",
        height_fog_density: f32 = "heightFogDensity",
        height_fog_falloff: f32 = "heightFogFalloff",
        height_fog_y: f32 = "heightFogY",
        override_horizon_fog: bool = "overrideHorizonFog",
        color_grading: String = "colorGrading",
        color_correct_curve_lift: [f32; 4] = "colorCorrectCurveLift",
        override_curve_lift: bool = "overrideCurveLift",
        override_exposure_settings: bool = "overrideExposureSettings",
        override_ev_curve_settings: bool = "overrideEVCurveSettings",
        dynamic_exposure: [f32; 2] = "dynamicExposure",
        target_exposure: f32 = "targetExposure",
        exposure_converge_speed: f32 = "exposureConvergeSpeed",
        exposure_curve_min: ExposurePoint = "exposureCurveMin",
        exposure_curve_med: ExposurePoint = "exposureCurveMed",
        exposure_curve_max: ExposurePoint = "exposureCurveMax",
        exposure_curve_adjust_clamp: [f32; 2] = "exposureCurveAdjustClamp",
        ldr_exposure_boost: f32 = "ldrExposureBoost",
        directional_light_attenuation: f32 = "directionalLightAttenuation",
        sun_shafts_attenuation: f32 = "sunShaftsAttenuation",
        far_plane_override: f32 = "farPlaneOverride",
    }
}

/// The final-pass material named by `postFXMaterial`: its shader, the
/// compile-time switches (`SIMPLE_VIGNETTE=1`, `BLOOM_ENABLED=0`, ...) and the
/// pixel-shader constants (`PS:Sharpen`, ...), all as text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PostFxMaterial {
    pub path: String,
    /// `Shader0`, e.g. `/EE/Shaders/PostFX/FinalScenePass.hlsl`.
    pub shader: Option<String>,
    pub switches: BTreeMap<String, String>,
    pub constants: BTreeMap<String, String>,
}

impl PostFxMaterial {
    pub fn from_properties(path: &str, props: &BTreeMap<String, (String, String)>) -> Self {
        let mut out = PostFxMaterial {
            path: path.to_string(),
            ..Default::default()
        };
        for (k, (v, _)) in props {
            if k == "Shader0" {
                out.shader = Some(v.clone());
            } else if k.starts_with("PS:") || k.starts_with("VS:") {
                out.constants.insert(k.clone(), v.clone());
            } else if !k.starts_with("TX:") && !k.starts_with("Shader") && !v.contains('{') {
                out.switches.insert(k.clone(), v.clone());
            }
        }
        out
    }

    pub fn to_json(&self) -> Json {
        json!({"path": self.path, "shader": self.shader, "switches": self.switches, "constants": self.constants})
    }
}

/// Everything a level object contributes to the look of a scene.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvironmentLighting {
    pub post_process: Option<PostProcess>,
    pub post_process_alt: Option<PostProcess>,
    pub lighting_params: Option<LightingParams>,
    pub zone: Option<ZoneLighting>,
    pub post_fx: Option<PostFxMaterial>,
}

impl EnvironmentLighting {
    pub fn to_json(&self) -> Json {
        json!({
            "postProcess": self.post_process.as_ref().map(PostProcess::to_json),
            "postProcessAlt": self.post_process_alt.as_ref().map(PostProcess::to_json),
            "lightingParams": self.lighting_params.as_ref().map(LightingParams::to_json),
            "zone": self.zone.as_ref().map(ZoneLighting::to_json),
            "postFxMaterial": self.post_fx.as_ref().map(PostFxMaterial::to_json),
        })
    }
}

/// Lays `layers` (a type chain root first, then the object's own table) over
/// each other field by field.
pub fn overlay_tables<'a>(layers: impl IntoIterator<Item = &'a Value>) -> Option<PostProcess> {
    let mut any = false;
    let mut out = PostProcess::default();
    for v in layers {
        any = true;
        out.overlay(&PostProcess::from_value(v));
    }
    any.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ee::parse_value;

    fn pp(text: &str) -> PostProcess {
        PostProcess::from_value(&parse_value(text).unwrap())
    }

    #[test]
    fn reads_typed_fields() {
        let p = pp("{targetExposure=0.07\nadaptationRateUp=3\ndynamicExposure={0.25,16}\ntoneMapType=TM_ACES_REF\nuseExposureEVCurve=1\nexposureCurveMin={sceneEV=-5\nadjustmentEV=-3.5}\nexposureCurveMed={adjustmentEV=-1}\nfogColor={102,110,126,255}\nlightMapBoost=1.5\npostFXMaterial=/Lotus/Types/ACESPost\nviewShake={ShakeSpeed=1}\n}");
        assert_eq!(p.target_exposure, Some(0.07));
        assert_eq!(p.adaptation_rate_up, Some(3.0));
        assert_eq!(p.dynamic_exposure, Some([0.25, 16.0]));
        assert_eq!(p.tone_map_type, Some(ToneMapType::AcesRef));
        assert_eq!(p.use_exposure_ev_curve, Some(true));
        assert_eq!(
            p.exposure_curve_min,
            Some(ExposurePoint {
                scene_ev: Some(-5.0),
                adjustment_ev: Some(-3.5)
            })
        );
        assert_eq!(p.exposure_curve_med.unwrap().scene_ev, None);
        assert_eq!(p.fog_color, Some([102.0, 110.0, 126.0, 255.0]));
        assert_eq!(p.light_map_boost, Some(1.5));
        assert_eq!(p.post_fx_material.as_deref(), Some("/Lotus/Types/ACESPost"));
        // Unknown keys survive as text
        assert_eq!(
            p.other.get("viewShake").map(String::as_str),
            Some("{ShakeSpeed=1}")
        );
        assert_eq!(p.set_count(), 10);
        let j = p.to_json();
        assert_eq!(j["toneMapType"], "TM_ACES_REF");
        assert_eq!(j["dynamicExposure"], json!([0.25, 16.0]));
        assert_eq!(j["targetExposure"], json!(0.07));
    }

    #[test]
    fn overlays_field_by_field() {
        let base = pp("{targetExposure=0.07\nadaptationRateUp=3\n}");
        let level = pp("{fogColor={1,2,3,4}\ntargetExposure=0.1\n}");
        let merged = overlay_tables([
            &parse_value("{targetExposure=0.07\nadaptationRateUp=3\n}").unwrap(),
            &parse_value("{fogColor={1,2,3,4}\ntargetExposure=0.1\n}").unwrap(),
        ])
        .unwrap();
        assert_eq!(merged.target_exposure, Some(0.1));
        assert_eq!(merged.adaptation_rate_up, base.adaptation_rate_up);
        assert_eq!(merged.fog_color, level.fog_color);
        assert!(overlay_tables([]).is_none());
    }

    #[test]
    fn keys_ignore_the_first_letter_case() {
        let z = ZoneLighting::from_value(
            &parse_value("{AmbientLight={50,50,50,0}\nzoneUsesAmbient=0\nReflectionSH1={0.03,0,0,0}\nExposureCurveMax={sceneEV=2\nadjustmentEV=2}\nBackDropId=X\n}").unwrap(),
        );
        assert_eq!(z.ambient_light, Some([50.0, 50.0, 50.0, 0.0]));
        assert_eq!(z.zone_uses_ambient, Some(false));
        assert_eq!(z.reflection_sh1, Some([0.03, 0.0, 0.0, 0.0]));
        assert_eq!(z.exposure_curve_max.unwrap().adjustment_ev, Some(2.0));
        assert_eq!(z.other.get("BackDropId").map(String::as_str), Some("X"));
    }

    #[test]
    fn unknown_tone_map_and_bad_values() {
        let p = pp("{toneMapType=TM_NEW\ntargetExposure=abc\n}");
        assert_eq!(p.tone_map_type, Some(ToneMapType::Other("TM_NEW".into())));
        assert_eq!(p.target_exposure, None);
        // A value that does not parse stays visible
        assert_eq!(
            p.other.get("targetExposure").map(String::as_str),
            Some("abc")
        );
    }

    #[test]
    fn color_grading_data_regions() {
        let p = pp("{colorGradingData={Global={Saturation={1,1,1,0.97},Contrast={1,1,1,1.15}},Highlights={Enable=1}}}");
        let g = p.color_grading_data.unwrap();
        assert_eq!(g.regions["Global"]["Saturation"], vec![1.0, 1.0, 1.0, 0.97]);
        assert_eq!(g.regions["Highlights"]["Enable"], vec![1.0]);
    }

    #[test]
    fn post_fx_material_split() {
        let mut props = BTreeMap::new();
        for (k, v) in [
            ("Shader0", "/EE/Shaders/PostFX/FinalScenePass.hlsl"),
            ("SIMPLE_VIGNETTE", "1"),
            ("PS:Sharpen", "{2.1,-0.275}"),
            ("TX:Frame", ""),
        ] {
            props.insert(k.to_string(), (v.to_string(), "/x".to_string()));
        }
        let m = PostFxMaterial::from_properties("/Lotus/Types/ACESPost", &props);
        assert_eq!(
            m.shader.as_deref(),
            Some("/EE/Shaders/PostFX/FinalScenePass.hlsl")
        );
        assert_eq!(m.switches["SIMPLE_VIGNETTE"], "1");
        assert!(m.constants.contains_key("PS:Sharpen") && !m.switches.contains_key("TX:Frame"));
    }
}
