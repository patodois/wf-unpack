use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use wf_unpack::cache::Package;
use wf_unpack::header::CommonHeader;
use wf_unpack::texture::{Texture, TextureHeader};
use wf_unpack::toc::glob_match;

#[derive(Parser)]
#[command(version, about = "Read Warframe .toc/.cache packages")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the files of a package
    List {
        toc: PathBuf,
        /// Only list paths containing this text, or matching it when it has * or ?
        #[arg(long)]
        filter: Option<String>,
    },
    /// Extract files whose path matches (`*` within a folder, `**` across folders)
    Extract {
        toc: PathBuf,
        pattern: String,
        /// Output directory
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Decompress every entry of every package in a Cache.Windows folder and summarise failures
    Check {
        dir: PathBuf,
        /// Only check packages whose name contains this text
        #[arg(long)]
        only: Option<String>,
    },
    /// Convert one texture to DDS (all mips) or PNG (top mip), chosen by the output extension
    Texture {
        /// Path inside the packages, e.g. /Lotus/Interface/Icons/Store/Foo.png
        path: String,
        #[arg(short, long)]
        out: PathBuf,
        /// The Cache.Windows folder
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
    },
    /// Count the asset types in an H. package (by the common header), with an example path for each
    Kinds { toc: PathBuf },
    /// Parse every texture header and check its mip chain against the B./F. sizes (reads only H.Texture)
    Textures {
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
    },
}

const DEFAULT_GAME_DIR: &str = "~/Games/Warframe/Cache.Windows";

/// Expands a leading `~/` (clap does not go through a shell for defaults).
fn expand_home(p: &Path) -> PathBuf {
    match (p.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => p.to_path_buf(),
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::List { toc, filter } => list(&toc, filter.as_deref()),
        Command::Extract { toc, pattern, out } => extract(&toc, &pattern, &out),
        Command::Check { dir, only } => check(&dir, only.as_deref()),
        Command::Texture { path, out, game } => texture_cmd(&expand_home(&game), &path, &out),
        Command::Kinds { toc } => kinds_cmd(&toc),
        Command::Textures { game } => textures_cmd(&expand_home(&game)),
    }
}

fn matches(filter: &str, path: &str) -> bool {
    if filter.contains(['*', '?']) {
        glob_match(filter, path)
    } else {
        path.contains(filter)
    }
}

fn list(path: &Path, filter: Option<&str>) -> Result<()> {
    use std::io::Write;
    let pkg = Package::open(path)?;
    let mut out = std::io::stdout().lock();
    let result = (|| -> std::io::Result<()> {
        writeln!(
            out,
            "{}: {} files in {} directories",
            pkg.name,
            pkg.toc.entries.len(),
            pkg.toc.dir_count
        )?;
        for e in &pkg.toc.entries {
            if filter.is_some_and(|f| !matches(f, &e.path)) {
                continue;
            }
            writeln!(out, "{:>10} {:>10}  {}", e.len, e.comp_len, e.path)?;
        }
        Ok(())
    })();
    match result {
        // `| head` closing the pipe is not an error
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

/// Rejects `..` and other odd components so entry paths cannot escape the output dir.
fn safe_join(base: &Path, name: &str) -> Option<PathBuf> {
    let rel = Path::new(name.trim_start_matches('/'));
    rel.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then(|| base.join(rel))
}

fn extract(path: &Path, pattern: &str, out: &Path) -> Result<()> {
    let mut pkg = Package::open(path)?;
    let selected: Vec<_> = pkg
        .toc
        .entries
        .iter()
        .filter(|e| matches(pattern, &e.path))
        .cloned()
        .collect();
    let (mut written, mut failed) = (0, 0);
    for e in &selected {
        let Some(dest) = safe_join(out, &e.path) else {
            eprintln!("skipping unsafe path {:?}", e.path);
            continue;
        };
        match pkg.read(e) {
            Ok(data) => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&dest, data).with_context(|| format!("writing {}", dest.display()))?;
                written += 1;
            }
            Err(err) => {
                eprintln!("{}: {err:#}", e.path);
                failed += 1;
            }
        }
    }
    eprintln!(
        "wrote {written} files to {} ({failed} failed)",
        out.display()
    );
    Ok(())
}

fn check(dir: &Path, only: Option<&str>) -> Result<()> {
    let mut tocs: Vec<_> = fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toc"))
        .collect();
    tocs.sort();
    let started = Instant::now();
    let (mut total_ok, mut total_failed, mut total_bytes, mut largest) =
        (0usize, 0usize, 0u64, (0u32, String::new()));
    for toc in tocs {
        let name = toc.file_stem().unwrap().to_string_lossy().into_owned();
        if only.is_some_and(|o| !name.contains(o)) {
            continue;
        }
        let pkg_started = Instant::now();
        let mut pkg = match Package::open(&toc) {
            Ok(p) => p,
            Err(e) => {
                println!("{name}: cannot open: {e:#}");
                continue;
            }
        };
        pkg.set_nocache();
        // Read in .cache order so the disk sees one forward pass
        let mut entries = pkg.toc.entries.clone();
        entries.sort_by_key(|e| e.offset);
        let (mut ok, mut stored, mut bytes) = (0usize, 0usize, 0u64);
        let mut errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for e in &entries {
            match pkg.read(e) {
                Ok(data) => {
                    ok += 1;
                    stored += usize::from(!e.is_compressed());
                    bytes += data.len() as u64;
                    if e.len > largest.0 {
                        largest = (e.len, format!("{name}:{}", e.path));
                    }
                }
                Err(err) => {
                    // Group by message without numbers so similar failures land together
                    let key: String = format!("{err:#}")
                        .chars()
                        .map(|c| if c.is_ascii_digit() { '#' } else { c })
                        .collect();
                    errors.entry(key).or_default().push(e.path.clone());
                }
            }
        }
        let failed: usize = errors.values().map(Vec::len).sum();
        println!(
            "{name}: {ok}/{} ok ({stored} stored), {failed} failed, {:.1} MB out, {:.1}s",
            entries.len(),
            bytes as f64 / 1e6,
            pkg_started.elapsed().as_secs_f64()
        );
        for (msg, list) in &errors {
            println!(
                "  {:>6}  {msg}\n          e.g. {}",
                list.len(),
                list.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            );
        }
        total_ok += ok;
        total_failed += failed;
        total_bytes += bytes;
    }
    println!(
        "total: {total_ok} ok, {total_failed} failed, {:.2} GB decompressed in {:.0}s; largest entry {} bytes ({})",
        total_bytes as f64 / 1e9,
        started.elapsed().as_secs_f64(),
        largest.0,
        largest.1
    );
    Ok(())
}

fn texture_cmd(game: &Path, path: &str, out: &Path) -> Result<()> {
    let tex = Texture::load(game, path)?;
    let png = out
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("png"));
    let data = if png { tex.to_png()? } else { tex.to_dds() };
    fs::write(out, data).with_context(|| format!("writing {}", out.display()))?;
    let h = &tex.header;
    eprintln!(
        "{path}: type {} format {:?} (code {}), header {}x{}, wrote {}x{} with {} mips to {}",
        h.common.kind,
        tex.format,
        h.format_code,
        h.width,
        h.height,
        tex.width,
        tex.height,
        if png { 1 } else { tex.mips.len() },
        out.display()
    );
    Ok(())
}

fn textures_cmd(game: &Path) -> Result<()> {
    let mut header_pkg = Package::open(&game.join("H.Texture.toc"))?;
    let body = Package::open(&game.join("B.Texture.toc"))?;
    let full = Package::open(&game.join("F.Texture.toc"))?;
    let size = |pkg: &Package, path: &str| pkg.toc.get(path).map_or(0, |e| e.len as usize);
    let mut stats: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: BTreeMap<String, String> = BTreeMap::new();
    let entries = header_pkg.toc.entries.clone();
    for e in &entries {
        let outcome = header_pkg
            .read(e)
            .and_then(|raw| TextureHeader::parse(&raw))
            .and_then(|h| {
                let layout = h.layout(size(&body, &e.path), size(&full, &e.path))?;
                Ok((h, layout.first))
            });
        let key = match outcome {
            Ok((h, first)) => format!(
                "ok   type {:>3} format {:>2}{}",
                h.common.kind,
                h.format_code,
                if first > 0 {
                    format!(" (top {first} levels not shipped)")
                } else {
                    String::new()
                }
            ),
            Err(err) => {
                let msg: String = format!("{err:#}")
                    .chars()
                    .map(|c| if c.is_ascii_digit() { '#' } else { c })
                    .collect();
                format!("FAIL {}", msg.chars().take(100).collect::<String>())
            }
        };
        examples
            .entry(key.clone())
            .or_insert_with(|| e.path.clone());
        *stats.entry(key).or_default() += 1;
    }
    for (key, n) in &stats {
        println!("{n:>7}  {key}   e.g. {}", examples[key]);
    }
    Ok(())
}

fn kinds_cmd(path: &Path) -> Result<()> {
    let mut pkg = Package::open(path)?;
    let mut kinds: BTreeMap<u16, (usize, String)> = BTreeMap::new();
    let mut failed = 0;
    let entries = pkg.toc.entries.clone();
    for e in &entries {
        match pkg.read(e).and_then(|raw| CommonHeader::parse(&raw)) {
            Ok(h) => kinds.entry(h.kind).or_insert_with(|| (0, e.path.clone())).0 += 1,
            Err(_) => failed += 1,
        }
    }
    for (kind, (n, example)) in &kinds {
        println!("{kind:>5} {n:>7}  e.g. {example}");
    }
    println!("{failed} entries without a readable common header");
    Ok(())
}
