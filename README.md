# wf-unpack

A small Rust tool that reads the game data of Warframe (Digital Extremes'
Evolution Engine) from your own install:

- `.toc` / `.cache` package pairs in `Cache.Windows/` (stored, Oodle and
  LZF blocks)
- textures, rebuilt from their `H.` / `B.` / `F.` parts and written as DDS
  (all mip levels) or PNG
- static models (type 102: weapons, props, level pieces) and skinned models
  (type 281: Warframe bodies, helmets, rigged weapons), exported to glTF 2.0
  with normals, tangents, both UV sets, materials and, for skinned models, the
  skeleton, inverse bind matrices, joints and weights
- levels (`.level`, `.static`): the objects a level places, with their types, transforms and attributes; its
  geometry (types 105/109), PhysX collision shapes, lights, spawn points and portals, exported to glTF with each
  model stored once and instanced by nodes

This project contains no game code or game data. It only reads files from a
copy of the game you already have installed.

## Build

```sh
cargo build --release
```

## Usage

```sh
C=~/Games/Warframe/Cache.Windows

# File list of a package, optionally filtered (substring, or a glob with * / ** / ?)
wf-unpack list $C/H.Texture.toc --filter /Lotus/Interface/Icons

# Extract raw (decompressed) entries
wf-unpack extract $C/B.Misc.toc '/Lotus/Interface/**.swf' -o out/

# Decompress every entry of every package and summarise failures
wf-unpack check $C

# One texture as DDS (all mips) or PNG (top mip), by output extension
wf-unpack texture /Lotus/Interface/Icons/Miscellaneous/Kuva.png -o kuva.dds --game $C
wf-unpack texture /Lotus/Interface/Icons/Miscellaneous/Kuva.png -o kuva.png --game $C

# A static model: print its layout, or export it to glTF (+ .bin + PNGs) with -o.
# Materials are resolved through Packages.bin and baked to glTF PBR textures;
# --texture base=/normal=/emissive=PATH replaces that with textures chosen by hand.
wf-unpack model /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx
wf-unpack model /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx -o out/
wf-unpack check-models --game $C

# Packages.bin: properties of an asset (and its parents), text search, full decode
wf-unpack props /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatinePBR
wf-unpack find-text PrimeGalatine_n.png
wf-unpack check-packages --game $C
wf-unpack check-materials --game $C

# A level: list what it places, or export it (models instanced, materials baked) to glTF;
# --collision also writes <name>_collision.gltf with the PhysX shapes
wf-unpack level /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level
wf-unpack level /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level -o out/ --collision
wf-unpack level /Lotus/Levels/Hub/PlayerShipInteriorOrbiter.level -o out/ --max-texture 512 --include-hidden
wf-unpack check-levels --game $C          # every level and static level parses (streaming)
wf-unpack check-levels --deep --game $C   # + descriptions, level geometry and PhysX of every model they place

# Surveys: asset types in an H. package, texture headers vs. B./F. sizes
wf-unpack kinds $C/H.Misc.toc
wf-unpack textures --game $C
```

`--game` defaults to `~/Games/Warframe/Cache.Windows`. Archives are never
loaded whole (`F.Texture.cache` alone is 22 GB): every entry is read with a
seek, and `check` asks macOS not to keep what it read in the file cache.

The tests in `tests/game_data.rs` run against a real install when
`WF_GAME_DIR` points at a `Cache.Windows` folder:

```sh
WF_GAME_DIR=~/Games/Warframe/Cache.Windows cargo test --release
```

## rewrite-bridge

With the `bridge` feature, `src/bridge.rs` implements
[rewrite-bridge](https://github.com/patodois/rewrite-bridge)'s `Provider` for Warframe. Other game
rewrites can then load Warframe models into the same scene as their own.

```sh
cargo build --release --features bridge
wf-unpack check-bridge --game $C              # load and validate every model
wf-unpack check-bridge --game $C --textures   # the same, baking every material
```

- `list` returns every static (type 102) and skinned (type 281) model, with
  the kind taken from the path: `/Lotus/Weapons` is Weapon,
  `/Lotus/Characters` is Character, anything else is Prop.
- `load` returns a validated `Model`. Geometry goes through `scene::prepare`,
  so it matches the glTF export. Each material slot of a model becomes one
  mesh holding only the vertices its triangles use (LOD 0). Skinned models
  get the bones as nodes under the model node, plus one skin.
- Units are metres already: Excalibur stands 1.84 m tall, feet at y = 0.
- The bridge has one UV set; the second one (`uv1`) is dropped.

## Viewer

`viewer/` is `wf-viewer`, a Bevy app that opens models straight from the
game files, with no export step. It uses the same scene preparation and baked
materials as the glTF export.

```sh
cargo build -p wf-viewer
# One or more model paths; N / P switch between them, drag to orbit, scroll to zoom
target/debug/wf-viewer /Lotus/Weapons/Tenno/SemiAutoRifle/PrimeBraton.fbx \
    /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx --game $C
# Skinned models are live: --pose-demo swings the left upper arm (GAME_L1_ARM1)
target/debug/wf-viewer /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx --pose-demo
# Save a frame and exit (with several paths: out-1.png, out-2.png, ...)
target/debug/wf-viewer /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx --screenshot out.png
```

Options: `--game <Cache.Windows>` (default `~/Games/Warframe/Cache.Windows`),
`--lod N`, `--screenshot <file.png>`, `--pose-demo`. A Bevy debug build
takes about 3 minutes the first time and about 3 GB in `target/`.
Dependencies are built optimised and without debug info, so Bevy runs
smoothly in debug builds and the folder stays small.

## Format notes

The module docs in `src/toc.rs`, `src/cache.rs`, `src/header.rs` and
`src/texture.rs` describe the layouts. Starting points were the
documentation and source of [LotusLib](https://github.com/Puxtril/LotusLib)
and [Warframe-Exporter](https://github.com/Puxtril/Warframe-Exporter) by
Puxtril (including its model readers for the static model layout, its readers
for levels, static levels and level geometry, and the `dev` branch's
Packages.bin reader); no code was copied. Warframe's public
[texturing guide](https://www.warframe.com/steamworkshop/texturing-guide)
describes the tint mask layering. Oodle streams are decoded with the pure-Rust
[oozextract](https://github.com/lvlvllvlvllvlvl/oozextract) and BCn blocks
with [texture2ddecoder](https://crates.io/crates/texture2ddecoder).

Tested against the Steam (Windows) build of 2026-10-22: `check` decodes all
1,090,424 entries of the 95 packages (132.7 GB once decompressed) with no
failures in about five minutes.

What this version learned beyond those sources:

- Texture headers: the first byte is the number of mip levels in `B.`, the
  second the number in `F.`; after width and height come the size of the
  largest `B.` level. Each part stores its levels smallest first. With that,
  69,018 of 69,150 texture headers match their `B.` / `F.` sizes exactly.
- About a third of textures (22,997) ship without their top one to six
  levels: the header says e.g. 512x512 but the largest level present is
  64x64. `texture` writes the largest level present.

Not handled yet: texture arrays (type 166), cubemaps (177, the
`_cmcross.hdr` files), volume textures (`_v`, type 174) and colour lookup tables
(`.cube`, format code 33); models, levels, sounds and the rest of `Misc`.

### Models

`src/model.rs` reads type 102 (static) and type 281 (skinned). `check-models`
reads every model of both types with its `B.`/`F.` parts. A model passes when
its header is read to the last byte, the part sizes are exact, the positions
lie inside the header's bounding box and, for skinned models, every joint
index is in range and the weights sum to 1.

- **Type 102:** 33,132 of 33,689 pass. 548 have a non-zero morph count
  (mostly particle-emitter meshes) and carry extra data that is not
  understood; 9 more have a `B.` part of unexpected size.
- **Type 281:** 9,013 of 9,057 pass; 44 have a `B.` part of unexpected size.

Skinned models (type 281) differ from type 102 as follows. The module docs
in `src/model.rs` and `read_skeleton` give the details.

- The header lists the weighted bone names, then the bone tree (name, u16
  parent, u16 unknown), and per mesh a bone map from the bone indices stored
  in vertices to weighted bones. Its counts come in a different order
  (indices, bones, vertices, morphs).
- `B.` adds, between the PhysX shapes and the vertices, one 3x4 row-major
  inverse bind matrix per weighted bone. Then comes the rest pose of every
  bone: quaternion (x, y, z, w), translation, and 12 unknown bytes. Two u16
  per bone twice and 8 bytes follow.
- Vertices are 32 bytes: the 24 of type 102, then four u8 bone indices and a
  u32 with three 10-bit weights; the fourth weight is what is left of 1.
- The skeleton is inside each model; no separate rig asset is needed. In
  8,544 of the 9,013 models the rest pose matches the inverse bind matrices
  to within 1e-3 (3.6e-7 for Excalibur). In the other 469 the stored rest pose differs
  from the bind pose, so the export places joints at their bind pose
  (inverse of the inverse bind matrix). Weighted bones with no name take the
  bone at their own index. Bones missing from the tree (attachments that ride
  another model's skeleton) become root-level joints at their bind pose.

Beyond Warframe-Exporter's description of the layout, this version worked
out:

- Normals and tangents are 24-bit octahedral vectors (12 bits per
  coordinate, big-endian across the three bytes); Warframe-Exporter does not
  export normals. Against normals rebuilt from the triangles the decoded ones
  average a cosine of 0.998; tangents come out perpendicular to them, and the
  position's fourth i16 (+/-255) is the bitangent sign.
- The space is left-handed: exported as stored, a sign's artwork comes out
  mirrored relative to its texture. The glTF export negates x (bones too:
  translation x and the quaternion's y and z) and swaps the winding;
  characters then face +z, glTF's front. A test pose confirms it: rotating
  `GAME_L1_ARM1` moves the character's left arm.
- Models without an `F.` part leave the `B.` counts at zero; everything is
  in `B.` then. An axis with a zero extent has every position at 0.

Not handled yet:

- Other model types: 132/134 (animations stored under `.fbx`), 108 (HLOD),
  166 (terrain meshes, `.tmesh`), the morph data above, and the newer packed
  rigged types (289-317). Warframe-Exporter's readers describe most of them.
  Level geometry (105/109) is covered in the next section.
- Animations.
- Vertex colours (stored in separate `.vc` entries), the byte after the normal
  (0 or 255), and what exactly the fourth vertex byte after the tangent holds
  (probably ambient occlusion).

### Levels

`src/level.rs` (with `src/ee.rs`, `src/physx.rs` and `src/level_export.rs`) reads
levels. Everything below was checked with `check-levels --deep` on the build
of 2026-10-22: all 4,601 `.level` files (3,000,087 objects) and all 411
`.static` files (809,052 objects) parse; the descriptions of all of them build;
all 10,404 type 109 and 1,222 type 105 level geometry files they place are read
in full (3.5 and 22.8 million triangles); 80,348 PhysX shapes parse. It takes
28 s and peaks at 560 MB (8 s and 77 MB without `--deep`).

Where things are:

| Package | Contents |
|---|---|
| `H.AnimRetarget` / `B.AnimRetarget` | `.level` (type 212), `.static` (type 30) and the procedural level entities (types 234, 281, 284, 286, 288) |
| `H.Misc` / `B.Misc` | the geometry of a level's CSG zones, `/Lotus/Levels/<Level>/<N>_c.fbx` (type 109) and `<N>_a.fbx` (type 105), and the placed models (type 102, 281) |
| `H.LightMap` / `B.LightMap` / `F.LightMap` | baked lighting: `<N>_clr.png`, `<N>_dir.png`, `<N>_I_l1.png`, reflection probes `<N>_cross.hdr` |
| `/Packages.bin` | the type of every object, with the properties it inherits |

- **A level** is a list of objects: type, position, rotation, scope (`/Layer3/`,
  `/Prefab2/`), and an attribute text in the notation of Packages.bin
  (`Key=Value` lines, `{a,b}` lists, `{k=v ...}` tables, nested). The layout is
  in the module docs of `level.rs`; the notation in `ee.rs`.
- **Types** are Packages.bin paths, or names relative to the level's folder
  (`DojoLight` in `/Lotus/Levels/ClanDojo/X.level` is
  `/Lotus/Levels/ClanDojo/DojoLight`). The chain of parents gives inherited
  properties: a decoration's `Mesh` or `MeshScale` often comes from its type. The
  root of the chain is the engine class: `/EE/Types/Engine/Light`, `CsgZone`,
  `PlayerSpawn`, `BlockingVolume`, `ZoneAttribs`, `/EE/Types/Alias/CsgSheet`,
  `/EE/Types/Framework/LevelPortal`; anything else with a `Mesh` is a placed model.
- **Paths** in an object's own text are relative to the folder named after the
  level (`Mesh=0_c.fbx` of `/Lotus/Levels/Hub/Orbiter.level` is
  `/Lotus/Levels/Hub/Orbiter/0_c.fbx`); inherited ones are relative to the type.
- **Space.** Positions, rotations and the vertices of level geometry share one
  left-handed, y-up space in metres; no halving is needed (Warframe-Exporter
  halves positions). In the dojo room below, the 5 x 3 m portal object sits exactly
  on the door-sized quad of the zone mesh, every placed model stands inside the
  zone's bounding box, and the PhysX meshes of the placed models lie on the
  visible geometry. `Transform::to_right_handed` negates x like `scene::prepare`.
- **CSG zones** (`/EE/Types/Engine/CsgZone`) are the architecture: one object per
  zone with `Mesh=<N>_c.fbx`, a transform of zero and a light map.
  Type 109 geometry is described in `model.rs`: the container of type 102
  with float positions, all in `B.`, with the material paths in the header.
  Type 105 is the same with packed vertices.
- **Static levels** (`Zone.StaticLevel=0.static`) hold the props of big levels:
  records of position, rotation, scale and model index (layout in `level.rs`). A
  zone lists the objects it owns in a group; every object is in at least one
  group, some in several, so each is placed once.
- **Hidden objects.** `StartHidden=1` objects (a quarter of the Orbiter's) are
  shown later by scripts. They are marked and left out of the glTF unless
  `--include-hidden`.
- **Lights** (`/EE/Types/Engine/Light` and its children) carry `Color` (0-255),
  `Brightness`, `Lumens`, `Radius`, `Angle`, `Length`, `Width`, `Height`,
  `Falloff`, `Temperature`, a projection `Texture` or `.ies` profile,
  `On=0`, `DirectionalLightDir`, and more; the description keeps the ones it
  names as fields and every other as text, and the glTF puts them in `extras`
  next to a plain `KHR_lights_punctual` light.
- **Collision.** A model header lists its PhysX shapes and `B.` starts with
  their cooked data (module docs of `physx.rs`). Types 5 and 6 are cooked PhysX
  3.4 convex and triangle meshes, read for every shape in the game files;
  types 1, 2 and 3 are sphere, capsule and box records. Levels also contain
  `BlockingVolume`s with `Dimensions` or `Length`/`Width`/`Height`; they are
  listed and put in `extras`, not drawn (their axes are not verified).
- **Spawn points** are `PlayerSpawn` and `NpcSpawnPoint` objects.
- **Environment.** `LevelInfo` (`PostProcessInfo.fogColor`,
  `distanceFogDensity`) and `ZoneAttribs` (`AmbientLight`, `FogColor`,
  `ReflectionMap`, `BackDropId`) are kept as environment settings and stored in
  the glTF scene's `extras`.
- **Tiles.** The connection points of a tile are `LevelPortal` objects
  (`Width`, `Height`); the description lists them. A procedural mission
  (`ProceduralLevel*` entities, types 234 to 288) holds the pool of tiles
  (`Prefab=<tile>.level`, `Likelihood`, `MaxInstances`, tags), but the choice and
  joining of tiles happens in game code, so a mission cannot be rebuilt from the
  files alone. Fixed levels can: dojo rooms (`/Lotus/Levels/ClanDojo`), the
  Orbiter, relays and hubs.

The dojo's obstacle course (`/Lotus/Levels/ClanDojo/ClanObstacleCourseA.level`) is
rebuilt in full: 1 zone (9,005 vertices, 8 materials, 5,766 collision
triangles), 498 placed models of 38 kinds, 8 lights, 4 blocking volumes, a
portal and two environment settings; `level ... -o` writes it in 6 s at a peak of
770 MB (texture baking is most of that; `--no-textures` peaks at 305 MB).

Not read, or not applied (the glTF and the description leave these out):

- Baked lighting. The light maps decode as ordinary textures (`texture
  --package LightMap`), but how a lightmap atlas, `LightMapCoords`,
  `LightMapNormalize` and the per-light indexed maps combine is not worked out,
  so surfaces are not lit like in the game. Reflection probes (`_cross.hdr`,
  type 183, a cube map) do not decode with the texture reader.
- Per-instance `MaterialParams` (tint and other overrides), attachments
  (`AttachParent`, `LocalPosition`), movers and their keys (instances are at rest),
  animations, particle effects, sounds, scripts and triggers. Skinned models
  are placed in their bind pose.
- HLOD models (type 108), terrain meshes (type 166) and landscapes (`.lndscp`);
  navigation meshes (`.nav`), BSPs, cover and probe data (`.bsp`, `.cvr`,
  `.epd`, `.lpd`, `.tmf`).
- Procedural tile selection and joining, as above.

### Packages.bin and materials

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
| `TX:TintMask` RGBA with `PS:TintColor0..3` | multiplies the base colour | layering from Warframe's texturing guide (R bottom, then G, B, A on top); blending in linear space is a guess |
| `TX:NormalMap` | normal | measured: green points down (DirectX), inverted for glTF; blue rebuilt |
| `TX:EmissiveMap` (red channel when `EMISSIVE_COMPRESSION=EC_COMPONENT`) | emissive | colour is `PS:EmissiveTintColorHi`; brightness (`EmissiveMapAtten`) ignored |

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
colours). Every tinted material slot in the game sets them, so the neutral
grey/gold fallback palette is never used for these. The green-down normal
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
- Opacity maps, detail maps, ambient-occlusion maps and vertex AO.
- Weapon skins and other material overrides outside the model's own list.

## Disclaimer

Not affiliated with or endorsed by Digital Extremes. Warframe is a trademark
of Digital Extremes Ltd. Use this tool for reading and studying game data
from your own install only; do not use it for cheating or to redistribute
game assets. Nothing extracted from the game belongs in this repository, its
issues or its pull requests. The game's own terms may restrict reading its
files; check them before using this tool.

## License

Licensed under either of MIT or Apache-2.0, at your option.
