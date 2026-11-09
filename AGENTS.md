# Agent guide

wf-unpack reads Warframe assets for developers studying, exporting or loading their own installation in another game engine.
Its optional `bridge` feature implements a rewrite-bridge provider; the workspace also contains a Bevy viewer.

## Toolchain, build and CI

- The README requires Rust 1.90+ and reports testing with 1.99; CI uses stable Rust.
- Library CI runs on Linux, macOS and Windows; viewer compilation runs on macOS.
- `bridge` is optional and off by default.
- Exact library and viewer CI commands:

```sh
cargo build -p wf-unpack --release
cargo test -p wf-unpack
cargo test -p wf-unpack --features bridge
cargo test -p wf-unpack --release --lib bc6h
cargo build -p wf-viewer
```

CI also runs `cargo check --locked` with `fuzz/` as the working directory.
The fuzz crate builds on stable; running its fuzz targets uses nightly, as documented there.
Release CI builds with `cargo build --release --locked --target <target> -p wf-unpack --features bridge`.
Its targets are `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`.

## Code map

- `src/toc.rs`, `cache.rs`, `packages.rs`: package indexes, entry reads and properties.
- `src/model.rs`, `texture.rs`, `material.rs`, `pbr.rs`: geometry and texture/material conversion.
- `src/animation.rs`, `animtree.rs`, `player.rs`: clips, state machines and movement data.
- `src/level.rs`, `scene.rs`, `collision.rs`, `physx.rs`: levels, placements and collision.
- `src/bridge*.rs`, `gltf.rs`, `main.rs`: provider, export and CLI.
- `viewer/`, `tests/`, `fuzz/`, `docs/`: renderer, regressions, fuzz targets and format notes.

## Project rules

- Read `.toc`/`.cache` and `Packages.bin` from the user's `Cache.Windows` directory.
- Game-data tests use `WF_GAME_DIR` and skip if the installation is absent; other tests need no game data.
- Large levels with full-size textures can need several GB; use `--max-texture 512` or `--no-textures` when appropriate.
- Keep asset caches and reads incremental rather than expanding the whole game installation.
- Oodle decoding is supplied by clean-room lztans; its implementation derives from prose, not existing decoder source.
- Keep format evidence and limitations documented in `docs/FORMATS.md`, `LEVELS.md` and related guides.
- Models, textures, extracts and screenshots remain local; none are repository fixtures.

## Public files

- Never commit game files, extracted assets, renders, decompiled code or secrets.
- The public-tree gate needs Python 3.9+ and Git; run it from the repository root:
  `python3 tools/check_public_tree.py` (checks Git's index, including staged new files).
- CI checks committed changes with `python3 tools/check_public_tree.py --range BASE..HEAD`.

## Contributing, license and AI

- Open an issue to discuss a problem or send a pull request with the change.
- Before a pull request, run the applicable CI commands above and the public-tree gate.
- License: MIT OR Apache-2.0, at your option; see `LICENSE-MIT` and `LICENSE-APACHE`.

The code was written with AI coding agents (Anthropic's Claude and OpenAI's GPT models) under the maintainer's direction.
AI-assisted contributions are welcome when their use is disclosed and the changes are reviewed.
