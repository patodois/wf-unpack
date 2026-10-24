# Using wf-unpack from another game (rewrite-bridge)

The provider, its ids and conventions, the player movement values. Part of the [wf-unpack README](../README.md).

[rewrite-bridge](https://github.com/patodois/rewrite-bridge) is a small crate of model, animation and
scene types that game rewrites share, so one game can use another's assets without knowing its file
formats. With the `bridge` feature, `wf_unpack::bridge::Warframe` implements its `Provider` for
Warframe: characters with their animations, levels as scenes, and (separately, from `wf_unpack::player`)
the numbers the player moves with. A rewrite needs none of Warframe's formats.

```toml
[dependencies]
wf-unpack = { git = "https://github.com/patodois/wf-unpack", features = ["bridge"] }
# crates.io gives every crate in the build the same rewrite-bridge: all 0.2.x requirements share one copy
rewrite-bridge = "0.2"
```

Every crate in one build has to share one copy of rewrite-bridge, or `Warframe` implements a `Provider` that
your code's `Provider` is not ("multiple different versions of crate `rewrite_bridge`"). From crates.io Cargo does
that for you: this crate asks for `0.2.1`, your `"0.2"` accepts it, and both resolve to one 0.2.x release. Another
minor line (`"0.3"`) would be a second copy. `cargo tree -i rewrite-bridge` must show a single copy.

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
wf-unpack check-bridge --game $C              # load and validate every model (heavy: see the README)
wf-unpack check-bridge --game $C --textures   # the same, baking every material
wf-unpack bridge-anim /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx --animations /Tenno/Movement/MeleeWeapon/Sword/
wf-unpack bridge-scene /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level [--max-texture 512 | --no-textures] [--lighting-environment]
wf-unpack check-bridge-scenes --game $C       # build and validate the scenes of every level (no textures)
wf-unpack player                              # the movement values with units and sources
wf-unpack check-conformance --id /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx   # check the provider's output against the bridge's conventions
```

`check-conformance` runs the bridge's conformance checker over what the provider returns, one asset at a time:
scale, orientation, winding, skins, materials, scenes. Its options, the texture limit it applies by default
(256 px) and its memory use are in the [README](../README.md#check-conformance). A model is loaded, checked and
dropped before the next.

The provider bakes a material with the same plan and bake as the glTF export (`pbr::MaterialBaker`), so the
base colour is the same image: the layered tint (0.5, lerped towards each of the four `PS:TintColor` by its
mask, doubled), with the tint colours of the materials themselves. Excalibur's body has a mean linear albedo of
0.051 and his helmet 0.073 either way. The bridge's `Material` holds the baked images, so the four tint colours
cannot be changed after loading (the player's colour picks), and the suit's energy colours (`model --suit`)
are not applied to the emissive images.

`check-bridge-scenes` builds the visible and collision scenes of all 4,601 levels without textures and validates
them: all pass (2,721,966 instances, 27,392 lights, 971,406 collision shapes) in 13 minutes at a peak of
0.79 GB (with `--lighting-environment`, which decodes the cube map of 543 levels: 12 minutes, 0.83 GB; the
pictures are cached by path and the packages are opened only on a miss). 12,747 model loads fail, 11,822 of them for model types the reader does not handle (HLOD and others,
see [LEVELS.md](LEVELS.md)) and the rest for morph targets and a few size mismatches; that drops 55,774 instances, 2 % of
the total. Of the 39,413 lights, 12,021 are left out: 8,073 switched off at the start, 552 directional, 2,599
without `Lumens` and 797 without `Color`. By default no scene has an environment; with
`--lighting-environment` 269 levels get the cube map picture, 41 the grey a content type gives and 4,291
none, and 274 levels name a cube map the install does not have (`environments` in the command's output).

**Conventions** (the bridge's, which Warframe's data is converted to): metres, +Y up, right-handed, a
model's front faces +Z. Warframe's space is left-handed with the same up and front; negating x converts
it, for vertices, bones, animation keys, instance transforms and light positions alike. The units are
metres already (Excalibur is 1.84 m tall).

**`list(filter)`** returns every static (type 102) and skinned (type 281) model, and every `.level` as
`AssetKind::Level` (4,601 of them). The kind comes from the path and, for `/Lotus/Characters`, from the
model's header (`bridge::is_whole_character`): `/Lotus/Weapons` is Weapon, anything else is Prop, and a model in
`/Lotus/Characters` is Character only when it is a whole one. That folder also holds the helmets, armour,
cloth, hair and reference meshes that are worn on a character (11,389 models there, 570 of them whole). A
whole character is skinned (a static model has no skeleton), its weighted bones include a head, a spine and
at least three of the four limbs (`GAME_C1_HEAD1`, `GAME_C1_SPINE3`, `GAME_L1_ARM1`, `GAME_R1_LEG2`; cloth hangs
on rigs of its own and a helmet weights a few bones of the body's), and its bind-pose mesh reaches down to
y = 0 within 0.3 m (the bridge places characters by their feet; a jacket or a pair of sleeves skinned to the
whole rig hangs from the shoulders). Full-length garments skinned to the whole rig (a Kubrow's armour, a body
suit) pass the rule, and creatures and machines whose bones are not named like a body (a Moa) do not.
Ids are asset paths. The type 105/109 geometry of a level's zones is loaded through its level, not listed.

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
  cannot be read (36 in the game, see [FORMATS.md](FORMATS.md#animations)) and clips with values the bridge cannot hold are
  left out and listed in `skipped` with the reason.
- Sockets and emissive strengths are not provided. The attachment bones (`GAME_R1_WEAPON1` and the like) are
  nodes of the model; `WeaponBone=GAME_R1_WEAPON1` is the only attachment the player data names.

**`load_scene(id)`** with a `.level` path returns a validated `Scene` (`wf.scene(path, &SceneOptions)` returns
more, see below in this file). The level's CSG zones and the props it places (including those of its `.static` files) are
`SceneModel::Inline` models, each loaded once however many instances use it; an instance with an
`OverrideMaterial` list gets its own variant. Instances are in the level's order, with position,
rotation and `MeshScale` turned right-handed. The dojo is 499 instances of 39 models.

- **Left out of the scene**, and counted in `SceneReport`: objects with `StartHidden=1` (a script shows them
  later) and lights with `On=0` (`SceneOptions::include_hidden` keeps both), objects with a scale of zero
  (invisible in the game), and models the reader does not handle (HLOD type 108, terrain meshes type 166,
  models with morph targets). Skinned models stand in their bind pose; movers, particle effects, sounds,
  scripts, triggers, baked lighting and per-instance material tints are not applied (see [LEVELS.md](LEVELS.md)).
- **Lights.** `Lumens` becomes candela (`intensity = Lumens / 4 pi`, the bridge's rule for a source given in
  lumens), `Color` (0..255) is decoded from sRGB, `Radius` is the range in metres. A light with `Angle` is a
  spot with an outer cone of `Angle / 2` and an inner cone of 0 (the data has none); `OMNI` types, lights
  without `Angle` and lights with an `Angle` of 0 (read as no cone) are point lights. **Unverified:** the game's light is taken to shine along its local +z;
  of the spot lights, +z is the axis that points downward in half of them (the others in at most a
  tenth), as ceiling lights do, but the data does not say. Colours match the usual sRGB values of the stored
  `Temperature` (2800 K is 255, 174, 94), which is why they are read as sRGB. Left out, with the count:
  lights without `Lumens` or `Color`, and directional lights (`Directional=1`; the data gives lumens, the
  bridge wants lux; their direction and colour are in `WarframeScene::atmosphere`). Area-light boxes (`Length`, `Width`, `Height`), projection textures, `.ies` profiles and
  falloff are not represented.
- **Environment.** None by default. The level's `LightingParams.globalCubeMap` and `skyLightColor` are
  lighting inputs of the baker (`giEnvironmentIntensity` weights the picture: 0 in all of Duviri's levels, 0.2
  or 1.0 for the deep-space ones), the data does not say either is the sky the game draws, and the bridge's
  `Environment` becomes a skybox and scene lighting in a host, so the scene does not claim it. 4,647 of the
  4,830 `LevelInfo` objects carry only the engine type's black (it is no statement of the level; none sets
  `skyLightColor` in its own table and 183 get one from a content type). The capture and the colour are in
  `WarframeScene::atmosphere` (`Atmosphere::sky`, with `giEnvironmentIntensity`); the float picture is
  `Warframe::probe(path)` (decoded once per path and cached; the packages are opened on a miss and
  dropped), the glTF export's `sky_equirect.hdr`. On request, `SceneOptions::lighting_environment`
  (`--lighting-environment`) puts the lighting capture in `Scene::environment`: the cube map as
  `Environment::Equirectangular` (512 x 256 for the 128 px cubes, 8-bit sRGB in the bridge's convention,
  x mirrored, the middle column -Z), `intensity` the cube's largest texel (the scale of the coding, not
  a sky brightness; mean relative error over all texels 1.0 %, 0.9 %, 8.7 % and 1.7 % for the four
  pictures in the install, [LIGHTING.md](LIGHTING.md)), else the colour a content type or the level gives,
  else none; `SceneReport::environment_source` says which and `environment_notes` lists a named picture
  that could not be used. 543 levels name a global cube map, 4 of the 10 pictures are in the install and
  serve 269 levels.
- **Sun, ambient light, fog and global illumination are beside the scene**, in
  `WarframeScene::atmosphere` ([`Atmosphere`](LIGHTING.md), `wf_unpack::atmosphere`): the suns with
  direction (the vector as stored, and unit length in the bridge's space), colour, `Brightness` and `Lumens`;
  the keyframes of time-of-day skies; zone ambient colours; level and zone fog; the GI volumes, probe files
  and `ReflectionSH1`; and `left_out()`, which lists what it does not turn into numbers, with the reason and
  the count. Why they are not in `Scene`: the bridge's directional light is in lux and the data gives
  `Lumens` (= 1600 x `Brightness`) with no conversion (code), so a directional light stays out of
  `Scene::lights` as before (566 lights in 434 levels) and its direction is in `suns`; `Scene` has one
  environment and no fog, ambient gradient or probe type, and the contract was not changed for this. Units:
  colours are 0..255 sRGB-coded with alpha last unless a field is named linear; densities are the game's own
  numbers; directions are unit vectors in the bridge's space (the vector is data; "toward the sun" is an
  inference, argued in [LIGHTING.md](LIGHTING.md) and `Sun::toward_sun_game`).
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

## Player movement values

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
