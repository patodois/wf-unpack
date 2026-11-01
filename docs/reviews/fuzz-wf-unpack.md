# Review of `claude/fuzz-wf-unpack-d45jqe` (commit 753f768) against `main` (4a7b335)

Reviewer: an independent cloud session. The reviewed branch was not changed. All inputs used here are synthetic.

## Verdict: BLOCKERS

The four fixes are correct for the crashing inputs and the code is sound, but there are five blockers. Two are gaps
in the tests (a fix line that no test pins), one is a limit that the repository's docs do not justify, one is
packaging, and one is the size of the committed corpus. None of them is a crash that the fuzz targets or my
own probes could reproduce on the branch as it stands.

## Blockers

### B1. Unframed Oodle limit `0xFF_FFFF` is not justified by the docs and can reject valid entries
`src/cache.rs:146-148` (`MAX_BLOCK_LEN`) and `src/cache.rs:166-169` (the `limit`).

- The module doc (`src/cache.rs:9-11`) says an entry whose first eight bytes are not a block header is "a single
  unframed block spanning the whole entry". It states no maximum. The 24-bit field belongs to the *block
  header*, and an unframed block has no header, so the field says nothing about its size.
- The repository's own ceiling for a decompressed entry is `MAX_ENTRY_LEN = 1 << 30` (`src/cache.rs:30`, enforced in
  `Package::read`). An unframed Oodle entry between 16 MiB and 1 GiB is accepted on `main` and now fails with
  "cannot come out of an unframed block". CLOUD-REPORT.md says the same under "Uncertain". The rules say a bound
  the docs do not justify must be flagged and that nothing may be invented, so this is a blocker.
- Failing scenario: a `.toc` entry with `len = 20_000_000`, `comp_len = 6_000_000`, first byte `0x8C`, with no block
  header. `main` decodes it; the branch returns `Err`.
- The allocation in that case is `len` bytes for an Oodle stream, which can in fact be large relative to the input.
  The Oodle ratio is not documented, so there is no input-derived bound that the docs support.
- Minimal fix: drop the `MAX_BLOCK_LEN` branch and use the documented ceiling for Oodle, `len <= MAX_ENTRY_LEN as usize`.
  Keep the LZF bound (see note N1). The test then asserts the allocation for an Oodle stub against `MAX_ENTRY_LEN`,
  or documents that for Oodle the declared size is bounded only by `MAX_ENTRY_LEN` (1 GiB for a 5-byte entry).
  If 1 GiB of zeroed memory from 5 bytes is not acceptable, the owner has to state a real Oodle bound in the
  docs first; the reviewer could not derive one.

### B2. The `w <= 0xFFFF` check in `read_radiance_hdr` is not tested (mutation P1 survives)
`src/probe.rs:285-289`, tests `tests/hardening.rs:229-250` (`radiance_*`).

- With the check removed (`true || w <= 0xFFFF`) every test passes. The existing test dies earlier (the size line
  is followed by text, not a `02 02 hh ll` marker), so it never reaches the code that the check protects.
- The check does matter: the marker comparison truncates `w` with `as u8`, so `w = 4294967304` (`0x1_0000_0008`) matches the
  marker `02 02 00 08`, and `vec![[0u8; 4]; w]` then asks for 17 GB. I confirmed this on the mutated copy: the process aborts with
  `memory allocation of 17179869216 bytes failed` from a 40-byte input. On the branch as it stands the check turns it into an `Err`.
- Minimal fix (test): add
  `b"\n\n-Y 1 +X 4294967304\n\x02\x02\x00\x08"` followed by 8 bytes, assert `is_err()` and that `largest_allocation` is below `SMALL`.

### B3. Other fix lines whose removal no test notices (the "every new test fails without its fix" check)
Mutations that survived (details in the table below):

- `src/probe.rs:306-312`/`:314-316` (P4): the literal-run check `c <= w - x`. Without it a literal longer than the scanline
  indexes `scan[x + i]` out of bounds and panics. The test inputs for literals are all shorter than the data that follows
  (`[9,1,2,3]`, `[4,1,2]`, `[100]`), so they fail in `get(pos..pos+c)` first. Needed: scanline `w=4` and a literal of 100 followed by 100 bytes.
- `src/probe.rs:314-318` (P6): the `get(pos..pos + c)` bound, restored to a padded read. No test needs it.
- `src/cache.rs:187-190` (C1): `dec <= len - written`. With `dec <= len` a second block can push the output past `len`
  and `decompress` returns more than `len` bytes without an error. Needed: two blocks whose sizes add up to more than `len`, assert `Err`.
- `src/cache.rs:150` and the "best ratio" test (C2): `lzf_at_its_best_ratio_is_still_accepted` uses 5 input bytes for 265 output bytes
  (ratio 53), so `LZF_MAX_RATIO = 87` also passes. The bound is exactly right (a 264-byte copy takes 3 bytes, see the
  module doc of `lzf_decompress`), but the test does not pin it. Needed: one literal plus 100 copies of `[7<<5, 255, 0]` (300002 bytes in, 26 400 001 out;
  I ran it on the branch: accepted in 71 ms) and the same with one input byte removed.
- `src/cache.rs:146` (C7) and `src/toc.rs:35` (T2, T4): boundary values (`0xFF_FFFF` accepted / `0x100_0000` rejected; a path of exactly 4096
  bytes accepted / 4097 rejected; the limit applied to files as well as directories) have no test.
- Why this is a blocker and not a note: the task requires each new test to fail without its fix, and the CLOUD-REPORT says every test was checked.
  Fix B2 and the C1/P4 cases at least; the boundary tests (C2, C7, T2, T4) are cheap and should go in the same commit.

### B4. `CLOUD-REPORT.md` is shipped in the published crate
`Cargo.toml:8-9` excludes only `/fuzz`. `cargo package -p wf-unpack --list` goes from 63 lines on `main` to 65 on the branch:
the extra lines are `CLOUD-REPORT.md` and `tests/hardening.rs`. `tests/hardening.rs` is an ordinary test and fine to publish; a process report is not
part of the library. CLOUD-REPORT.md also says `cargo package --list` "shows no `fuzz/` file" without saying that it now shows itself.
Fix: `exclude = ["/fuzz", "/CLOUD-REPORT.md"]`, or move the report out of the crate root (for example under `docs/reviews/`, which is
where the owner now keeps reviews) and keep it out of the package.

### B5. The committed corpus is not small
`fuzz/corpus/` holds 7 679 files, 2 285 929 bytes (about 2.2 MiB; the report says "7.5k"). `fuzz cmin` over the committed files of three
targets barely reduces it (entity_text 1291 to 1283, cache_decompress 702 to 660, glob 525 to 508), so the set is
minimal in libFuzzer's feature sense but that sense keeps thousands of near-identical inputs. The contents are synthetic: I
searched them for game strings; the only paths are `/Lotus/...` names that already appear in the repository's docs and tests (`/Lotus/x`,
`/Lotus/a.png`, ...), and `gen_seeds.py` reads nothing but its own output and the temporary test dump. The largest file is 23 KB.
A source repository does not need this. Fix: commit only the seeds produced by `fuzz/gen_seeds.py` (the script is already there and
is deterministic) and the five `crash-*` inputs, and let each fuzz run grow its own corpus (`/fuzz/corpus` can be listed in `.gitignore`
apart from `seed-*` and `crash-*`). Roughly 100 files. The `crash-radiance_hdr` file is 49 bytes, the report says the artifact was 59.

## Per-fix assessment (item 1)

| Fix | Whole class of input? | Justified by docs? |
| --- | --- | --- |
| `cache::decompress` grows the output block by block | Yes: framed blocks (each at most 16 MiB, constant per call, then error), stored, unframed LZF. I ran 1000 hostile 16 MiB headers with 1-byte payloads: 11 ms and an error; 200 000 empty blocks: 1.6 ms; 200 000 one-byte stored blocks: 1.8 ms. | Block size 24 bits: yes (module doc). LZF ratio 88: yes, derived from the documented LZF encoding. Oodle 0xFF_FFFF: **no**, see B1. |
| `glob_match` as a reachable-set pass | Yes. Exhaustively equal to the old matcher on patterns over `a/*?` up to 5 characters and paths over `a/b` up to 5 (the test), and I mutated each rule (G1 to G5, all caught by `toc::tests::globs`). `**`, `*`, `?` and bytes are handled as before, including non-ASCII (byte-wise as before). Time is P x S: 64 KB pattern x 4 KB path takes 0.37 s, 64 KB x 64 KB takes 2.8 s (release). | Yes: it only reproduces the documented glob semantics. |
| `read_radiance_hdr` | Panics on `data[pos]`, `scan[x+i]` and the capacity are gone; `x <= w` is kept as an invariant so `w - x` cannot underflow; `w = 0` with any `h` costs 4 bytes of input per row. Amplification remains inherent to the format: a 65 535-wide scanline of 127-pixel runs is about 4 KB of input for 786 KB of output (about 190x). The function is documented "for tests". | The 16-bit width follows from the marker the function itself checks. No docs needed, but untested, see B2. |
| `Toc::parse` path limit 4096 | Memory is now at most entries x 4096; before it was quadratic in the chain depth. Applies to files as well as directories. | **Not justified by the toc docs or `docs/`.** The name field is 64 bytes (`src/toc.rs:11-18`), so a limit follows from no layout fact. The only support is that `animation.rs` and `animtree.rs` use 4096 for their strings. Kept as a non-blocking note (N2) because a bound is unavoidable and 4096 is far above real paths; the owner should confirm the number. |

## Non-blocking notes

- N1. `decompress` is `pub` but only `Package::read` enforces `MAX_ENTRY_LEN`. A direct caller can still pass `len = usize::MAX`
  with a framed Oodle block of 16 MiB: the cost is bounded by block size per iteration and input length, not by `len`, so there is no
  unbounded allocation, but a `len > MAX_ENTRY_LEN` check inside `decompress` would be cheap and matches the doc comment on the constant.
- N2. See the path limit above. Also each directory still keeps its full path, so the worst case is entries x 4096 bytes (about 43x the file size).
- N3. `model_vertex` hides a real panic: `decode_vertex`, `decode_float_vertex` and `decode_skinned_vertex` are public, return values, not
  `Result`, and panic on a slice shorter than one record. The target pads its input to 32 bytes, so it can never see it. The report discloses
  this and the docs say "one 24-byte vertex", so it is a documented precondition, but the harness does hide it. A `Result` or a `debug_assert` with a message would be better.
- N4. Harness quality: no `catch_unwind`, no `panic = "abort"` override, results are dropped with `let _ =` (errors are expected, panics are not swallowed). Good.
  The target set matches the report: 18 targets. `packages_bin` cannot reach the zstd text path (disclosed), and `TextReader::text` is 95% covered only by failing frames.
- N5. The fuzz crate is not built by CI, so it can rot silently. A `cargo +nightly fuzz build` job (or `cargo check` in `fuzz/`) would keep it honest.
- N6. `radiance_valid_scanline_still_reads` has a comment that is hard to follow ("130 - 128 = 2 would be too short").
- N7. No MSRV (`rust-version`) is declared in `Cargo.toml` and the code on `main` already uses `is_multiple_of` and `as_chunks`
  (recent stable); the CI uses `dtolnay/rust-toolchain@stable`. The branch adds nothing newer. There is no Java in this repository (the Java check does not apply).

## CLOUD-REPORT.md versus the diff (item 7)

Correct: 18 targets; fixes 1 to 4 and their locations; the `exclude`; the `tests/hardening.rs` tests; the left-out list; the clippy warning at
`src/player.rs:680` (present on `main`, I reproduced it); 4096 and 0xFF_FFFF being conventions/readings, not game facts.
Inaccurate or missing:
- "Each was checked to fail with the fix reverted": true for the whole-file reverts (R1 to R3 below), not for every fix line (B2, B3).
- "cargo package --list shows no fuzz/ file" omits that the crate now also lists `CLOUD-REPORT.md` and `tests/hardening.rs` (B4).
- "about 2.2 MB, 7.5k files": 7 679 files (B5). The radiance artifact is described as 59 bytes; the file is 49 bytes.
- The fuzz-time table (30 minutes per target, the 64 KB runs, the pre-fix runs) cannot be checked from the diff; I ran 10 minutes per target myself (below) and found nothing.
- "Coverage was not measured": true; I measured it (below).

## Mutations made (scratch copy `wt2`, `cargo test -p wf-unpack` after each; "caught" means a test failed)

| id | mutation | result |
| --- | --- | --- |
| R1 | revert `src/cache.rs` to `main` | caught: `cache::declared_size_does_not_drive_the_allocation` |
| R2 | revert `src/probe.rs` to `main` | caught: `probe::radiance_malformed_runs_are_errors`, `probe::radiance_size_line_does_not_drive_the_allocation` |
| R3 | revert `src/toc.rs` to `main` | caught: `toc::a_chain_of_nested_directories_is_an_error_not_quadratic_memory`, `glob::many_wildcards_do_not_take_exponential_time` |
| C1 | `dec <= len - written` to `dec <= len` | **survived** |
| C2 | `LZF_MAX_RATIO` 88 to 87 | **survived** |
| C3 | Oodle unframed limit to `usize::MAX` | caught (declared_size test) |
| C4 | LZF unframed limit to `usize::MAX` | caught (declared_size test) |
| C5 | unframed limit check disabled | caught (declared_size test) |
| C6 | `Vec::new()` back to `vec![0; len]` | caught (declared_size test) |
| C7 | `MAX_BLOCK_LEN` `0xFF_FFFF` to `0x100_0000` | **survived** |
| P1 | `w <= 0xFFFF` disabled | **survived** (confirmed 17 GB abort with a crafted 40-byte input, B2) |
| P2 | `with_capacity(w * h)` back | caught (process aborted: allocation of the test input's capacity) |
| P3 | repeat-run check `run <= w - x` disabled | caught (`radiance_malformed_runs_are_errors`) |
| P4 | literal-run check `c <= w - x` disabled | **survived** |
| P5 | `data[pos]` unchecked again | caught (`radiance_malformed_runs_are_errors`) |
| P6 | literal slice bound replaced by a padded read | **survived** |
| T1 | path limit disabled | caught (nested directories test) |
| T2 | `<=` to `<` | **survived** (boundary) |
| T3 | limit 4096 to 1 MiB | caught (nested directories test) |
| T4 | limit ignores the name length | **survived** |
| G1 | `**` no longer keeps earlier positions | caught: `toc::tests::globs` |
| G2 | `*` may cross `/` | caught: `toc::tests::globs` |
| G3 | `?` matches `/` | caught: `toc::tests::globs` |
| G4 | answer `reach[0]` instead of `reach[len]` | caught: `toc::tests::globs` |
| G5 | `**` treated as `*` | caught: `toc::tests::globs` |

The G mutations are caught by the existing unit test `toc::tests::globs`, not only by `tests/hardening.rs`.

## Tests, fuzzing, coverage (items 3, 4, 5)

- Branch checked out in a scratch worktree. `cargo build -p wf-unpack --release`, `cargo test -p wf-unpack` (138 + 26 + 9 tests, no failures),
  `cargo test -p wf-unpack --features bridge` (159 + 39 + ...), `cargo test -p wf-unpack --release --lib bc6h`, `cargo fmt --all -- --check` and
  `python3 tools/check_public_tree.py --range origin/main..HEAD` all pass. `cargo clippy -p wf-unpack --all-targets`: the one `player.rs:680` warning, also on `main`.
  `cargo build -p wf-viewer` (macOS job) was not run: the container is Linux.
- Fuzzing: nightly, ASan (cargo-fuzz default), overflow checks and debug assertions (the fuzz crate's release profile),
  `-max_total_time=600 -rss_limit_mb=2048 -timeout=10 -max_len=8192`, four targets at a time on 4 cores, starting from a copy of the committed corpus.
  All 18 targets exited 0 after 600 s with no crash, timeout or OOM (runs per target between 0.2 M and 17.5 M).
  I did not run longer inputs (`-max_len` above 8192) or UBSan separately (Rust has no UB sanitizer beyond ASan on safe code; the library has one `unsafe` block, the macOS `fcntl`).
- Coverage of the committed corpus (`cargo +nightly fuzz coverage`, line coverage by function): `cache::decompress` 35/35, `lzf_decompress` 32/32, `read_radiance_hdr` 59/59,
  `glob_match` 29/29, `clip_filter_matches` 3/3, `Toc::parse` 51/51, `RawLevel::parse` 62/62, `StaticLevel::parse` 55/78, `ModelHeader::parse` 168/169, `Model::from_parts` 97/100,
  `AnimHeader::parse` 90/90, `animation::decode` 119/137, `TextureHeader::parse` 33/33, `parse_triangle_mesh` 29/29, `PackagesBin::parse` 84/85, `TextReader::text` 21/22,
  `Attributes::parse` 7/7, `CommonHeader::parse` 24/24, `decode_bc6h_block` 8/8, `parse_properties` 16/16, the three vertex decoders 11/11, 15/15, 17/18.
  The harnesses reach the parsers.
- Extra hostile-input probes (release, scratch test file, not committed): glob 64 KB x 64 KB 2.8 s; 1000 framed 16 MiB headers 11 ms; 200 000 empty blocks 1.6 ms;
  unframed Oodle of 5 bytes declaring 16 MiB 13 ms; maximum-ratio LZF 71 ms; 17 GB radiance width (only with P1) aborts.

## Commands run

```
git fetch origin claude/fuzz-wf-unpack-d45jqe main
git diff --stat origin/main...origin/claude/fuzz-wf-unpack-d45jqe
git worktree add -f <scratch>/wt origin/claude/fuzz-wf-unpack-d45jqe      # and wt2 for mutations
rustup toolchain install nightly --profile minimal -c rust-src ; cargo install cargo-fuzz
rustup component add llvm-tools-preview --toolchain nightly
cargo build -p wf-unpack --release
cargo test -p wf-unpack ; cargo test -p wf-unpack --features bridge ; cargo test -p wf-unpack --release --lib bc6h
cargo fmt --all -- --check ; cargo clippy -p wf-unpack --all-targets
python3 tools/check_public_tree.py --range origin/main..HEAD
cargo package -p wf-unpack --list --allow-dirty        # on main and on the branch, diffed
cd fuzz && cargo +nightly fuzz build
cargo +nightly fuzz run <target> <corpus copy> -- -max_total_time=600 -rss_limit_mb=2048 -timeout=10 -max_len=8192   # all 18
cargo +nightly fuzz coverage <target> corpus/<target> ; llvm-cov report --show-functions (nightly llvm-cov)
cargo +nightly fuzz cmin <target> <corpus copy>         # entity_text, cache_decompress, glob
python3 mut.py   # the 25 mutations above, each followed by cargo test -p wf-unpack
cargo test --release --test extra -- --nocapture         # scratch hostile-input probes
```
