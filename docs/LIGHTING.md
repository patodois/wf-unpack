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

**Not done:** the directional term's scale `S`, light probes (`.lpd`),
`FillLightingBuffer` (global illumination volumes, sky and directional light,
`AmbientLight*`), the lights' CPU-side colour mapping, `TM_ACES_REF` and the
other tone maps, bloom, the lightmap `LightMapTint` / `lightMapBoost` scaling
(applied outside the pixel shaders), and the clustered (non-indexed) lights.
Because of the first four, a level cannot be lit as in the game from the
files alone: rendered with everything that is decoded (`tools/render`), the
dojo's lightmaps and indexed lights leave most surfaces dark.

`tools/render` is a headless three.js/Playwright renderer for the glTF files
(`npm install` there, then `node shot.cjs out.png level.gltf "pos=..&tgt=..&game=1"`);
`game=1` lights level meshes with the lightmaps by the formula above, `lights=assumed`
adds the indexed lights, `post=game` exposes and tone maps as the game's final
pass does. Parameters are documented at the top of `view.html`.
