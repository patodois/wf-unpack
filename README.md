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
- the Tenno's movement values (gravity, speeds, jump, camera, crouch rule), read from Packages.bin with the entity
  and key of each, and the characters, animations and levels as [rewrite-bridge](https://github.com/patodois/rewrite-bridge)
  models, clips and scenes for other games (see "Using wf-unpack from another game")

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

## Using wf-unpack from another game (rewrite-bridge)

[rewrite-bridge](https://github.com/patodois/rewrite-bridge) is a small crate of model, animation and
scene types that game rewrites share, so one game can use another's assets without knowing its file
formats. With the `bridge` feature, `wf_unpack::bridge::Warframe` implements its `Provider` for
Warframe: characters with their animations, levels as scenes, and (separately, from `wf_unpack::player`)
the numbers the player moves with. A rewrite needs none of Warframe's formats.

```toml
[dependencies]
wf-unpack = { git = "https://github.com/patodois/wf-unpack", features = ["bridge"] }
rewrite-bridge = { git = "https://github.com/patodois/rewrite-bridge" }
```

```rust
// Inside a function that returns Result<_, rewrite_bridge::BoxError>
use rewrite_bridge::{Provider, SceneModel, Target};
use wf_unpack::bridge::{animated_id, collision_id, Warframe};
use wf_unpack::player::Player;

let game = std::path::Path::new("/path/to/Cache.Windows");
// Textures are baked at full size by default; a limit keeps big levels small
let wf = Warframe::open(game)?.with_max_texture(Some(1024));

// A character with the clips of one stance. The clips animate the model's own nodes.
let sword = "/Lotus/Animations/Tenno/Movement/MeleeWeapon/Sword/";
let excalibur = wf.load_animated(&animated_id(
    "/Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx",
    sword,
))?;
let jog = excalibur
    .animation(&format!("{sword}JogForward_anim.fbx"))
    .unwrap();
// Root motion is the root bone's translation channel; events are in a side table
let root = excalibur.model.nodes.iter().position(|n| n.name == "GAME_C1_ROOT").unwrap();
let root_track = jog
    .channels
    .iter()
    .find(|c| c.node == root && c.target == Target::Translation)
    .unwrap();
let info = wf.clip_info(&jog.name).unwrap();
println!(
    "{} s, {} keys of root motion, StepL at {:?} s",
    jog.duration_s,
    root_track.times.len(),
    info.event_time("StepL")
);

// A level as a scene: models once, instances many times, lights in candela
let level = "/Lotus/Levels/ClanDojo/ClanObstacleCourseA.level";
let scene = wf.load_scene(level)?;
let SceneModel::Inline(first) = &scene.models[scene.instances[0].model] else { unreachable!() };
println!("{} instances of {} models; first: {}", scene.instances.len(), scene.models.len(), first.name);
// Its collision, as a second scene to build static physics bodies from
let collision = wf.load_scene(&collision_id(level))?;
println!("{} collision instances", collision.instances.len());

// The movement values, each with the Packages.bin entity and key it came from
let player = Player::load(game)?;
println!(
    "max speed {} ({}), jump {}, gravity {}",
    player.ground.max_speed.value,
    player.ground.max_speed.cite(),
    player.jump.height.value,
    player.gravity.world.value[1]
);
```

```sh
cargo build --release --features bridge
wf-unpack check-bridge --game $C              # load and validate every model
wf-unpack check-bridge --game $C --textures   # the same, baking every material
wf-unpack bridge-anim /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx --animations /Tenno/Movement/MeleeWeapon/Sword/
wf-unpack bridge-scene /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level [--max-texture 512 | --no-textures]
wf-unpack check-bridge-scenes --game $C       # build and validate the scenes of every level (no textures)
wf-unpack player                              # the movement values with units and sources
```

`check-bridge-scenes` builds the visible and collision scenes of all 4,601 levels without textures and validates
them: all pass (2,721,966 instances, 27,392 lights, 971,406 collision shapes) in 16 minutes at a peak of
0.74 GB. 12,747 model loads fail, 11,822 of them for model types the reader does not handle (HLOD and others,
see Levels below) and the rest for morph targets and a few size mismatches; that drops 55,774 instances, 2 % of
the total. Of the 39,413 lights, 12,021 are left out: 8,073 switched off at the start, 552 directional, 2,599
without `Lumens` and 797 without `Color`.

**Conventions** (the bridge's, which Warframe's data is converted to): metres, +Y up, right-handed, a
model's front faces +Z. Warframe's space is left-handed with the same up and front; negating x converts
it, for vertices, bones, animation keys, instance transforms and light positions alike. The units are
metres already (Excalibur is 1.84 m tall).

**`list(filter)`** returns every static (type 102) and skinned (type 281) model, with the kind taken
from the path (`/Lotus/Weapons` is Weapon, `/Lotus/Characters` is Character, anything else Prop), and
every `.level` as `AssetKind::Level` (4,601 of them). Ids are asset paths. The type 105/109 geometry of a
level's zones is loaded through its level, not listed.

**`load(id)`** returns a validated `Model`. Geometry goes through `scene::prepare`, so it matches the glTF
export. Each material slot becomes one mesh holding only the vertices its LOD 0 triangles use. Skinned
models get the bones as nodes under the model node (node 0), plus one skin. The bridge has one UV set; the
second one is dropped.

**`load_animated(id)`** with `<model>?animations=<filter>` (build it with `animated_id(model, filter)`)
returns the model and the clips that fit it. The filter is a substring of the clip's path, or a glob with
`*`, `**` and `?`; `|` separates several. A clip fits when every bone of one of its skeletons is a bone of
the model, by name, which is how `anim <model>` and the game retarget; all 81 bones of the Warframe rig are
in every Warframe's skeleton. Without the suffix no clip is loaded (a skeleton fits about 8,300 of them), and
a filter that matches more than 2,000 clips is an error that says so (`with_max_clips` changes it).

- A clip is named by the path of its animation file (`.../Sword/JogForward_anim.fbx`); a file with several
  takes or skeletons adds `#skeleton:take`. Every bone has a translation and a rotation channel on the
  bone's node (a bone the clip does not animate holds the clip's stored transform for it, which the game
  applies while the clip plays); scale channels exist where the clip animates scale. Interpolation is
  linear. `duration_s` is the header's `1 / rate`.
- **Root motion** is the root bone's translation channel (a Sword `JogForward` moves it 3.52 m in z), as in
  the data.
- **Events** (`StepL`, `StepR`, `JumpStartImpulse`, sound cues) have no place in the bridge's `Animation`, so
  they are in a side table: `Warframe::clip_info(name)`, or `Warframe::load_animated_clips(id)` which returns
  the same `ClipInfo` per clip, the clips it left out, and the count of animation files for other skeletons.
  A `ClipInfo` has the events with their time in seconds (`event_time("StepL")`) and as the fraction the data
  stores, the whole clip's root motion in the bridge's space, the `Rate` play-speed multiplier (not applied)
  and the clip's Packages.bin entity merged with its header text (`RootAdjustment`, `RootConditioning`,
  `TransitionTime`, ...). Events come from the clip's header text, or, when that has none, from its
  Packages.bin entity (393 of the 8,314 clips that fit Excalibur have them only there, among them the
  player's bullet jump).
- To make the keys valid for the bridge, times are clamped to the clip and keys that do not come after the
  previous one are dropped (none were, in all 8,314 clips), and rotations are normalised. Files that
  cannot be read (36 in the game, see Animations below) and clips with values the bridge cannot hold are
  left out and listed in `skipped` with the reason.
- Sockets and emissive strengths are not provided. The attachment bones (`GAME_R1_WEAPON1` and the like) are
  nodes of the model; `WeaponBone=GAME_R1_WEAPON1` is the only attachment the player data names.

**`load_scene(id)`** with a `.level` path returns a validated `Scene` (`wf.scene(path, &SceneOptions)` returns
more, see below). The level's CSG zones and the props it places (including those of its `.static` files) are
`SceneModel::Inline` models, each loaded once however many instances use it; an instance with an
`OverrideMaterial` list gets its own variant. Instances are in the level's order, with position,
rotation and `MeshScale` turned right-handed. The dojo is 499 instances of 39 models.

- **Left out of the scene**, and counted in `SceneReport`: objects with `StartHidden=1` (a script shows them
  later) and lights with `On=0` (`SceneOptions::include_hidden` keeps both), objects with a scale of zero
  (invisible in the game), and models the reader does not handle (HLOD type 108, terrain meshes type 166,
  models with morph targets). Skinned models stand in their bind pose; movers, particle effects, sounds,
  scripts, triggers, baked lighting and per-instance material tints are not applied (see Levels below).
- **Lights.** `Lumens` becomes candela (`intensity = Lumens / 4 pi`, the bridge's rule for a source given in
  lumens), `Color` (0..255) is decoded from sRGB, `Radius` is the range in metres. A light with `Angle` is a
  spot with an outer cone of `Angle / 2` and an inner cone of 0 (the data has none); `OMNI` types, lights
  without `Angle` and lights with an `Angle` of 0 (read as no cone) are point lights. **Unverified:** the game's light is taken to shine along its local +z;
  of the spot lights, +z is the axis that points downward in half of them (the others in at most a
  tenth), as ceiling lights do, but the data does not say. Colours match the usual sRGB values of the stored
  `Temperature` (2800 K is 255, 174, 94), which is why they are read as sRGB. Left out, with the count:
  lights without `Lumens` or `Color`, and directional lights (`Directional=1`; the data gives lumens, the
  bridge wants lux). Area-light boxes (`Length`, `Width`, `Height`), projection textures, `.ies` profiles and
  falloff are not represented.
- **Environment.** `Environment::Color` is the level's `LightingParams.skyLightColor` (the dojo's is black),
  when it has one. Fog, ambient light and the cube maps a level names are not a flat sky; they stay in the
  description (`WarframeScene::description.environments`) in the game's units.
- **Collision** is a second scene, `load_scene(&collision_id(level))` (the level path + `?collision`): its
  models are the PhysX shapes of the placed models, named like the visual models (the asset path), one
  mesh per shape named by what the shape is (`triangles`, `convex`, `box half_extents=[..]`,
  `sphere radius=..`, `capsule radius=.. half_height=..`). Triangle and convex meshes are the cooked
  shapes; boxes, spheres and capsules are tessellated (the exact parameters are in the name). Its instances
  have the visual scene's transforms, so a physics world is built by walking `instances` and creating one
  static body per instance from its model's meshes. The zones' triangle meshes are included, so
  the architecture collides (the dojo: 5,766 triangles in the zone). It has no lights, no environment, and
  one plain material. `BlockingVolume`s are not in it (their axes and whether their sizes are full or half
  extents are not verified; they are in `description.volumes`, with the spawn points and portals).
- Textures are baked at full size by default. Every model owns copies of its images (the bridge's `Material`
  owns them), and baking decodes the source textures at full size first, so a level costs memory in proportion
  to its models. The dojo's scene (39 models, 1.04 GB of images) peaks at about 2.1 GB of resident memory at
  full size (runs between 1.4 and 2.1 GB), 1.6 GB with `with_max_texture(Some(1024))`, 1.1 GB with 512 and
  0.45 GB without textures (`with_textures(false)`). The big hubs need a limit.

### Player movement values

`wf_unpack::player::Player::load(game)` reads the numbers the Tenno moves with from Packages.bin. Nothing
is typed in: it follows `TennoAvatar` to the controllers it names (`TennoWalkAttribs`, `TennoMotion`,
`CameraController`, `TennoClimbController`, `InputController`), the world's `PhysicsMgrDesc` and
`SlideAction`, and every value is a `Sourced` number that keeps the entity and key it came from. A missing
value is an error, not a default. The data has no units; each field's documentation says which unit it is
read as, and the interpretations are listed in `player::READINGS`.

| Field | Value | Source |
|---|---|---|
| `gravity.world`, `gravity.additional` | (0, -15, 0), -9 | `PhysicsMgrDesc.Gravity`, `TennoWalkAttribs.AdditionalGravity` |
| `capsule` | height 1.5, radius 0.45, step 0.45, slope limit 65 | `TennoWalkAttribs` |
| `ground.max_speed`, walk/jog fractions | 7.5, 0.4, 0.8 | `TennoWalkAttribs` |
| `ground` / `air` acceleration and drags | 12, 8.5, 10 / 10, 0.05, 0.2 | `TennoWalkAttribs` |
| `slide` | accel 6, drag 0.7, end 1.66, jump speed 5 | `TennoWalkAttribs`, `TennoMotion`, `SlideAction` |
| `jump` | height 5, `NumJumps` 2, air multiplier 0.75 | `TennoWalkAttribs`, `TennoAvatar` |
| `bullet_jump` | speed 2.5, gravity multiplier duration 0.8 | `TennoClimbController` |
| `roll` | 8 m forward, `ROOTC_ADJUST_LINEAR` | the roll clip's entity and header |
| `camera` | view offset (0.5, -0.1, -1.5), fov 65, pitch -89..89, collision radius 0.25 | `CameraController` |
| `crouch` | tap `ACTION`, hold `HOLD_CROUCH` after 0.15 s, `JUMP`/`MELEE` cancel the tap | `InputController.DemuxInputs` |

Readings that are not in the data: `JumpHeight` is the take-off speed in m/s (5 m/s up against a gravity of
15 gives an apex of 0.83 m, the Sword `Jump` clip's root rises 0.93 m), `AdditionalGravity` adds to the
fall, `BulletJumpSpeed` is a multiple of `MaxSpeed`, `NumJumps` counts the ground jump. What the data does not
provide is not approximated and is listed in `player::LEFT_OUT` (aim glide, wall latch, mantle heights, the
bullet jump's height and direction, ...). The module reads data only: how a character is simulated with it
is game code. `wf-unpack player [--notes]` prints everything.

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
  Across the 39,413 lights of all levels, `Lumens` is exactly `1600 x Brightness` in all 35,751 lights that
  have both (3,662 have no `Lumens`); `Angle` ranges from 0 (about 1,000 lights) to 170 degrees, which reads as a full cone angle;
  `Color` matches the usual sRGB values of the stored `Temperature` (2800 K is 255, 174, 94), so it is read
  as sRGB; 1,509 lights set `ColorMode=LCM_TEMPERATURE` and keep their `Color` as well. Lights with
  `Directional=1` (566) have `DirectionalLightDir` and lumens, not
  an illuminance; 1,046 lights with `Lumens` have no `Color`; 8,073 start with `On=0`. Of the spot lights, the local +z axis is the one that points downward in half of them
  (every other axis in at most a tenth, except the unrotated lights' -y), which is why the bridge scene
  reads +z as the direction a light shines; the data does not state it.
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
