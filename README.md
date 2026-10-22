# wf-unpack

A small Rust tool that reads the game data of Warframe (Digital Extremes'
Evolution Engine) from your own install:

- `.toc` / `.cache` package pairs in `Cache.Windows/` (stored, Oodle and
  LZF blocks)
- textures, rebuilt from their `H.` / `B.` / `F.` parts and written as DDS
  (all mip levels) or PNG
- static models (type 102: weapons, props, level pieces), exported to
  glTF 2.0 with normals, tangents, both UV sets and every LOD

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

# A static model: print its layout, or export it to glTF (+ .bin) with -o.
# Materials are not resolved yet; --texture attaches a game texture by hand.
wf-unpack model /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx
wf-unpack model /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx -o out/ \
    --texture base=/Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine_t.png \
    --texture normal=/Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine_n.png
wf-unpack check-models --game $C

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
Puxtril (including its model readers for the static model layout); no code
was copied. Oodle streams are decoded with the pure-Rust
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

`src/model.rs` reads type 102, the static models (weapons, props, level
pieces). `check-models` reads all 33,689 of them with their `B.`/`F.` parts:
33,132 decode and pass the checks (header read to its last byte, part sizes
exact, positions inside the header's bounding box); 548 have a non-zero morph
count (mostly particle-emitter meshes) and carry extra data that is not
understood; 9 more have a `B.` part of unexpected size.

Beyond Warframe-Exporter's description of the layout, this version worked
out:

- Normals and tangents are 24-bit octahedral vectors (12 bits per
  coordinate, big-endian across the three bytes); Warframe-Exporter does not
  export normals. Against normals rebuilt from the triangles the decoded ones
  average a cosine of 0.998; tangents come out perpendicular to them, and the
  position's fourth i16 (+/-255) is the bitangent sign.
- The space is left-handed: exported as stored, a sign's artwork comes out
  mirrored relative to its texture. The glTF export negates z and swaps the
  winding.
- Models without an `F.` part leave the `B.` counts at zero; everything is
  in `B.` then. An axis with a zero extent has every position at 0.

Not handled yet:

- Materials. A model names its materials (`Materials={...}` in the header
  text), but the material entries in `H.Misc` list only shader sources; the
  texture paths are in `/Packages.bin`. That file's current layout (version 46)
  differs from LotusLib's description: about 200 unknown bytes before the
  reference list, and the entity list is now grouped by folder
  (`u32 len, folder, u8, u32 count`, then `name, 3 bytes, parent` per entity;
  21,860 folders and 467,796 entities read cleanly to the end of the file).
  The property text sits in a zstd blob with a dictionary (the dictionary's
  magic number is at offset 327,162), but where each entity's frame starts is
  not worked out. Until then `--texture` attaches textures by hand.
- Skinned and other model types: 281 (9,057 rigged), 132/134 (animations
  stored under `.fbx`), 105/109 (level geometry), 108 (HLOD), and the
  morph data above. Warframe-Exporter's readers describe most of them.
- Vertex colours (stored in separate `.vc` entries), the byte after the normal
  (0 or 255), and what exactly the fourth vertex byte after the tangent holds
  (probably ambient occlusion).

## Disclaimer

Not affiliated with or endorsed by Digital Extremes. Warframe is a trademark
of Digital Extremes Ltd. Use this tool for reading and studying game data
from your own install only; do not use it for cheating or to redistribute
game assets.

## License

Licensed under either of MIT or Apache-2.0, at your option.
