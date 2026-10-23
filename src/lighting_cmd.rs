//! The `probe`, `postprocess` and `lightmaps` commands.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use wf_unpack::level::load_description;
use wf_unpack::lightmap::{LightmapPage, LightmapParams};
use wf_unpack::pbr::linear_to_srgb;
use wf_unpack::probe::{radiance_hdr, Probe, FACE_NAMES};
use wf_unpack::texture::TexturePackages;

fn stem(path: &str) -> &str {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .rsplit_once('.')
        .map_or(path, |(s, _)| s)
}

pub fn probe_cmd(game: &Path, path: &str, out: Option<&Path>) -> Result<()> {
    let p = Probe::load(game, path)?;
    let mean = p.mean_clamped(100.0);
    println!(
        "{path}: BC6H cube map, faces {}x{}, {} mip levels stored; mean linear RGB, clamped at 100, ({:.4}, {:.4}, {:.4}), largest finite texel {:.3}",
        p.size, p.size, p.mip_levels, mean[0], mean[1], mean[2], p.max_finite()
    );
    let unhit = p
        .faces
        .iter()
        .flatten()
        .filter(|t| t[0] >= 65_000.0 || t[1] >= 65_000.0 || t[2] >= 65_000.0)
        .count();
    println!(
        "{unhit} of {} texels are at the half-float maximum",
        6 * p.size * p.size
    );
    if let Some(out) = out {
        fs::create_dir_all(out)?;
        let name = stem(path);
        let (w, h) = (p.size * 4, p.size * 2);
        let eq = out.join(format!("{name}_equirect.hdr"));
        fs::write(&eq, radiance_hdr(w, h, &p.to_equirect(w, h))?)?;
        println!("wrote {} ({w}x{h})", eq.display());
        for (f, face) in p.faces.iter().enumerate() {
            let file = out.join(format!("{name}_{}.hdr", FACE_NAMES[f]));
            fs::write(&file, radiance_hdr(p.size, p.size, face)?)?;
        }
        println!("wrote the six faces as {name}_<px|nx|py|ny|pz|nz>.hdr");
    }
    Ok(())
}

fn fmt_opt<T: std::fmt::Debug>(name: &str, v: &Option<T>, lines: &mut Vec<String>) {
    if let Some(v) = v {
        lines.push(format!("  {name}: {v:?}"));
    }
}

pub fn postprocess_cmd(game: &Path, path: &str, json: bool) -> Result<()> {
    let d = load_description(game, path)?;
    if json {
        let all: Vec<_> = d
            .environments
            .iter()
            .map(|e| {
                serde_json::json!({
                    "kind": format!("{:?}", e.kind), "type": e.type_path, "scope": e.scope,
                    "lighting": e.lighting.to_json(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(());
    }
    for e in &d.environments {
        println!("{:?} {} (scope {}):", e.kind, e.type_path, e.scope);
        let l = &e.lighting;
        if let Some(p) = &l.post_process {
            println!(" PostProcessInfo, {} typed fields:", p.set_count());
            let j = p.to_json();
            if let Some(m) = j.as_object() {
                for (k, v) in m {
                    println!("  {k}: {v}");
                }
            }
        }
        if let Some(p) = &l.post_process_alt {
            println!(" PostProcessInfoAlt, {} typed fields", p.set_count());
        }
        if let Some(m) = &l.post_fx {
            println!(
                " final pass material {}: shader {:?}, {} switches, {} constants",
                m.path,
                m.shader,
                m.switches.len(),
                m.constants.len()
            );
            for (k, v) in &m.switches {
                println!("  {k}={v}");
            }
        }
        if let Some(lp) = &l.lighting_params {
            println!(" LightingParams:");
            if let Some(m) = lp.to_json().as_object() {
                for (k, v) in m {
                    println!("  {k}: {v}");
                }
            }
        }
        if let Some(z) = &l.zone {
            println!(" zone settings:");
            let mut lines = Vec::new();
            let j = z.to_json();
            if let Some(m) = j.as_object() {
                for (k, v) in m {
                    lines.push(format!("  {k}: {v}"));
                }
            }
            for line in lines {
                println!("{line}");
            }
        }
        let mut unused: Vec<String> = Vec::new();
        fmt_opt("backdrop", &e.backdrop, &mut unused);
        for line in unused {
            println!("{line}");
        }
    }
    Ok(())
}

pub fn lightmaps_cmd(game: &Path, path: &str, out: Option<&Path>) -> Result<()> {
    let d = load_description(game, path)?;
    // Pages by (clr, dir)
    let mut pages: BTreeMap<(String, Option<String>), Vec<usize>> = BTreeMap::new();
    for (i, inst) in d.instances.iter().enumerate() {
        if let Some(lm) = &inst.lightmap {
            if let (Some(c), dir) = lm.pages() {
                pages
                    .entry((c.to_string(), dir.map(str::to_string)))
                    .or_default()
                    .push(i);
            }
        }
    }
    let lightmapped = d.instances.iter().filter(|i| i.lightmap.is_some()).count();
    println!(
        "{}: {lightmapped} of {} instances have a lightmap, in {} pages",
        d.path,
        d.instances.len(),
        pages.len()
    );
    let mut pk = TexturePackages::open_family(game, "LightMap")?;
    if let Some(out) = out {
        fs::create_dir_all(out)?;
    }
    for (n, ((clr, dir), insts)) in pages.iter().enumerate() {
        let page = LightmapPage::load(&mut pk, clr, dir.as_deref())?;
        println!(
            "page {n}: {clr} {}x{} + {} {:?}; {} instances",
            page.clr.width,
            page.clr.height,
            dir.as_deref().unwrap_or("-"),
            page.dir.as_ref().map(|a| (a.width, a.height)),
            insts.len()
        );
        // Ambient light at the centre of each instance's rectangle: a handful of examples
        for &i in insts.iter().take(3) {
            let inst = &d.instances[i];
            let lm = inst.lightmap.as_ref().unwrap();
            let params = LightmapParams {
                coords: lm.coords,
                normalize: lm.normalize,
                normalize1: lm.normalize1,
            };
            let s = page.sample(&params, [0.5, 0.5]);
            println!(
                "  {} {:?}: chroma ({:.3}, {:.3}, {:.3}) luma {:.4} direction {:?} gain^2 {:.3}",
                d.models[inst.model],
                lm.coords,
                s.chroma[0],
                s.chroma[1],
                s.chroma[2],
                s.luma,
                s.directional,
                s.gain
            );
        }
        if let Some(inst_idx) = insts.first() {
            let lm = d.instances[*inst_idx].lightmap.as_ref().unwrap();
            for ix in &lm.indexed {
                println!(
                    "  indexed {}: lights {:?} -> {:?}",
                    ix.texture, ix.lights, ix.light_indices
                );
            }
        }
        if let Some(out) = out {
            // The ambient term of every texel with each object's own constants, as sRGB
            let mut rgba = vec![0u8; page.clr.width * page.clr.height * 4];
            for &i in insts {
                let lm = d.instances[i].lightmap.as_ref().unwrap();
                let params = LightmapParams {
                    coords: lm.coords,
                    normalize: lm.normalize,
                    normalize1: lm.normalize1,
                };
                let (w, h) = (page.clr.width, page.clr.height);
                let x0 = (lm.coords[0] * w as f32).floor() as usize;
                let y0 = (lm.coords[1] * h as f32).floor() as usize;
                let x1 = (((lm.coords[0] + lm.coords[2]) * w as f32).ceil() as usize).min(w);
                let y1 = (((lm.coords[1] + lm.coords[3]) * h as f32).ceil() as usize).min(h);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let uv = [
                            ((x as f32 + 0.5) / w as f32 - lm.coords[0]) / lm.coords[2],
                            ((y as f32 + 0.5) / h as f32 - lm.coords[1]) / lm.coords[3],
                        ];
                        let a = page.sample(&params, uv).ambient();
                        let o = (y * w + x) * 4;
                        for k in 0..3 {
                            rgba[o + k] = (linear_to_srgb(a[k] * 2.0) * 255.0).round() as u8;
                        }
                        rgba[o + 3] = 255;
                    }
                }
            }
            let file = out.join(format!("lm{n}_ambient.png"));
            let mut png_bytes = Vec::new();
            let mut enc = png::Encoder::new(
                &mut png_bytes,
                page.clr.width as u32,
                page.clr.height as u32,
            );
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()?.write_image_data(&rgba)?;
            fs::write(&file, png_bytes).with_context(|| file.display().to_string())?;
            println!("  wrote {} (ambient x2, sRGB)", file.display());
        }
    }
    Ok(())
}

pub fn palette_cmd(game: &Path, suit: &str) -> Result<()> {
    let p = wf_unpack::palette::SuitPalette::resolve(game, suit)?;
    println!("{}: body entity {:?}", p.suit, p.base_suit);
    let show = |c: &[f32; 4]| format!("({}, {}, {}, {})", c[0], c[1], c[2], c[3]);
    for (i, e) in p.energy.iter().enumerate() {
        match e {
            Some(c) => println!(
                "  energy colour {}: {} (0..255) = #{:02x}{:02x}{:02x}",
                i + 1,
                show(c),
                c[0] as u8,
                c[1] as u8,
                c[2] as u8
            ),
            None => println!("  energy colour {}: not set", i + 1),
        }
    }
    for part in &p.parts {
        println!("  part {} ({}):", part.part, part.mesh);
        for m in &part.materials {
            println!("    material {}", m.path);
            for (i, t) in m.tint.iter().enumerate() {
                if let Some(t) = t {
                    println!("      PS:TintColor{i} {}", show(t));
                }
            }
            for (name, c) in [
                ("PS:EmissiveTintColorHi", &m.emissive_hi),
                ("PS:EmissiveTintColorLo", &m.emissive_lo),
                ("PS:EmissiveTintColor", &m.emissive_tint),
            ] {
                if let Some(c) = c {
                    println!("      {name} {}", show(c));
                }
            }
            if m.multi_tint_emission {
                println!("      emission tinted by the energy colours (EMISSIVE_MULTI_TINT)");
            }
        }
    }
    Ok(())
}

/// Runs over every level: the lightmap pages and probes they name, and the typed settings.
pub fn check_lighting_cmd(game: &Path) -> Result<()> {
    use std::collections::BTreeSet;
    use wf_unpack::cache::Package;
    use wf_unpack::texture::{Format, TextureHeader};

    let started = std::time::Instant::now();
    let mut head = Package::open(&game.join("H.AnimRetarget.toc"))?;
    let mut body = Package::open(&game.join("B.AnimRetarget.toc"))?;
    let levels: Vec<String> = head
        .toc
        .entries
        .iter()
        .filter(|e| e.path.ends_with(".level"))
        .map(|e| e.path.clone())
        .collect();
    let bin = wf_unpack::packages::PackagesBin::load(game)?;
    let mut types = wf_unpack::level::TypeResolver::new(&bin)?;
    let mut pages: BTreeSet<String> = BTreeSet::new();
    let mut probes: BTreeSet<String> = BTreeSet::new();
    let (mut with_lightmaps, mut lightmapped, mut failed) = (0usize, 0usize, 0usize);
    let mut indexed = (0usize, 0usize); // entries, with every light found
    let mut other_keys: BTreeMap<String, usize> = BTreeMap::new();
    let (mut pp_levels, mut pp_fields, mut zones) = (0usize, 0usize, 0usize);
    let mut tone: BTreeMap<String, usize> = BTreeMap::new();
    for path in &levels {
        let d = (|| -> Result<_> {
            let h = head.read_path(path)?.unwrap_or_default();
            let b = body.read_path(path)?.unwrap_or_default();
            let raw = wf_unpack::level::RawLevel::parse(&h, &b)?;
            wf_unpack::level::LevelDescription::build(&raw, path, &mut types)
        })();
        let d = match d {
            Ok(d) => d,
            Err(_) => {
                failed += 1;
                continue;
            }
        };
        let n = d.instances.iter().filter(|i| i.lightmap.is_some()).count();
        if n > 0 {
            with_lightmaps += 1;
            lightmapped += n;
        }
        for i in &d.instances {
            if let Some(lm) = &i.lightmap {
                for t in &lm.textures {
                    pages.insert(t.clone());
                }
                for ix in &lm.indexed {
                    pages.insert(ix.texture.clone());
                    indexed.0 += 1;
                    indexed.1 += usize::from(ix.light_indices.iter().all(Option::is_some));
                }
            }
        }
        for e in &d.environments {
            if let Some(r) = &e.reflection_map {
                probes.insert(r.clone());
            }
            if let Some(p) = &e.lighting.post_process {
                pp_levels += 1;
                pp_fields += p.set_count();
                for k in p.other.keys() {
                    *other_keys.entry(k.clone()).or_default() += 1;
                }
                if let Some(t) = &p.tone_map_type {
                    *tone.entry(t.name().to_string()).or_default() += 1;
                }
            }
            zones += usize::from(e.lighting.zone.is_some());
        }
    }
    println!(
        "{} levels ({failed} failed to describe): {with_lightmaps} have lightmaps ({lightmapped} lightmapped instances), {} distinct page textures named, {} probes named",
        levels.len(),
        pages.len(),
        probes.len()
    );
    println!(
        "post-process: {pp_levels} PostProcessInfo tables with {pp_fields} typed fields, {zones} zone settings; tone map types {tone:?}"
    );
    println!(
        "indexed light maps: {} entries, {} with every light found in the level",
        indexed.0, indexed.1
    );
    let mut other: Vec<_> = other_keys.into_iter().collect();
    other.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!(
        "PostProcessInfo keys kept as text: {:?}",
        other.iter().take(12).collect::<Vec<_>>()
    );

    // Page headers against the sizes of their B./F. parts, without reading the pixels
    let mut h = Package::open_in(game, "H.LightMap")?.context("no H.LightMap")?;
    let b = Package::open_in(game, "B.LightMap")?;
    let f = Package::open_in(game, "F.LightMap")?;
    let (mut ok, mut missing, mut bad) = (0usize, 0usize, 0usize);
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let mut errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in &pages {
        let Some(raw) = h.read_path(p)? else {
            missing += 1;
            continue;
        };
        let result = TextureHeader::parse(&raw).and_then(|th| {
            let len = |pk: &Option<Package>| {
                pk.as_ref()
                    .and_then(|k| k.toc.get(p))
                    .map_or(0, |e| e.len as usize)
            };
            th.layout(len(&b), len(&f)).map(|l| (th.format, l))
        });
        match result {
            Ok((fmt, _)) => {
                ok += 1;
                *formats
                    .entry(format!("{:?}", fmt.map(|f: Format| f)))
                    .or_default() += 1;
            }
            Err(e) => {
                bad += 1;
                errors
                    .entry(
                        format!("{e:#}")
                            .chars()
                            .filter(|c| !c.is_ascii_digit())
                            .collect(),
                    )
                    .or_default()
                    .push(p.clone());
            }
        }
    }
    println!("page textures: {ok} headers fit their B./F. sizes, {bad} do not, {missing} not in H.LightMap; formats {formats:?}");
    for (m, l) in errors.iter().take(3) {
        println!("  {} x {m} e.g. {}", l.len(), l[0]);
    }
    // Probes: decode every one
    let mut pk = TexturePackages::open_family(game, "LightMap")?;
    let (mut probe_ok, mut probe_fail) = (0usize, 0usize);
    let mut sizes: BTreeMap<usize, usize> = BTreeMap::new();
    let mut other_pk = TexturePackages::open(game)?;
    for p in &probes {
        let cube = if pk.contains(p) {
            pk.load_cube(p)
        } else {
            other_pk.load_cube(p)
        };
        match cube.and_then(|c| Probe::from_texture(&c)) {
            Ok(pr) => {
                probe_ok += 1;
                *sizes.entry(pr.size).or_default() += 1;
            }
            Err(e) => {
                probe_fail += 1;
                if probe_fail <= 3 {
                    println!("  probe {p}: {e:#}");
                }
            }
        }
    }
    println!("reflection probes: {probe_ok} decoded, {probe_fail} failed; face sizes {sizes:?}");
    println!(
        "{:.1}s, peak memory {}",
        started.elapsed().as_secs_f64(),
        crate::level_cmd::peak_rss_text()
    );
    Ok(())
}
