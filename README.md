# wf-unpack

A small Rust tool that reads the game data of Warframe (Digital Extremes'
Evolution Engine) from your own install:

- `.toc` / `.cache` package pairs in `Cache.Windows/` (stored, Oodle and
  LZF blocks)
- textures, rebuilt from their `H.` / `B.` / `F.` parts and written as DDS
  (all mip levels) or PNG

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

## Format notes

The module docs in `src/toc.rs`, `src/cache.rs`, `src/header.rs` and
`src/texture.rs` describe the layouts. Starting points were the
documentation and source of [LotusLib](https://github.com/Puxtril/LotusLib)
and [Warframe-Exporter](https://github.com/Puxtril/Warframe-Exporter) by
Puxtril; no code was copied. Oodle streams are decoded with the pure-Rust
[oozextract](https://github.com/lvlvllvlvllvlvl/oozextract) and BCn blocks
with [texture2ddecoder](https://crates.io/crates/texture2ddecoder).

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

### Models (first look, not implemented)

- Models live in `H.Misc` / `B.Misc` / `F.Misc` under `.fbx` paths. The
  common header's type selects the layout: 102/105/106/86/96 static, 99/105
  and 103/109/112/113 level geometry, 159-170 terrain, 269-288 rigged,
  289-317 "packed" (current characters and weapons; `kinds` on `H.Misc` finds
  33,689 entries of type 102 and 9,057 of 281).
- The `H.` part is a long, version-specific header: physics shapes, bone
  names, vertex/face/bone/morph counts (also separate counts for what lives in
  `B.`), a bone tree, mesh infos (material names, index ranges per LOD) and
  PhysX meshes. Warframe-Exporter has one reader per type number, mostly
  fixed-size skips between known fields.
- The `B.` part starts with the PhysX mesh blobs, then inverse bind matrices
  (3x4 floats per weighted bone), bone rest poses, and then vertices:
  positions as four `i16` normalised by 32767 (scaled by two "ensmallening"
  vectors from the header), then normals, tangents, two UV sets, AO, bone
  indices and weights; indices follow. When `B.` has fewer vertices than the
  header, the full-detail buffers are in `F.`.

## Disclaimer

Not affiliated with or endorsed by Digital Extremes. Warframe is a trademark
of Digital Extremes Ltd. Use this tool for reading and studying game data
from your own install only; do not use it for cheating or to redistribute
game assets.

## License

Licensed under either of MIT or Apache-2.0, at your option.
