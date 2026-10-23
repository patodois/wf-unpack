//! The `anim` and `check-anims` commands, and the clips for `model --animations`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};

use wf_unpack::animation::{Animation, AnimationPackages, KIND_ANIMATION, SOURCE_FPS};
use wf_unpack::gltf::Clip;
use wf_unpack::groupkey::GroupKey;
use wf_unpack::header::CommonHeader;
use wf_unpack::model::Model;
use wf_unpack::scene::SceneSkin;
use wf_unpack::toc::{clip_filter_matches, Entry};

/// Animations live under these folders; a few have no `.fbx` extension.
fn may_be_animation(e: &Entry) -> bool {
    e.path.contains("Animations/") || e.path.ends_with("_anim.fbx")
}

/// A short name for a clip: its path without the folder prefix and extension.
fn clip_name(path: &str) -> String {
    let p = path
        .strip_prefix("/Lotus/Animations/")
        .unwrap_or(path.trim_start_matches('/'));
    p.strip_suffix("_anim.fbx")
        .or_else(|| p.strip_suffix(".fbx"))
        .unwrap_or(p)
        .to_string()
}

/// Bone names of a skinned model.
fn model_bones(game: &Path, path: &str) -> Result<Vec<String>> {
    let model = Model::load(game, path)?;
    let sk = model.skeleton.context("the model has no skeleton")?;
    Ok(sk.bones.into_iter().map(|b| b.name).collect())
}

pub fn anim_cmd(game: &Path, target: &str, filter: Option<&str>) -> Result<()> {
    let mut pk = AnimationPackages::open(game)?;
    // A path in the packages is either one clip or a model; anything else is a skeleton name
    let entry = pk.entries().iter().find(|e| e.path == target).cloned();
    if let Some(e) = &entry {
        if let Some((raw, _)) = pk.header(e)? {
            let clips = pk.decode_with(&e.path, &raw)?;
            return describe_file(&clips);
        }
    }
    let (skeleton, bones): (Option<&str>, Option<HashSet<String>>) = if entry.is_some() {
        (None, Some(model_bones(game, target)?.into_iter().collect()))
    } else {
        (Some(target), None)
    };
    let mut found: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut count = 0;
    for i in 0..pk.entries().len() {
        let e = pk.entries()[i].clone();
        if !may_be_animation(&e) || filter.is_some_and(|f| !clip_filter_matches(f, &e.path)) {
            continue;
        }
        let Some((_, h)) = pk.header(&e)? else {
            continue;
        };
        for s in &h.skeletons {
            let usable = match (&skeleton, &bones) {
                (Some(name), _) => s.name == *name,
                (_, Some(set)) => s.bones.iter().all(|b| set.contains(&b.name)),
                _ => false,
            };
            if !usable {
                continue;
            }
            for a in &s.actions {
                count += 1;
                found.entry(s.name.clone()).or_default().push((
                    e.path.clone(),
                    format!(
                        "{:>6.2}s {:>4} keys {:>3} bones  root {:>5.2} m  {}",
                        1.0 / a.rate,
                        a.frames,
                        s.bones.len(),
                        s.root_translation.iter().map(|v| v * v).sum::<f32>().sqrt(),
                        e.path
                    ),
                ));
            }
        }
    }
    if count == 0 {
        bail!("no clip found for {target} (a skeleton name, a model or a clip path)");
    }
    // The total first, so a long list below can be skimmed or cut off
    match (&skeleton, filter) {
        (None, Some(f)) => println!(
            "fits this model: {count} clips ({} skeletons) matching {f:?}",
            found.len()
        ),
        (None, None) => println!("fits this model: {count} clips ({} skeletons)", found.len()),
        (Some(name), Some(f)) => println!("skeleton {name}: {count} clips matching {f:?}"),
        (Some(name), None) => println!("skeleton {name}: {count} clips"),
    }
    for (name, mut lines) in found {
        lines.sort();
        println!("skeleton {name}: {} clips", lines.len());
        for (_, l) in lines {
            println!("  {l}");
        }
    }
    Ok(())
}

fn describe_file(clips: &[Animation]) -> Result<()> {
    for a in clips {
        println!("{}", a.path);
        println!(
            "skeleton {} ({} bones), take {:?}: {} keys over {:.3} s ({:.1} keys/s, source {} fps)",
            a.skeleton,
            a.bones.len(),
            a.name,
            a.times.len(),
            a.duration,
            a.times.len() as f32 / a.duration.max(1e-6),
            SOURCE_FPS
        );
        let count = |f: &dyn Fn(&wf_unpack::animation::BoneTrack) -> bool| {
            a.tracks.iter().filter(|t| f(t)).count()
        };
        println!(
            "tracks: {} translation, {} rotation, {} scale animated; {} bones constant",
            count(&|t| t.translation.is_some()),
            count(&|t| t.rotation.is_some()),
            count(&|t| t.scale.is_some()),
            count(&|t| t.translation.is_none() && t.rotation.is_none() && t.scale.is_none())
        );
        let r = a.root_translation;
        println!(
            "root motion: rotation {:?}, translation {:.3?} (root bone moves {:?})",
            a.root_rotation,
            r,
            a.root_displacement()
        );
        for e in &a.events {
            println!(
                "event at {:.3} ({:.3} s): {} {} {}",
                e.phase,
                e.phase * a.duration,
                e.name,
                e.param,
                e.direction
            );
        }
        if let Some(rate) = a.attribute("Rate") {
            println!("play rate attribute: {rate}");
        }
        println!("keys (s): {:.3?}", a.times);
    }
    Ok(())
}

/// Channels of every compatible clip whose path matches `filter`, for the glTF
/// of a model whose skin is `skin`. A clip is compatible when all the bones of
/// one of its skeletons exist in the model.
pub fn clips_for_skin(game: &Path, skin: &SceneSkin, filter: &str) -> Result<Vec<Clip>> {
    let bone_index: HashMap<&str, usize> = skin
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let mut pk = AnimationPackages::open(game)?;
    let mut clips = Vec::new();
    let (mut wrong_skeleton, mut seen) = (0usize, 0usize);
    for i in 0..pk.entries().len() {
        let e = pk.entries()[i].clone();
        if !may_be_animation(&e) || !clip_filter_matches(filter, &e.path) {
            continue;
        }
        let Some((raw, h)) = pk.header(&e)? else {
            continue;
        };
        seen += 1;
        let usable: Vec<usize> = h
            .skeletons
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.bones
                    .iter()
                    .all(|b| bone_index.contains_key(b.name.as_str()))
            })
            .map(|(i, _)| i)
            .collect();
        if usable.is_empty() {
            wrong_skeleton += 1;
            continue;
        }
        if clips.len() + usable.len() > 400 {
            bail!("--animations {filter:?} matches too many clips; narrow it");
        }
        let all = pk.decode_with(&e.path, &raw)?;
        let single = all.len() == 1;
        for a in &all {
            let skel_index = a.skeleton_index;
            if !usable.contains(&skel_index) {
                continue;
            }
            let map: Vec<usize> = a
                .bones
                .iter()
                .map(|b| bone_index[b.name.as_str()])
                .collect();
            let channels = a
                .channels(true)
                .into_iter()
                .map(|mut c| {
                    c.node = map[c.node];
                    c
                })
                .collect();
            let base = clip_name(&e.path);
            clips.push(Clip {
                name: if single {
                    base
                } else {
                    format!("{base}:{}:{}", a.skeleton, a.name)
                },
                channels,
                extras: Some(clip_extras(a)),
            });
        }
    }
    eprintln!(
        "animations: {} clips for this skeleton ({seen} animation files match {filter:?}, {wrong_skeleton} use other skeletons)",
        clips.len()
    );
    Ok(clips)
}

/// What glTF has no field for: duration, root motion (in the exported, right-handed
/// space), events with their time in seconds, and the header's attribute text.
fn clip_extras(a: &Animation) -> serde_json::Value {
    let (t, q) = (a.root_translation, a.root_rotation);
    serde_json::json!({
        "source": a.path,
        "skeleton": a.skeleton,
        "duration": a.duration,
        "rootMotion": {
            "translation": [0.0 - t[0], t[1], t[2]],
            "rotation": [q[0], 0.0 - q[1], 0.0 - q[2], q[3]],
        },
        "events": a.events.iter().map(|e| serde_json::json!({
            "time": e.phase * a.duration,
            "name": e.name,
            "param": e.param,
            "direction": e.direction,
        })).collect::<Vec<_>>(),
        "attributes": a.attributes,
    })
}

/// Peak resident set size of this process, where the platform tells.
fn peak_rss() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: getrusage fills the struct we hand it
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } == 0;
        // macOS reports bytes
        ok.then_some(ru.ru_maxrss as u64)
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let kb = status
            .lines()
            .find_map(|l| l.strip_prefix("VmHWM:"))?
            .trim()
            .trim_end_matches("kB")
            .trim()
            .parse::<u64>()
            .ok()?;
        Some(kb * 1024)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn angle(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let d: f32 = (0..4).map(|i| a[i] * b[i]).sum::<f32>().abs().min(1.0);
    2.0 * d.acos()
}

/// Decodes every animation file, one at a time, and summarises what it found.
pub fn check_anims_cmd(game: &Path) -> Result<()> {
    let started = Instant::now();
    let mut pk = AnimationPackages::open(game)?;
    pk.set_nocache();
    let mut order: Vec<usize> = (0..pk.entries().len()).collect();
    // One forward pass over the .cache
    order.sort_by_key(|&i| pk.entries()[i].offset);
    let (mut files, mut clips, mut failed_files) = (0usize, 0usize, 0usize);
    let mut errors: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut flags: BTreeMap<u16, usize> = BTreeMap::new();
    let (mut tracks, mut frame0_checked, mut frame0_off) = (0usize, 0usize, 0usize);
    let (mut worst_rot, mut worst_pos) = (0.0f32, 0.0f32);
    let mut offenders: Vec<(f32, String)> = Vec::new();
    let (mut keys, mut non_ascending) = (0usize, 0usize);
    let (mut scale_min, mut scale_max) = (f32::MAX, f32::MIN);
    let (mut whole_30, mut clips_total_duration) = (0usize, 0.0f64);
    let mut skeletons: BTreeMap<String, usize> = BTreeMap::new();
    let mut compression: BTreeMap<String, (usize, usize, String)> = BTreeMap::new();
    for &i in &order {
        let e = pk.entries()[i].clone();
        // Only entries that start with an animation header are read further
        let Ok(raw) = pk.read_header(&e) else {
            continue;
        };
        let Ok(common) = CommonHeader::parse(&raw) else {
            continue;
        };
        if common.kind != KIND_ANIMATION {
            continue;
        }
        files += 1;
        let comp = common
            .attributes
            .lines()
            .find_map(|l| l.trim().strip_prefix("Compressed="))
            .unwrap_or("(none)")
            .to_string();
        match pk.decode_with(&e.path, &raw) {
            Ok(list) => {
                compression.entry(comp).or_insert((0, 0, e.path.clone())).0 += 1;
                for a in &list {
                    clips += 1;
                    *skeletons.entry(a.skeleton.clone()).or_default() += 1;
                    let w = f64::from(a.duration * SOURCE_FPS);
                    if (w - w.round()).abs() < 0.02 {
                        whole_30 += 1;
                    }
                    clips_total_duration += f64::from(a.duration);
                    keys += a.times.len();
                    non_ascending += a.times.windows(2).filter(|w| w[1] <= w[0]).count();
                    for (bone, t) in a.tracks.iter().enumerate() {
                        tracks += 1;
                        *flags.entry(t.flags).or_default() += 1;
                        for v in t.scale.iter().flatten().flatten() {
                            scale_min = scale_min.min(*v);
                            scale_max = scale_max.max(*v);
                        }
                        if let Some(first) = t.rotation.as_ref().and_then(|r| r.first()) {
                            frame0_checked += 1;
                            let d = angle(first, &t.initial_rotation);
                            worst_rot = worst_rot.max(d);
                            frame0_off += usize::from(d > 0.02);
                        }
                        if let Some(first) = t.translation.as_ref().and_then(|p| p.first()) {
                            frame0_checked += 1;
                            let d = (0..3)
                                .map(|k| (first[k] - t.initial_translation[k]).abs())
                                .fold(0.0, f32::max);
                            worst_pos = worst_pos.max(d);
                            if d > 0.005 {
                                frame0_off += 1;
                                offenders.push((
                                    d,
                                    format!(
                                        "{} {} (flags {}): initial {:?}, first key {:?}",
                                        a.path,
                                        a.bones[bone].name,
                                        t.flags,
                                        t.initial_translation,
                                        first
                                    ),
                                ));
                            }
                        }
                    }
                }
            }
            Err(err) => {
                compression.entry(comp).or_insert((0, 0, e.path.clone())).1 += 1;
                failed_files += 1;
                let key = squash(&format!("{:#}", err.root_cause()));
                let slot = errors.entry(key).or_insert((0, e.path.clone()));
                slot.0 += 1;
            }
        }
    }
    println!(
        "animation files: {} ok, {failed_files} failed ({files} files of type 132), {clips} clips, {tracks} bone tracks, {keys} keys",
        files - failed_files
    );
    println!(
        "channel flags (per bone track): {}",
        flags
            .iter()
            .map(|(f, n)| format!("{f}: {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "first key vs. the stored initial transform: {frame0_checked} animated components compared, {frame0_off} differ (rotation by > 0.02 rad or translation by > 5 mm); worst rotation {worst_rot:.4} rad, worst translation {worst_pos:.4} m"
    );
    println!("scale channel values range from {scale_min:.3} to {scale_max:.3}");
    println!(
        "durations that are a whole number of 1/30 s: {whole_30} of {clips} clips; total {:.0} s of animation; key times not strictly ascending: {non_ascending}",
        clips_total_duration
    );
    offenders.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (d, text) in offenders.iter().take(5) {
        println!("  first-key mismatch {d:.4}: {text}");
    }
    let mut sk: Vec<_> = skeletons.into_iter().collect();
    sk.sort_by_key(|s| std::cmp::Reverse(s.1));
    println!(
        "{} skeleton names; most used: {}",
        sk.len(),
        sk.iter()
            .take(8)
            .map(|(n, c)| format!("{n} {c}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "Compressed attribute (ok/failed): {}",
        compression
            .iter()
            .map(|(k, (ok, bad, ex))| format!("{k}: {ok}/{bad} (e.g. {ex})"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !errors.is_empty() {
        println!("files the reader does not handle, counted per message (N is a number):");
    }
    for (msg, (n, example)) in &errors {
        println!("  {n:>5}  {msg}\n         e.g. {example}");
    }
    println!(
        "{:.1}s{}",
        started.elapsed().as_secs_f64(),
        peak_rss().map_or(String::new(), |b| format!(
            ", peak RSS {:.0} MB",
            b as f64 / 1e6
        ))
    );
    Ok(())
}

/// Messages grouped without their numbers.
fn squash(msg: &str) -> String {
    msg.group_key()
}
