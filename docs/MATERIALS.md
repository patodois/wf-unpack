# Packages.bin and materials

How a material is resolved through Packages.bin and baked to glTF PBR textures. Part of the [wf-unpack README](../README.md).

`src/packages.rs` reads `/Packages.bin` (version 46): every asset's type,
parent and property text. Its module docs give the layout. Since version 45
the sections are separated by junk bytes whose lengths come from a 64-bit
LCG (Knuth's MMIX constants) seeded by a u32 in the header, and entities are
grouped by folder. LotusLib's `dev` branch documents this; no code was
copied. The texts are zstd frames without magic numbers, sharing one
dictionary. `check-packages` decodes all 467,795 entities with no failures:
203,948 texts, 221.8 MB, in about half a second. The header counts 467,796;
one path appears twice.

`src/material.rs` resolves a model's materials. A model names them in its
attribute text (`Materials={...}`), relative to its folder. A material
inherits every property of its parent chain, usually up to
`/EE/Materials/Defaults/PBR`. Its own properties are in Packages.bin or in
the attribute text of its `H.Misc` entry; both are read. `src/pbr.rs` turns
the result into glTF metallic-roughness textures:

| Game input | glTF | Status |
|---|---|---|
| `TX:DiffuseMap` (when `DIFFUSE_MAP=1`) | base colour | known |
| `TX:PackMap` R | metalness; with `SPECULARMAP_IS_METALMAP=0` a specular level instead | metal mask guessed (binary, follows gold accents); specular reading guessed from the packing note below |
| `TX:PackMap` G | base colour (grey) when there is no diffuse map | measured: 0.99 correlation with a diffuse map's luminance |
| `TX:PackMap` B | roughness | guessed: lower on metal, ~0.9 on painted wood |
| `TX:PackMap` A with `DETAILING=TINT_MASK_PACK_MAP` | tint mask | measured: a 2x2 grid of half-size masks, R top-left, G top-right, B bottom-left, A bottom-right |
| `TX:RoughnessMap`, `TX:SpecularMap` | roughness, metalness or specular level | used when enabled |
| `TX:TintMask` RGBA (or the pack map's alpha quadrants) with `PS:TintColor0..3` | multiplies the base colour | decoded from the fill shaders: start at 0.5, lerp towards each tint by its mask (0, 1, 2, 3 in turn), double, keep the length at 0.04 or more; no gamma anywhere |
| `TX:NormalMap` | normal | measured: green points down (DirectX), inverted for glTF; blue rebuilt |
| `TX:EmissiveMap` (red channel when `EMISSIVE_COMPRESSION=EC_COMPONENT`) | emissive | decoded from the fill shaders: the single-channel map squared, times `EmissiveTintColor` (or `lerp(Lo, Hi, map^2)` for multi-tint), baked into the texture. An RGB map (`EC_RGB`) is used as stored, unsquared: the decoded shaders do not say whether the game squares it, and its `EmissiveMap` texture type has no `ColorSpace` entry to settle it; `PS:EmissiveMapAtten` is `KHR_materials_emissive_strength` |

Each pack map's own `H.Texture` attribute text names the source images it
was built from (`Diffuse=`, `Roughness=`, `Specular=`, and for
`TINT_MASK_PACK_MAP` materials also `TintMask=`). For 519 pack maps the named
tint mask ships too. I compared 26 of them: in the 19 with a non-trivial mask,
the four alpha quadrants match the tint mask's R, G, B and A channels for
93-100 % of the pixels after thresholding. (Most of the other seven are blank
masks; one was built from a different version of the mask.)

With `SPECULARMAP_IS_METALMAP=0` (15,340 slots, e.g. Mag's materials and
the Prime Braton that inherits from them), R reads as specular level: mostly
dark with bright metal parts, while G holds the diffuse colour, dark on
metals. The export turns the specular level into metalness as
`(linear(s) - 0.04) / 0.96`, and blends the base colour towards the tinted
specular colour on metal (the materials set `TINT_MASK_AFFECTS_SPECULAR=1`).
This conversion is an approximation.

Tint colours are the material's own `PS:TintColor0..3` (its default player
colours; every tinted slot sets them). **They are linear values used as they
are**: the fill shaders' bytecode applies no gamma to the tint constants, to
`PS:DiffuseColor` or to the emissive colours. Earlier versions read them as
sRGB, which made Excalibur near-black (his primary, 0.35, came out as 0.10).
His store icon (`/Lotus/Interface/Icons/StoreIcons/Warframes/Excalibur.png`,
a render of the default look) shows a light grey primary, a dark secondary and
red accents; Mag's shows steel blue, bright aqua and copper, which are his
`TintColor0`, `1` and `2` read as linear. The tint is `2 * lerp-chain`, so a tint
colour of 0.5 means "unchanged". The tint alpha (0.5) is never read by the
fill shaders. The green-down normal
convention comes from a curl test: the normal maps are integrable only with
green pointing down. The tangent's w is set so glTF's bitangent points to the
top of the texture. It is checked against the UV derivatives of every
triangle (99.9-100 % agree). Versions before the skinned-model commit had
this sign backwards.

`check-materials` over the 49,410 material slots of the 33,689 static
models:

- Base colour: 21,720 from a diffuse texture, 11,472 from a pack map's grey
  channel, 16,040 constant colour.
- Tint: 11,744 from pack-map alpha quadrants, 1,386 from a separate tint
  mask, 36,102 none.
- Workflow: 33,892 metalness, 15,340 specular.
- 22,948 get a metallic-roughness texture; 30,883 a normal map; 8,671 an
  emissive map.
- Failures: 178 materials missing from Packages.bin, and 4,626 models with no
  `Materials=` list (mostly particle-emitter meshes).

Not reproduced (counted per slot by `check-materials`):

- The extra blend of `TINT_MASK_PACK_MAP_BLEND` (927; tinted like
  `TINT_MASK_PACK_MAP` otherwise) and `TINT_MASK_PACKMAP` (170).
- Vertex-colour tinting and layer blending (`VERTEX_*_TINT_STACK`, `LAYERS=`):
  only the first layer is used.
- `TINT_COLOR`/`TINT_CHANNELS` and other rarer `DETAILING` modes.
- The newer character-detail materials (`/EE/Materials/Defaults/CharacterPBRDetail`,
  e.g. the 2025 Skana and Lato): they use `TX:MacroPackMap`, `TX:DetailMask`
  and per-layer normal maps, and come out as constants for now.
- Opacity maps, detail maps, ambient-occlusion maps and vertex AO; the wet
  albedo squaring, the hue fix-up of multi-tint emission and the `CLOAK` effect.
- Weapon skins and other material overrides outside the model's own list.
