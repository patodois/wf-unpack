# Levels

How `.level` and `.static` files, level geometry (types 105/109) and PhysX collision are read. Part of the [wf-unpack README](../README.md).

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
portal and two environment settings; `level ... -o` writes it, with the lightmap pages and
the probe, in 11 to 20 s at a peak of about 650 MB (texture baking is most of
that; `--no-textures` is lower).

Not read, or not applied (the glTF and the description leave these out):

- Baked lighting is decoded and written (see [LIGHTING.md](LIGHTING.md)) but only its
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
