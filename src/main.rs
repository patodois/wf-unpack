use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use wf_unpack::cache::Package;
use wf_unpack::gltf;
use wf_unpack::header::CommonHeader;
use wf_unpack::model::{Model, ModelHeader, KIND_STATIC};
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
    /// Read a static model; print its layout, and export it to glTF with -o
    Model {
        /// Path inside the packages, e.g. /Lotus/Weapons/Tenno/Melee/Swords/PrimeGalatine/PrimeGalatine.fbx
        path: String,
        /// Output directory for <name>.gltf + <name>.bin
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Level of detail, 0 (finest) to 4
        #[arg(long, default_value_t = 0)]
        lod: usize,
        /// Attach a game texture to every mesh, as base=PATH, normal=PATH or emissive=PATH
        /// (materials are not resolved automatically yet)
        #[arg(long = "texture")]
        textures: Vec<String>,
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
    },
    /// Parse every static model (type 102) in H.Misc with its B./F. parts and summarise failures
    CheckModels {
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
    },
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
        Command::CheckModels { game } => check_models_cmd(&expand_home(&game)),
        Command::Model {
            path,
            out,
            lod,
            textures,
            game,
        } => model_cmd(&expand_home(&game), &path, out.as_deref(), lod, &textures),
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

/// Share of triangles whose winding agrees with their vertex normals.
fn winding_agreement(model: &Model, tris: &[u32]) -> f64 {
    let (mut agree, mut total) = (0usize, 0usize);
    for t in tris.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| model.vertices[i as usize]);
        let e1: [f32; 3] = std::array::from_fn(|k| b.position[k] - a.position[k]);
        let e2: [f32; 3] = std::array::from_fn(|k| c.position[k] - a.position[k]);
        let face = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let n: [f32; 3] = std::array::from_fn(|k| a.normal[k] + b.normal[k] + c.normal[k]);
        let dot = face[0] * n[0] + face[1] * n[1] + face[2] * n[2];
        if dot != 0.0 {
            total += 1;
            agree += usize::from(dot > 0.0);
        }
    }
    agree as f64 / total.max(1) as f64
}

fn model_cmd(
    game: &Path,
    path: &str,
    out: Option<&Path>,
    lod: usize,
    textures: &[String],
) -> Result<()> {
    let read = |name: &str| -> Result<Vec<u8>> {
        Ok(match Package::open_in(game, name)? {
            Some(mut pkg) => pkg.read_path(path)?.unwrap_or_default(),
            None => Vec::new(),
        })
    };
    let raw = read("H.Misc")?;
    anyhow::ensure!(!raw.is_empty(), "{path} is not in H.Misc");
    let header = ModelHeader::parse(&raw)?;
    let (body, full) = (read("B.Misc")?, read("F.Misc")?);
    eprintln!(
        "header: {} vertices, {} indices, {} morphs, {} bones, {} PhysX shapes {:?}; parts: B. {} bytes, F. {} bytes",
        header.vertex_count,
        header.index_count,
        header.morph_count,
        header.bone_count,
        header.physx_lens.len(),
        header.physx_lens,
        body.len(),
        full.len()
    );
    let model = Model::from_parts(header, &body, &full)?;
    let h = &model.header;
    println!(
        "{path}: {} vertices ({} in B.), {} indices ({} in B.), scale {:?}",
        h.vertex_count,
        h.body_vertex_count,
        h.index_count,
        h.body_index_count,
        h.scale()
    );
    let (lo, hi) = model.bounds();
    println!(
        "bounds: header {:?}..{:?}, decoded {lo:?}..{hi:?}",
        &h.bounds_min[..3],
        &h.bounds_max[..3]
    );
    println!("materials: {:?}", h.materials());
    for (i, m) in h.meshes.iter().enumerate() {
        let tris = model.lod_indices(i, 0)?;
        println!(
            "mesh {i} {:?}: LOD triangles {:?}, LOD0 winding agrees with normals for {:.1}%",
            m.name,
            m.lod_counts.map(|c| c / 3),
            100.0 * winding_agreement(&model, tris)
        );
    }
    let Some(out) = out else { return Ok(()) };
    let name = Path::new(path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut images = Vec::new();
    for spec in textures {
        let (slot, tex_path) = spec
            .split_once('=')
            .with_context(|| format!("--texture {spec}: expected SLOT=PATH"))?;
        let slot = match slot {
            "base" => gltf::Slot::BaseColor,
            "normal" => gltf::Slot::Normal,
            "emissive" => gltf::Slot::Emissive,
            _ => anyhow::bail!("unknown texture slot {slot} (base, normal or emissive)"),
        };
        let tex = Texture::load(game, tex_path)?;
        let file = format!(
            "{}.png",
            Path::new(tex_path)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
        );
        fs::create_dir_all(out)?;
        fs::write(out.join(&file), tex.to_png()?)?;
        images.push((slot, file));
    }
    let mut materials: Vec<gltf::Material> = h
        .materials()
        .into_iter()
        .map(|name| gltf::Material {
            name,
            images: images.clone(),
        })
        .collect();
    // One material per mesh, named after the attribute list when it has one
    materials.resize_with(h.meshes.len(), || gltf::Material {
        name: "material".into(),
        images: images.clone(),
    });
    gltf::write(
        &model,
        &name,
        &materials,
        &gltf::Options {
            lod,
            mirror_z: true,
        },
        out,
    )?;
    let tris: usize = (0..h.meshes.len())
        .map(|m| model.lod_indices(m, lod).map_or(0, |t| t.len() / 3))
        .sum();
    eprintln!(
        "wrote {} with {tris} triangles (LOD {lod}) and {} vertices",
        out.join(format!("{name}.gltf")).display(),
        model.vertices.len()
    );
    Ok(())
}

fn check_models_cmd(game: &Path) -> Result<()> {
    let started = Instant::now();
    let open = |name: &str| -> Result<Package> {
        let pkg = Package::open(&game.join(format!("{name}.toc")))?;
        pkg.set_nocache();
        Ok(pkg)
    };
    let (mut head, mut body, mut full) = (open("H.Misc")?, open("B.Misc")?, open("F.Misc")?);
    // A path can appear twice in a .toc; the later entry is the live one
    let mut entries: Vec<_> = head
        .toc
        .entries
        .iter()
        .filter(|e| std::ptr::eq(head.toc.get(&e.path).unwrap(), *e))
        .cloned()
        .collect();
    let duplicates = head.toc.entries.len() - entries.len();
    entries.sort_by_key(|e| e.offset);

    let mut fbx_kinds: BTreeMap<u16, usize> = BTreeMap::new();
    let mut errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut ok, mut vertices, mut triangles) = (0usize, 0u64, 0u64);
    for e in &entries {
        let Ok(raw) = head.read(e) else { continue };
        let Ok(common) = CommonHeader::parse(&raw) else {
            continue;
        };
        if common.kind != KIND_STATIC {
            if e.path.ends_with(".fbx") {
                *fbx_kinds.entry(common.kind).or_default() += 1;
            }
            continue;
        }
        let result = ModelHeader::parse(&raw).and_then(|header| {
            let b = body.read_path(&e.path)?.unwrap_or_default();
            let f = full.read_path(&e.path)?.unwrap_or_default();
            let model = Model::from_parts(header, &b, &f)?;
            model.check_bounds()?;
            Ok(model)
        });
        match result {
            Ok(model) => {
                ok += 1;
                vertices += model.vertices.len() as u64;
                triangles += (0..model.header.meshes.len())
                    .map(|m| model.lod_indices(m, 0).map_or(0, |t| t.len() as u64 / 3))
                    .sum::<u64>();
            }
            Err(err) => {
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
        "type {KIND_STATIC}: {ok}/{} ok, {failed} failed ({vertices} vertices, {triangles} LOD0 triangles) in {:.0}s; {duplicates} duplicate H.Misc paths skipped",
        ok + failed,
        started.elapsed().as_secs_f64()
    );
    for (msg, list) in &errors {
        println!(
            "  {:>6}  {msg}\n          e.g. {}",
            list.len(),
            list.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        );
    }
    println!("other .fbx types (not supported): {fbx_kinds:?}");
    Ok(())
}
