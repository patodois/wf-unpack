use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod level_cmd;

use wf_unpack::cache::Package;
use wf_unpack::gltf;
use wf_unpack::header::CommonHeader;
use wf_unpack::material::{self, MaskLayout, Material, Plan, Source};
use wf_unpack::model::{Model, ModelHeader, KIND_SKINNED, KIND_STATIC};
use wf_unpack::packages::PackagesBin;
use wf_unpack::pbr;
use wf_unpack::texture::TexturePackages;
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
        /// Package family holding the texture: Texture, or LightMap for level lightmaps
        #[arg(long, default_value = "Texture")]
        package: String,
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
    /// Print the Packages.bin text of an entity and of each entity it derives from
    Props {
        path: String,
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
    },
    /// List Packages.bin entities whose text contains a string
    FindText {
        needle: String,
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
    },
    /// Decode the text of every Packages.bin entity and summarise failures
    CheckPackages {
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
    },
    /// Resolve the materials of every static model and report which texture slots they get
    CheckMaterials {
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
        /// Also count every TX:/flag key seen (for studying the format)
        #[arg(long)]
        survey: bool,
    },
    /// Load every static and skinned model through the rewrite-bridge provider and validate it
    #[cfg(feature = "bridge")]
    CheckBridge {
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
        /// Also bake every material's textures (much slower)
        #[arg(long)]
        textures: bool,
        /// Only models whose path contains this text
        #[arg(long, default_value = "")]
        filter: String,
        /// Stop after this many models
        #[arg(long)]
        limit: Option<usize>,
    },
    /// List what a level places (models, lights, volumes, spawns); with -o, export it to glTF
    Level {
        /// Path inside the packages, e.g. /Lotus/Levels/ClanDojo/ClanObstacleCourseA.level
        path: String,
        /// Print every light, spawn and volume and the full model list
        #[arg(short, long)]
        verbose: bool,
        /// Output directory for <name>.gltf, <name>.bin and the baked PNGs
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Level of detail of every model, 0 (finest) to 4
        #[arg(long, default_value_t = 0)]
        lod: usize,
        /// Also place objects that start hidden
        #[arg(long)]
        include_hidden: bool,
        /// Skip texture baking: flat material colours only
        #[arg(long)]
        no_textures: bool,
        /// Shrink baked textures to at most this many pixels on a side
        #[arg(long)]
        max_texture: Option<u32>,
        #[arg(long, default_value = DEFAULT_GAME_DIR)]
        game: PathBuf,
    },
    /// Parse every level and summarise failures, with the peak memory used
    CheckLevels {
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
    match (
        p.strip_prefix("~"),
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
    ) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => p.to_path_buf(),
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::List { toc, filter } => list(&toc, filter.as_deref()),
        Command::Extract { toc, pattern, out } => extract(&toc, &pattern, &out),
        Command::Check { dir, only } => check(&dir, only.as_deref()),
        Command::Texture {
            path,
            out,
            game,
            package,
        } => texture_cmd(&expand_home(&game), &path, &out, &package),
        Command::Kinds { toc } => kinds_cmd(&toc),
        #[cfg(feature = "bridge")]
        Command::CheckBridge {
            game,
            textures,
            filter,
            limit,
        } => check_bridge_cmd(&expand_home(&game), textures, &filter, limit),
        Command::CheckMaterials { game, survey } => {
            check_materials_cmd(&expand_home(&game), survey)
        }
        Command::Props { path, game } => props_cmd(&expand_home(&game), &path),
        Command::FindText { needle, game } => find_text_cmd(&expand_home(&game), &needle),
        Command::CheckPackages { game } => check_packages_cmd(&expand_home(&game)),
        Command::CheckModels { game } => check_models_cmd(&expand_home(&game)),
        Command::Model {
            path,
            out,
            lod,
            textures,
            game,
        } => model_cmd(&expand_home(&game), &path, out.as_deref(), lod, &textures),
        Command::Level {
            path,
            verbose,
            out,
            lod,
            include_hidden,
            no_textures,
            max_texture,
            game,
        } => level_cmd::level_cmd(
            &expand_home(&game),
            &path,
            verbose,
            out.as_deref(),
            &wf_unpack::level_export::ExportOptions {
                lod,
                include_hidden,
                textures: !no_textures,
                max_texture,
            },
        ),
        Command::CheckLevels { game } => level_cmd::check_levels(&expand_home(&game)),
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

fn texture_cmd(game: &Path, path: &str, out: &Path, package: &str) -> Result<()> {
    let tex = TexturePackages::open_family(game, package)?.load(path)?;
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

/// Attaches the given textures to every mesh (the `--texture` override).
fn manual_materials(
    game: &Path,
    textures: &[String],
    names: &[String],
    meshes: usize,
    out: &Path,
) -> Result<Vec<gltf::Material>> {
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
    Ok((0..meshes)
        .map(|i| {
            let mut m =
                gltf::Material::named(names.get(i).cloned().unwrap_or_else(|| "material".into()));
            m.images = images.clone();
            m
        })
        .collect())
}

fn describe(s: &Source) -> String {
    let short = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    match s {
        Source::Channel(p, c) => format!("{} ({})", short(p), ["R", "G", "B", "A"][*c]),
        Source::Rgb(p) => short(p),
        Source::Constant(v) => format!("{v}"),
    }
}

/// Resolves each mesh's material through Packages.bin and bakes its textures into `out`.
fn auto_materials(
    game: &Path,
    model_path: &str,
    names: &[String],
    meshes: usize,
    out: &Path,
) -> Result<Vec<gltf::Material>> {
    let mut baker = pbr::MaterialBaker::open(game)?;
    let short = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    fs::create_dir_all(out)?;
    let mut result = Vec::new();
    for i in 0..meshes {
        let Some(name) = names.get(i) else {
            result.push(gltf::Material::named("material"));
            continue;
        };
        let mut mat = gltf::Material::named(short(name));
        let plan = match baker.plan(model_path, name) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("material {name}: {e:#}; exported without textures");
                result.push(mat);
                continue;
            }
        };
        eprintln!("material {}: {}", plan.name, describe_plan(&plan));
        let baked = baker.bake(&plan)?;
        for (slot, suffix, img) in [
            (gltf::Slot::BaseColor, "base", &baked.base_color),
            (
                gltf::Slot::MetallicRoughness,
                "metal_rough",
                &baked.metallic_roughness,
            ),
            (gltf::Slot::Normal, "normal", &baked.normal),
            (gltf::Slot::Emissive, "emissive", &baked.emissive),
        ] {
            if let Some(img) = img {
                let file = format!("{}_{suffix}.png", plan.name);
                fs::write(out.join(&file), img.to_png()?)?;
                mat.images.push((slot, file));
            }
        }
        mat.base_color_factor = baked.base_color_factor;
        mat.metallic_factor = baked.metallic_factor;
        mat.roughness_factor = baked.roughness_factor;
        mat.emissive_factor = baked.emissive_factor;
        result.push(mat);
    }
    Ok(result)
}

/// One line describing where each input of a material comes from.
fn describe_plan(plan: &Plan) -> String {
    let short = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    format!(
        "albedo {}, tint {}, roughness {}, {} {}, normal {}, emissive {}{}",
        describe(&plan.albedo),
        plan.tint.as_ref().map_or("none".into(), |t| format!(
            "{}{}{}",
            short(&t.mask),
            if t.layout == MaskLayout::PackAlphaQuadrants {
                " (alpha quadrants)"
            } else {
                ""
            },
            if t.default_colors {
                ", default palette"
            } else {
                ""
            }
        )),
        describe(&plan.roughness),
        if plan.specular_workflow {
            "specular"
        } else {
            "metal"
        },
        describe(&plan.metalness),
        plan.normal.as_deref().map_or("none".into(), short),
        plan.emissive
            .as_ref()
            .map_or("none".into(), |(t, _, _)| short(t)),
        if plan.unsupported.is_empty() {
            String::new()
        } else {
            format!("; not reproduced: {}", plan.unsupported.join(", "))
        }
    )
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
    if let Some(sk) = &model.skeleton {
        let weighted = model
            .vertices
            .iter()
            .filter(|v| v.weights.iter().any(|&w| w > 0.0))
            .count();
        println!(
            "skeleton: {} bones, {} joints, bind pose error {:.2e}; {weighted} of {} vertices weighted; root {}",
            sk.bones.len(),
            sk.joints.len(),
            sk.bind_error(),
            model.vertices.len(),
            sk.bones.first().map_or("-", |b| b.name.as_str())
        );
        let errors = sk.joint_bind_errors();
        for (j, e) in errors
            .iter()
            .enumerate()
            .filter(|(_, e)| **e > 1e-3)
            .take(8)
        {
            let b = &sk.bones[sk.joints[j]];
            println!(
                "  joint {j} {} (parent {:?}): bind error {e:.3}, rotation {:?}, translation {:?}",
                b.name,
                b.parent.map(|p| &sk.bones[p].name),
                b.rotation,
                b.translation
            );
        }
    }
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
    let materials = if textures.is_empty() {
        auto_materials(game, path, &h.materials(), h.meshes.len(), out)?
    } else {
        manual_materials(game, textures, &h.materials(), h.meshes.len(), out)?
    };
    let scene = wf_unpack::scene::prepare(&model, lod, true)?;
    gltf::write(&scene, &name, &materials, out)?;
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

    #[derive(Default)]
    struct Stats {
        ok: usize,
        vertices: u64,
        triangles: u64,
        bones: u64,
        rest_differs: usize,
        errors: BTreeMap<String, Vec<String>>,
    }
    let mut stats: BTreeMap<u16, Stats> = BTreeMap::new();
    let mut fbx_kinds: BTreeMap<u16, usize> = BTreeMap::new();
    for e in &entries {
        let Ok(raw) = head.read(e) else { continue };
        let Ok(common) = CommonHeader::parse(&raw) else {
            continue;
        };
        if common.kind != KIND_STATIC && common.kind != KIND_SKINNED {
            if e.path.ends_with(".fbx") {
                *fbx_kinds.entry(common.kind).or_default() += 1;
            }
            continue;
        }
        let st = stats.entry(common.kind).or_default();
        let result = ModelHeader::parse(&raw).and_then(|header| {
            let b = body.read_path(&e.path)?.unwrap_or_default();
            let f = full.read_path(&e.path)?.unwrap_or_default();
            let model = Model::from_parts(header, &b, &f)?;
            model.check_bounds()?;
            if model.skeleton.is_some() {
                for v in &model.vertices {
                    let sum: f32 = v.weights.iter().sum();
                    anyhow::ensure!(
                        sum == 0.0 || (sum - 1.0).abs() < 1e-3,
                        "vertex weights sum to {sum}"
                    );
                }
            }
            Ok(model)
        });
        match result {
            Ok(model) => {
                st.ok += 1;
                st.vertices += model.vertices.len() as u64;
                st.triangles += (0..model.header.meshes.len())
                    .map(|m| model.lod_indices(m, 0).map_or(0, |t| t.len() as u64 / 3))
                    .sum::<u64>();
                if let Some(sk) = &model.skeleton {
                    st.bones += sk.bones.len() as u64;
                    st.rest_differs += usize::from(sk.bind_error() >= 1e-3);
                }
            }
            Err(err) => {
                let key: String = format!("{err:#}")
                    .chars()
                    .map(|c| if c.is_ascii_digit() { '#' } else { c })
                    .collect();
                st.errors.entry(key).or_default().push(e.path.clone());
            }
        }
    }
    println!(
        "{:.0}s; {duplicates} duplicate H.Misc paths skipped",
        started.elapsed().as_secs_f64()
    );
    for (kind, st) in &stats {
        let failed: usize = st.errors.values().map(Vec::len).sum();
        print!(
            "type {kind}: {}/{} ok, {failed} failed ({} vertices, {} LOD0 triangles",
            st.ok,
            st.ok + failed,
            st.vertices,
            st.triangles
        );
        if *kind == KIND_SKINNED {
            print!(
                ", {} bones; {} with a stored rest pose that differs from the bind pose",
                st.bones, st.rest_differs
            );
        }
        println!(")");
        for (msg, list) in &st.errors {
            println!(
                "  {:>6}  {msg}\n          e.g. {}",
                list.len(),
                list.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            );
        }
    }
    println!("other .fbx types (not supported): {fbx_kinds:?}");
    Ok(())
}

fn load_packages_bin(game: &Path) -> Result<PackagesBin> {
    PackagesBin::load(game)
}

fn props_cmd(game: &Path, path: &str) -> Result<()> {
    let bin = load_packages_bin(game)?;
    let mut texts = bin.texts()?;
    let mut misc = Package::open(&game.join("H.Misc.toc"))?;
    if let Some(raw) = misc.read_path(path)? {
        let attrs = CommonHeader::parse(&raw)?.attributes;
        if !attrs.is_empty() {
            println!("== {path} (H.Misc attribute text)\n{attrs}");
        }
    }
    let mut cur = Some(path.to_string());
    while let Some(p) = cur {
        println!("== {p}");
        match texts.text(&p)? {
            Some(text) => println!("{text}"),
            None => println!("(not in Packages.bin)"),
        }
        cur = bin.parent(&p).map(str::to_string);
    }
    Ok(())
}

fn check_packages_cmd(game: &Path) -> Result<()> {
    let started = Instant::now();
    let bin = load_packages_bin(game)?;
    let parsed = started.elapsed().as_secs_f64();
    let mut texts = bin.texts()?;
    let (mut ok, mut with_text, mut bytes, mut largest) = (0usize, 0usize, 0u64, 0usize);
    let mut errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut missing_parents = 0usize;
    for path in bin.paths() {
        if bin.parent(path).is_some_and(|p| !bin.contains(p)) {
            missing_parents += 1;
        }
        match texts.text(path) {
            Ok(Some(t)) => {
                ok += 1;
                if !t.is_empty() {
                    with_text += 1;
                    bytes += t.len() as u64;
                    largest = largest.max(t.len());
                }
            }
            Ok(None) => unreachable!(),
            Err(err) => {
                let key: String = format!("{err:#}")
                    .chars()
                    .map(|c| if c.is_ascii_digit() { '#' } else { c })
                    .collect();
                errors.entry(key).or_default().push(path.to_string());
            }
        }
    }
    let failed: usize = errors.values().map(Vec::len).sum();
    println!(
        "Packages.bin version {}: {} entities, {ok} ok ({with_text} with text, {:.1} MB decoded, largest {largest} bytes), {failed} failed; \
         {missing_parents} parents not in the file; parsed in {parsed:.1}s, all in {:.1}s",
        bin.version,
        bin.len(),
        bytes as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    for (msg, list) in &errors {
        println!(
            "  {:>6}  {msg}\n          e.g. {}",
            list.len(),
            list.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        );
    }
    Ok(())
}

fn check_materials_cmd(game: &Path, survey: bool) -> Result<()> {
    let started = Instant::now();
    let bin = load_packages_bin(game)?;
    let mut texts = bin.texts()?;
    let mut head = Package::open(&game.join("H.Misc.toc"))?;
    head.set_nocache();
    let mut misc = Package::open(&game.join("H.Misc.toc"))?;
    let textures = TexturePackages::open(game)?;
    let entries: Vec<_> = head
        .toc
        .entries
        .iter()
        .filter(|e| std::ptr::eq(head.toc.get(&e.path).unwrap(), *e))
        .cloned()
        .collect();

    let mut outcomes: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut note = |key: String, example: &str| {
        outcomes
            .entry(key)
            .or_insert_with(|| (0, example.to_string()))
            .0 += 1;
    };
    let mut keys: BTreeMap<String, usize> = BTreeMap::new();
    // Plans are small; keep one per material path (None when it failed)
    let mut plans: std::collections::HashMap<String, std::result::Result<Plan, String>> =
        Default::default();
    let (mut models, mut slots, mut textured_models) = (0usize, 0usize, 0usize);
    for e in &entries {
        let Ok(raw) = head.read(e) else { continue };
        let Ok(common) = CommonHeader::parse(&raw) else {
            continue;
        };
        if common.kind != KIND_STATIC {
            continue;
        }
        let Ok(header) = ModelHeader::parse(&raw) else {
            continue;
        };
        models += 1;
        let names = header.materials();
        if names.is_empty() {
            note("model has no Materials= list".into(), &e.path);
            continue;
        }
        let mut all_textured = true;
        for name in &names {
            slots += 1;
            let path = material::resolve_path(material::folder(&e.path), name);
            if !plans.contains_key(&path) {
                let plan = pbr::plan_material(
                    &bin,
                    &mut texts,
                    &mut misc,
                    |t| textures.contains(t),
                    &e.path,
                    name,
                )
                .map_err(|e| {
                    let msg = format!("{e:#}");
                    if msg.contains("not in Packages.bin") {
                        "material not in Packages.bin".to_string()
                    } else {
                        msg
                    }
                });
                if survey {
                    if let Ok(m) = Material::resolve(&bin, &mut texts, &path, "") {
                        for (k, (v, _)) in &m.props {
                            if !k.contains(':') && !k.starts_with("Shader") {
                                *keys.entry(format!("{k}={v}")).or_default() += 1;
                            }
                        }
                    }
                }
                plans.insert(path.clone(), plan);
            }
            let plan = match &plans[&path] {
                Ok(p) => p,
                Err(msg) => {
                    note(format!("failed: {msg}"), &path);
                    all_textured = false;
                    continue;
                }
            };
            let base = match (&plan.albedo, &plan.tint) {
                (Source::Rgb(_), _) => "diffuse texture",
                (Source::Channel(..), Some(_)) => "pack map albedo + tint mask",
                (Source::Channel(..), None) => "pack map albedo, untinted",
                (Source::Constant(_), Some(_)) => "constant colour + tint mask",
                (Source::Constant(_), None) => "constant colour only",
            };
            all_textured &= !matches!(plan.albedo, Source::Constant(_));
            note(format!("base colour: {base}"), &path);
            note(
                format!(
                    "tint: {}",
                    match &plan.tint {
                        None => "none".to_string(),
                        Some(t) => format!(
                            "{}{}",
                            if t.layout == MaskLayout::PackAlphaQuadrants {
                                "pack map alpha quadrants"
                            } else {
                                "separate tint mask"
                            },
                            if t.default_colors {
                                ", default palette"
                            } else {
                                ", material colours"
                            }
                        ),
                    }
                ),
                &path,
            );
            note(
                format!(
                    "workflow: {}",
                    if plan.specular_workflow {
                        "specular"
                    } else {
                        "metalness"
                    }
                ),
                &path,
            );
            let tex = |s: &Source| !matches!(s, Source::Constant(_));
            note(
                format!(
                    "metallic-roughness: {}",
                    if tex(&plan.roughness) || tex(&plan.metalness) {
                        "texture"
                    } else {
                        "constants"
                    }
                ),
                &path,
            );
            note(
                format!(
                    "normal map: {}",
                    if plan.normal.is_some() { "yes" } else { "no" }
                ),
                &path,
            );
            note(
                format!(
                    "emissive map: {}",
                    if plan.emissive.is_some() { "yes" } else { "no" }
                ),
                &path,
            );
            for u in &plan.unsupported {
                note(format!("not reproduced: {u}"), &path);
            }
        }
        textured_models += usize::from(all_textured);
    }
    println!(
        "{models} static models, {slots} material slots ({} distinct materials); {textured_models} models get a base colour texture on every mesh; {:.0}s",
        plans.len(),
        started.elapsed().as_secs_f64()
    );
    for (k, (n, example)) in &outcomes {
        println!("{n:>7}  {k}   e.g. {example}");
    }
    if survey {
        let mut sorted: Vec<_> = keys.into_iter().collect();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.1));
        for (k, n) in sorted.iter().take(80) {
            println!("{n:>7}  {k}");
        }
    }
    Ok(())
}

fn find_text_cmd(game: &Path, needle: &str) -> Result<()> {
    let bin = load_packages_bin(game)?;
    let mut texts = bin.texts()?;
    let mut paths: Vec<&str> = bin.paths().collect();
    paths.sort_unstable();
    for path in paths {
        let text = texts.text(path)?.unwrap_or_default();
        for line in text.lines().filter(|l| l.contains(needle)) {
            println!("{path}: {}", line.trim());
        }
    }
    Ok(())
}

#[cfg(feature = "bridge")]
fn check_bridge_cmd(game: &Path, textures: bool, filter: &str, limit: Option<usize>) -> Result<()> {
    use rewrite_bridge::Provider;
    let started = Instant::now();
    let provider = wf_unpack::bridge::Warframe::open(game)?.with_textures(textures);
    let models: Vec<(String, u16)> = provider
        .models()?
        .into_iter()
        .filter(|(p, _)| p.contains(filter))
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    eprintln!(
        "{} models listed in {:.0}s",
        models.len(),
        started.elapsed().as_secs_f64()
    );
    #[derive(Default)]
    struct Stats {
        ok: usize,
        meshes: usize,
        triangles: u64,
        joints: u64,
        textured: usize,
        errors: BTreeMap<String, Vec<String>>,
    }
    let mut stats: BTreeMap<u16, Stats> = BTreeMap::new();
    for (n, (path, kind)) in models.iter().enumerate() {
        if n > 0 && n % 2000 == 0 {
            eprintln!(
                "{n}/{} models, {:.0}s",
                models.len(),
                started.elapsed().as_secs_f64()
            );
        }
        let st = stats.entry(*kind).or_default();
        match provider.load(path) {
            Ok(m) => {
                st.ok += 1;
                st.meshes += m.meshes.len();
                st.triangles += m
                    .meshes
                    .iter()
                    .map(|x| x.indices.len() as u64 / 3)
                    .sum::<u64>();
                st.joints += m.skins.iter().map(|s| s.joints.len() as u64).sum::<u64>();
                st.textured += usize::from(m.materials.iter().any(|x| x.base_color.is_some()));
            }
            Err(err) => {
                // The innermost causes say what went wrong; the outermost is the path
                let mut chain = Vec::new();
                let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err.as_ref());
                while let Some(e) = cur {
                    chain.push(e.to_string());
                    cur = e.source();
                }
                let msg = chain
                    .into_iter()
                    .filter(|m| m != path)
                    .collect::<Vec<_>>()
                    .join(": ");
                let key: String = msg
                    .chars()
                    .map(|c| if c.is_ascii_digit() { '#' } else { c })
                    .collect();
                st.errors.entry(key).or_default().push(path.clone());
            }
        }
    }
    println!(
        "{:.0}s, textures {}",
        started.elapsed().as_secs_f64(),
        if textures { "baked" } else { "skipped" }
    );
    for (kind, st) in &stats {
        let failed: usize = st.errors.values().map(Vec::len).sum();
        println!(
            "type {kind}: {}/{} load and validate, {failed} failed ({} meshes, {} triangles, {} joints{})",
            st.ok,
            st.ok + failed,
            st.meshes,
            st.triangles,
            st.joints,
            if textures { format!(", {} with a base colour texture", st.textured) } else { String::new() }
        );
        for (msg, list) in &st.errors {
            println!(
                "  {:>6}  {msg}\n          e.g. {}",
                list.len(),
                list.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            );
        }
    }
    Ok(())
}
