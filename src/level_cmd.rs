//! The `level` and `check-levels` commands.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};

use wf_unpack::cache::Package;
use wf_unpack::header::CommonHeader;
use wf_unpack::level::{
    InstanceRole, LevelDescription, RawLevel, StaticLevel, TypeResolver, KIND_LEVEL,
    KIND_STATIC_LEVEL,
};
use wf_unpack::level_export::{export_collision, export_level, load_collision, ExportOptions};
use wf_unpack::model::{
    Model, ModelHeader, KIND_LEVEL_FLOAT, KIND_LEVEL_PACKED, KIND_SKINNED, KIND_STATIC,
};
use wf_unpack::packages::PackagesBin;
use wf_unpack::physx::read_shapes;

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

/// Groups error texts with their numbers masked, keeping a few examples.
fn note(errors: &mut BTreeMap<String, Vec<String>>, err: &anyhow::Error, example: &str) {
    let key: String = format!("{err:#}")
        .chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .collect();
    errors.entry(key).or_default().push(example.to_string());
}

fn print_errors(errors: &BTreeMap<String, Vec<String>>) {
    for (msg, list) in errors {
        println!(
            "  {:>6}  {msg}\n          e.g. {}",
            list.len(),
            list.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        );
    }
}

/// Reads every level and, with `deep`, builds its description and checks the
/// level geometry and PhysX shapes of every model the descriptions refer to.
/// Levels, static levels and models are handled one at a time.
pub fn check_levels(game: &Path, deep: bool) -> Result<()> {
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
    let bin = if deep {
        Some(PackagesBin::load(game)?)
    } else {
        None
    };
    let mut types = match &bin {
        Some(b) => Some(TypeResolver::new(b)?),
        None => None,
    };

    let mut kinds: BTreeMap<u16, (usize, String)> = BTreeMap::new();
    let (mut ok, mut objects, mut attr_failures) = (0usize, 0usize, 0usize);
    let mut errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut object_types: BTreeMap<String, usize> = BTreeMap::new();
    let (mut statics_ok, mut static_objects) = (0usize, 0usize);
    let mut static_errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Deep totals
    let mut described = 0usize;
    let mut desc_errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut instances, mut lights, mut volumes, mut spawns, mut sheets, mut hidden) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let (mut portals, mut environments) = (0usize, 0usize);
    let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
    for e in &entries {
        let Ok(raw) = head.read(e) else { continue };
        let Ok(common) = CommonHeader::parse(&raw) else {
            continue;
        };
        let k = kinds
            .entry(common.kind)
            .or_insert_with(|| (0, e.path.clone()));
        k.0 += 1;
        if common.kind == KIND_STATIC_LEVEL {
            let result = body
                .read_path(&e.path)
                .and_then(|b| StaticLevel::parse(&raw, &b.unwrap_or_default()));
            match result {
                Ok(s) => {
                    statics_ok += 1;
                    static_objects += s.objects.len();
                }
                Err(err) => note(&mut static_errors, &err, &e.path),
            }
            continue;
        }
        if common.kind != KIND_LEVEL {
            continue;
        }
        let result = body
            .read_path(&e.path)
            .and_then(|b| RawLevel::parse(&raw, &b.unwrap_or_default()));
        let level = match result {
            Ok(level) => level,
            Err(err) => {
                note(&mut errors, &err, &e.path);
                continue;
            }
        };
        ok += 1;
        objects += level.objects.len();
        for o in &level.objects {
            if !deep {
                *object_types.entry(o.type_path.clone()).or_default() += 1;
            }
            if o.attrs().is_err() {
                attr_failures += 1;
            }
        }
        let Some(types) = types.as_mut() else {
            continue;
        };
        match LevelDescription::build(&level, &e.path, types) {
            Ok(mut d) => {
                let mut failed = false;
                for path in d.static_levels.clone() {
                    let parsed = head
                        .read_path(&path)
                        .and_then(|h| {
                            let b = body.read_path(&path)?;
                            StaticLevel::parse(&h.unwrap_or_default(), &b.unwrap_or_default())
                        })
                        .with_context(|| path.clone());
                    match parsed {
                        Ok(s) => d.add_static_level(&path, &s),
                        Err(err) => {
                            note(&mut desc_errors, &err, &e.path);
                            failed = true;
                        }
                    }
                }
                if !failed {
                    described += 1;
                }
                instances += d.instances.len();
                hidden += d.instances.iter().filter(|i| i.hidden).count();
                lights += d.lights.len();
                volumes += d.volumes.len();
                spawns += d.spawns.len();
                sheets += d.sheets.len();
                portals += d.portals.len();
                environments += d.environments.len();
                referenced.extend(d.models);
            }
            Err(err) => note(&mut desc_errors, &err, &e.path),
        }
    }
    let failed: usize = errors.values().map(Vec::len).sum();
    println!("H.AnimRetarget entries by type (count, example):");
    for (kind, (n, example)) in &kinds {
        println!("  {kind:>4} {n:>6}  {example}");
    }
    println!(
        "levels (type {KIND_LEVEL}): {ok}/{} parse, {failed} fail; {objects} objects, {attr_failures} attribute texts fail",
        ok + failed
    );
    print_errors(&errors);
    let static_failed: usize = static_errors.values().map(Vec::len).sum();
    println!(
        "static levels (type {KIND_STATIC_LEVEL}): {statics_ok}/{} parse, {static_failed} fail; {static_objects} objects",
        statics_ok + static_failed
    );
    print_errors(&static_errors);
    if !deep {
        println!("{} distinct object types", object_types.len());
        println!(
            "{:.1}s, peak memory {}",
            started.elapsed().as_secs_f64(),
            peak_rss_text()
        );
        return Ok(());
    }

    let desc_failed: usize = desc_errors.values().map(Vec::len).sum();
    println!(
        "descriptions: {described}/{ok} built with their static levels, {desc_failed} errors; {instances} instances ({hidden} start hidden), {lights} lights, {volumes} volumes, {spawns} spawns, {portals} portals, {environments} environment settings, {sheets} sheets; {} distinct models",
        referenced.len()
    );
    print_errors(&desc_errors);

    // The models the levels place, in package order
    let (mut mh, mut mb, mut mf) = (
        open(game, "H.Misc")?,
        open(game, "B.Misc")?,
        open(game, "F.Misc")?,
    );
    let mut paths: Vec<_> = referenced.into_iter().collect();
    paths.sort_by_key(|p| mh.toc.get(p).map_or(u64::MAX, |e| e.offset));
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut geometry: BTreeMap<u16, (usize, usize, u64)> = BTreeMap::new();
    let mut geometry_errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut shapes_ok, mut shapes_models, mut shape_errors) = (0usize, 0usize, BTreeMap::new());
    for p in &paths {
        let Some(entry) = mh.toc.get(p).cloned() else {
            *by_kind.entry("not in H.Misc".into()).or_default() += 1;
            continue;
        };
        let Ok(raw) = mh.read(&entry) else { continue };
        let Ok(common) = CommonHeader::parse(&raw) else {
            *by_kind.entry("unreadable header".into()).or_default() += 1;
            continue;
        };
        *by_kind.entry(format!("type {}", common.kind)).or_default() += 1;
        if !matches!(
            common.kind,
            KIND_STATIC | KIND_SKINNED | KIND_LEVEL_PACKED | KIND_LEVEL_FLOAT
        ) {
            continue;
        }
        let level_mesh = matches!(common.kind, KIND_LEVEL_PACKED | KIND_LEVEL_FLOAT);
        let result = ModelHeader::parse(&raw).and_then(|header| {
            let b = mb.read_path(p)?.unwrap_or_default();
            let shapes = read_shapes(&header.physx_shapes, &b)?;
            // Level geometry is read in full; the other kinds are covered by check-models
            if level_mesh {
                let f = mf.read_path(p)?.unwrap_or_default();
                let m = Model::from_parts(header, &b, &f)?;
                Ok((shapes.len(), Some(m.indices.len() as u64 / 3)))
            } else {
                Ok((shapes.len(), None))
            }
        });
        match result {
            Ok((n, tris)) => {
                shapes_ok += n;
                shapes_models += usize::from(n > 0);
                if let Some(t) = tris {
                    let g = geometry.entry(common.kind).or_default();
                    g.0 += 1;
                    g.2 += t;
                }
            }
            Err(err) => {
                if level_mesh {
                    geometry.entry(common.kind).or_default().1 += 1;
                    note(&mut geometry_errors, &err, p);
                } else {
                    note(&mut shape_errors, &err, p);
                }
            }
        }
    }
    println!("models the levels place, by type: {by_kind:?}");
    for (kind, (n, bad, tris)) in &geometry {
        println!(
            "level geometry type {kind}: {n}/{} read in full ({tris} triangles), {bad} fail",
            n + bad
        );
    }
    print_errors(&geometry_errors);
    println!("PhysX shapes parsed: {shapes_ok} in {shapes_models} models");
    print_errors(&shape_errors);
    println!(
        "{:.1}s, peak memory {}",
        started.elapsed().as_secs_f64(),
        peak_rss_text()
    );
    Ok(())
}

pub fn level_cmd(
    game: &Path,
    level_path: &str,
    verbose: bool,
    out: Option<&Path>,
    collision: bool,
    opts: &ExportOptions,
) -> Result<()> {
    let d = wf_unpack::level::load_description(game, level_path)?;
    let zones = d
        .instances
        .iter()
        .filter(|i| i.role == InstanceRole::Zone)
        .count();
    let hidden = d.instances.iter().filter(|i| i.hidden).count();
    println!(
        "{}: {} instances of {} models ({zones} zones, {hidden} start hidden), {} sheets, {} lights, {} volumes, {} spawns, {} portals, {} environment settings",
        d.path,
        d.instances.len(),
        d.models.len(),
        d.sheets.len(),
        d.lights.len(),
        d.volumes.len(),
        d.spawns.len(),
        d.portals.len(),
        d.environments.len()
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
        for e in &d.environments {
            println!(
                "{:?} settings {} at {:?}: ambient {:?} fog {:?} density {:?} reflection {:?} backdrop {:?}",
                e.kind,
                e.type_path,
                e.position,
                e.ambient_light,
                e.fog_color,
                e.fog_density,
                e.reflection_map,
                e.backdrop
            );
        }
        for p in &d.portals {
            println!(
                "portal {} at {:?} width {:?} height {:?}",
                p.type_path, p.transform.translation, p.width, p.height
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
        if !r.no_geometry.is_empty() {
            println!(
                "{} models have no triangles at this LOD (collision or trigger shapes): {} instances",
                r.no_geometry.len(),
                r.instances_without_geometry
            );
        }
        for (m, e) in &r.model_failures {
            println!("model failed: {m}: {e}");
        }
        for (m, e) in &r.material_failures {
            println!("material failed: {m}: {e}");
        }
        if collision {
            let c = load_collision(game, &d)?;
            let (shapes, nodes) = export_collision(&d, &c, out, name)?;
            println!(
                "wrote {}_collision.gltf: {shapes} shapes of {} models, {nodes} nodes",
                out.join(name).display(),
                c.per_model
                    .iter()
                    .flatten()
                    .filter(|s| !s.is_empty())
                    .count()
            );
            for (m, e) in &c.failures {
                println!("collision failed: {m}: {e}");
            }
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
