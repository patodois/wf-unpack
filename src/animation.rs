//! Skeletal animations: asset type 132, stored under `*_anim.fbx` (and, with
//! no extension, a few hundred poses/blend files that are other types).
//!
//! An animation is split over `H.Misc` (header) and `B.Misc` (body). Type 134
//! (`*_cin.fbx`, cinematics) has another layout and an `F.` part; it is not read.
//!
//! # Header
//!
//! After the [common header](crate::header) (whose attribute text may hold
//! `Attribs={ Rate=... }` and an `Events={...}` list):
//!
//! ```text
//! str32 path, str32 path2 (usually empty)
//! [90]  unknown (the same bytes in 21,182 of 23,300 clips)
//! u32   skeleton_count                       1 in most clips; cutscene clips carry several
//! skeleton_count x {
//!   str32 name                               "Tenno" for Warframes, "ExcaliburBody1", "Marine", ...
//!   u32   unk;  [1] if unk > 0               unk is always 0
//!   [36]  f32 unknown, f32 x4 root rotation (x, y, z, w), f32 x3 root translation, f32 unknown
//!   u32   bone_count
//!   bone_count x { str32 name, u16 parent, u16 id }   parents before children; the root's is 0
//!   u32   action_count                       always 1 outside cutscenes
//!   action_count x {
//!     [8]   unknown
//!     f32   rate                             1 / duration in seconds
//!     [4]   unknown
//!     str32 name                             "Take 001"
//!     [68]  17 x f32: position scale xyz, 3e-11, position offset xyz, 0,
//!                     scale scale xyz, 3e-11, scale offset xyz, 0, 0
//!     u32   frame_stride                     bytes per frame in the frame data
//!     u32   frame_offset                     of this action's frames in the frame data
//!   }
//!   [26]  unknown (holds the frame stride again)
//!   u32   frame_data_len
//!   action_count x { [12] bone count x3, u32 frame_count }
//!   [8]   zeros
//! }
//! u32 error_count, error_count x str32       always 0 in practice
//! ```
//!
//! # Body
//!
//! Per skeleton, in order: the frame data (`frame_data_len` bytes), then per
//! action:
//!
//! ```text
//! bone_count x { f32 x4 rotation, f32 x3 translation, f32 x3 ? }   initial transform
//! bone_count x u16 flags
//! bone_count x u16 offset                    of the bone's data inside a frame
//! frame_count x f32 time                     0..1, ascending; key spacing is irregular
//! ```
//!
//! The frame data is `frame_count` frames of `frame_stride` bytes. Within a
//! frame, bone `i` has its data at `offset[i]`, in this order:
//!
//! | flag | data | decoding |
//! |---|---|---|
//! | 1 | 3 x i16 | translation `offset + v * scale` per axis (header) |
//! | 2 | 3 x u16 | rotation, smallest three (below) |
//! | 4 | f32 | translation x (frame 0 equals the initial x) |
//! | 8 | 3 x i16 | scale `offset + v * scale` per axis (header) |
//! | 32, 0 | none | not animated: the initial transform |
//!
//! Rotations are 48 bits: the three smaller quaternion components as 15-bit
//! offset-binary integers (value 16384 is zero, divided by `16384 * sqrt(2)`),
//! the index of the largest component in the low two bits of the third word,
//! and the largest component's sign and magnitude rebuilt from unit length.
//! Component storage:
//!
//! ```text
//! word 1: D aaaaaaaaaaaaaa E    word 2: G H bbbbbbbbbbbbbb    word 3: cccccccccccccc JJ
//! first small  = b (+ H as bit 14)    second = a (+ D)    third = c (+ E)
//! "first/second/third" fill the remaining indices (skipping J) in ascending order
//! largest sign = G (1 = negative)
//! ```
//!
//! Time is a fraction of the duration; the duration is `1 / rate` seconds.
//! Almost every clip was sampled at 30 frames per second, so `duration * 30`
//! is a whole number.
//!
//! The header's root rotation and translation are the whole clip's root
//! motion; the same displacement is in the root bone's translation track.
//!
//! Handedness follows [`crate::scene`]: with `mirror_x`, translation x is
//! negated and the quaternion becomes (x, -y, -z, w).
//!
//! Format notes: Puxtril's Warframe-Exporter (`animation2` branch) documents
//! the header and body layout and the quaternion packing in outline; the
//! position/scale ranges, the time base and the root motion were worked out
//! here by checking the decoded first frame against each bone's stored
//! initial transform.

use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

use crate::bytes::Reader;
use crate::cache::Package;
use crate::header::CommonHeader;
use crate::toc::Entry;

/// The asset type of `*_anim.fbx` files.
pub const KIND_ANIMATION: u16 = 132;
/// The rate the source animations were sampled at (the time base is a
/// fraction of the duration; durations are whole multiples of 1/30 s).
pub const SOURCE_FPS: f32 = 30.0;

/// A bone of an animation's skeleton.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimBone {
    pub name: String,
    /// The root has none.
    pub parent: Option<usize>,
}

/// Per-action header fields.
#[derive(Debug, Clone)]
pub struct ActionHeader {
    pub name: String,
    pub frames: u32,
    /// 1 / duration in seconds.
    pub rate: f32,
    pub frame_stride: u32,
    pub frame_offset: u32,
    pub position_scale: [f32; 3],
    pub position_offset: [f32; 3],
    pub scale_scale: [f32; 3],
    pub scale_offset: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct SkeletonHeader {
    pub name: String,
    pub bones: Vec<AnimBone>,
    pub actions: Vec<ActionHeader>,
    pub frame_data_len: u32,
    /// Root motion of the whole clip: rotation (x, y, z, w) and translation.
    pub root_rotation: [f32; 4],
    pub root_translation: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct AnimHeader {
    pub common: CommonHeader,
    pub skeletons: Vec<SkeletonHeader>,
}

fn f32_at(b: &[u8], i: usize) -> f32 {
    f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())
}

impl AnimHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let common = CommonHeader::parse(data)?;
        ensure!(
            common.kind == KIND_ANIMATION,
            "not an animation (type {})",
            common.kind
        );
        let mut r = Reader::at(data, common.len)?;
        r.str32(4096)?;
        r.str32(4096)?;
        r.skip(90)?;
        let skeleton_count = r.u32()? as usize;
        ensure!(
            skeleton_count < 64,
            "implausible skeleton count {skeleton_count}"
        );
        let mut skeletons = Vec::with_capacity(skeleton_count);
        for _ in 0..skeleton_count {
            let name = r.str32(4096)?;
            let unk = r.u32()?;
            if unk > 0 {
                r.skip(1)?;
            }
            let root = r.bytes(36)?;
            let root_rotation = [
                f32_at(root, 1),
                f32_at(root, 2),
                f32_at(root, 3),
                f32_at(root, 4),
            ];
            let root_translation = [f32_at(root, 5), f32_at(root, 6), f32_at(root, 7)];
            let bone_count = r.u32()? as usize;
            ensure!(bone_count < 4096, "implausible bone count {bone_count}");
            let mut bones = Vec::with_capacity(bone_count);
            for i in 0..bone_count {
                let name = r.str32(256)?;
                let parent = usize::from(r.u16()?);
                r.skip(2)?;
                ensure!(
                    i == 0 || (parent < bone_count && parent != i),
                    "bone {name} has parent {parent} at index {i}"
                );
                bones.push(AnimBone {
                    name,
                    parent: (i > 0).then_some(parent),
                });
            }
            let action_count = r.u32()? as usize;
            ensure!(
                action_count < 4096,
                "implausible action count {action_count}"
            );
            let mut actions = Vec::with_capacity(action_count);
            for _ in 0..action_count {
                r.skip(8)?;
                let rate = r.f32()?;
                r.skip(4)?;
                let name = r.str32(4096)?;
                let m = r.bytes(68)?;
                let frame_stride = r.u32()?;
                let frame_offset = r.u32()?;
                let v3 = |at: usize| [f32_at(m, at), f32_at(m, at + 1), f32_at(m, at + 2)];
                actions.push(ActionHeader {
                    name,
                    frames: 0,
                    rate,
                    frame_stride,
                    frame_offset,
                    position_scale: v3(0),
                    position_offset: v3(4),
                    scale_scale: v3(8),
                    scale_offset: v3(12),
                });
            }
            r.skip(26)?;
            let frame_data_len = r.u32()?;
            for a in &mut actions {
                r.skip(12)?;
                a.frames = r.u32()?;
            }
            r.skip(8)?;
            skeletons.push(SkeletonHeader {
                name,
                bones,
                actions,
                frame_data_len,
                root_rotation,
                root_translation,
            });
        }
        let error_count = r.u32()? as usize;
        ensure!(error_count < 4096, "implausible error count {error_count}");
        for _ in 0..error_count {
            r.str32(65536)?;
        }
        ensure!(
            r.pos() == data.len(),
            "header not read to the end ({} of {} bytes)",
            r.pos(),
            data.len()
        );
        Ok(AnimHeader { common, skeletons })
    }
}

/// A channel flag bit, as stored per bone.
mod flag {
    pub const TRANSLATION: u16 = 1;
    pub const ROTATION: u16 = 2;
    pub const TRANSLATION_X: u16 = 4;
    pub const SCALE: u16 = 8;
    /// No data; like 0 the bone keeps its initial transform.
    pub const STATIC: u16 = 32;
}

/// Bytes a bone's flags take in every frame.
fn channel_size(flags: u16) -> usize {
    6 * usize::from(flags & flag::TRANSLATION != 0)
        + 6 * usize::from(flags & flag::ROTATION != 0)
        + 4 * usize::from(flags & flag::TRANSLATION_X != 0)
        + 6 * usize::from(flags & flag::SCALE != 0)
}

/// What one bone does during a clip. A component without keys keeps the
/// value it has in the initial transform for the whole clip.
#[derive(Debug, Clone)]
pub struct BoneTrack {
    pub flags: u16,
    pub initial_rotation: [f32; 4],
    pub initial_translation: [f32; 3],
    /// One value per key, when animated.
    pub translation: Option<Vec<[f32; 3]>>,
    pub rotation: Option<Vec<[f32; 4]>>,
    pub scale: Option<Vec<[f32; 3]>>,
}

/// An event listed in the header attributes (footsteps, effects, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimEvent {
    /// Fraction of the clip (0..1).
    pub phase: f32,
    pub name: String,
    pub param: String,
    pub direction: String,
}

/// One decoded clip.
#[derive(Debug, Clone)]
pub struct Animation {
    /// Path of the file in the packages.
    pub path: String,
    /// Skeleton name from the header ("Tenno" for many Warframe clips; others use
    /// "KirkDropSuit", "ExcaliburBody1", ... for the same 81 bones).
    pub skeleton: String,
    /// Index of that skeleton among the file's skeletons.
    pub skeleton_index: usize,
    /// Take name inside the file ("Take 001").
    pub name: String,
    pub bones: Vec<AnimBone>,
    pub duration: f32,
    /// Key times in seconds, ascending, first 0 and last `duration`.
    pub times: Vec<f32>,
    /// One track per bone.
    pub tracks: Vec<BoneTrack>,
    pub events: Vec<AnimEvent>,
    /// The common header's attribute text.
    pub attributes: String,
    /// Root motion of the whole clip in the game's space: rotation (x, y, z, w)
    /// and translation.
    pub root_rotation: [f32; 4],
    pub root_translation: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Translation,
    Rotation,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    /// Linear for vectors, spherical for rotations.
    Linear,
    Step,
}

/// A glTF-style animation channel on one bone of the animation's skeleton.
#[derive(Debug, Clone)]
pub struct Channel {
    /// Index into [`Animation::bones`].
    pub node: usize,
    pub target: Target,
    /// Seconds. A single key (time 0) is a constant.
    pub times: Vec<f32>,
    /// Flat values: 3 per key for translation and scale, 4 (x, y, z, w) for rotation.
    pub values: Vec<f32>,
    pub interpolation: Interpolation,
}

impl Animation {
    /// Value of a `Key=value` line of the attribute text.
    pub fn attribute(&self, key: &str) -> Option<&str> {
        self.attributes.lines().find_map(|l| {
            let (k, v) = l.trim().split_once('=')?;
            (k == key).then_some(v)
        })
    }

    /// Channels for every bone: animated components and, for the rest,
    /// the initial transform as a one-key constant (translation and rotation
    /// only; a missing scale is 1). `mirror_x` converts to the right-handed
    /// space of [`crate::scene`]. Rotations are made continuous (a quaternion
    /// and its negative are the same rotation) so interpolation takes the
    /// short way.
    pub fn channels(&self, mirror_x: bool) -> Vec<Channel> {
        let mx = if mirror_x { -1.0 } else { 1.0 };
        let mut out = Vec::new();
        for (node, t) in self.tracks.iter().enumerate() {
            match t.translation.as_ref().filter(|v| !v.is_empty()) {
                Some(v) => out.push(Channel {
                    node,
                    target: Target::Translation,
                    times: self.times.clone(),
                    values: v.iter().flat_map(|p| [p[0] * mx, p[1], p[2]]).collect(),
                    interpolation: Interpolation::Linear,
                }),
                None => out.push(Channel {
                    node,
                    target: Target::Translation,
                    times: vec![0.0],
                    values: vec![
                        t.initial_translation[0] * mx,
                        t.initial_translation[1],
                        t.initial_translation[2],
                    ],
                    interpolation: Interpolation::Linear,
                }),
            }
            let flip = |q: &[f32; 4]| [q[0], q[1] * mx, q[2] * mx, q[3]];
            match t.rotation.as_ref().filter(|v| !v.is_empty()) {
                Some(v) => {
                    let mut values = Vec::with_capacity(v.len() * 4);
                    let mut prev: Option<[f32; 4]> = None;
                    for q in v {
                        let mut q = flip(q);
                        if let Some(p) = prev {
                            let dot: f32 = (0..4).map(|i| p[i] * q[i]).sum();
                            if dot < 0.0 {
                                q = q.map(|c| -c);
                            }
                        }
                        prev = Some(q);
                        values.extend(q);
                    }
                    out.push(Channel {
                        node,
                        target: Target::Rotation,
                        times: self.times.clone(),
                        values,
                        interpolation: Interpolation::Linear,
                    });
                }
                None => out.push(Channel {
                    node,
                    target: Target::Rotation,
                    times: vec![0.0],
                    values: flip(&t.initial_rotation).to_vec(),
                    interpolation: Interpolation::Linear,
                }),
            }
            if let Some(v) = t.scale.as_ref().filter(|v| !v.is_empty()) {
                out.push(Channel {
                    node,
                    target: Target::Scale,
                    times: self.times.clone(),
                    values: v.iter().flatten().copied().collect(),
                    interpolation: Interpolation::Linear,
                });
            }
        }
        out
    }

    /// Displacement of the root bone between the first and last key, in the
    /// game's space, when the root is animated.
    pub fn root_displacement(&self) -> Option<[f32; 3]> {
        let v = self.tracks.first()?.translation.as_ref()?;
        let (a, b) = (v.first()?, v.last()?);
        Some([b[0] - a[0], b[1] - a[1], b[2] - a[2]])
    }
}

/// Decodes a 48-bit packed quaternion (x, y, z, w) from its three words.
pub fn unpack_quaternion(w1: u16, w2: u16, w3: u16) -> [f32; 4] {
    let largest = usize::from(w3 & 3);
    // 15-bit offset-binary components; the top bit sits in a flag, 14 bits in the words
    let comp = |top: u16, low: u16| -> f32 {
        let stored = (i32::from(top & 1) << 14) | i32::from(low & 0x3FFF);
        (stored - 16384) as f32 / (16384.0 * std::f32::consts::SQRT_2)
    };
    let a = comp(w1 >> 15, w1 >> 1);
    let b = comp(w2 >> 14, w2);
    let c = comp(w1, w3 >> 2);
    let mut q = [0.0f32; 4];
    let mut small = (0..4).filter(|&i| i != largest);
    q[small.next().unwrap()] = b;
    q[small.next().unwrap()] = a;
    q[small.next().unwrap()] = c;
    let l = (1.0 - a * a - b * b - c * c).max(0.0).sqrt();
    q[largest] = if w2 >> 15 == 1 { -l } else { l };
    q
}

/// Parses events out of the attribute text (`Events={ { Time=..., Event=..., ... } }`).
pub fn parse_events(attributes: &str) -> Vec<AnimEvent> {
    let Some(start) = attributes.find("Events={") else {
        return Vec::new();
    };
    let mut events: Vec<AnimEvent> = Vec::new();
    for line in attributes[start..].lines() {
        let Some((k, v)) = line.trim().split_once('=') else {
            continue;
        };
        let v = v.trim().trim_end_matches(',');
        match k {
            "Time" => events.push(AnimEvent {
                phase: v.parse().unwrap_or(f32::NAN),
                name: String::new(),
                param: String::new(),
                direction: String::new(),
            }),
            "Event" => {
                if let Some(e) = events.last_mut() {
                    e.name = v.to_string();
                }
            }
            "Param" => {
                if let Some(e) = events.last_mut() {
                    e.param = v.to_string();
                }
            }
            "Direction" => {
                if let Some(e) = events.last_mut() {
                    e.direction = v.to_string();
                }
            }
            _ => {}
        }
    }
    events
}

/// Decodes every clip of an animation file from its `H.` and `B.` parts.
pub fn decode(path: &str, header: &[u8], body: &[u8]) -> Result<Vec<Animation>> {
    let h = AnimHeader::parse(header)?;
    let attributes = h.common.attributes.clone();
    let events = parse_events(&attributes);
    let mut out = Vec::new();
    let mut pos = 0usize;
    for (skeleton_index, sk) in h.skeletons.iter().enumerate() {
        let nb = sk.bones.len();
        let end = pos
            .checked_add(sk.frame_data_len as usize)
            .context("frame data length overflow")?;
        let frame_data = body
            .get(pos..end)
            .context("frame data runs past the end of the body")?;
        pos = end;
        for a in &sk.actions {
            let mut r = Reader::at(body, pos)?;
            let mut tracks = Vec::with_capacity(nb);
            for _ in 0..nb {
                let initial_rotation = r.f32x4()?;
                let initial_translation = [r.f32()?, r.f32()?, r.f32()?];
                r.skip(12)?;
                tracks.push(BoneTrack {
                    flags: 0,
                    initial_rotation,
                    initial_translation,
                    translation: None,
                    rotation: None,
                    scale: None,
                });
            }
            for t in &mut tracks {
                t.flags = r.u16()?;
            }
            let offsets: Vec<usize> = (0..nb)
                .map(|_| r.u16().map(usize::from))
                .collect::<Result<_>>()?;
            let frames = a.frames as usize;
            ensure!(
                frames <= (body.len() - r.pos()) / 4,
                "{frames} frames do not fit in the body"
            );
            let time_scale: Vec<f32> = (0..frames).map(|_| r.f32()).collect::<Result<_>>()?;
            pos = r.pos();

            ensure!(
                a.rate.is_finite() && a.rate > 0.0,
                "invalid rate {}",
                a.rate
            );
            let duration = 1.0 / a.rate;
            ensure!(
                time_scale.iter().all(|t| t.is_finite()),
                "key times are not finite"
            );
            if let Some(k) = time_scale.windows(2).position(|w| w[1] < w[0]) {
                bail!(
                    "key times of {} are not ascending at {k}: {:?}",
                    sk.name,
                    &time_scale[k.saturating_sub(1)..(k + 3).min(time_scale.len())]
                );
            }

            // The offsets must describe exactly the bytes the flags call for
            let stride = a.frame_stride as usize;
            for (i, t) in tracks.iter().enumerate() {
                ensure!(
                    t.flags
                        & !(flag::TRANSLATION
                            | flag::ROTATION
                            | flag::TRANSLATION_X
                            | flag::SCALE
                            | flag::STATIC)
                        == 0,
                    "unsupported channel flags {} on {}",
                    t.flags,
                    sk.bones[i].name
                );
                ensure!(
                    t.flags & flag::TRANSLATION == 0 || t.flags & flag::TRANSLATION_X == 0,
                    "bone {} has two translation channels",
                    sk.bones[i].name
                );
                let next = offsets.get(i + 1).copied().unwrap_or(stride);
                ensure!(
                    next.checked_sub(offsets[i]) == Some(channel_size(t.flags)),
                    "channel layout of {} (flags {}, offsets {}..{}) does not match",
                    sk.bones[i].name,
                    t.flags,
                    offsets[i],
                    next
                );
            }
            let frame_offset = a.frame_offset as usize;
            ensure!(
                frame_offset
                    .checked_add(frames * stride)
                    .is_some_and(|e| e <= frame_data.len()),
                "frame data too short: {frames} frames of {stride} bytes at {frame_offset} in {} bytes",
                frame_data.len()
            );

            // A clip without frames has nothing to animate whatever the flags say: its tracks stay
            // at the initial pose (an empty key list would be an empty accessor and has no first key)
            for t in tracks.iter_mut().filter(|_| frames > 0) {
                if t.flags & (flag::TRANSLATION | flag::TRANSLATION_X) != 0 {
                    t.translation = Some(Vec::with_capacity(frames));
                }
                if t.flags & flag::ROTATION != 0 {
                    t.rotation = Some(Vec::with_capacity(frames));
                }
                if t.flags & flag::SCALE != 0 {
                    t.scale = Some(Vec::with_capacity(frames));
                }
            }
            for f in 0..frames {
                let base = frame_offset + f * stride;
                let frame = &frame_data[base..base + stride];
                for (t, &off) in tracks.iter_mut().zip(&offsets) {
                    let mut fr = Reader::at(frame, off)?;
                    if t.flags & flag::TRANSLATION != 0 {
                        let v = [fr.i16()?, fr.i16()?, fr.i16()?];
                        t.translation
                            .as_mut()
                            .unwrap()
                            .push(std::array::from_fn(|k| {
                                a.position_offset[k] + f32::from(v[k]) * a.position_scale[k]
                            }));
                    }
                    if t.flags & flag::ROTATION != 0 {
                        let q = unpack_quaternion(fr.u16()?, fr.u16()?, fr.u16()?);
                        t.rotation.as_mut().unwrap().push(q);
                    }
                    if t.flags & flag::TRANSLATION_X != 0 {
                        let x = fr.f32()?;
                        t.translation.as_mut().unwrap().push([
                            x,
                            t.initial_translation[1],
                            t.initial_translation[2],
                        ]);
                    }
                    if t.flags & flag::SCALE != 0 {
                        let v = [fr.i16()?, fr.i16()?, fr.i16()?];
                        t.scale.as_mut().unwrap().push(std::array::from_fn(|k| {
                            a.scale_offset[k] + f32::from(v[k]) * a.scale_scale[k]
                        }));
                    }
                }
            }
            out.push(Animation {
                path: path.to_string(),
                skeleton: sk.name.clone(),
                skeleton_index,
                name: a.name.clone(),
                bones: sk.bones.clone(),
                duration,
                times: time_scale.iter().map(|t| t * duration).collect(),
                tracks,
                events: events.clone(),
                attributes: attributes.clone(),
                root_rotation: sk.root_rotation,
                root_translation: sk.root_translation,
            });
        }
    }
    if pos != body.len() {
        bail!("body not read to the end ({pos} of {} bytes)", body.len());
    }
    Ok(out)
}

/// `H.Misc` and `B.Misc`, opened to read animations one entry at a time.
pub struct AnimationPackages {
    head: Package,
    body: Package,
}

impl AnimationPackages {
    pub fn open(dir: &Path) -> Result<Self> {
        let open = |name: &str| {
            Package::open_in(dir, name)?
                .with_context(|| format!("{name}.toc is not in {}", dir.display()))
        };
        Ok(AnimationPackages {
            head: open("H.Misc")?,
            body: open("B.Misc")?,
        })
    }

    /// Asks the OS not to cache what is read (for sweeps).
    pub fn set_nocache(&self) {
        self.head.set_nocache();
        self.body.set_nocache();
    }

    /// Every `H.Misc` entry, in the order of the table of contents.
    pub fn entries(&self) -> &[Entry] {
        &self.head.toc.entries
    }

    /// The raw `H.Misc` bytes of an entry.
    pub fn read_header(&mut self, entry: &Entry) -> Result<Vec<u8>> {
        self.head.read(entry)
    }

    /// Reads an entry's header and returns it when it is an animation (type 132).
    pub fn header(&mut self, entry: &Entry) -> Result<Option<(Vec<u8>, AnimHeader)>> {
        let raw = self.head.read(entry)?;
        match CommonHeader::parse(&raw) {
            Ok(c) if c.kind == KIND_ANIMATION => {
                let h = AnimHeader::parse(&raw).with_context(|| entry.path.clone())?;
                Ok(Some((raw, h)))
            }
            _ => Ok(None),
        }
    }

    /// Decodes an animation file whose header bytes are already read.
    pub fn decode_with(&mut self, path: &str, header: &[u8]) -> Result<Vec<Animation>> {
        let body = self
            .body
            .read_path(path)?
            .with_context(|| format!("{path} has no B.Misc part"))?;
        decode(path, header, &body).with_context(|| path.to_string())
    }

    /// Decodes every clip of the animation at `path`.
    pub fn load(&mut self, path: &str) -> Result<Vec<Animation>> {
        let raw = self
            .head
            .read_path(path)?
            .with_context(|| format!("{path} is not in H.Misc"))?;
        self.decode_with(path, &raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: f32 = std::f32::consts::FRAC_1_SQRT_2;

    /// Packs a quaternion the way `unpack_quaternion` reads it.
    fn pack_quaternion(q: [f32; 4]) -> [u16; 3] {
        let largest = (0..4)
            .max_by(|&a, &b| q[a].abs().partial_cmp(&q[b].abs()).unwrap())
            .unwrap();
        let small: Vec<usize> = (0..4).filter(|&i| i != largest).collect();
        let enc = |v: f32| -> u16 {
            let v = (v * 16384.0 * std::f32::consts::SQRT_2).round() as i32 + 16384;
            v.clamp(0, 32767) as u16
        };
        let b = enc(q[small[0]]);
        let a = enc(q[small[1]]);
        let c = enc(q[small[2]]);
        let g = u16::from(q[largest] < 0.0);
        let w1 = (((a >> 14) & 1) << 15) | ((a & 0x3FFF) << 1) | ((c >> 14) & 1);
        let w2 = (g << 15) | (((b >> 14) & 1) << 14) | (b & 0x3FFF);
        let w3 = ((c & 0x3FFF) << 2) | largest as u16;
        [w1, w2, w3]
    }

    fn norm(q: [f32; 4]) -> [f32; 4] {
        let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        q.map(|v| v / n)
    }

    #[test]
    fn quaternions_round_trip() {
        for q in [
            [0.0, 0.0, 0.0, 1.0],
            [0.5, -0.5, 0.5, 0.5],
            [-0.4047, 0.4688, -0.5442, 0.5658],
            [R, 0.0, 0.0, -R],
            [0.1, 0.2, 0.3, -0.9],
            [-0.9, 0.1, 0.2, 0.3],
            [0.05, -0.99, 0.01, 0.02],
        ] {
            let q = norm(q);
            let [a, b, c] = pack_quaternion(q);
            let d = unpack_quaternion(a, b, c);
            for i in 0..4 {
                assert!((q[i] - d[i]).abs() < 2e-4, "{q:?} -> {d:?}");
            }
        }
    }

    /// Words stored for the first frame of GAME_C1_SPINE2 in a real clip.
    #[test]
    fn quaternion_from_stored_words() {
        // Offset-binary: 0x3ECC is 16076 = 16384 - 308 and so on
        let q = unpack_quaternion(0x7D99, 0x3EB7, 0x0D0B);
        assert!((q[0] - (16055.0 - 16384.0) / 23170.475).abs() < 1e-6);
        assert!((q[1] - (16076.0 - 16384.0) / 23170.475).abs() < 1e-6);
        assert!((q[2] - (17218.0 - 16384.0) / 23170.475).abs() < 1e-6);
        assert!(q[3] > 0.999);
    }

    #[test]
    fn events_are_parsed() {
        let attrs = "Events={\n    {\n        Time=0.0277778\n        Event=StepL\n        Param=0\n        Direction=BOTH\n    },\n    {\n        Time=0.5\n        Event=StepR\n        Param=1\n        Direction=BOTH\n    }\n}\n";
        let e = parse_events(attrs);
        assert_eq!(e.len(), 2);
        assert_eq!(e[1].name, "StepR");
        assert_eq!(e[1].param, "1");
        assert!((e[0].phase - 0.0277778).abs() < 1e-7);
        assert!(parse_events("Attribs={\n    Rate=2\n}\n").is_empty());
    }

    /// A clip with three bones: the root moves in z, bone 1 rotates, bone 2 is
    /// constant. Three keys at 0, 0.5 and 1 over 2 seconds.
    fn synthetic() -> (Vec<u8>, Vec<u8>) {
        synthetic_frames(3)
    }

    /// The same clip cut to its first `frames` frames (0 to 3), flags unchanged.
    fn synthetic_frames(frames: usize) -> (Vec<u8>, Vec<u8>) {
        let attrs = "Attribs={\n    Rate=1.5\n}\n";
        let mut h = crate::header::tests::build(&["/a_anim.fbx"], attrs, 132);
        let s32 = |h: &mut Vec<u8>, s: &str| {
            h.extend((s.len() as u32).to_le_bytes());
            h.extend(s.as_bytes());
        };
        s32(&mut h, "/Lotus/Animations/Test/a_anim.fbx");
        s32(&mut h, "");
        h.extend([0u8; 90]);
        h.extend(1u32.to_le_bytes());
        s32(&mut h, "Test");
        h.extend(0u32.to_le_bytes());
        let mut root = Vec::new();
        for v in [0.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0] {
            root.extend(v.to_le_bytes());
        }
        h.extend(root);
        h.extend(3u32.to_le_bytes());
        for (name, parent) in [("ROOT", 0u16), ("HIP", 0), ("ARM", 1)] {
            s32(&mut h, name);
            h.extend(parent.to_le_bytes());
            h.extend(0u16.to_le_bytes());
        }
        h.extend(1u32.to_le_bytes());
        h.extend([0u8; 8]);
        h.extend(0.5f32.to_le_bytes());
        h.extend([0u8; 4]);
        s32(&mut h, "Take 001");
        let mut m = Vec::new();
        for v in [
            0.001f32, 0.001, 0.001, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0,
        ] {
            m.extend(v.to_le_bytes());
        }
        h.extend(m);
        h.extend(12u32.to_le_bytes()); // stride: root 6 + hip 6
        h.extend(0u32.to_le_bytes());
        h.extend([0u8; 26]);
        h.extend((12 * frames as u32).to_le_bytes());
        h.extend([0u8; 12]);
        h.extend((frames as u32).to_le_bytes());
        h.extend([0u8; 8]);
        h.extend(0u32.to_le_bytes());

        let mut b = Vec::new();
        // Frame data: root z = 0, 1000, 2000 (units of 0.001 from offset 1.0)
        for (z, angle) in [(-1000i16, 0.0f32), (0, 0.5), (1000, 1.0)]
            .into_iter()
            .take(frames)
        {
            for v in [0i16, 0, z] {
                b.extend(v.to_le_bytes());
            }
            let q = norm([0.0, (angle / 2.0).sin(), 0.0, (angle / 2.0).cos()]);
            for w in pack_quaternion(q) {
                b.extend(w.to_le_bytes());
            }
        }
        // Initial transforms
        let init = |b: &mut Vec<u8>, q: [f32; 4], t: [f32; 3]| {
            for v in q.into_iter().chain(t).chain([0.0; 3]) {
                b.extend(v.to_le_bytes());
            }
        };
        init(&mut b, [0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0]);
        init(&mut b, [0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0]);
        init(&mut b, [0.0, 0.0, R, R], [0.5, 0.0, 0.0]);
        for f in [1u16, 2, 0] {
            b.extend(f.to_le_bytes());
        }
        for o in [0u16, 6, 12] {
            b.extend(o.to_le_bytes());
        }
        for t in [0.0f32, 0.5, 1.0].into_iter().take(frames) {
            b.extend(t.to_le_bytes());
        }
        (h, b)
    }

    #[test]
    fn decodes_a_synthetic_clip() {
        let (h, b) = synthetic();
        let clips = decode("/Lotus/Animations/Test/a_anim.fbx", &h, &b).unwrap();
        assert_eq!(clips.len(), 1);
        let c = &clips[0];
        assert_eq!((c.skeleton.as_str(), c.name.as_str()), ("Test", "Take 001"));
        assert_eq!(c.bones.len(), 3);
        assert_eq!(c.bones[2].parent, Some(1));
        assert_eq!(c.bones[0].parent, None);
        assert!((c.duration - 2.0).abs() < 1e-6);
        assert!((c.times[2] - c.duration).abs() < 1e-6);
        assert_eq!(c.root_translation, [0.0, 0.0, 2.0]);
        let root = c.tracks[0].translation.as_ref().unwrap();
        assert!((root[0][2] - 0.0).abs() < 1e-5 && (root[2][2] - 2.0).abs() < 1e-5);
        assert!((c.root_displacement().unwrap()[2] - 2.0).abs() < 1e-5);
        let hip = c.tracks[1].rotation.as_ref().unwrap();
        let expect = norm([0.0, (0.25f32).sin(), 0.0, (0.25f32).cos()]);
        for i in 0..4 {
            assert!((hip[1][i] - expect[i]).abs() < 1e-3);
        }
        assert!(c.tracks[1].translation.is_none());
        assert!(c.tracks[2].rotation.is_none());
    }

    #[test]
    fn channels_mirror_and_hold_constants() {
        let (h, b) = synthetic();
        let c = decode("p", &h, &b).unwrap().remove(0);
        let ch = c.channels(true);
        // Root translation: 3 keys, x negated (zero here), z kept
        let t0 = ch
            .iter()
            .find(|c| c.node == 0 && c.target == Target::Translation)
            .unwrap();
        assert_eq!(t0.times.len(), 3);
        assert!((t0.values[8] - 2.0).abs() < 1e-5);
        // Bone 2 is constant: one key at 0 with the initial transform, mirrored
        let r2 = ch
            .iter()
            .find(|c| c.node == 2 && c.target == Target::Rotation)
            .unwrap();
        assert_eq!(r2.times, [0.0]);
        assert_eq!(r2.values, [0.0, -0.0, -R, R]);
        let t2 = ch
            .iter()
            .find(|c| c.node == 2 && c.target == Target::Translation)
            .unwrap();
        assert_eq!(t2.values, [-0.5, 0.0, 0.0]);
        // Every bone has a rotation and a translation channel
        assert_eq!(ch.len(), 6);
    }

    #[test]
    fn rotations_stay_continuous() {
        let (h, b) = synthetic();
        let mut c = decode("p", &h, &b).unwrap().remove(0);
        // Flip the middle key to the other sign of the same rotation
        let r = c.tracks[1].rotation.as_mut().unwrap();
        r[1] = r[1].map(|v| -v);
        let ch = c.channels(false);
        let r1 = ch
            .iter()
            .find(|c| c.node == 1 && c.target == Target::Rotation)
            .unwrap();
        for k in 1..3 {
            let dot: f32 = (0..4)
                .map(|i| r1.values[(k - 1) * 4 + i] * r1.values[k * 4 + i])
                .sum();
            assert!(dot > 0.0);
        }
    }

    #[test]
    fn a_clip_without_frames_stays_at_its_initial_pose() {
        // Flags call for a moving root and a rotating hip, but there are no frames: the tracks
        // must not hold empty key lists (check-anims indexed their first key, and the glTF
        // export wrote an accessor of no keys)
        let (h, b) = synthetic_frames(0);
        let c = decode("p", &h, &b).unwrap().remove(0);
        assert!(c.times.is_empty());
        assert_eq!(c.tracks[0].flags, 1);
        assert_eq!(c.tracks[1].flags, 2);
        assert!(c
            .tracks
            .iter()
            .all(|t| t.translation.is_none() && t.rotation.is_none() && t.scale.is_none()));
        let ch = c.channels(false);
        assert_eq!(ch.len(), 6);
        assert!(ch.iter().all(|c| c.times == [0.0] && !c.values.is_empty()));
        assert!(c.root_displacement().is_none());
        // Hand-built tracks with empty key lists come out the same
        let mut c = c;
        c.tracks[0].translation = Some(Vec::new());
        c.tracks[1].rotation = Some(Vec::new());
        c.tracks[2].scale = Some(Vec::new());
        let ch = c.channels(false);
        assert_eq!(ch.len(), 6);
        assert!(ch.iter().all(|c| c.times == [0.0]));
        // One frame is a clip with keys
        let (h, b) = synthetic_frames(1);
        let one = decode("p", &h, &b).unwrap().remove(0);
        assert_eq!(one.tracks[0].translation.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn rejects_inconsistent_data() {
        let (h, b) = synthetic();
        // Body one byte long
        assert!(decode("p", &h, &b[..b.len() - 1]).is_err());
        // A flag the format does not define
        let mut bad = b.clone();
        let flags_at = 36 + 3 * 40;
        bad[flags_at] = 64;
        assert!(decode("p", &h, &bad).is_err());
        // Not an animation
        let other = crate::header::tests::build(&[], "", 102);
        assert!(AnimHeader::parse(&other).is_err());
    }
}
