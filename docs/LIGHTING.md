# Lighting and post-processing

Reflection probes, post-process settings, lightmaps and default colours. Part of the [wf-unpack README](../README.md).

The shaders are in the game files too: `B.ShaderPermutationDx11` (and
`B.ShaderDx11`) hold, per shader family, many permutations. Each entry is a
header, a 512-entry `u16` table (a runtime option key to a frame index; 65,536
entries for `FinalScenePass`), and then frames: a LEB128 decompressed size and a
magic-less zstd frame with one compressed block (first byte `00`, a window
descriptor, a 3-byte block header). Put `28 B5 2F FD` in front and decompress
with a dictionary from `B.ShaderDictionaryDx11` (three zstd dictionaries one
after the other: vertex 512 KiB, pixel 4 MiB, compute 128 KiB). The result is
a DXBC container without its leading `DXBC` (only `ISGN`, `OSGN` and `SHEX`),
whose checksum is D3D's modified MD5 over bytes 20 on (it matches on all 6,085
frames tried). The formulas below were read from that bytecode, and the
lightmap one was also run in an emulator on random inputs (agreement to 1e-7).
A constant record in `H.ShaderDx11` is the data block followed by the name it
belongs to; its slot byte is the `cb0` register, which is how constants were
named. This reader does not decode shaders itself; `src/pipeline.rs` has the
resulting formulas.

**Lightmaps** (`src/lightmap.rs`). Per object: `LightMap={X_clr.png,X_dir.png}`,
`LightMapCoords={ox,oy,sx,sy}` (rectangle of the page for mesh UV set 1),
`LightMapNormalize={r,g,b,w}`, `LightMapNormalize1`, `IndexedLightMaps`. `_clr`
is BC3: RGB chroma (largest channel about 1.0), A a luma stored squared, per
object normalised to 1.0. `_dir` is BC5 holding two pages side by side (half
the resolution of `_clr`; its top mip is not shipped). The decode:

```
a = uv1 * coords.zw + coords.xy            (cubic B-spline filtering, clamped, no v flip)
chroma = clr.rgb * normalize.rgb           luma = clr.a^2 * normalize.w
d = (2 dL.x - 1, 2 dL.y - 1, 2 dR.x - 1)   gain = dR.y^2 * S         (L, R: the two dir pages)
light = (luma + max(dot(n, gain * M d), 0)) * chroma
```

Exact and applied: everything but the directional term (`luma * chroma`, the
ambient part, is what a texture can hold). Not identified: `S`, the scale of the
directional term (it is not `LightMapNormalize1.w`: the shader constant table
puts that elsewhere), and `M`, which sits in a block named `ProcRotation` (the
procedural-tile rotation, the identity in a fixed level). The raw `d` and
`dR.y^2` are exposed, the pages are exported unchanged as `lm<N>_clr.png` /
`lm<N>_dir.png`, and the node extras carry `coords`, `normalize`, `normalize1`,
the page number and the indexed light maps. `lm_idx<N>.png` are the indexed
light map pages: RGBA, one light per channel, per entry of `IndexedLightMaps`
(the lights are named `Layer3/DojoLight29`: scope, type, instance; the
description resolves them to the level's lights). The fill shaders use those
channels as masks: `mask * colour * max(1 - (w d^2)^2, 0)^2 * (1 + k (1/(1+d^2) -
1)) * N.L`, with `w = 1 / radius^2` (an inference). How `Brightness`, `Lumens`,
`Color`, `Temperature` and `PhysicalFalloff` become the colour and `k` is
CPU-side code and is not in the data, so the renders label that mapping as an
assumption (colour = linear `Color` times `Brightness`, `k` = 1).

**Reflection probes** (`src/probe.rs`, `src/bc6h.rs`). `ZoneAttribs.ReflectionMap`
names a type 183 texture in `H.LightMap`: BC6H, six faces per mip level
(smallest level first; a level is its six faces), Direct3D face order
(+X, -X, +Y, -Y, +Z, -Z) and orientation, which was confirmed by comparing the
texels on both sides of all 12 cube edges. `texture2ddecoder` clamps BC6H to
8 bits, so `src/bc6h.rs` is its decoder with float output. (That crate also
treats the half-way endpoint, 512 for 10 bits, as the saturated one instead of
the all-ones endpoint, and mixes sign-magnitude into the signed case; an
earlier version of this decoder had both, and 72,344 texels of the game's
probes came out at 65,504 where the data holds ordinary values. The probes
have no texel at the half-float maximum.) `wfu probe` writes
an equirectangular `.hdr` (RLE Radiance) and six face files; `level -o` writes
`probe<N>_equirect.hdr`. The dojo's probe holds values up to 1.74 (the
emissive strips); the brightest texel of any probe is 10,944.

**Post-processing and zone settings** (`src/postprocess.rs`). `LevelInfo.PostProcessInfo`
(about 100 fields: exposure, tone map, colour grading, bloom, fog, `lightMapTint`,
`lightMapBoost`, ...), `PostProcessInfoAlt`, `LightingParams` and
`ZoneAttribs` become typed structs; a type chain and the object's own text are
overlaid field by field. Anything unknown stays as text. Field names come from
the executable's property table; every `PostProcessInfo` in Packages.bin and
in the levels reads. Fields neither the type nor the level sets have defaults
inside the executable, not in the data. `wfu postprocess <level>` prints them
and the level export puts them in the scene extras. The dojo: fog and
`postFXMaterial=/Lotus/Types/ACESPost` from the level, `targetExposure=0.07` and
`adaptationRateUp=3` from `LevelInfo`; the zone sets `UseLightMap=1`,
`ZoneUsesAmbient=0` and the probe.

What the shaders do with those settings (`src/pipeline.rs`): exposure is
`clamp(target / L, min, max)` with `L` the centre-weighted geometric mean of the
unexposed scene's luma (or a piecewise curve in log2 luma when the curve
points are set); the final pass multiplies by it and by a vignette and looks the
result up in a 32^3 colour volume that `ColorLUT_cs` builds from the 3-zone
grade, the tone curve chosen by `toneMapType` (`LINEAR`, `UNCHARTED`,
`ACES_REF`, `ACES_OPT`, `FILMIC`, `CUSTOM`, `ACES_1000`) and the sRGB encoding.
`ACES_OPT` is implemented; the others are described in the notes but not
ported. The tone map a level gets when it sets none is a default in the
executable.

**Default colours** (`src/palette.rs`, `wfu palette`, `model --suit`). A suit
names its body `Mesh`, `DefaultEnergyColor` / `DefaultEnergyColor1` (0..255) and a
`DefaultCustomization` (helmet, ...). Nothing is picked on a new account, so the
default look is each material's own `PS:TintColor0..3` and
`PS:EmissiveTintColorHi/Lo`; the energy colours are what the energy slots start
as. Excalibur: tints 0.35, 0.085, 0.065 grey and (0.26, 0.0075, 0.0037) red,
energy `#4db0e0` / `#3692be`; body and helmet agree. `--suit` puts the energy
colours on the emissive parts of multi-tint materials (that this is what the
engine does is inferred).

`check-lighting` over the build of 2026-10-22 (36 s, peak 526 MB): 1,952 of 4,601
levels have lightmaps (1,376,939 lightmapped instances) naming 28,488 page
textures, whose headers all fit their `B.`/`F.` sizes (BC1, BC3, BC5; 4 are
not in `H.LightMap`); 8,688 reflection probes decode (128 or 256 pixel faces) and 22 are
type 177 cube textures, not read; 4,830 `PostProcessInfo` tables read with
50,452 typed fields (`toneMapType` is `TM_ACES_REF` in 754, `TM_UNCHARTED` in 15,
`TM_CUSTOM` in 2, `TM_ACES_OPT` in 1; the rest set none). Of 248,724
`IndexedLightMaps` entries only 8,341 have all their lights among the level's
own lights (the others name lights of prefabs or sub-levels, which a level
description does not contain).

**Sky, sun, ambient light, fog and global illumination** (`src/atmosphere.rs`,
`wfu atmosphere <level> [--json]`, `check-atmosphere`). `Atmosphere::of(&description)` collects what a
level says about them, with the unit and the source of every value and a list of what it leaves out;
the glTF export puts the same in the scene `extras` (`atmosphere`, `sky`) and the bridge provider
returns it beside the scene (`WarframeScene::atmosphere`, see [BRIDGE.md](BRIDGE.md)). What the data
holds, over the 4,601 levels of the build of 2026-10-22 (`check-atmosphere`, 9 s, 0.5 GB):

| | where | what the data defines | what it does not |
|---|---|---|---|
| lighting capture | `LightingParams.globalCubeMap`, a global cube map the baker lights with (not shown to be the drawn sky, see below): 543 levels name one, 10 distinct pictures, 4 are in the install (`H.LightMap`, 128 px BC6H cube maps, linear radiance, the largest texel 0.30 to 1.125), 269 of the 543 levels get one | the picture, exactly, and its weight `giEnvironmentIntensity`; its orientation was checked against the geometry that was captured (below) | 6 pictures are editor files or not shipped (`ZarimanGlobalcubemap`, `1999WinterIBL`, `GrineerForestDayGlobalR`, `SentientDevourer/0_pcube0.png` ...), `iblImageFile` (114 uses name a picture) and `iblCubeMap` (4) are never in the install; a time-of-day sky's `skyLight` is an editor `_sh.hdr` path |
| sky light colour | `LightingParams.skyLightColor`, 0..255 alpha last, beside the `bakingOptions:*` | the colour: 4,647 of 4,830 `LevelInfo` objects carry only the engine type's black, none sets it in its own table, 183 get one from a content type (`PlayerShipLevelInfo` grey 128, `MasteryTestLevelInfo` 200, grey 69/84/86) | that it is the sky the game draws |
| sun direction and colour | a `Light` with `Directional=1`: `DirectionalLightDir`, `Color`, `Brightness`, `Lumens` (566 lights, 434 levels); `PostProcessInfo.sunPosition` and `sunColor` (635 and 588 objects) | the vector, the colour, `Brightness` and `Lumens` | the strength in lux or radiance (code), whether the vector points to or from the sun (read as toward, below) |
| a sun over the day | `DynamicSky` (44 objects, 28 levels): `TimeOfDayKeys` (hour, `sunColor`, `sunBrightness`, `fogColor`, `skyLight`, `exposure` ...), `IlluminationKeys`, `Latitude`, `SunDeclination`, `TimeRate`, `ExplicitSunDirection` (4 sky objects use it) | the keyframes and the sky material's constants (`MaterialParams`, the names `A` to `I` and `Z`, 3 or 4 numbers each), as stored (`level::DynamicSky`) | the sun's position at an hour, how the keys blend, and the sky's radiance formula the constants go into (script and shader code; the formula's shader is `/EE/Materials/Defaults/DynamicSkyboxMaterial`); Cetus's explicit direction points the other way from its directional light, with `FlipSunDirection=1`; the sky material's `SunDir` constant is a snapshot of a value that code overwrites (7 distinct vectors over the 44 skies, one of them in 22 skies, another in 13, at different hours of day) |
| ambient light | `ZoneAttribs.AmbientLight`, `AmbientLightMid`, `AmbientLightLow`, `ZoneUsesAmbient` (1,945 zones in 1,128 levels carry colours; 26 of 11,664 zones set `ZoneUsesAmbient=1`, 14 with colours) | three colours per zone, and whether the zone uses them | the blend between the three (shader code); the other 11,638 zones do not use them |
| fog | `PostProcessInfo.fogColor`, `distanceFogDensity`, `heightFog*`, `linearFogDensity`, `horizonFog`, `extinction`, `fogTexture` (2,403 levels), and a zone's own (`OverrideLevelFog`, 6,756 zones) | the settings, as stored | the unit of the densities and the attenuation law (shader code) |
| GI volumes | `GIVolume` objects: 1,350 in 857 levels (`EnableGIVolumes`), a box of probes (`Dimensions`, `CellSize`), 5,794 textures named (type 175 volume textures, all in `H.LightMap`), an indirection texture and its size in bricks, `FallbackBand0`, `Band1NormalizationScale/Offset` | placement, size, spacing and every parameter | the voxels: packed spherical-harmonic bands whose reconstruction is shader code |
| probes | `LevelInfo.LightProbeData` (`.lpd`, 2,032) and `EnvProbesData` (`.epd`, 3,585), all in `H.Misc`; `GlobalProbePosition`; a zone's `ReflectionSH1` (four numbers) | the file names and the position | the payload of the files |

*The sun's direction.* `DirectionalLightDir` is read as pointing **toward** the sun: its y is positive in
557 of the 566 directional lights; `PostProcessInfo.sunPosition` is a position of the sun in the sky
(RailjackInteriorV2's is (0, 28.7, 95.8), 100 m out, like `DynamicSky.SkyboxSunDistance=100`), and
in the 348 levels that set both, the light's direction and that position agree within 8 degrees in
148; in the other 200 the two stored suns disagree (Cetus's by 145 degrees) and both are listed, since the
data does not say which the game draws. The data does not say "toward" in words. The length of the vector
differs from 1 by more than 0.01 in 171 lights (a direction, not a place).

*The sky picture's orientation.* The cube is in the game's space (see `probe.rs`). Checked on the one
case where the geometry that was captured is in the install: `/Lotus/Levels/Backdrops/UranusRegion.level`
places the planet, its rings and the nebulae around the zone that `1_cross.hdr` was baked at. Rendering
those meshes from that point in a 360 degree view and the sky picture side by side, in the bridge's
convention, put the planet at the same azimuth and height (at the picture's edge, about 20 degrees up in both), and
the rings, which extend to one side of the planet only, run the same way in both (to u = 0.36 in the
picture and 0.39 in the render; compared by eye, the renderer's own mapping having been checked on five
coloured spheres first): a mirrored picture would put them on the other side.

*What the global cube map and the sky light colour are.* Lighting inputs, not shown to be the sky the game
draws. `LightingParams` is the baking table (it holds the `bakingOptions:*`), and `LightingParams` pairs
the cube map with `giEnvironmentIntensity`, the weight the baker gives it as a light (on `LevelInfo`
objects: 0.2 with the deep-space nebula in 130 and 1.0 in 26, 0.2 with the Uranus picture in 16, 0 with
Duviri's `LandscapeC` in 50), and `skyIsSphere=1` in 535 levels. Duviri's `LandscapeC` is a stock mountain
panorama whose brightest region (azimuth -101 degrees, 32 degrees up in the bridge's convention) is 121
degrees away from the sun of the level that names it (TownA's directional light: azimuth +20, 32 up), and
the baker gave it no weight there. The Orbiter's `LevelInfo` type gives a grey sky light colour (128) and
its windows show a nebula. The contract's `Environment` is "the sky seen behind everything", which a host
turns into a skybox and scene lighting, so the bridge scene leaves it empty by default and carries both in
`Atmosphere::sky` (with `giEnvironmentIntensity`). The float picture is `Warframe::probe(path)` and
`Probe::to_bridge_equirect`, and the glTF export writes it as `sky_equirect.hdr`.
`SceneOptions::lighting_environment` (off by default; `--lighting-environment` on `bridge-scene` and
`check-bridge-scenes`) puts them in `Scene::environment` on request, documented as the level's lighting
capture: the picture as 8-bit sRGB in the bridge's equirectangular convention, four times the face edge wide,
`intensity` the cube's largest texel, which is the scale of the coding and not a brightness of anything;
else the colour (a content type's or the level's, never the engine's black); else nothing. The 8-bit
coding is lossy at the dark end: over all texels the mean relative error is 1.0 % (DeepSpaceGlobal),
0.9 % (LandscapeC), 8.7 % (DeepSpaceRedGlobal) and 1.7 % (Uranus), and 2.4 % of the non-zero channel values
of DeepSpaceRedGlobal come out 0 (the real-data test measures this). A cube face over 512 pixels is refused
before anything is allocated (the install's are 128 and 256).

**Not done:** the directional term's scale `S`, the voxels of the GI volumes and the payload of `.lpd` /
`.epd` files, `FillLightingBuffer`'s shader (the reconstruction of probe light, the sky and directional
light in it, the three-colour ambient blend), the strength of a directional light in lux, a time-of-day
sun's position and key blending, the fog law, the lights' CPU-side colour mapping, `TM_ACES_REF` and the
other tone maps, bloom, the lightmap `LightMapTint` / `lightMapBoost` scaling (applied outside the pixel
shaders), and the clustered (non-indexed) lights. Because of the first of these and the probe
reconstruction, a level cannot be lit as in the game from the files alone: rendered with everything that
is decoded (`tools/render`), the dojo's lightmaps and indexed lights leave most surfaces dark.

`tools/render` is a headless three.js/Playwright renderer for the glTF files
(`npm install` there, then `node shot.cjs out.png level.gltf "pos=..&tgt=..&game=1"`);
`game=1` lights level meshes with the lightmaps by the formula above, `lights=assumed`
adds the indexed lights, `post=game` exposes and tone maps as the game's final
pass does, `sky=1` draws the level's global cube map (its lighting capture, not necessarily the sky) behind it (with a disc where its first
directional light is, the direction only) and `skylight=1` lights it from that picture, `pano=1`
renders a 360 degree view from a point and `unlit=1` draws base colours alone. Parameters are documented at the top of `view.html`.
