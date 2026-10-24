//! The commands that exercise the rewrite-bridge provider on animations and levels.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use rewrite_bridge::SceneModel;

use wf_unpack::bridge::{animated_id, Warframe};
use wf_unpack::bridge_scene::{EnvironmentSource, SceneOptions};
use wf_unpack::cache::Package;
use wf_unpack::groupkey::GroupKey;
use wf_unpack::level::{LevelDescription, RawLevel, StaticLevel, TypeResolver, KIND_LEVEL};
use wf_unpack::packages::PackagesBin;

use crate::level_cmd::peak_rss_text;

/// Loads a skinned model with its compatible clips through the provider, validates it and prints
/// what came back.
pub fn bridge_anim_cmd(
    game: &Path,
    model: &str,
    filter: &str,
    max_clips: usize,
    textures: bool,
    verbose: bool,
) -> Result<()> {
    let started = Instant::now();
    let wf = Warframe::open(game)?
        .with_textures(textures)
        .with_max_clips(max_clips);
    let loaded = wf.load_animated_clips(&animated_id(model, filter))?;
    let m = &loaded.model;
    println!(
        "{}: {} nodes, {} clips ({} skipped, {} animation files use other skeletons), validated",
        m.model.name,
        m.model.nodes.len(),
        m.animations.len(),
        loaded.skipped.len(),
        loaded.other_skeletons
    );
    for a in &m.animations {
        let i = &loaded.info[&a.name];
        let events = i
            .events
            .iter()
            .map(|e| format!("{}@{:.2}s", e.name, e.time_s))
            .collect::<Vec<_>>()
            .join(" ");
        if verbose || m.animations.len() <= 40 {
            println!(
                "  {:7.3} s  {:3} channels  root {:.2} m {}{}  {}",
                a.duration_s,
                a.channels.len(),
                i.root_translation.iter().map(|v| v * v).sum::<f32>().sqrt(),
                if i.events_from_header {
                    ""
                } else {
                    "(events from Packages.bin) "
                },
                a.name,
                events
            );
        }
    }
    for s in &loaded.skipped {
        println!("  skipped {}: {}", s.path, s.reason);
    }
    println!(
        "{:.1}s, peak RSS {}",
        started.elapsed().as_secs_f64(),
        peak_rss_text()
    );
    Ok(())
}

/// Builds the scene of one level through the provider and prints the report.
pub fn bridge_scene_cmd(
    game: &Path,
    level: &str,
    textures: bool,
    max_texture: Option<u32>,
    include_hidden: bool,
) -> Result<()> {
    let started = Instant::now();
    let wf = Warframe::open(game)?
        .with_textures(textures)
        .with_max_texture(max_texture);
    let built = wf.scene(
        level,
        &SceneOptions {
            include_hidden,
            ..SceneOptions::default()
        },
    )?;
    let r = &built.report;
    let images: usize = built
        .scene
        .models
        .iter()
        .map(|m| match m {
            SceneModel::Inline(m) => m
                .materials
                .iter()
                .flat_map(|x| [&x.base_color, &x.metallic_roughness, &x.normal, &x.emissive])
                .flatten()
                .map(|i| i.rgba.len())
                .sum::<usize>(),
            _ => 0,
        })
        .sum();
    println!(
        "{}: {} instances of {} models ({} triangles stored, {} placed), {} lights, environment {:?}; scene validated",
        built.scene.name,
        r.instances,
        r.models,
        r.triangles_stored,
        r.triangles_placed,
        r.lights,
        built.scene.environment
    );
    println!(
        "collision scene: {} instances of {} models, {} shapes; validated",
        r.collision_instances, r.collision_models, r.collision_shapes
    );
    println!(
        "left out: {} hidden instances, {} instances without geometry, {} lost to failed models, {} with invalid transforms; lights {:?}",
        r.hidden_skipped,
        r.instances_without_geometry,
        r.instances_lost,
        r.invalid_transforms,
        r.lights_left_out
    );
    println!(
        "environment from {:?}{}",
        r.environment_source,
        if r.environment_notes.is_empty() {
            String::new()
        } else {
            format!("; not usable: {:?}", r.environment_notes)
        }
    );
    let a = &built.atmosphere;
    println!(
        "beside the scene (atmosphere): {} suns, {} dynamic skies, fog {}, {} zones, {} GI volumes; left out: {}",
        a.suns.len(),
        a.dynamic_skies.len(),
        a.fog.is_some(),
        a.zones.len(),
        a.gi.volumes.len(),
        a.left_out()
            .iter()
            .map(|l| format!("{} x{}", l.what, l.count))
            .collect::<Vec<_>>()
            .join(", ")
    );
    for (p, why) in r.model_failures.iter().chain(&r.collision_failures) {
        println!("  failed {p}: {why}");
    }
    let d = &built.description;
    println!(
        "not in the scenes (in the description): {} spawns, {} portals, {} blocking volumes, {} environment settings",
        d.spawns.len(),
        d.portals.len(),
        d.volumes.len(),
        d.environments.len()
    );
    println!(
        "images in the scene: {} MB; {:.1}s, peak RSS {}",
        images / 1_000_000,
        started.elapsed().as_secs_f64(),
        peak_rss_text()
    );
    Ok(())
}

/// Builds the visible and collision scenes of every level (without textures) and counts what
/// passes validation.
pub fn check_bridge_scenes_cmd(game: &Path, filter: &str, limit: Option<usize>) -> Result<()> {
    let started = Instant::now();
    let wf = Warframe::open(game)?.with_textures(false);
    let mut levels: Vec<String> = wf
        .levels()?
        .into_iter()
        .filter(|p| p.contains(filter))
        .collect();
    if let Some(n) = limit {
        levels.truncate(n);
    }
    let mut head = Package::open_in(game, "H.AnimRetarget")?.expect("H.AnimRetarget");
    let mut body = Package::open_in(game, "B.AnimRetarget")?.expect("B.AnimRetarget");
    let bin = PackagesBin::load(game)?;
    let mut types = TypeResolver::new(&bin)?;
    let (mut ok, mut instances, mut models, mut lights, mut shapes) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    let (mut lights_out, mut failures_models, mut lost, mut invalid) =
        (0usize, 0usize, 0usize, 0usize);
    let mut errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut model_errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut left_out: BTreeMap<&'static str, usize> = BTreeMap::new();
    // Where each scene's environment came from, and the sky pictures that could not be used
    let mut env_sources: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut env_unusable: BTreeMap<String, usize> = BTreeMap::new();
    for (n, path) in levels.iter().enumerate() {
        if n > 0 && n % 250 == 0 {
            eprintln!(
                "{n}/{} levels, {:.0}s, {}",
                levels.len(),
                started.elapsed().as_secs_f64(),
                peak_rss_text()
            );
        }
        let built = (|| -> Result<_> {
            let h = head.read_path(path)?.unwrap_or_default();
            let b = body.read_path(path)?.unwrap_or_default();
            anyhow::ensure!(
                wf_unpack::header::CommonHeader::parse(&h)?.kind == KIND_LEVEL,
                "not a level"
            );
            let raw = RawLevel::parse(&h, &b)?;
            let mut d = LevelDescription::build(&raw, path, &mut types)?;
            for s in d.static_levels.clone() {
                let (h, b) = (head.read_path(&s)?, body.read_path(&s)?);
                let sl = StaticLevel::parse(&h.unwrap_or_default(), &b.unwrap_or_default())?;
                d.add_static_level(&s, &sl);
            }
            wf.scene_of(d, &SceneOptions::default())
        })();
        match built {
            Ok(w) => {
                ok += 1;
                let r = &w.report;
                instances += r.instances;
                models += r.models;
                lights += r.lights;
                shapes += r.collision_shapes;
                lights_out += r.lights_left_out.values().sum::<usize>();
                failures_models += r.model_failures.len() + r.collision_failures.len();
                lost += r.instances_lost;
                invalid += r.invalid_transforms;
                for (why, c) in &r.lights_left_out {
                    *left_out.entry(why).or_default() += c;
                }
                *env_sources
                    .entry(match r.environment_source {
                        EnvironmentSource::None => "none",
                        EnvironmentSource::SkyLightColor => "sky light colour",
                        EnvironmentSource::GlobalCubeMap { .. } => "global cube map",
                    })
                    .or_default() += 1;
                for note in &r.environment_notes {
                    *env_unusable.entry(note.clone()).or_default() += 1;
                }
                for (_, why) in r.model_failures.iter().chain(&r.collision_failures) {
                    let key: String = why.group_key();
                    *model_errors.entry(key).or_default() += 1;
                }
            }
            Err(e) => {
                let key: String = format!("{e:#}")
                    .replace(path.as_str(), "<level>")
                    .group_key();
                errors.entry(key).or_default().push(path.clone());
            }
        }
    }
    let failed: usize = errors.values().map(Vec::len).sum();
    println!(
        "{ok}/{} levels build and validate ({failed} failed): {instances} instances, {models} models, {lights} lights, {shapes} collision shapes",
        levels.len()
    );
    println!(
        "left out: {lights_out} lights {left_out:?}; {failures_models} models failed ({lost} instances lost), {invalid} invalid transforms"
    );
    println!("environments: {env_sources:?}");
    for (note, n) in &env_unusable {
        println!("  {n:>6} levels name a sky picture that cannot be used: {note}");
    }
    for (why, n) in &model_errors {
        println!("  {n:>6} model failures: {why}");
    }
    for (msg, list) in &errors {
        println!(
            "  {:>6}  {msg}\n          e.g. {}",
            list.len(),
            list.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        );
    }
    println!(
        "{:.0}s, peak RSS {}",
        started.elapsed().as_secs_f64(),
        peak_rss_text()
    );
    Ok(())
}

/// Texture size `check-conformance` bakes at unless told otherwise: a level at full size peaked at
/// 7 GB of resident memory, and the checks look at the geometry and the material slots, not at
/// the pixels.
pub const CONFORMANCE_MAX_TEXTURE: u32 = 256;

/// What `check-conformance` takes besides the checker's own options.
#[derive(Debug, PartialEq)]
pub struct ConformanceOptions {
    pub max_texture: u32,
    pub no_textures: bool,
    /// The rest, for `rewrite_bridge::conformance::cli::run`.
    pub checker_args: Vec<String>,
}

/// Takes `--max-texture N` and `--no-textures` out of the arguments (they work before and after
/// the checker's options) and returns them with the arguments the checker keeps.
pub fn conformance_options(
    max_texture: Option<u32>,
    no_textures: bool,
    args: Vec<String>,
) -> Result<ConformanceOptions> {
    let (mut max_texture, mut no_textures) = (max_texture, no_textures);
    let mut checker_args = Vec::with_capacity(args.len());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--no-textures" => no_textures = true,
            "--max-texture" => {
                let v = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--max-texture needs a number of pixels"))?;
                max_texture = Some(v.parse().map_err(|_| {
                    anyhow::anyhow!("--max-texture {v:?} is not a number of pixels")
                })?);
            }
            _ => checker_args.push(a),
        }
    }
    Ok(ConformanceOptions {
        max_texture: max_texture.unwrap_or(CONFORMANCE_MAX_TEXTURE),
        no_textures,
        checker_args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn conformance_texture_options() {
        let o = conformance_options(None, false, v(&["--kind", "level", "--limit", "5"])).unwrap();
        assert_eq!(o.max_texture, CONFORMANCE_MAX_TEXTURE);
        assert!(!o.no_textures);
        assert_eq!(o.checker_args, v(&["--kind", "level", "--limit", "5"]));

        // after the checker's options too, and the checker never sees them
        let o = conformance_options(
            None,
            false,
            v(&[
                "--kind",
                "level",
                "--max-texture",
                "128",
                "--no-textures",
                "-v",
            ]),
        )
        .unwrap();
        assert_eq!(o.max_texture, 128);
        assert!(o.no_textures);
        assert_eq!(o.checker_args, v(&["--kind", "level", "-v"]));

        // the ones clap parsed before the first checker option
        let o = conformance_options(Some(2048), true, vec![]).unwrap();
        assert_eq!((o.max_texture, o.no_textures), (2048, true));

        assert!(conformance_options(None, false, v(&["--max-texture"])).is_err());
        assert!(conformance_options(None, false, v(&["--max-texture", "big"])).is_err());
    }
}
