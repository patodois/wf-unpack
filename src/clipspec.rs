//! What Packages.bin says about an animation clip.
//!
//! A clip has two descriptions: the attribute text in the common header of its
//! `H.Misc` entry (which [`crate::animation`] reads: `Rate`, `Events`, ...) and,
//! for most of the clips the game scripts refer to, an entity in `/Packages.bin`
//! with the same kind of text plus a few keys the header never has
//! (`RootAdjustment`, `RootConditioning`, `TrimStart`, `TrimEnd`). Some clips,
//! for example the player's `Player/Movement` rolls and bullet jump, carry their
//! events only there. The entity is named like the clip file
//! (`/Lotus/Animations/Player/Movement/RollForward_anim.fbx`) or, for derived
//! clips, without the extension; its parent chain is followed, so a clip inherits
//! what the entity it derives from sets.
//!
//! The header text of the clip itself can be merged in with [`ClipSpec::apply_header`]; its
//! keys win. Some clips have only a header (the player's `RollForwardMelee` sets its events and
//! `RootConditioning` there and inherits `RootAdjustment` from its entity's parent).
//!
//! Nothing is defaulted: a key the data does not set is `None`.

use std::collections::BTreeMap;

use anyhow::Result;

use crate::animation::AnimEvent;
use crate::ee::{Attributes, Value};
use crate::level::TypeResolver;

/// The values Packages.bin gives a clip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipSpec {
    /// The entity that was looked up.
    pub path: String,
    /// The entity (or, after [`ClipSpec::apply_header`], the header text) sets at least one of
    /// the keys below.
    pub in_packages: bool,
    /// `Attribs.Rate`: a play-speed multiplier (the clip's length in seconds is
    /// divided by it when the game plays it).
    pub rate: Option<f32>,
    /// `Attribs.TransitionTime`, seconds: how long the game blends the clip in.
    /// The data states no unit; seconds is the reading.
    pub transition_time: Option<f32>,
    /// `Attribs.TransitionOutTime`, seconds (reading): how long the game blends the clip out.
    pub transition_out_time: Option<f32>,
    /// `Events`: `Time` is a fraction of the clip (0..1), as in the header text.
    pub events: Vec<AnimEvent>,
    /// `Attribs.StartEvents`: names sent when the clip starts.
    pub start_events: Vec<String>,
    /// `Attribs.EndEvents`: names sent when the clip ends.
    pub end_events: Vec<String>,
    /// `RootAdjustment.overriddenMotion` when `useExistingMotion=0`: the
    /// displacement the game gives the character over the whole clip, in the
    /// game's (left-handed) space. Metres is the reading; the data states no unit.
    pub root_override: Option<[f32; 3]>,
    /// `RootConditioning`, e.g. `ROOTC_ADJUST_LINEAR`.
    pub root_conditioning: Option<String>,
    /// `TrimStart`; the data states no unit (frames is the reading).
    pub trim_start: Option<f32>,
    /// `TrimEnd`; see `trim_start`.
    pub trim_end: Option<f32>,
    /// Which entity of the parent chain set each key (`Attribs`, `Events`,
    /// `RootAdjustment`, `RootConditioning`, `TrimStart`, `TrimEnd`).
    pub sources: BTreeMap<String, String>,
}

fn strings(v: &Value) -> Vec<String> {
    v.items()
        .iter()
        .filter_map(|x| x.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn flag(v: Option<&Value>) -> bool {
    v.and_then(Value::text).is_some_and(|t| t.trim() == "1")
}

/// Events in the notation of a Packages.bin `Events` value.
pub fn events_of(v: &Value) -> Vec<AnimEvent> {
    v.items()
        .into_iter()
        .filter_map(|e| {
            let phase = e.get("Time")?.float()?;
            let name = e.get("Event")?.text()?.trim().to_string();
            let text = |k: &str| {
                e.get(k)
                    .and_then(Value::text)
                    .map(|t| t.trim().to_string())
                    .unwrap_or_default()
            };
            Some(AnimEvent {
                phase,
                name,
                param: text("Param"),
                direction: text("Direction"),
            })
        })
        .collect()
}

/// Sets the fields of `spec` for every key `get` provides; `source(key)` names where the key
/// came from.
fn absorb<'a>(
    spec: &mut ClipSpec,
    get: &dyn Fn(&str) -> Option<&'a Value>,
    source: &dyn Fn(&str) -> String,
) {
    for key in [
        "Attribs",
        "Events",
        "RootAdjustment",
        "RootConditioning",
        "TrimStart",
        "TrimEnd",
    ] {
        if get(key).is_some() {
            spec.sources.insert(key.to_string(), source(key));
        }
    }
    spec.in_packages = !spec.sources.is_empty();
    if let Some(attribs) = get("Attribs") {
        let float = |k: &str| attribs.get(k).and_then(Value::float);
        spec.rate = float("Rate").or(spec.rate);
        spec.transition_time = float("TransitionTime").or(spec.transition_time);
        spec.transition_out_time = float("TransitionOutTime").or(spec.transition_out_time);
        if let Some(v) = attribs.get("StartEvents") {
            spec.start_events = strings(v);
        }
        if let Some(v) = attribs.get("EndEvents") {
            spec.end_events = strings(v);
        }
    }
    if let Some(events) = get("Events") {
        spec.events = events_of(events);
    }
    if let Some(adj) = get("RootAdjustment") {
        spec.root_override = if flag(adj.get("useExistingMotion")) {
            None
        } else {
            adj.get("overriddenMotion")
                .and_then(Value::floats)
                .and_then(|v| <[f32; 3]>::try_from(v).ok())
        };
    }
    if let Some(v) = get("RootConditioning").and_then(Value::text) {
        spec.root_conditioning = Some(v.trim().to_string());
    }
    if let Some(v) = get("TrimStart").and_then(Value::float) {
        spec.trim_start = Some(v);
    }
    if let Some(v) = get("TrimEnd").and_then(Value::float) {
        spec.trim_end = Some(v);
    }
}

/// Reads the entity at `path` (and what it inherits). A path that is not in
/// Packages.bin gives a spec with `in_packages == false`.
pub fn clip_spec(types: &mut TypeResolver, path: &str) -> Result<ClipSpec> {
    let mut spec = ClipSpec {
        path: path.to_string(),
        ..ClipSpec::default()
    };
    let Some(ty) = types.resolve(path)? else {
        return Ok(spec);
    };
    absorb(
        &mut spec,
        &|key| ty.props.get(key).map(|(v, _)| v),
        &|key| {
            ty.props
                .get(key)
                .map(|(_, e)| e.clone())
                .unwrap_or_default()
        },
    );
    Ok(spec)
}

impl ClipSpec {
    /// Lets the clip's own header text (the attribute text in its `H.Misc` entry, which
    /// [`crate::header::CommonHeader`] reads) override what the entity says, key by key: a key
    /// the header sets wins, one it leaves out keeps the entity's value. `sources` then names the
    /// clip's path with `(H.Misc header)` for the keys the header set.
    pub fn apply_header(&mut self, attributes: &str) -> Result<()> {
        let attrs = Attributes::parse(attributes)?;
        let path = self.path.clone();
        absorb(self, &|key| attrs.get(key), &|_| {
            format!("{path} (H.Misc header)")
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::PackagesBin;

    fn bin() -> PackagesBin {
        PackagesBin::parse(&crate::packages::tests::build(&[
            (
                "/Lotus/Anim/",
                "Roll_anim.fbx",
                "",
                Some("Attribs={\nTransitionTime=0.1\nEndEvents={Done,Look}}\nRootAdjustment={\nuseExistingMotion=0\noverriddenMotion={0,0,8}\n}\n"),
            ),
            (
                "/Lotus/Anim/",
                "RollMelee",
                "Roll_anim.fbx",
                Some("RootConditioning=ROOTC_ADJUST_LINEAR\nTrimStart=3\n"),
            ),
            (
                "/Lotus/Anim/",
                "Jump_anim.fbx",
                "",
                Some("Events={\n{\nTime=0.01\nEvent=JumpStartImpulse\nParam=0\nDirection=BOTH\n}\n}\n"),
            ),
        ]))
        .unwrap()
    }

    #[test]
    fn reads_attributes_and_follows_the_parent() {
        let bin = bin();
        let mut types = TypeResolver::new(&bin).unwrap();
        let s = clip_spec(&mut types, "/Lotus/Anim/RollMelee").unwrap();
        assert!(s.in_packages);
        assert_eq!(s.root_override, Some([0.0, 0.0, 8.0]));
        assert_eq!(s.root_conditioning.as_deref(), Some("ROOTC_ADJUST_LINEAR"));
        assert_eq!(s.transition_time, Some(0.1));
        assert_eq!(s.end_events, ["Done", "Look"]);
        assert_eq!(s.trim_start, Some(3.0));
        assert_eq!(s.trim_end, None);
        assert_eq!(s.rate, None);
        // Each key remembers the entity that set it
        assert_eq!(s.sources["RootAdjustment"], "/Lotus/Anim/Roll_anim.fbx");
        assert_eq!(s.sources["RootConditioning"], "/Lotus/Anim/RollMelee");
    }

    #[test]
    fn the_clips_own_header_text_wins_key_by_key() {
        let bin = bin();
        let mut types = TypeResolver::new(&bin).unwrap();
        let mut s = clip_spec(&mut types, "/Lotus/Anim/RollMelee").unwrap();
        s.apply_header(
            "Attribs={\nTransitionOutTime=0.2\n}\nEvents={{Time=0.3,Event=Foley}}\nRootConditioning=ROOTC_X\n",
        )
        .unwrap();
        // Set by the header
        assert_eq!(s.transition_out_time, Some(0.2));
        assert_eq!(s.events.len(), 1);
        assert_eq!(s.events[0].name, "Foley");
        assert_eq!(s.root_conditioning.as_deref(), Some("ROOTC_X"));
        assert_eq!(
            s.sources["RootConditioning"],
            "/Lotus/Anim/RollMelee (H.Misc header)"
        );
        // Left to the entity: the header's Attribs has no TransitionTime or EndEvents
        assert_eq!(s.transition_time, Some(0.1));
        assert_eq!(s.end_events, ["Done", "Look"]);
        assert_eq!(s.root_override, Some([0.0, 0.0, 8.0]));
        assert_eq!(s.sources["RootAdjustment"], "/Lotus/Anim/Roll_anim.fbx");
        // A header can add to a clip with no entity at all
        let mut none = clip_spec(&mut types, "/Lotus/Anim/Missing_anim.fbx").unwrap();
        assert!(!none.in_packages);
        none.apply_header("Attribs={\nRate=0.9\n}\n").unwrap();
        assert!(none.in_packages);
        assert_eq!(none.rate, Some(0.9));
    }

    #[test]
    fn reads_events_and_unknown_paths() {
        let bin = bin();
        let mut types = TypeResolver::new(&bin).unwrap();
        let s = clip_spec(&mut types, "/Lotus/Anim/Jump_anim.fbx").unwrap();
        assert_eq!(s.events.len(), 1);
        assert_eq!(s.events[0].name, "JumpStartImpulse");
        assert!((s.events[0].phase - 0.01).abs() < 1e-6);
        assert_eq!(s.events[0].direction, "BOTH");
        assert!(s.root_override.is_none());
        let none = clip_spec(&mut types, "/Lotus/Anim/Missing_anim.fbx").unwrap();
        assert!(!none.in_packages && none.events.is_empty());
    }
}
