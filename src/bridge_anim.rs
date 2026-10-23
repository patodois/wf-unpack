//! Animation clips as rewrite-bridge [`Animation`]s.
//!
//! [`crate::animation::Animation`] holds a decoded type 132 clip in the game's
//! space, with one track per bone of the clip's skeleton. The bridge wants
//! glTF-style channels on the nodes of a bridge model, so [`convert`] does three
//! things:
//!
//! - **Nodes.** A clip names its bones; the bridge model built by
//!   [`crate::bridge`] has the model node as node 0 and the bones after it, so a
//!   bone goes to the node of the same name. `node_of_bone[i]` is the node of
//!   the clip's bone `i`.
//! - **Handedness.** As for the models: x is negated, so translations are
//!   `(-x, y, z)` and rotations `(x, -y, -z, w)`
//!   (see [`crate::animation::Animation::channels`]).
//! - **Time.** Key times are seconds from the start; the clip's `duration_s` is
//!   the header's `1 / rate`, the time of its last key.
//!
//! Every bone gets a translation and a rotation channel. An animated component
//! has its keys; a component the clip does not animate is a one-key channel of the
//! clip's stored initial transform, which is what the game applies to that bone
//! while the clip plays (it can differ from the model's rest pose).
//! Scale channels exist only where the clip animates scale.
//!
//! **Root motion** is the root bone's translation channel, as the data has it: a
//! Sword `JogForward` moves the root 3.52 m in z. The header also records the
//! whole clip's root displacement; that is in [`ClipInfo`] with the same
//! handedness.
//!
//! **Events** (`StepL`, `StepR`, `JumpStartImpulse`, sound cues) are not part of
//! the bridge's `Animation`, so they travel in [`ClipInfo`], a side table keyed
//! by the clip's name that [`crate::bridge::Warframe::clip_info`] and
//! [`crate::bridge::AnimatedClips`] return.
//!
//! **Interpolation** is linear (spherical for rotations): the data has no
//! interpolation field.
//!
//! What the conversion changes, and why, is limited to what the bridge's
//! validation demands:
//!
//! - Key times are clamped to `[0, duration]` and keys that do not come after
//!   the previous one are dropped (the bridge wants strictly increasing times).
//! - Rotations are normalised (the data stores a 48-bit packed quaternion or an
//!   f32 quaternion; both are unit up to rounding).
//!
//! A clip with a non-finite value or a rotation of length zero is not converted;
//! the caller gets the reason instead of a broken clip.

use anyhow::{bail, ensure, Result};
use rewrite_bridge::{Animation as BridgeAnimation, Channel, Interpolation, Target};

use crate::animation::{Animation, Target as GameTarget};
use crate::clipspec::ClipSpec;

/// An event of a clip, with its time in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipEvent {
    /// Seconds from the start of the clip (`phase * duration`).
    pub time_s: f32,
    /// Fraction of the clip (0..1), as the data stores it.
    pub phase: f32,
    pub name: String,
    pub param: String,
    pub direction: String,
}

/// What the bridge's `Animation` has no field for.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipInfo {
    /// The asset path of the animation file.
    pub source: String,
    /// The skeleton name in the clip's header (`Tenno`, `ExcaliburBody1`, ...).
    pub skeleton: String,
    /// The take inside the file (`Take 001`).
    pub take: String,
    pub duration_s: f32,
    /// The whole clip's root displacement, rotation and translation, in the bridge's space
    /// (x negated). It equals the root bone's translation channel from its first to its last key.
    pub root_rotation: [f32; 4],
    pub root_translation: [f32; 3],
    /// Events, in the order the data lists them.
    pub events: Vec<ClipEvent>,
    /// Where the events come from: the clip's own header text (`true`), or, when that has none,
    /// the clip's entity in Packages.bin (`false`). Some clips (the player's rolls and bullet
    /// jump) have them only there.
    pub events_from_header: bool,
    /// `Rate` of the header text (or, without one, of the Packages.bin entity): the play-speed
    /// multiplier the game applies at run time. Not applied to the channels.
    pub rate: Option<f32>,
    /// What the clip's Packages.bin entity and its own header text say together (header keys win;
    /// `RootAdjustment`, `RootConditioning`, `TransitionTime`, ...), when either sets any.
    pub spec: Option<ClipSpec>,
    /// Keys dropped because their time was not after the previous key's (summed over channels).
    pub dropped_keys: usize,
}

impl ClipInfo {
    /// The time in seconds of the first event with this name.
    pub fn event_time(&self, name: &str) -> Option<f32> {
        self.events
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.time_s)
    }
}

fn to_target(t: GameTarget) -> Target {
    match t {
        GameTarget::Translation => Target::Translation,
        GameTarget::Rotation => Target::Rotation,
        GameTarget::Scale => Target::Scale,
    }
}

/// Clamps to `[0, duration]`, keeps keys whose time exceeds the previous kept one, and normalises
/// rotations. Returns the number of keys dropped.
fn clean(
    times: &mut Vec<f32>,
    values: &mut Vec<f32>,
    components: usize,
    duration: f32,
    what: &str,
) -> Result<usize> {
    ensure!(
        values.len() == times.len() * components,
        "{what}: {} values for {} keys",
        values.len(),
        times.len()
    );
    ensure!(
        values.iter().all(|v| v.is_finite()) && times.iter().all(|t| t.is_finite()),
        "{what}: a value or key time is not finite"
    );
    let mut kept_times: Vec<f32> = Vec::with_capacity(times.len());
    let mut kept_values: Vec<f32> = Vec::with_capacity(values.len());
    let mut dropped = 0;
    for (t, v) in times.iter().zip(values.chunks_exact(components)) {
        let t = t.clamp(0.0, duration);
        if kept_times.last().is_some_and(|&p| t <= p) {
            dropped += 1;
            continue;
        }
        kept_times.push(t);
        kept_values.extend_from_slice(v);
    }
    if components == 4 {
        for q in kept_values.as_chunks_mut::<4>().0 {
            let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
            ensure!(len > 1e-6, "{what}: a rotation of length zero");
            for c in q.iter_mut() {
                *c /= len;
            }
        }
    }
    *times = kept_times;
    *values = kept_values;
    Ok(dropped)
}

/// Converts a decoded clip. `node_of_bone[i]` is the bridge node of the clip's bone `i`; `name`
/// is the clip's name in the bridge model. `spec` is the clip's Packages.bin entity, used for
/// events and the play rate when the header text has none.
pub fn convert(
    clip: &Animation,
    node_of_bone: &[usize],
    name: &str,
    spec: Option<ClipSpec>,
) -> Result<(BridgeAnimation, ClipInfo)> {
    ensure!(
        node_of_bone.len() == clip.bones.len(),
        "{} nodes for {} bones",
        node_of_bone.len(),
        clip.bones.len()
    );
    ensure!(
        clip.duration.is_finite() && clip.duration > 0.0,
        "duration {}",
        clip.duration
    );
    let mut channels = Vec::new();
    let mut dropped_keys = 0;
    for c in clip.channels(true) {
        let target = to_target(c.target);
        let mut times = c.times;
        let mut values = c.values;
        let what = format!("bone {} {:?}", clip.bones[c.node].name, c.target);
        dropped_keys += clean(
            &mut times,
            &mut values,
            target.components(),
            clip.duration,
            &what,
        )?;
        channels.push(Channel::new(
            node_of_bone[c.node],
            target,
            Interpolation::Linear,
            times,
            values,
        ));
    }
    let header_events = !clip.events.is_empty();
    let events = if header_events {
        &clip.events
    } else {
        match &spec {
            Some(s) => &s.events,
            None => &clip.events,
        }
    };
    let events: Vec<ClipEvent> = events
        .iter()
        .map(|e| ClipEvent {
            time_s: e.phase * clip.duration,
            phase: e.phase,
            name: e.name.clone(),
            param: e.param.clone(),
            direction: e.direction.clone(),
        })
        .collect();
    if events
        .iter()
        .any(|e| !e.time_s.is_finite() || !e.phase.is_finite())
    {
        bail!("an event has no valid time");
    }
    let (t, q) = (clip.root_translation, clip.root_rotation);
    let info = ClipInfo {
        source: clip.path.clone(),
        skeleton: clip.skeleton.clone(),
        take: clip.name.clone(),
        duration_s: clip.duration,
        root_rotation: [q[0], -q[1], -q[2], q[3]],
        root_translation: [-t[0], t[1], t[2]],
        events,
        events_from_header: header_events,
        rate: clip
            .attribute("Rate")
            .and_then(|r| r.trim().parse().ok())
            .or_else(|| spec.as_ref().and_then(|s| s.rate)),
        spec: spec.filter(|s| s.in_packages),
        dropped_keys,
    };
    Ok((
        BridgeAnimation {
            name: name.to_string(),
            duration_s: clip.duration,
            channels,
        },
        info,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{AnimBone, AnimEvent, BoneTrack};

    fn bone(name: &str, parent: Option<usize>) -> AnimBone {
        AnimBone {
            name: name.into(),
            parent,
        }
    }

    fn track() -> BoneTrack {
        BoneTrack {
            flags: 0,
            initial_rotation: [0.0, 0.0, 0.0, 1.0],
            initial_translation: [1.0, 2.0, 3.0],
            translation: None,
            rotation: None,
            scale: None,
        }
    }

    /// A two-bone clip of 2 s: the root moves 4 m in z over three keys, the child is constant.
    fn clip() -> Animation {
        let mut root = track();
        root.translation = Some(vec![[0.5, 0.0, 0.0], [0.5, 0.0, 2.0], [0.5, 0.0, 4.0]]);
        Animation {
            path: "/Lotus/Animations/X/Walk_anim.fbx".into(),
            skeleton: "Tenno".into(),
            skeleton_index: 0,
            name: "Take 001".into(),
            bones: vec![bone("ROOT", None), bone("ARM", Some(0))],
            duration: 2.0,
            times: vec![0.0, 1.0, 2.0],
            tracks: vec![root, track()],
            events: vec![AnimEvent {
                phase: 0.25,
                name: "StepL".into(),
                param: "0".into(),
                direction: "BOTH".into(),
            }],
            attributes: "Attribs={\n    Rate=1.5\n}\n".into(),
            root_rotation: [0.0, 0.5, 0.5, 0.5],
            root_translation: [0.0, 0.0, 4.0],
        }
    }

    #[test]
    fn nodes_handedness_and_time() {
        let (a, info) = convert(&clip(), &[1, 2], "walk", None).unwrap();
        assert_eq!((a.name.as_str(), a.duration_s), ("walk", 2.0));
        // Root translation: 3 keys on node 1, x negated, z kept
        let t = a
            .channels
            .iter()
            .find(|c| c.node == 1 && c.target == Target::Translation)
            .unwrap();
        assert_eq!(t.times, [0.0, 1.0, 2.0]);
        assert_eq!(t.values[0], -0.5);
        assert_eq!(t.values[8], 4.0);
        // The child is constant: one key at 0 with its initial transform, mirrored
        let c = a
            .channels
            .iter()
            .find(|c| c.node == 2 && c.target == Target::Translation)
            .unwrap();
        assert_eq!(
            (c.times.clone(), c.values.clone()),
            (vec![0.0], vec![-1.0, 2.0, 3.0])
        );
        // Every bone has translation and rotation, nobody has scale
        assert_eq!(a.channels.len(), 4);
        assert_eq!(a.validate(3), Ok(()));
        assert!(a.validate(2).is_err());
        // The side table: root motion mirrored, events in seconds, the header's rate
        assert_eq!(info.root_translation, [-0.0, 0.0, 4.0]);
        assert_eq!(info.root_rotation, [0.0, -0.5, -0.5, 0.5]);
        assert_eq!(info.event_time("StepL"), Some(0.5));
        assert!(info.events_from_header);
        assert_eq!(info.rate, Some(1.5));
        assert_eq!(info.skeleton, "Tenno");
    }

    #[test]
    fn events_fall_back_to_packages_bin() {
        let mut c = clip();
        c.events.clear();
        c.attributes.clear();
        let spec = ClipSpec {
            path: c.path.clone(),
            in_packages: true,
            rate: Some(0.9),
            events: vec![AnimEvent {
                phase: 0.5,
                name: "JumpStartImpulse".into(),
                param: String::new(),
                direction: String::new(),
            }],
            ..ClipSpec::default()
        };
        let (_, info) = convert(&c, &[1, 2], "roll", Some(spec)).unwrap();
        assert!(!info.events_from_header);
        assert_eq!(info.event_time("JumpStartImpulse"), Some(1.0));
        assert_eq!(info.rate, Some(0.9));
        assert!(info.spec.is_some());
    }

    #[test]
    fn keys_are_made_valid_and_the_loss_is_counted() {
        let mut c = clip();
        // A repeated time, and a last key a hair past the duration
        c.times = vec![0.0, 1.0, 1.0];
        c.tracks[0].translation = Some(vec![[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]);
        let (a, info) = convert(&c, &[1, 2], "x", None).unwrap();
        let t = a
            .channels
            .iter()
            .find(|c| c.node == 1 && c.target == Target::Translation)
            .unwrap();
        assert_eq!(t.times, [0.0, 1.0]);
        assert_eq!(t.values.len(), 6);
        assert_eq!(info.dropped_keys, 1);
        assert_eq!(a.validate(3), Ok(()));
        let mut c = clip();
        c.times = vec![0.0, 1.0, 2.000_001];
        let (a, _) = convert(&c, &[1, 2], "x", None).unwrap();
        let t = a
            .channels
            .iter()
            .find(|c| c.node == 1 && c.target == Target::Translation)
            .unwrap();
        assert_eq!(t.times.last().copied(), Some(2.0));
    }

    #[test]
    fn rotations_are_normalised_and_garbage_is_refused() {
        let mut c = clip();
        c.tracks[1].initial_rotation = [0.0, 0.0, 0.0, 2.0];
        let (a, _) = convert(&c, &[1, 2], "x", None).unwrap();
        assert_eq!(a.validate(3), Ok(()));
        c.tracks[1].initial_rotation = [0.0; 4];
        assert!(convert(&c, &[1, 2], "x", None)
            .unwrap_err()
            .to_string()
            .contains("length zero"));
        let mut c = clip();
        c.tracks[0].translation = Some(vec![[f32::NAN, 0.0, 0.0]; 3]);
        assert!(convert(&c, &[1, 2], "x", None).is_err());
        assert!(convert(&clip(), &[1], "x", None).is_err());
    }
}
