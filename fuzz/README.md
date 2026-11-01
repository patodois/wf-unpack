# Fuzzing wf-unpack

Coverage-guided fuzz targets (cargo-fuzz / libFuzzer) for every reader that takes untrusted bytes or
text. This crate needs a nightly toolchain and is **not** part of the workspace, the published package
or CI (`[workspace]` in `Cargo.toml`, `exclude` in the root `Cargo.toml`).

```sh
cargo install cargo-fuzz
cd fuzz
cargo +nightly fuzz run toc_parse            # one target, until it finds something
cargo +nightly fuzz run lzf -- -max_total_time=600 -rss_limit_mb=2048 -timeout=10
cargo +nightly fuzz list
```

| target | entry point |
| --- | --- |
| `toc_parse` | `toc::Toc::parse` |
| `glob` | `toc::glob_match`, `toc::clip_filter_matches` |
| `cache_decompress` | `cache::decompress`, `cache::block_header` (first 4 bytes: declared size) |
| `lzf` | `cache::lzf_decompress` (first 2 bytes: output size) |
| `common_header` | `header::CommonHeader::parse` |
| `packages_bin` | `packages::PackagesBin::parse`, `texts`, `lineage` |
| `entity_text` | the property text of three chained Packages.bin entities, through `level::TypeResolver`, `clipspec`, `material`, `postprocess` |
| `ee_attributes` | `ee::Attributes::parse`, `ee::parse_value` |
| `material_text` | `material::parse_properties`, `parse_floats`, `resolve_path` |
| `animation` | `animation::AnimHeader::parse`, `animation::decode` |
| `animtree` | `animtree::Fsm`, `Tree`, `MovementSet` |
| `level` | `level::RawLevel::parse`, `StaticLevel::parse` |
| `model` | `model::ModelHeader::parse`, `Model::from_parts` |
| `model_vertex` | the three vertex decoders (one record of the documented size) |
| `texture` | `texture::TextureHeader::parse`, `Texture::from_parts`, `CubeTexture::from_parts`, `to_rgba`/`to_dds`/`to_png` |
| `physx` | `physx::parse_triangle_mesh`, `parse_convex_mesh` |
| `radiance_hdr` | `probe::read_radiance_hdr` |
| `bc6h` | `bc6h::decode_bc6h_block` |

Targets that read several parts of an asset (`H.`, `B.`, `F.`) split their input with
`wf_unpack_fuzz::split`: the first `2 * (parts - 1)` bytes are little-endian `u16` lengths.

`corpus/` holds a minimised corpus (made with `-merge=1`) seeded with synthetic inputs: `gen_seeds.py`
builds them from the layouts documented in `docs/` and in the module docs, plus the byte strings the unit
tests build. The `crash-*` files are the inputs that once made a reader fail; each has a regular test in
`tests/hardening.rs`. No game data is used anywhere.
