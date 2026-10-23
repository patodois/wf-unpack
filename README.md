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
- skeletal animations (type 132): idle, walk, run, sprint, jump, slide and the other ~25,000 clips, as
  glTF animations on a model's skeleton
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

# Animations: the clips that fit a model's skeleton, or a skeleton name, or one clip in detail.
# --animations adds every matching, compatible clip to the glTF as an animation.
wf-unpack anim /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx --filter /Tenno/Movement/MeleeWeapon/Sword/
wf-unpack anim /Lotus/Animations/Tenno/Movement/MeleeWeapon/Sword/JogForward_anim.fbx
wf-unpack model /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx -o out/ --animations Tenno/Movement/MeleeWeapon/Sword/
wf-unpack check-anims --game $C

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

# Lighting: reflection probe (type 183) to HDR files, post-process settings, lightmap pages, Warframe default colours
wf-unpack probe /Lotus/Levels/ClanDojo/ClanObstacleCourseA/0_cross.hdr -o out/
wf-unpack postprocess /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level [--json]
wf-unpack lightmaps /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level -o out/
wf-unpack palette /Lotus/Powersuits/Excalibur/Excalibur
wf-unpack check-lighting --game $C        # every level: pages, probes, post-process fields
wf-unpack model /Lotus/Characters/Tenno/Excalibur/ExcaliburBody_skel.fbx -o out/ --suit /Lotus/Powersuits/Excalibur/Excalibur

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
for levels, static levels and level geometry, the `dev` branch's Packages.bin
reader and the `animation2` branch's outline of the animation header, body and
quaternion packing); no code was copied. Warframe's public
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

- Other model types: 134 (cinematic animations), 108 (HLOD), 166 (terrain
  meshes, `.tmesh`), the morph data above, and the newer packed rigged types
  (289-317). Warframe-Exporter's readers describe most of them. Animations
  (type 132) and level geometry (105/109) are covered in the sections below.
- Vertex colours (stored in separate `.vc` entries), the byte after the normal
  (0 or 255), and what exactly the fourth vertex byte after the tangent holds
  (probably ambient occlusion).

### Animations

`src/animation.rs` reads type 132 (`*_anim.fbx`: 24,676 files, 24,987 clips,
about 17 hours of animation). The module docs have the byte layout; the facts
that matter:

- **Where.** Header in `H.Misc`, body in `B.Misc`, no `F.` part, almost all
  under `/Lotus/Animations/` (a few hundred have no extension). The header
  holds the skeleton (bone names and parents) and, per clip, the frame count,
  rate and quantisation ranges; the body holds the quantised frames.
- **Time.** Keys are fractions of the duration, ascending, with irregular
  spacing (redundant keys were dropped). The duration is `1 / rate`; it is a
  whole number of 1/30 s in all 24,987 clips, so the source was sampled at 30
  frames per second. The `Rate=` attribute in the common header is a play-speed
  multiplier the game applies at run time; it is not applied here.
- **Tracks.** Per bone, flags say what is animated: translation (3 x i16,
  `offset + v * scale` with ranges from the header), rotation (48-bit packed
  quaternion, smallest three), x translation as an f32 (2,081 tracks, e.g. the
  `NW0A` wire bones), scale (3 x i16 like translation). A bone with flags 0 or
  32 keeps the initial transform stored in the body; so does every component
  that is not animated. Decoded first keys equal the stored initial transforms
  for 1,216,708 of 1,216,793 animated components (the rest are clips whose
  initial transforms hold garbage such as 4.9e11 m, or ranges of tens of metres).
  A rotation and its negative are the same rotation; `Animation::channels`
  flips signs so interpolation takes the short way.
- **Root motion.** The skeleton header stores the clip's total root rotation and
  translation (a Sword `JogForward` moves 3.52 m in z in 0.83 s, a
  `Sprint90Left` turns 90 degrees); the same displacement is in the root bone's
  translation track, so exported clips move the character. The glTF
  `extras` of each animation repeat it, with the events and the attribute text.
- **Events.** The attribute text lists events (`StepL`, `StepR`, sound
  cues); their `Time` is a fraction of the clip (the two steps of a jog are half a cycle apart).
- **Handedness.** Same as the models: x negated, rotations (x, -y, -z, w). A
  sprint-left clip turns the character toward its left in the exported glTF.

How a clip links to a character: the model carries its own skeleton and the
animation names its bones, so the link is the bone names. All Warframe clips use
the same 81 bones (`GAME_C1_ROOT` ... `GAME_R1_TRICORDERATTACH`), each with the
same parent in every Warframe's model; `Excalibur_skel.fbx` has these plus
`Body1` and `Helmet1`. The skeleton name in the animation header is only the
rig the clip was authored on (`Tenno` 3,654 clips, `KirkDropSuit` 1,998,
`ExcaliburBody1` 235, `WispBody1`, `MagBody1`, ...; Titania's `FaerieBody1` has
141 bones), so `anim <model>` lists clips by bone names, which is what the game
retargets by. In Packages.bin, Excalibur's suit (`/Lotus/Powersuits/Excalibur/ExcaliburBaseSuit`)
names `Mesh=/Lotus/Characters/Tenno/Excalibur/ExcaliburBody_skel.fbx` and an
`AnimationSet` (`ExcaliburNobleAnims`, whose `ExcaliburNobleIdleSubTree.animtree`
names `Excalibur_skel.fbx`); the controller is the default
`/Lotus/Types/Player/TennoAnimController` (`AnimTree=Animation/SimpleTennoAnimTree.animtree`),
and some suits override it with `DefaultAnimControllerOverride`. The `.animtree`
files (in `H.AnimRetarget`), the `.animfsm` state machines (type 383) and the
movement sets (types 14 and 19, e.g. `/Lotus/Animations/Tenno/Movement/Unarmed/UnarmedMovement`)
list clip paths as plain text; they are not decoded, so which clip the game
picks when is not derived.

Movement clips for an Excalibur with his sword out
(`/Lotus/Animations/Tenno/Movement/MeleeWeapon/Sword/`; the same names exist
under the other stances, e.g. `Unarmed/`, `Rifle/`, `Bow/`):

| Motion | Clip | Length | Root motion |
|---|---|---|---|
| idle | `Idle_anim.fbx` (also `IdleNoble`, `IdleAgile`) | 1.97 s | none |
| walk | `WalkForward_anim.fbx` | 1.10 s | 1.99 m |
| run | `JogForward_anim.fbx` | 0.83 s | 3.52 m |
| sprint | `Sprint_anim.fbx` (`Sprint90Left/Right`) | 0.57 s | 3.69 m |
| jump | `JumpStart_anim.fbx`, `Jump_anim.fbx`, `JumpEnd_anim.fbx` | 0.17 / 1.13 / 0.27 s | |
| slide | `KneeSlide_anim.fbx` | 1.97 s | none |

`check-anims` decodes every type-132 file, one entry at a time (peak RSS 274 MB,
about 15 s): 24,640 files ok, 36 not decoded; 24,987 clips, 2,166,135 bone
tracks, 1,217,467 keys; all key times ascend and all durations are multiples of
1/30 s. Checked by rendering: Excalibur with the Sword `JogForward`, `Sprint`,
`Jump`, `Idle`, `KneeSlide` and `WalkForward` clips in headless three.js
reads as a run, a sprint, a jump and a slide.

Not read, and why:

- Type 134 (`*_cin.fbx`, 1,469 cinematic clips): another header, an `F.` part;
  not looked at beyond its first bytes.
- 36 type-132 files fail: 19 with `Compressed=AC_DISABLED` (8; raw frames, not
  understood) or `AC_ENABLED_POSITION_ENHANCE` (11; four more header bytes and
  another frame layout), 11 with a `CinematicCamera` skeleton (extra channel
  flags 64/66 and key times that do not fit), and a few others (a root channel
  flag 64, two headers whose skeleton count is nonsense, short frame data in a
  few portal effects). `check-anims` lists them.
- Three floats after each bone's initial translation: the same in nearly every
  bone (0, 0.0078, 2.0037) and garbage in others; they are not a scale and are
  ignored. Scale channels (flag 8, 5,497 tracks) follow the translation
  scheme and are decoded, but nothing in the data checks them.
- Interpolation: the data has no interpolation field; keys are exported as
  linear (spherical for rotations).
- The `Rate` play-speed attribute, additive and partial-body blending
  (`BlendMaskSmooth`, masks in the trees), foot IK, and everything in the
  animation trees and state machines.
- Clips of other skeletons (creatures, weapon rigs, cameras) decode, but
  `--animations` only adds clips whose every bone is in the model.

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
portal and two environment settings; `level ... -o` writes it, with the lightmap pages and
the probe, in 11 to 20 s at a peak of about 650 MB (texture baking is most of
that; `--no-textures` is lower).

Not read, or not applied (the glTF and the description leave these out):

- Baked lighting is decoded and written (see "Lighting" below) but only its
  non-directional part is exact: the directional gain scale, the indexed
  light maps (`_I_l1`), light probes and the lights' own contribution are
  not decoded.
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
| `TX:TintMask` RGBA (or the pack map's alpha quadrants) with `PS:TintColor0..3` | multiplies the base colour | decoded from the fill shaders: start at 0.5, lerp towards each tint by its mask (0, 1, 2, 3 in turn), double, keep the length at 0.04 or more; no gamma anywhere |
| `TX:NormalMap` | normal | measured: green points down (DirectX), inverted for glTF; blue rebuilt |
| `TX:EmissiveMap` (red channel when `EMISSIVE_COMPRESSION=EC_COMPONENT`) | emissive | decoded from the fill shaders: map squared, times `EmissiveTintColor` (or `lerp(Lo, Hi, map^2)` for multi-tint), baked into the texture; `PS:EmissiveMapAtten` is `KHR_materials_emissive_strength` |

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

## Lighting and post-processing

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
8 bits, so `src/bc6h.rs` is its decoder with float output. `wfu probe` writes
an equirectangular `.hdr` (RLE Radiance) and six face files; `level -o` writes
`probe<N>_equirect.hdr`. The dojo's probe holds values up to 32,192 (the
emissive strips).

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

## Disclaimer

Not affiliated with or endorsed by Digital Extremes. Warframe is a trademark
of Digital Extremes Ltd. Use this tool for reading and studying game data
from your own install only; do not use it for cheating or to redistribute
game assets. Nothing extracted from the game belongs in this repository, its
issues or its pull requests. The game's own terms may restrict reading its
files; check them before using this tool.

## License

Licensed under either of MIT or Apache-2.0, at your option.
