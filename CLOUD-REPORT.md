# Cloud report: coverage-guided fuzzing of wf-unpack

Branch `claude/fuzz-wf-unpack-d45jqe`. Everything below used synthetic inputs only; no game file or game
data was read, downloaded or committed.

## What was done

- Added `fuzz/` (cargo-fuzz / libFuzzer, nightly only): 18 targets, one per public entry point that takes
  untrusted bytes or text (list and layout in `fuzz/README.md`). The crate has its own `[workspace]`, is
  `exclude`d from the published package (`exclude = ["/fuzz", "/CLOUD-REPORT.md", "/docs/reviews"]` in the root
  `Cargo.toml`; `cargo package --list` shows no `fuzz/` file and no report; it lists `tests/hardening.rs`, an ordinary
  test) and CI only compiles it (`cargo check --locked` in `fuzz/`), it does not run it. `fuzz/target`, `fuzz/artifacts` and
  `fuzz/coverage` are git-ignored.
- `fuzz/gen_seeds.py` builds the synthetic seeds from the documented layouts and from the byte strings the
  unit tests build (the latter were dumped with a temporary, uncommitted wrapper around the test builders).
- Ran every target for at least 30 minutes (table below), libFuzzer with ASan, overflow checks and debug
  assertions on, `-rss_limit_mb=2048 -timeout=10 -max_len=8192`. Four of them (texture, model, animation,
  toc_parse) also ran 10 minutes at `-max_len=65536` (600000 for toc_parse).
- Committed corpus: the seeds of `fuzz/gen_seeds.py` and the `crash-*` regression inputs only, 127 files, 40,202 bytes
  (the first version of the branch had the whole libFuzzer corpus, 7,679 files, 2,285,929 bytes; what a run adds is
  git-ignored).
- Regular tests for every bug are in `tests/hardening.rs` (an integration test with a global allocator
  that records the largest single allocation of a call, so "allocation follows the input" is a real
  assertion). Each fix line was checked by removing or weakening it and seeing a test fail (the mutation list is in the review
  follow-up below).

## Bugs

| # | Where | Input shape | Failure | Fix | Tests (`tests/hardening.rs`) |
|---|---|---|---|---|---|
| 1 | `cache::decompress` | 4 to 5 bytes of entry data with a `.toc` `len` of 3.37 billion | `vec![0; len]` up front: out of memory (libFuzzer: `malloc(3372932899)`) | The output grows block by block; each block is limited by the entry size and by the 24-bit size of the block header. An unframed entry must be a size its input can make: LZF at most 88 bytes out per byte in (a 264-byte back-reference takes 3 input bytes). An unframed Oodle stream has no size field, so its buffer starts at one block (262,144 bytes) and doubles until the entry size, which fails at the first size past the end of the stream; `len` is capped at `MAX_ENTRY_LEN` (1 GiB) | `cache::declared_size_does_not_drive_the_allocation`, `cache::lzf_at_its_best_ratio_is_still_accepted`, `cache::unframed_lzf_limit_is_88_per_input_byte`, `cache::unframed_oodle_may_be_larger_than_a_block_header_can_say`, `cache::declared_size_over_the_largest_entry_is_refused`, `cache::blocks_may_not_add_up_to_more_than_the_entry` |
| 2 | `toc::glob_match` (and `clip_filter_matches`) | pattern with a few dozen `**` that cannot match | exponential backtracking: hang (libFuzzer timeout, 327-byte input) | Rewritten as a token-by-token pass over the set of reachable path positions, O(pattern x path) | `glob::many_wildcards_do_not_take_exponential_time`, `glob::agrees_with_the_backtracking_matcher_on_small_inputs` (the old matcher is kept in the test and compared on every pattern up to 5 characters over `a/*?` and every path up to 5 over `a/b`) |
| 3 | `probe::read_radiance_hdr` | size line `-Y 5 +X 122777777` (49-byte input); runs longer than the scanline; truncated runs | out of memory (`Vec::with_capacity(w*h)`, `vec![..; w]`), index-out-of-bounds panics on `data[pos]`, `scan[x+i]` | Width must fit the 16-bit scanline marker; the output grows per scanline; runs are checked against the rest of the scanline and the data | `probe::radiance_size_line_does_not_drive_the_allocation`, `probe::radiance_malformed_runs_are_errors`, `probe::radiance_valid_scanline_still_reads`, `probe::radiance_width_must_fit_the_marker`, `probe::radiance_literal_longer_than_the_scanline_is_an_error`, `probe::radiance_literal_cut_short_at_the_end_is_an_error` |
| 4 | `toc::Toc::parse` | a chain of N nested directories with 60-byte names (found by reading the code, then confirmed with a test; libFuzzer at 8 KB inputs could not reach it) | every directory keeps its full path, so memory is quadratic: a 384 KB `.toc` took about 480 MB, more for a bigger file | A full path over 4096 bytes is an error: a resource cap, documented in the README (the other readers use the same number for path strings; the longest path in the author's install is 158 bytes) | `toc::a_chain_of_nested_directories_is_an_error_not_quadratic_memory`, `toc::a_deep_but_plausible_path_is_accepted`, `toc::the_path_limit_is_4096_bytes_for_files_and_directories` |

Crash inputs in the corpus: `corpus/glob/crash-timeout-exponential-star` (the 327-byte artifact),
`corpus/glob/crash-many-doublestars`, `corpus/radiance_hdr/crash-oom-size-line` (the artifact),
`corpus/cache_decompress/crash-declared-size`, `corpus/toc_parse/crash-deep-directory-chain` (hand-made,
6.7 KB). The original OOM artifact of bug 1 was deleted by mistake before it was copied; `crash-declared-size`
is rebuilt from the report line (`malloc(3372932899)`) and the unit test, not the libFuzzer file. The glob
and radiance artifacts were not shrunk with `-minimize_crash` (it takes minutes per step on a timeout and
the readers were already fixed); the test inputs are hand-minimal instead.

## Fuzzing time per target

Seconds of libFuzzer run time (the runs listed as "pre-fix" crashed or were stopped early and are not
counted in the 30 minutes). All figures are on 4 cores, one process per target.

| target | time | note |
|---|---|---|
| animation | 1801 s (+601 s at 64 KB) | clean |
| animtree | 1802 s | clean |
| bc6h | 1802 s | clean |
| cache_decompress | 1801 s | clean after fix 1 (the run that found it is not counted) |
| common_header | 1802 s | clean |
| ee_attributes | 1802 s | clean |
| entity_text | 1801 s | clean (added after the first round) |
| glob | 1802 s after fix 2 (+24 s pre-fix) | clean after the fix |
| level | 1801 s | clean |
| lzf | 1801 s | clean |
| material_text | 1801 s | clean |
| model | 1801 s (+601 s at 64 KB) | clean |
| model_vertex | 1801 s | clean |
| packages_bin | 1801 s | clean |
| physx | 1801 s | clean |
| radiance_hdr | 1801 s after fix 3 (+9 s pre-fix) | clean after the fix |
| texture | 1802 s (+601 s at 64 KB) | clean |
| toc_parse | 1802 s before fix 4 + 1802 s after it (+601 s at 600 KB) | clean |

Plus a 20 s and a 45 s smoke pass over every target before the seeds existed (that pass found the
`cache_decompress` OOM and the model_vertex panic described below).

## Left out, and why

- `model_vertex`: `decode_vertex`, `decode_float_vertex` and `decode_skinned_vertex` index a slice of
  exactly one record (24 or 32 bytes) and panic on a shorter one. Their docs say "one 24-byte vertex" and
  the only caller chunks the vertex data, so I treated it as a documented precondition and the target pads
  the input to a full record. Turning them into `Result`-returning functions would change the public API;
  I did not do that unasked. Say so if you want it.
- Entry points that take a game directory or a `Package`/`TexturePackages` (`Package::open`, `bridge*`,
  `Model::load`, `palette::resolve`, ...) are not targets: they only add file I/O around the readers that are
  fuzzed. `Package::read_raw` already checks the entry against the `.cache` length before allocating.
- `atmosphere`, `gltf`, `scene`, `collision`, `lightmap`, `pbr` and the `*_cmd` modules work on already
  parsed values, not on bytes.
- `Packages.bin` compressed texts (`TextReader::text`) are decompressed into a buffer sized from a length in
  the frame, capped at 64 MiB. That is bounded by a constant but not by the input; I did not change it
  because I could not justify a tighter bound from the code or docs (zstd has no simple ratio limit).
  The `packages_bin` target only reaches stored (uncompressed) texts, because it cannot produce a valid zstd
  frame for the dictionary; the compressed path is covered by libFuzzer only through mutated seeds that fail
  early.
- Coverage was not measured by this report (the independent review measured it and found the harnesses reach
  the parsers). The deepest parsers (`model`, `animation`, `level`, `texture`) depend on the seeds built from
  the unit tests, and the real files may reach branches synthetic seeds do not.

## Uncertain

- The first version of fix 1 capped an unframed Oodle entry at the 24-bit size of a block header. That field
  belongs to the header an unframed entry does not have, so the cap was not justified and could reject valid
  entries; the review found it (B1) and it is gone: the buffer now grows with the stream (see Review
  follow-up).
- The 4096-byte path limit (fix 4) is a resource cap, not a fact of the format. It is documented in the
  README; the owner should confirm the number.
- `cargo clippy -p wf-unpack --all-targets` reports one warning in `src/player.rs:680` that was there before this branch.

## Review follow-up

The independent review (`docs/reviews/fuzz-wf-unpack.md` on its own branch) found five blockers. They are fixed on
`fix/fuzz-review`; for each fix a test fails when the fix is removed (mutation checked, one at a time, with
`cargo test --all-features --test hardening`).

| Review point | Change | Test that fails without it |
|---|---|---|
| B1 Oodle 0xFF_FFFF limit | Removed. Unframed Oodle decodes into a buffer that starts at one block and doubles | `unframed_oodle_may_be_larger_than_a_block_header_can_say`, `declared_size_does_not_drive_the_allocation` |
| B2 `w <= 0xFFFF` | Test with the width 4294967304 | `radiance_width_must_fit_the_marker` |
| B3 P4, P6 | Tests with a literal longer than the scanline and a literal cut short in the last channel | `radiance_literal_longer_than_the_scanline_is_an_error`, `radiance_literal_cut_short_at_the_end_is_an_error` |
| B3 C1 | Two blocks that add up past the entry | `blocks_may_not_add_up_to_more_than_the_entry` |
| B3 C2 | 100 maximum-ratio copies; the limit is pinned from both sides | `lzf_at_its_best_ratio_is_still_accepted`, `unframed_lzf_limit_is_88_per_input_byte` |
| B3 T2, T4 | Paths of exactly 4096 and 4097 bytes, for files and directories | `the_path_limit_is_4096_bytes_for_files_and_directories` |
| B4 package | `exclude` lists the report and `docs/reviews` | `cargo package --list` |
| B5 corpus | Seeds and `crash-*` only | n/a |
| N1 | `decompress` refuses `len > MAX_ENTRY_LEN` | `declared_size_over_the_largest_entry_is_refused` |
