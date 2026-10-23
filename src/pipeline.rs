//! What the game's shaders do with the settings of [`crate::postprocess`]
//! and with light values, as formulas. Everything here was read from the
//! game's own DXBC (decompressed from `B.ShaderPermutationDx11` /
//! `B.ShaderDx11`) and is listed with what was and was not verified.
//!
//! # Exposure and the final pass
//!
//! `FinalScenePass.hlsl` (material `/Lotus/Types/ACESPost`) computes the
//! exposure in its vertex shader from the 1x1 adapted luminance `L`:
//!
//! ```text
//! simple mode:  E = clamp(target / L, min, max)            ExposureParams = (min, max, target, bias)
//! curve mode (ExposureCurveMinMed != 0):
//!   x = log2(max(L, 1e-6)) + 3
//!   v = x < x1 ? y1 + (x1 - x) / (x1 - x0) * (y0 - y1) : y1 + (x - x1) / (x2 - x1) * (y2 - y1)
//!   E = 2^-(clamp(v, cmin, cmax) + bias)
//! ```
//!
//! with `dynamicExposure = {min, max}`, `targetExposure = target`, and the
//! curve points of `exposureCurveMin/Med/Max` (`sceneEV`, `adjustmentEV`)
//! loaded into the constants by the engine (the CPU side is not visible; the
//! mapping of the data fields to the constants is the obvious one and
//! unverified). The adapted luminance (`AutoExposure.hlsl`) converges to the
//! Gaussian-weighted (5 x 5, centre-weighted) geometric mean of the
//! unexposed scene's Rec709 luma, so in the steady state `E = clamp(target /
//! L, min, max)` brings that mean to `target`; the adaptation rates only set
//! how fast. The pixel shader then multiplies the scene colour by `E` and by
//! the vignette (`SIMPLE_VIGNETTE`), looks the result up in a 32^3 colour
//! volume built from the tone mapper and the colour grade, and writes it
//! (dither and grain aside).
//!
//! # Tone mapping
//!
//! The colour volume is built by `ColorLUT_cs.hlsl`: the tone curve named by
//! `toneMapType`, applied after the three-zone colour grade, then the sRGB
//! encoding. [`aces_opt`] is the `TM_ACES_OPT` curve exactly as the shader
//! has it; `TM_ACES_REF` (a full reference-transform implementation) and the
//! other types are described in the README but not implemented here. The
//! type a level gets when it sets none is a default compiled into the
//! executable.
//!
//! # Lights
//!
//! Both places that evaluate a point light (the fill shaders for lights with
//! `IndexLightmap=1`, the clustered resolve for the rest) use [`light_falloff`]
//! times the light colour, the surface's `N.L` and the BRDF, with no 1/pi in
//! the diffuse term. `Brightness`, `Lumens`, `Color` and `Temperature` do not
//! appear in any shader: the engine turns them into the linear colour and the
//! `w` and `k` constants on the CPU, and that step is not in the data.

/// `clamp(target / L, min, max)`: the exposure that brings luminance `l` to `target`.
pub fn exposure_simple(l: f32, min: f32, max: f32, target: f32) -> f32 {
    (target / l).clamp(min, max)
}

/// The piecewise-linear exposure curve in log2 luminance. `min_med` is `(x0, y0, x1, y1)`,
/// `max` is `(x2, y2, clamp_min, clamp_max)`.
pub fn exposure_curve(l: f32, min_med: [f32; 4], max: [f32; 4], bias: f32) -> f32 {
    let x = l.max(1e-6).log2() + 3.0;
    let [x0, y0, x1, y1] = min_med;
    let [x2, y2, cmin, cmax] = max;
    let v = if x < x1 {
        y1 + (x1 - x) / (x1 - x0) * (y0 - y1)
    } else {
        y1 + (x - x1) / (x2 - x1) * (y2 - y1)
    };
    2f32.powf(-(v.clamp(cmin, cmax) + bias))
}

/// The vignette factor of `SIMPLE_VIGNETTE` at screen position `uv` (0..1).
pub fn vignette(uv: [f32; 2]) -> f32 {
    let (x, y) = ((uv[0] - 0.5) * 3.5556, (uv[1] - 0.5) * 3.5556);
    let r = 0.25 * (x * x + y * y).sqrt();
    let d = 1.0 + r * r;
    1.0 / (d * d)
}

/// The `TM_ACES_OPT` curve (a Narkowicz fit), per channel.
pub fn aces_opt(c: f32) -> f32 {
    c * (2.51 * c + 0.03) / (c * (2.43 * c + 0.59) + 0.14)
}

/// The sRGB encoding the colour volume ends with.
pub fn srgb_oetf(c: f32) -> f32 {
    let c = c.max(0.0);
    if c < 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// The coordinate the final pass looks the exposed colour up at in the colour
/// volume (before the half-texel scale `0.96875 u + 0.015625`).
// The constants are the shader's, digit for digit, though f32 rounds them
#[allow(clippy::excessive_precision)]
pub fn lut_coordinate(c: f32) -> f32 {
    ((c + 0.002_667_719_27).max(f32::MIN_POSITIVE).log2() / 14.0 + 0.610_726_953).clamp(0.0, 1.0)
}

/// The attenuation of a point light at squared distance `d2`: `w` is the
/// window constant (1 / radius^2, an inference from the shader's use), `k`
/// blends the window alone (0) with the window times `1 / (1 + d^2)` (1).
pub fn light_falloff(d2: f32, w: f32, k: f32) -> f32 {
    if w <= 0.0 {
        return 1.0;
    }
    let t = (1.0 - (w * d2) * (w * d2)).max(0.0);
    t * t * (1.0 + k * (1.0 / (1.0 + d2) - 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_modes() {
        // Target 0.07, limits 0.25..16: a scene at 0.035 needs x2; a bright one hits the lower limit
        assert!((exposure_simple(0.035, 0.25, 16.0, 0.07) - 2.0).abs() < 1e-6);
        assert_eq!(exposure_simple(10.0, 0.25, 16.0, 0.07), 0.25);
        assert_eq!(exposure_simple(1e-6, 0.25, 16.0, 0.07), 16.0);
        // Curve: at x = x1 the exposure is 2^-(y1 + bias)
        let l = 2f32.powf(-3.0 + 1.0);
        let e = exposure_curve(l, [-2.0, -1.0, 1.0, -0.5], [3.0, 1.0, -4.0, 4.0], 0.25);
        assert!((e - 2f32.powf(0.5 - 0.25)).abs() < 1e-5);
    }

    #[test]
    #[allow(clippy::excessive_precision)]
    fn lut_coordinate_matches_the_documented_grey() {
        // The offset colour 0.18 sits at 0.434018, the constant the colour volume builder inverts
        assert!((lut_coordinate(0.18 - 0.002_667_719_27) - 0.434_018).abs() < 1e-4);
        assert_eq!(lut_coordinate(-1.0), 0.0);
    }

    #[test]
    fn tone_curve_and_vignette() {
        assert!(aces_opt(0.0).abs() < 0.01 && (aces_opt(1.0) - 0.804).abs() < 0.01);
        assert!(aces_opt(100.0) > 0.98 && aces_opt(100.0) < 1.05);
        assert!((srgb_oetf(0.18) - 0.4614).abs() < 1e-3);
        assert_eq!(vignette([0.5, 0.5]), 1.0);
        assert!(vignette([0.0, 0.0]) < vignette([0.5, 0.0]));
    }

    #[test]
    fn falloff_window() {
        // Without a window (w = 0) there is no attenuation
        assert_eq!(light_falloff(100.0, 0.0, 1.0), 1.0);
        // At the radius the window closes
        assert_eq!(light_falloff(900.0, 1.0 / 900.0, 0.0), 0.0);
        // k = 1 multiplies in 1 / (1 + d^2)
        let f0 = light_falloff(25.0, 1.0 / 900.0, 0.0);
        let f1 = light_falloff(25.0, 1.0 / 900.0, 1.0);
        assert!((f1 / f0 - 1.0 / 26.0).abs() < 1e-5);
    }
}
