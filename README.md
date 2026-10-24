# wf-unpack

[![rewrite-bridge on crates.io](https://img.shields.io/crates/v/rewrite-bridge.svg?label=rewrite-bridge)](https://crates.io/crates/rewrite-bridge)
[![rewrite-bridge docs](https://img.shields.io/docsrs/rewrite-bridge?label=docs.rs)](https://docs.rs/rewrite-bridge)

A Rust tool and library that reads Warframe's game data (Digital Extremes' Evolution Engine) from your own
install, and turns it into things other software understands: glTF 2.0 models, PNG and DDS textures,
animations, whole levels, and the numbers the Tenno moves with.

It is for people who want to look at, convert or study Warframe's assets, and for people who write a **game
rewrite**: a new engine for an old game that loads the original's assets, possibly mixed with another game's
(a mash-up). For the second kind, wf-unpack is a *provider* for [rewrite-bridge](https://github.com/patodois/rewrite-bridge),
a small crate of model, animation and scene types that rewrites share (see [Using it from another game](#using-it-from-another-game)).

What it reads:

- `.toc` / `.cache` package pairs in `Cache.Windows/` (stored, Oodle and LZF blocks)
- textures, rebuilt from their `H.` / `B.` / `F.` parts and written as DDS (all mip levels) or PNG
- static models (type 102: weapons, props, level pieces) and skinned models (type 281: Warframe bodies,
  helmets, rigged weapons), exported to glTF 2.0 with normals, tangents, both UV sets, materials and, for skinned
  models, the skeleton, inverse bind matrices, joints and weights
- skeletal animations (type 132): idle, walk, run, sprint, jump, slide and the other ~25,000 clips, as glTF
  animations on a model's skeleton
- animation state machines (`.animfsm`) and trees (`.animtree`): which clip plays when, with the blend times
- levels (`.level`, `.static`): the objects a level places, its geometry (types 105/109), PhysX collision
  shapes, lights, spawn points and portals, exported to glTF with each model stored once and instanced; and
  what they say about their sky, sun, ambient light, fog and baked global illumination
- the Tenno's movement values (gravity, speeds, jump, camera, crouch rule), read from Packages.bin with the
  entity and key of each

This project contains no game code or game data. It only reads files from a copy of the game you already have
installed. Words you may not know are explained in the [glossary](#glossary) at the end.

## Download

Programs built for each release are on the [Releases page](https://github.com/patodois/wf-unpack/releases). Pick the archive for your system:

| System | Archive |
|--------|---------|
| macOS, Apple silicon | `wf-unpack-<tag>-macos-arm64.tar.gz` |
| macOS, Intel | `wf-unpack-<tag>-macos-x86_64.tar.gz` |
| Linux, x86_64 (glibc 2.35 or newer) | `wf-unpack-<tag>-linux-x86_64.tar.gz` |
| Windows, x86_64 | `wf-unpack-<tag>-windows-x86_64.zip` |

Each archive holds the program, this README, both license files and an `INSTALL.txt`. `SHA256SUMS` on the release page has
the checksum of every archive (`shasum -a 256 -c SHA256SUMS` on macOS, `sha256sum -c SHA256SUMS` on Linux,
`Get-FileHash` on Windows). They are built with the `bridge` feature, so the rewrite-bridge commands are included. The 3D viewer (`wf-viewer`) is not in the archives: it needs extra system libraries to build on Linux, so build it from source (see [Build](#build)).

**The binaries contain no game data.** They are the program only; it reads the files of a copy of Warframe that you already have
installed.

They are not code-signed, so the first run may be blocked:

- **macOS (Gatekeeper)**: a file downloaded in a browser is quarantined, and macOS refuses to open it ("Apple could not
  verify ..."). Remove the mark once, from the folder you extracted to: `xattr -d com.apple.quarantine ./wf-unpack`. Or open
  System Settings > Privacy & Security and press "Open Anyway" after the first refusal.
- **Windows (SmartScreen)**: "Windows protected your PC" appears the first time. Choose "More info", then "Run anyway". Or
  right-click the downloaded `.zip`, open Properties and tick "Unblock" before you extract it.

To build it yourself instead, see [Build](#build).

## Prerequisites

- **Rust 1.90 or newer** ([rustup.rs](https://rustup.rs)); the code is written and tested with 1.99.
- **Warframe's `Cache.Windows` folder**, about 51 GB, from a Windows install of the game. It holds the
  `.toc` and `.cache` files (`H.Misc.toc`, `F.Texture.cache`, ...). wf-unpack only reads it and never changes it.
  - On **Windows** it is inside the game's install folder, for Steam
    `...\steamapps\common\Warframe\Cache.Windows`.
  - On **macOS** there is no Warframe build, so the folder has to come from a Windows install: either install
    Steam and Warframe in a [CrossOver](https://www.codeweavers.com/crossover) bottle (the folder is then
    `~/Library/Application Support/CrossOver/Bottles/<bottle>/drive_c/Program Files (x86)/Steam/steamapps/common/Warframe/Cache.Windows`),
    or copy the folder from a Windows PC.
  - On **Linux**, Steam with Proton puts it under `~/.steam/steam/steamapps/common/Warframe/Cache.Windows`.
- **Where wf-unpack looks.** `~/Games/Warframe/Cache.Windows` is only the author's layout and the default of
  `--game`. Pass `--game <folder>` to any command, or symlink your folder there:
  `mkdir -p ~/Games/Warframe && ln -s "<your Cache.Windows>" ~/Games/Warframe/Cache.Windows`.
- **Memory and disk.** Most commands need well under 1 GB of RAM. The exceptions are said next to the command
  below. Output is small except for level exports with full-size textures.

## Build

```sh
git clone https://github.com/patodois/wf-unpack && cd wf-unpack
cargo build --release          # about a minute on a recent machine, plus fetching the dependencies
export PATH=$PWD/target/release:$PATH   # so that `wf-unpack ...` below finds the program
```

Without the `export` line, write `cargo run --release -- list ...` or `target/release/wf-unpack list ...`
instead of `wf-unpack list ...`.

The commands that talk to rewrite-bridge (`check-bridge`, `bridge-anim`, `bridge-scene`, `check-bridge-scenes`,
`check-conformance`) only exist when built with the `bridge` feature, which downloads [rewrite-bridge](https://crates.io/crates/rewrite-bridge) from crates.io:

```sh
cargo build --release --features bridge
```

## Try it

A first look needs no export step and takes a few seconds:

```sh
C=~/Games/Warframe/Cache.Windows

wf-unpack list $C/H.Texture.toc --limit 10      # the first files of a package: size, stored size, path
wf-unpack model /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx      # layout, no files written
wf-unpack model /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx -o out/   # glTF + PNGs
wf-unpack texture /Lotus/Interface/Icons/Miscellaneous/Kuva.png -o kuva.png
```

`-o` is a **folder** for `model`, `level`, `extract`, `probe` and `lightmaps` (`-o out/` gets `out/<name>.gltf`,
`out/<name>.bin` and the PNGs); for `texture` it is a **file**, and its extension (`.dds` or `.png`) picks the
format. Open the `.gltf` in Blender, three.js or Godot.

## Usage

All commands take `--game <folder>` (default `~/Games/Warframe/Cache.Windows`; the examples set `C` to it).
Model and clip paths are the game's own: `/Lotus/...`. Commands marked **heavy** read a lot of the game; times
are from a Mac with an SSD and 16 GB, on the Steam build of 2026-10-22.

```sh
# File list of a package, optionally filtered (substring, or a glob with * / ** / ?), with --limit
wf-unpack list $C/H.Texture.toc --filter /Lotus/Interface/Icons --limit 20

# Extract raw (decompressed) entries into a folder
wf-unpack extract $C/B.Misc.toc '/Lotus/Interface/**.swf' -o out/

# HEAVY: decompress every entry of every package and summarise failures. Reads all 51 GB: about 5 minutes,
# little RAM. To try it on one package first: --only picks packages by name.
wf-unpack check $C --only H.Texture
wf-unpack check $C

# One texture as DDS (all mips) or PNG (top mip), by output extension
wf-unpack texture /Lotus/Interface/Icons/Miscellaneous/Kuva.png -o kuva.dds --game $C

# A static model: print its layout, or export it to glTF (+ .bin + PNGs) into a folder with -o.
# Materials are resolved through Packages.bin and baked to glTF PBR textures;
# --texture base=/normal=/emissive=PATH replaces that with textures chosen by hand.
wf-unpack model /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx -o out/
wf-unpack check-models --game $C            # HEAVY: every model; about 1 minute, 0.3 GB

# Animations: the clips that fit a model's skeleton, a skeleton name, or one clip in detail.
# --filter and --animations take the same filter: a substring of the clip's path (/Tenno/Movement/MeleeWeapon/Sword/),
# or a glob with * / ** / ?, and | between several. The output starts with "fits this model: N".
# --animations adds every matching, compatible clip to the glTF as an animation.
wf-unpack anim /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx --filter /Tenno/Movement/MeleeWeapon/Sword/
wf-unpack anim /Lotus/Animations/Tenno/Movement/MeleeWeapon/Sword/JogForward_anim.fbx
wf-unpack model /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx -o out/ --animations /Tenno/Movement/MeleeWeapon/Sword/
wf-unpack check-anims --game $C             # HEAVY: every clip; 20 s, 0.3 GB

# What Excalibur's controller does: the locomotion state machine (states, transitions, blend times,
# conditions), the variables, the clips of each state (--stance Sword: only that folder's clips) and
# the blend masks
wf-unpack animtree /Lotus/Powersuits/Excalibur/ExcaliburBaseSuit
wf-unpack animtree /Lotus/Powersuits/Excalibur/ExcaliburBaseSuit --stance Sword
wf-unpack check-animtrees --game $C         # every state machine and tree; 1 s

# Packages.bin: properties of an asset (and its parents), text search, full decode
wf-unpack props /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatinePBR
wf-unpack find-text PrimeGalatine_n.png
wf-unpack check-packages --game $C          # 1 s, 0.3 GB
wf-unpack check-materials --game $C         # 16 s, 0.6 GB

# A level: list what it places, or export it (models instanced, materials baked) to glTF;
# --collision also writes <name>_collision.gltf with the PhysX shapes.
# A big level with full-size textures needs several GB of RAM: add --max-texture 512 or --no-textures.
wf-unpack level /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level
wf-unpack level /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level -o out/ --collision
wf-unpack level /Lotus/Levels/Hub/PlayerShipInteriorOrbiter.level -o out/ --max-texture 512 --include-hidden
wf-unpack check-levels --game $C          # every level and static level parses (streaming); 12 s, 0.1 GB
wf-unpack check-levels --deep --game $C   # + descriptions, level geometry and PhysX of every model they place; 35 s, 0.6 GB

# Sky, sun, ambient light, fog and GI of a level, with what the data does not say (--json for the glTF extras' form)
wf-unpack atmosphere /Lotus/Levels/Hub/PlayerShipInteriorOrbiter.level [--json]
wf-unpack check-atmosphere --game $C      # every level: named files in the install, how the suns agree; 9 s and 0.5 GB

# Lighting: reflection probe (type 183) to HDR files, post-process settings, lightmap pages, Warframe default colours
wf-unpack probe /Lotus/Levels/ClanDojo/ClanObstacleCourseA/0_cross.hdr -o out/
wf-unpack postprocess /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level [--json]
wf-unpack lightmaps /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level -o out/
wf-unpack palette /Lotus/Powersuits/Excalibur/Excalibur
wf-unpack check-lighting --game $C        # every level: pages, probes, post-process fields, about 35 s and 0.5 GB
wf-unpack model /Lotus/Characters/Tenno/Excalibur/ExcaliburBody_skel.fbx -o out/ --suit /Lotus/Powersuits/Excalibur/Excalibur

# Surveys: asset types in an H. package, texture headers vs. B./F. sizes
wf-unpack kinds $C/H.Misc.toc
wf-unpack textures --game $C                # 1 s
```

Archives are never loaded whole (`F.Texture.cache` alone is 22 GB): every entry is read with a seek, and
`check` asks macOS not to keep what it read in the file cache.

### What the check commands should print

The `check-*` commands are regression checks for the readers. Several of them list **failures that are
expected**: files whose format the reader does not handle yet, counted per message (digits in a message are
shown as `N`). They exit 0 either way. On the Steam build of 2026-10-22:

| Command | Expected |
|---|---|
| `check` | 1,090,424 entries of 95 packages, 0 failures |
| `check-models` | type 102: 33,132 of 33,689 pass (548 with morph targets, 9 whose `B.` part has a size the reader does not know); type 281: 9,013 of 9,057 pass (44 with such a `B.` part). Those messages (`B. part is N bytes, expected N`) describe the reader's limit, not a damaged install |
| `check-animtrees` | state machines 419 of 419; trees 581 of 906 decoded (the rest use node types that are not decoded yet, each listed with its count); movement and aim-pose sets 548 of 561 |
| `check-lighting` | reflection probes 8,688 decoded, 22 failed (cubemaps of type 177, which are not probes) |
| `check-anims` | 24,640 of 24,676 animation files decode; the 36 others are listed per message (see [docs/FORMATS.md](docs/FORMATS.md#animations)) |
| `check-packages` | 467,795 of 467,795 entities decode |
| `check-bridge` | the same models as `check-models`, except five type-281 skeletons whose rest pose has a NaN scale in the game data (rewrite-bridge 0.2's `Model::validate` rejects those): type 102 33,132 of 33,689, type 281 9,008 of 9,057 |
| `check-levels` | all 4,601 `.level` and 411 `.static` files parse, 0 fail |
| `check-atmosphere` | 4,601 levels, 0 fail: 566 directional lights in 434 levels, 635 `sunPosition` in 634, 1,350 GI volumes in 857 levels (5,794 textures, 2,032 `.lpd`, 3,585 `.epd` named, all in the install), 44 dynamic skies in 28; 543 levels name a global cube map, 10 distinct, 4 in the install (269 levels) |

If your numbers are far from these, the game has been updated; open an issue with the version.

## Using it from another game

With the `bridge` feature, `wf_unpack::bridge::Warframe` implements rewrite-bridge's `Provider`: characters with
their animations, levels as scenes, and (separately, from `wf_unpack::player`) the numbers the player moves with.
The other game needs none of Warframe's formats. To use it:

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
use rewrite_bridge::Provider;
use wf_unpack::bridge::Warframe;

// Textures are baked at full size by default; a limit keeps big levels small
let wf = Warframe::open(std::path::Path::new("/path/to/Cache.Windows"))?.with_max_texture(Some(1024));
let model = wf.load("/Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx")?;
```

See [docs/BRIDGE.md](docs/BRIDGE.md) for the ids, the conventions, clips, levels as scenes, lights, collision and
the player movement values, and rewrite-bridge's [guide to writing a provider](https://github.com/patodois/rewrite-bridge/blob/main/docs/WRITING_A_PROVIDER.md)
for what a provider is and how the pieces fit.

The commands that exercise the provider (build with `--features bridge`):

```sh
wf-unpack check-bridge --game $C              # HEAVY: load and validate every model (42,746); 45 s, 0.3 GB
wf-unpack check-bridge --game $C --textures   # the same, baking every material: about an hour, 1 GB (1,000 models took 90 s)
wf-unpack bridge-anim /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx --animations /Tenno/Movement/MeleeWeapon/Sword/
wf-unpack bridge-scene /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level [--max-texture 512 | --no-textures] [--lighting-environment]
wf-unpack check-bridge-scenes --game $C       # HEAVY: build and validate the scenes of every level (no textures): 13 minutes, 0.8 GB
wf-unpack player                              # the movement values with units and sources
```

### check-conformance

`check-conformance` runs rewrite-bridge's conformance checker over what the provider returns, one asset at a
time: scale, orientation, winding, skins, materials, scenes.

```sh
wf-unpack check-conformance --id /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx
wf-unpack check-conformance --kind level --limit 5
```

- Put `--game` (and `--max-texture`, `--no-textures`) first, or anywhere; the other options are the checker's:
  `--id` (repeatable), `--filter`, `--kind`, `--limit`, `--compare-load`, `--allow`, `-v`, `--strict`; `--help` lists
  them.
- **Textures are baked at 256 pixels at most unless you say otherwise** (`--max-texture N`), and `--no-textures`
  skips baking. A level baked at full size needs several GB: `--kind level --limit 5` peaked at 7.1 GB of
  resident memory (18.5 GB footprint) before there was a limit. With the default it peaks at about 1.0 GB
  (1.5 GB footprint) and takes about 2 minutes; `--no-textures` takes 6 s and 0.5 GB. The checks look at
  geometry and the material slots, not at pixels.
- A model id with `?animations=FILTER` is checked with its clips (`list` does not give those ids, hence
  `--allow provider.unlisted_id`).
- Exit status 1 means an asset has an error; warnings (such as `orientation.not_upright` on a level's loose
  props) do not change it unless `--strict`.
- A model is loaded, checked and dropped before the next, so the peak is that of one asset.

The provider bakes a material with the same plan and bake as the glTF export, so the base colour is the same
image; see [docs/MATERIALS.md](docs/MATERIALS.md).

## Viewer

`viewer/` is `wf-viewer`, a Bevy app that opens models straight from the game files, with no export step. It
uses the same scene preparation and baked materials as the glTF export.

```sh
cargo build -p wf-viewer       # first build about 3 minutes and 3 GB in target/
# One or more model paths; N / P switch between them, drag to orbit, scroll to zoom
target/debug/wf-viewer /Lotus/Weapons/Tenno/SemiAutoRifle/PrimeBraton.fbx \
    /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx --game $C
# Skinned models are live: --pose-demo swings the left upper arm (GAME_L1_ARM1)
target/debug/wf-viewer /Lotus/Characters/Tenno/Excalibur/Excalibur_skel.fbx --pose-demo
# Save a frame and exit (with several paths: out-1.png, out-2.png, ...)
target/debug/wf-viewer /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx --screenshot out.png
```

Options: `--game <Cache.Windows>`, `--lod N`, `--screenshot <file.png>`, `--pose-demo`. Dependencies are built
optimised and without debug info, so Bevy runs smoothly in debug builds and the folder stays small.

## Tests

```sh
cargo test --release                                                    # unit tests, no game files needed
WF_GAME_DIR=~/Games/Warframe/Cache.Windows cargo test --release         # also the tests that read your install
WF_GAME_DIR=~/Games/Warframe/Cache.Windows cargo test --release --features bridge   # and the provider
```

## More documentation

- [docs/BRIDGE.md](docs/BRIDGE.md): the rewrite-bridge provider and the player movement values
- [docs/FORMATS.md](docs/FORMATS.md): packages, textures, models, animations, animation trees
- [docs/LEVELS.md](docs/LEVELS.md): levels, level geometry, collision
- [docs/MATERIALS.md](docs/MATERIALS.md): Packages.bin and how materials are baked
- [docs/LIGHTING.md](docs/LIGHTING.md): probes, post-process, lightmaps
- [rewrite-bridge](https://github.com/patodois/rewrite-bridge): the shared types and the conformance checker;
  its `docs/WRITING_A_PROVIDER.md` explains the concepts above from the other side

## Glossary

- **Game rewrite**: a new engine that loads an old game's original assets. A **mash-up** loads assets of
  several games into one.
- **rewrite-bridge**, **Provider**: the shared crate, and the trait a game implements to hand out its
  assets as the crate's `Model`, `AnimatedModel` and `Scene` types.
- **`.toc` / `.cache`**: a package is two files. The `.toc` is the index (paths, sizes, offsets), the `.cache`
  holds the data. Packages come in families: `H.*` holds an asset's header (sizes, attribute text), and `B.*`
  and `F.*` hold its data in two parts (for a texture, `F.` has the largest mip levels and `B.` the smaller
  ones; a model's vertices are split between them the same way).
- **Type 102 / 281 / 132 / 105 / 109 / 183 / 212**: the number in an asset's header that says what it is:
  static model, skinned model, animation, level geometry, level geometry, reflection probe, level.
- **Packages.bin**: the database of named entities (materials, clip settings, the player's controllers).
- **Tenno**: the player's characters. A **Warframe** is a Tenno's suit.
- **LOD**: level of detail, a model stored at several sizes; 0 is the finest.
- **PhysX**: the physics engine whose collision shapes the levels carry.
- **HLOD**: a distant, merged version of a group of models (type 108). Not read.
- **glTF**: the open 3D file format the exports use (`.gltf` + `.bin` + PNGs). **PBR**: physically based
  materials (base colour, metal and roughness, normal map).
- **RSS**: resident set size, the memory a process holds in RAM. **Footprint** is macOS's stricter count.
