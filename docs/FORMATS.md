# Formats: packages, textures, models, animations, animation trees

File layouts and what was learned about them. Part of the [wf-unpack README](../README.md); levels are in [LEVELS.md](LEVELS.md), materials in [MATERIALS.md](MATERIALS.md), lighting in [LIGHTING.md](LIGHTING.md).

# Format notes

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

## Models

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
  (type 132) and level geometry (105/109) are covered in the Animations section below and in [LEVELS.md](LEVELS.md).
- Vertex colours (stored in separate `.vc` entries), the byte after the normal
  (0 or 255), and what exactly the fourth vertex byte after the tangent holds
  (probably ambient occlusion).

## Animations

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
list clip paths as plain text; the next section decodes them.

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
- The `Rate` play-speed attribute (the trees carry the rates they play clips at; see below),
  the blending itself and foot IK.
- Clips of other skeletons (creatures, weapon rigs, cameras) decode, but
  `--animations` only adds clips whose every bone is in the model.

## Animation trees and state machines

`src/animtree.rs` reads `.animfsm` (type 383, 419 files in `H.Misc`/`B.Misc`) and `.animtree` (type 388, 906
files in `H.AnimRetarget`/`B.AnimRetarget`), and follows Packages.bin from a suit to the tree its controller
runs. The module docs have the byte layouts; the facts that matter:

- **One container.** Both are a graph of nodes: the `H.` part holds a type list and each node's body, the `B.`
  part the child lists (`-1` is an empty slot). Node `i`'s children mean what its type says: a `Select` has one
  slot per value of its variable, a state machine node one per exposed state, a blend two inputs. A tree ends with
  four string pools (clips, state machines, sub-trees, errors) that nodes index.
- **State machines.** Every node is a state: name, the child slot that plays in it, conditions, transitions
  (blend time in seconds, a small mode word, conditions), events, and the transition's target taken from the edge
  list. The last transition of a state is stored in another layout and its target is the first edge. A condition is
  `variable operator text value`: `v108 == JOG(1.0)`. The operator is 0 `==`, 1 `!=`; 2, 3, 4, 5 are
  `<`, `<=`, `>`, `>=` (read from the speed checks: `Moving` goes to `Idle` on `v126 < 0.05`).
- **Trees.** 36 node types decode: clip nodes (a table from up to three variables to a clip index, plus a blend time),
  `Select` (one child per value of a variable), `SpeedBlend` (clip, speed, rate), `SingleAxisBlend` (direction and
  pitch points), partial-body and additive blends, request listeners, random/sequence nodes, sub-trees, IK nodes.
  `check-animtrees`: all 419 state machines (3,258 states, 9,576 transitions) and 581 of 906 trees (134,139 nodes);
  the rest need one of 13 more node types that are not read (`AimPosesAnimationNode` 101 trees, `ParametricMovementNode` 81,
  `LookIK` 65, `GenericParametric3DBlendNode` 37, ...).
- **Variables are numbers.** The data never names them. What it keeps are the texts they are compared with
  (`JOG`, `FBA_ROLL`, `MELEE_ONE_HAND`) and, where an editor expression was cut into pieces, the name inside it
  (`18 || Action == 19` gives `Action` for v2, `Turn Angle` for v142). The controller's actions set `IV_` variables to
  enum names, which names v77 `IV_JUMP`, v79 `IV_JUMP_STYLE`, v105 `IV_MOVE_ACTION`. The rest stay `vNN`.

Excalibur's chain: `ExcaliburBaseSuit` (`Mesh=ExcaliburBody_skel.fbx`, default customization
`AnimationSet=ExcaliburNobleAnims`, which replaces `PRN_IDLE_OVERRIDE` with `ExcaliburNobleIdleSubTree.animtree`),
no `DefaultAnimControllerOverride`, so `/Lotus/Types/Player/TennoAnimController`
(`AnimTree=Animation/SimpleTennoAnimTree.animtree`, with the actions that set `IV_` variables). That tree has 1,936
nodes, 910 clips, 45 state machines, 10 sub-trees. Its root machine `FsmRoot` exposes `Injury`, `Locomotion`,
`Requested Animation`...; the `Locomotion` child leads through IK blends to `FsmTestLocomotion`:

| State (exposed child) | Goes to (blend in seconds) |
|---|---|
| Idle | ZipLine, `EnterWallrun`, GrapplingHook (0.3) on `v108` == ZIPLINE, WALLRUN, GRAPPLINGHOOK; branch state #8 (0.3) on `v126 > 0.1`, which leads to Moving or SprintStart (0.3); branch state #1 on `v15`/`v90` |
| Moving | Sprinting (0.3) `v108 == SPRINT`; Slide (0.3) `v108 == SLIDE`; ZipLine, `EnterWallrun`, GrapplingHook (0.3); Idle (0.3) `v126 < 0.05`; branch state #1 |
| Sprinting | SprintStop (0.3) `v126 < 0.05`; Moving (0.3) when `v108 != SPRINT` and `v76 == WS_FIRE`, else Moving (0.05) when `v108 != SPRINT`; branch #1 on `v62 != FBA_NONE` |
| SprintStart, SprintStop | Sprinting, Idle (0.3), no condition |
| Slide | Idle (0.3) when `v108 != SLIDE` |
| ZipLine, GrapplingHook, `ExitWallrun` | Moving (0.3) |
| branch #1 (`v62 == FBA_ROLL`) | FullBodyAction, Roll (0.2); Roll goes to branch #2 (`v126 > 0.1`: Idle or Moving, 0.3), FullBodyAction to Idle (0.3) |

(`v108` is the move state: WALK 0, JOG 1, SPRINT 2, SLIDE 3, ZIPLINE 4, WALLRUN 5, GRAPPLINGHOOK 6; `v126` is the
movement speed, which every `SpeedBlend` is keyed on.) The 38 transitions of this machine blend 0.3 s, except the
two of the roll branch (0.2 s) and Sprinting back to Moving when the weapon is not firing (0.05 s). Over all 419
machines the 9,576 transitions are 0.3 s (7,633), 0.1 s (614), 0.2 s (354), 0.15 s (159), 0.6 s (152) and so on;
375 are 0 (instant), 67 of them in the 45 machines of the Excalibur tree, none in the locomotion machine.
Inside the states, `Select` nodes blend 0.3 s in 308 of 344 cases (0.1, 0.15, 0.5, 1 and 0 for the rest) and clip nodes
0.3 s in 387 of 392 (five are 0).

What plays in `Moving` for an Excalibur with a sword: `Select v95` (melee action) -> `v99` (parry) -> `v158` (stance:
`MELEE_ONE_HAND` is 3) -> `v44` (crouch: 0 stands) -> `v100` (weapon class; 5, 6, 16, 24, 25 are the Sword) ->
a `SpeedBlend` on `v126` with three points:

| Clip | Speed (m/s) | Rate |
|---|---|---|
| `Sword/Idle` | 0 | 0.508 |
| `Sword/WalkForward` | 1.81 | 0.909 |
| `Sword/JogForward` | 4.22 | 1.2 |

The speed is the clip's own root speed (3.52 m in 0.83 s is 4.22 m/s; `cargo test` checks it), so the blend is
by speed, not by distance. Direction is a `SingleAxisBlend` on `v65` (the angle between movement and facing) with
`BackLeft`, `Left`, `Forward`, `Right`, `BackRight` at -180, -90, 0, 90, 180 degrees; pitch is another on `v151`
(`Down`, `Forward`, `Up`). Turning in place is a decorator with `SpinLeft`/`SpinRight` entries, each with two lists of conditions on `v65` (to
start: 165..180 degrees and `v29 == 10`; to stay: -180..-135); procedural turning nodes limit the turn rate (`v143`
between -360 and 360 degrees at 360 degrees per second; `v65` between -60 and 60 at 4,320 degrees per second). Upper and lower
body are partial-body blends: each names a mask in Packages.bin (`TennoUpperBodyRequestMask`, `TennoRifleAimMask`...)
with a weight per bone (`GAME_C1_HIP1` 0, `GAME_C1_SPINE1` 0.75, `GAME_C1_SPINE2` 0.9, the rest 1), a weight variable and
fade times (0 s in 46 of the 47 plain mask blends, 0.15 s in one); additive blends take a variable weight. Idle variations: the idle
sub-tree (`TennoIdleSubTree`; Excalibur's `ExcaliburNobleIdleSubTree` is the same graph with the Noble clips) runs
`RandomIdleFsm` (states `Idle` and `Decoration`) and has 12 two-way `Random` nodes, e.g. between `Idle` and `IdleNoble`.

All 1,163 assets the tree names exist (910 clips of type 132, 106 movement sets of type 14, 147 aim-pose sets of
type 19), and all 910 clips decode.

Read, but only in part:

- Movement sets (type 14) and aim-pose sets (type 19), which `ParametricAnimationNode`s name: the `H.` part is
  read (548 of the 561 paths the decoded trees name; the others are other types). A movement set lists the clips of a
  directional locomotion (`SwordSprinting`: `JogForward`, `Sprint`, `Sprint90Left`, `Sprint90Right`; an aim-walk set:
  idle, forward, back, left, right) and six numbers (`0 12 180 720 1 10` for sprints, `1 5 180 360 1 10` for aim-walks);
  an aim-pose set names a clip and the bone it aims. Their `B.` tables are not read.

Lives only in code, so not in the data (and not reproduced here):

- Variable names and the number of values of an enum; what drives each variable (speed, direction, grounded...):
  `v126` is called the speed because it is what the speed blends use.
- The order in which conditions and transitions are tested, what the transition mode word and the state
  conditions do (they read as guards on the state: `Idle` and `Moving` both carry `v62 != FBA_NONE`), and the
  operator of the short condition layout (no operator is stored).
- The shape of a blend (linear or not), what `Random` flags and weights mean, when idle variations are requested
  (the `Decoration` request is set by game code), the request numbers of listener nodes (set by the controller's
  actions), and foot, look and reach IK.
- Which `NamedSubTreeNode` an animation set key (`PRN_IDLE_OVERRIDE`) fills.
- The controller of a suit that names none: the player's avatars carry `AnimControllerType=TennoAnimController`; the
  suit itself does not.

Not decoded: 13 node types (above), the aim-offset pose grid inside `AimPosesAnimationNode`, and the sub-trees
of the Excalibur tree that hold it (`WallRunningSubTree`, `GroundFireSubTree`, `TennoParryAimposes`).
