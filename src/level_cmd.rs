//! The `level` and `check-levels` commands.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};

use wf_unpack::cache::Package;
use wf_unpack::header::CommonHeader;
use wf_unpack::level::{InstanceRole, LevelDescription, RawLevel, TypeResolver, KIND_LEVEL};
use wf_unpack::level_export::{export_level, ExportOptions};
use wf_unpack::packages::PackagesBin;

/// Peak resident memory of this process, in bytes.
pub fn peak_rss() -> Option<u64> {
    #[cfg(unix)]
    {
        // SAFETY: getrusage fills the struct we hand it
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
            return None;
        }
        let max = usage.ru_maxrss as u64;
        // macOS reports bytes, Linux kilobytes
        Some(if cfg!(target_os = "macos") {
            max
        } else {
            max * 1024
        })
    }
    #[cfg(not(unix))]
    None
}

pub fn peak_rss_text() -> String {
    peak_rss().map_or("unknown".to_string(), |b| {
        format!("{:.0} MB", b as f64 / 1e6)
    })
}

fn open(game: &Path, name: &str) -> Result<Package> {
    let pkg = Package::open(&game.join(format!("{name}.toc")))?;
    pkg.set_nocache();
    Ok(pkg)
}

pub fn check_levels(game: &Path) -> Result<()> {
    let started = Instant::now();
    let (mut head, mut body) = (open(game, "H.AnimRetarget")?, open(game, "B.AnimRetarget")?);
    let mut entries: Vec<_> = head
        .toc
        .entries
        .iter()
        .filter(|e| std::ptr::eq(head.toc.get(&e.path).unwrap(), *e))
        .cloned()
        .collect();
    entries.sort_by_key(|e| e.offset);

    let mut kinds: BTreeMap<u16, (usize, String)> = BTreeMap::new();
    let (mut ok, mut objects, mut attr_failures) = (0usize, 0usize, 0usize);
    let mut errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut types: BTreeMap<String, usize> = BTreeMap::new();
    for e in &entries {
        let Ok(raw) = head.read(e) else { continue };
        let Ok(common) = CommonHeader::parse(&raw) else {
            continue;
        };
        let k = kinds
            .entry(common.kind)
            .or_insert_with(|| (0, e.path.clone()));
        k.0 += 1;
        if common.kind != KIND_LEVEL {
            continue;
        }
        let result = body
            .read_path(&e.path)
            .and_then(|b| RawLevel::parse(&raw, &b.unwrap_or_default()));
        match result {
            Ok(level) => {
                ok += 1;
                objects += level.objects.len();
                for o in &level.objects {
                    *types.entry(o.type_path.clone()).or_default() += 1;
                    if o.attrs().is_err() {
                        attr_failures += 1;
                    }
                }
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
    println!("H.AnimRetarget entries by type (count, example):");
    for (kind, (n, example)) in &kinds {
        println!("  {kind:>4} {n:>6}  {example}");
    }
    println!(
        "levels (type {KIND_LEVEL}): {ok}/{} parse, {failed} fail; {objects} objects, {attr_failures} attribute texts fail; {} distinct object types",
        ok + failed,
        types.len()
    );
    for (msg, list) in &errors {
        println!(
            "  {:>6}  {msg}\n          e.g. {}",
            list.len(),
            list.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        );
    }
    println!(
        "{:.1}s, peak memory {}",
        started.elapsed().as_secs_f64(),
        peak_rss_text()
    );
    Ok(())
}

/// Loads a level's description: reads the level, resolves its object types.
pub fn load_description(game: &Path, level_path: &str) -> Result<LevelDescription> {
    let read = |name: &str| -> Result<Vec<u8>> {
        let mut pkg = open(game, name)?;
        Ok(pkg.read_path(level_path)?.unwrap_or_default())
    };
    let head = read("H.AnimRetarget")?;
    anyhow::ensure!(!head.is_empty(), "{level_path} is not in H.AnimRetarget");
    let body = read("B.AnimRetarget")?;
    let raw = RawLevel::parse(&head, &body).with_context(|| level_path.to_string())?;
    let bin = PackagesBin::load(game)?;
    let mut types = TypeResolver::new(&bin)?;
    LevelDescription::build(&raw, level_path, &mut types)
}

pub fn level_cmd(
    game: &Path,
    level_path: &str,
    verbose: bool,
    out: Option<&Path>,
    opts: &ExportOptions,
) -> Result<()> {
    let d = load_description(game, level_path)?;
    let zones = d
        .instances
        .iter()
        .filter(|i| i.role == InstanceRole::Zone)
        .count();
    let hidden = d.instances.iter().filter(|i| i.hidden).count();
    println!(
        "{}: {} instances of {} models ({zones} zones, {hidden} start hidden), {} sheets, {} lights, {} volumes, {} spawns",
        d.path,
        d.instances.len(),
        d.models.len(),
        d.sheets.len(),
        d.lights.len(),
        d.volumes.len(),
        d.spawns.len()
    );
    let mut per_model = vec![0usize; d.models.len()];
    for i in &d.instances {
        per_model[i.model] += 1;
    }
    let mut order: Vec<usize> = (0..d.models.len()).collect();
    order.sort_by_key(|&m| std::cmp::Reverse(per_model[m]));
    println!("models by instance count:");
    for &m in order.iter().take(if verbose { usize::MAX } else { 15 }) {
        println!("  {:>5}  {}", per_model[m], d.models[m]);
    }
    let mut skipped: Vec<_> = d.skipped.iter().collect();
    skipped.sort_by_key(|(_, &n)| std::cmp::Reverse(n));
    println!("not turned into geometry ({} kinds):", skipped.len());
    for (why, n) in skipped.iter().take(if verbose { usize::MAX } else { 15 }) {
        println!("  {n:>5}  {why}");
    }
    if verbose {
        for l in &d.lights {
            println!(
                "light {} {:?} at {:?}: color {:?} brightness {:?} lumens {:?} radius {:?} angle {:?} on {:?}",
                l.type_path, l.light_type, l.position, l.color, l.brightness, l.lumens, l.radius, l.angle, l.on
            );
        }
        for s in &d.spawns {
            println!(
                "spawn {:?} {} at {:?} tag {:?}",
                s.kind, s.type_path, s.position, s.tag
            );
        }
        for v in &d.volumes {
            println!(
                "volume {} at {:?} dims {:?} l/w/h {:?}/{:?}/{:?}",
                v.type_path, v.transform.translation, v.dimensions, v.length, v.width, v.height
            );
        }
    }
    if let Some(out) = out {
        let name = level_path
            .rsplit('/')
            .next()
            .unwrap_or("level")
            .trim_end_matches(".level");
        let started = Instant::now();
        let r = export_level(game, &d, out, name, opts)?;
        println!(
            "wrote {}: {} models stored ({} triangles), {} meshes, {} nodes placing {} triangles, {} lights, {} materials, buffer {:.1} MB",
            out.join(format!("{name}.gltf")).display(),
            r.models_written,
            r.triangles_stored,
            r.meshes,
            r.nodes,
            r.triangles_placed,
            r.lights,
            r.materials,
            r.buffer_bytes as f64 / 1e6
        );
        if r.hidden_skipped > 0 {
            println!(
                "{} instances start hidden and were left out (--include-hidden)",
                r.hidden_skipped
            );
        }
        for (m, e) in &r.model_failures {
            println!("model failed: {m}: {e}");
        }
        for (m, e) in &r.material_failures {
            println!("material failed: {m}: {e}");
        }
        println!(
            "{} instances lost with failed models; {:.1}s, peak memory {}",
            r.instances_lost,
            started.elapsed().as_secs_f64(),
            peak_rss_text()
        );
    }
    Ok(())
}
