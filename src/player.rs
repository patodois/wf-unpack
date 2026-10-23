//! The Tenno's movement values, read from `/Packages.bin` with the entity and key each one comes from.
//!
//! A game that wants to move a Warframe like Warframe does needs numbers: gravity, how fast the
//! character runs, how high the jump goes. They are not in the animation or model files but in the
//! entity text of `/Packages.bin`. [`Player::load`] follows the references the game follows,
//! starting at the player's avatar type [`AVATAR`]:
//!
//! ```text
//! TennoAvatar
//!   PhysicsBehavior=TennoWalk        -> ControllerAttributes=TennoWalkAttribs   (speeds, capsule, jump)
//!   MotionControllerType=TennoMotion                                            (turn rate, slide end)
//!   CameraControllerType=CameraController                                       (camera offsets)
//!   ClimbControllerType=TennoClimbController                                    (bullet jump)
//!   InputControllerType=InputController                                         (crouch tap / hold)
//! /EE/Types/Physics/AsyncPhysicsMgrDesc -> PhysicsMgrDesc                       (world gravity)
//! /EE/Types/Game/SlideAction                                                    (jump out of a slide)
//! ```
//!
//! # The rules
//!
//! - Nothing is typed in by hand. Every number is a [`Sourced`] value that remembers the entity
//!   (`entity`) and the key (`key`) it was read from. A value that is missing is an error, not a
//!   default.
//! - The data has **no units**. Each field's documentation says what the number is and which unit
//!   it is read as; where that is an interpretation (a name or a comparison with the animations
//!   suggests it, the data does not say), the documentation says "reading". [`Player::rows`] gives
//!   the same information for printing, and [`READINGS`] lists the interpretations in one place.
//! - What the data does not provide (the aim glide, wall latch, mantle heights, ...) is not
//!   invented; [`LEFT_OUT`] lists it.
//! - Space: lengths are metres, angles degrees, all in the game's own space: x to the right, y up,
//!   z forward, which is left-handed (see [`crate::scene`] for the right-handed conversion used by
//!   the other modules: negate x). Vectors here are not converted.
//!
//! This module reads data only. How a character is simulated with it (the integrator, the order of
//! the steps, which clip plays when) is game code and not part of this crate.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::cache::Package;
use crate::clipspec::{clip_spec, ClipSpec};
use crate::ee::Value;
use crate::header::CommonHeader;
use crate::level::{TypeInfo, TypeResolver};
use crate::material::{folder, resolve_path};
use crate::packages::PackagesBin;

/// The player's avatar type: its `Mesh` is Excalibur's skeleton and it names every controller below.
pub const AVATAR: &str = "/Lotus/Types/Player/TennoAvatar";
/// The region manager's physics description; the world gravity is in what it inherits.
pub const PHYSICS_MGR: &str = "/EE/Types/Physics/AsyncPhysicsMgrDesc";
/// The action the knee slide is made of; its `JumpSpeed` is what a jump out of a slide launches with.
pub const SLIDE_ACTION: &str = "/EE/Types/Game/SlideAction";
/// The clip whose events are `JumpStartImpulse`, `GetUpFromSlide`, `BulletJump`, `BulletJumpStart` and
/// `BulletJumpEnd`: the only Tenno clip that carries the bullet jump events (found with
/// `find-text`). Its events are in Packages.bin only, not in its `H.Misc` header text.
pub const BULLET_JUMP_CLIP: &str = "/Lotus/Animations/Player/Movement/JumpRollForwardB_anim.fbx";
/// The forward roll of a character with a melee weapon out. The data has four forward rolls in this
/// folder (`RollForwardMelee`, `RollForwardPhysics`, `RollForwardTechHit`, `RollForwardTechHitBack`);
/// which one plays is the animation tree's choice, which is not decoded, so choosing this one is a
/// convention of this module. All four derive from the entity `RollForward_anim.fbx`, which sets the
/// `RootAdjustment`.
pub const ROLL_CLIP: &str = "/Lotus/Animations/Player/Movement/RollForwardMelee";

/// A value with the entity (type path) that set it and the key it was read from.
#[derive(Debug, Clone, PartialEq)]
pub struct Sourced<T> {
    pub value: T,
    /// The Packages.bin entity whose text set the key. It can be a parent of the type that was
    /// asked: `TennoWalkAttribs` inherits `SlopeLimit` from `/EE/Types/Physics/ControllerAttribs`.
    pub entity: String,
    pub key: String,
}

impl<T> Sourced<T> {
    /// `entity: key`, for reports.
    pub fn cite(&self) -> String {
        format!("{}: {}", self.entity, self.key)
    }
}

/// The physical size and camera anchors of the avatar. See the notes on each field.
#[derive(Debug, Clone, PartialEq)]
pub struct Capsule {
    /// `TennoWalkAttribs.Height`. Metres (reading). The key names are those of PhysX's
    /// `PxCapsuleControllerDesc` (`height`, `radius`, `stepOffset`, `slopeLimit`, `contactOffset`);
    /// there `height` is the distance between the two cap centres, so the standing capsule would be
    /// `height + 2 * radius` = 2.4 m tall. The data does not say which reading the game uses.
    pub height: Sourced<f32>,
    /// `TennoWalkAttribs.CapsuleRadius`. Metres (reading).
    pub radius: Sourced<f32>,
    /// `TennoWalkAttribs.CapsuleCrouchRadius`. Metres (reading).
    pub crouch_radius: Sourced<f32>,
    /// `TennoWalkAttribs.StepHeight`: the highest step the controller climbs. Metres (reading).
    pub step_height: Sourced<f32>,
    /// `TennoWalkAttribs.SkinWidth`: the collision margin. Metres (reading).
    pub skin_width: Sourced<f32>,
    /// `TennoWalkAttribs.SlopeLimit`: the steepest walkable slope. Degrees (reading: the inherited
    /// default in `ControllerAttribs` is 45, which only makes sense as an angle).
    pub slope_limit: Sourced<f32>,
    /// `TennoAvatar.EyeOffset`: the camera pivot above the feet while standing, `(0, y, 0)`.
    /// Metres (reading). Loading fails if x or z is not 0.
    pub eye_offset: Sourced<[f32; 3]>,
    /// `TennoAvatar.CrouchOffset`: the same while crouching or sliding. Metres (reading).
    pub crouch_offset: Sourced<[f32; 3]>,
}

/// Speed and acceleration on the ground.
#[derive(Debug, Clone, PartialEq)]
pub struct Ground {
    /// `TennoWalkAttribs.MaxSpeed`: the sprint speed. Metres per second (reading).
    pub max_speed: Sourced<f32>,
    /// `TennoWalkAttribs.WalkSpeedFraction`: walking speed as a fraction of `max_speed` (the key name
    /// says "fraction"; the unit-less 0..1 reading is the data's own).
    pub walk_fraction: Sourced<f32>,
    /// `TennoWalkAttribs.JogSpeedFraction`: the speed at full stick without the sprint button, as a
    /// fraction of `max_speed`.
    pub jog_fraction: Sourced<f32>,
    /// `TennoWalkAttribs.Acceleration`. Unit not stated; used as a rate per second that multiplies
    /// the wish speed (reading, from how the values behave in a Quake-style integrator).
    pub acceleration: Sourced<f32>,
    /// `TennoWalkAttribs.AccelDrag`: friction while the stick is held. 1/s (reading).
    pub accel_drag: Sourced<f32>,
    /// `TennoWalkAttribs.DecelDrag`: friction with no stick. 1/s (reading).
    pub decel_drag: Sourced<f32>,
    /// `TennoMotion.RunTurnRate`: how fast the character turns toward its heading. Degrees per
    /// second (reading).
    pub run_turn_rate: Sourced<f32>,
}

/// Movement in the air.
#[derive(Debug, Clone, PartialEq)]
pub struct Air {
    /// `TennoWalkAttribs.AirAcceleration`. Same unit as [`Ground::acceleration`].
    pub acceleration: Sourced<f32>,
    /// `TennoWalkAttribs.AirAccelDrag`. 1/s (reading).
    pub accel_drag: Sourced<f32>,
    /// `TennoWalkAttribs.AirDecelDrag`. 1/s (reading).
    pub decel_drag: Sourced<f32>,
}

/// The knee slide.
#[derive(Debug, Clone, PartialEq)]
pub struct Slide {
    /// `TennoWalkAttribs.SlideAccel`: sideways steering. Metres per second squared (reading).
    pub accel: Sourced<f32>,
    /// `TennoWalkAttribs.SlideDrag`: the slide's speed decays at this rate. 1/s (reading).
    pub drag: Sourced<f32>,
    /// `TennoMotion.SlideEndThreshold`: the slide ends below this speed (and, reading, a slide
    /// can start at or above it). Metres per second (reading).
    pub end_threshold: Sourced<f32>,
    /// `SlideAction.JumpSpeed`: the vertical launch of a jump out of a slide. Metres per second
    /// (reading: the same unit and value as [`Jump::height`]).
    pub jump_speed: Sourced<f32>,
    /// `SlideAction.GravityAdjust`. Meaning not determined (-35); not applied by anything here.
    pub gravity_adjust: Sourced<f32>,
}

/// Jumps.
#[derive(Debug, Clone, PartialEq)]
pub struct Jump {
    /// `TennoWalkAttribs.JumpHeight`. The key says height, but 5 m is far more than the jump
    /// clips show, so the reading is the take-off speed in metres per second: rising against the
    /// world's gravity of 15 m/s^2 it reaches `5^2 / (2 * 15)` = 0.83 m, close to the 0.93 m the
    /// root bone of the Sword stance's `Jump` clip rises.
    pub height: Sourced<f32>,
    /// `TennoAvatar.NumJumps`: the number of jumps between landings. Reading: it counts the first
    /// jump from the ground, so 2 is one jump and one air jump; walking off an edge counts as the
    /// first.
    pub num_jumps: Sourced<f32>,
    /// `TennoAvatar.AirJumpHeightMultiplier`: multiplies [`Jump::height`] for the air jump. Unit-less.
    pub air_jump_height_multiplier: Sourced<f32>,
    /// `TennoClimbController.DoubleJumpSpeed` (0.35). Unit and axis not stated; nothing here
    /// applies it (see [`LEFT_OUT`]).
    pub double_jump_speed: Sourced<f32>,
    /// `TennoAvatar.AnimTriggeredJump`: reading from the key name, the jump's impulse happens at
    /// the clip's `JumpStartImpulse` event rather than at the button press. Flag (1 = yes).
    pub anim_triggered: Sourced<bool>,
}

/// Gravity.
#[derive(Debug, Clone, PartialEq)]
pub struct Gravity {
    /// `PhysicsMgrDesc.Gravity` (through [`PHYSICS_MGR`]): the world's gravity vector, `(0, -15, 0)`.
    /// Metres per second squared (reading: the usual unit of a PhysX scene's gravity).
    pub world: Sourced<[f32; 3]>,
    /// `TennoWalkAttribs.AdditionalGravity` (-9): extra gravity of the controlled avatar. Same unit
    /// and sign convention as the world's y (negative pulls down). Reading: it applies while the
    /// avatar is falling, so a fall accelerates at `15 + 9 = 24` and a rise at 15; the data does
    /// not say when it applies.
    pub additional: Sourced<f32>,
}

/// The bullet jump (crouch button down at the jump, or out of a slide).
#[derive(Debug, Clone, PartialEq)]
pub struct BulletJump {
    /// `TennoClimbController.BulletJumpSpeed`. Reading: a multiple of [`Ground::max_speed`], so the
    /// horizontal launch speed is `2.5 * 7.5` m/s. The data has no unit.
    pub speed: Sourced<f32>,
    /// `TennoClimbController.BulletJumpGravityMultiplierDuration`. Seconds (reading). The data does
    /// not give the multiplier it names (see [`LEFT_OUT`]).
    pub gravity_multiplier_duration: Sourced<f32>,
    /// What the data says about [`BULLET_JUMP_CLIP`] (its events `BulletJumpStart`,
    /// `JumpStartImpulse`, ... are in Packages.bin only).
    pub clip: ClipSpec,
}

/// The roll (a tap of the crouch button with the stick deflected).
#[derive(Debug, Clone, PartialEq)]
pub struct Roll {
    /// What the data says about [`ROLL_CLIP`]: its Packages.bin entity, which inherits from
    /// `RollForward_anim.fbx`, and the attribute text of its own `H.Misc` header, which wins.
    /// `root_override` is `RootAdjustment.overriddenMotion` (`0, 0, 8`, from the parent entity),
    /// the distance the roll covers (metres, reading); `root_conditioning` is
    /// `ROOTC_ADJUST_LINEAR` (from the header), which reads as "the missing distance between the
    /// clip's own root travel and `root_override` is added linearly over the clip"; `end_events`
    /// are `DodgeRollFinished` and `EnableLookIK` (from the parent entity).
    pub clip: ClipSpec,
}

/// The third-person camera.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    /// `CameraController.ViewOffset`: where the camera sits relative to the pivot, in the camera's
    /// own axes `(right, up, forward)` with forward negative behind the character: `(0.5, -0.1,
    /// -1.5)`. Metres (reading).
    pub view_offset: Sourced<[f32; 3]>,
    /// `CameraController.ViewOffsetSpeed`: how fast the eye height moves between the standing and
    /// the crouching offset. Metres per second (reading).
    pub view_offset_speed: Sourced<f32>,
    /// `CameraController.BaseFovOverride`. Degrees (reading); the data does not say whether it is
    /// the vertical or the horizontal angle.
    pub base_fov: Sourced<f32>,
    /// `CameraController.PitchClamp`: lowest and highest pitch. Degrees (reading).
    pub pitch_clamp: Sourced<[f32; 2]>,
    /// `CameraController.CollisionRadius`: the sphere swept from the pivot to the camera so walls
    /// pull it in. Metres (reading).
    pub collision_radius: Sourced<f32>,
    /// `CameraController.CameraSmoothTime`. Seconds (reading).
    pub smooth_time: Sourced<f32>,
    /// `CameraController.CameraRotationSmoothTime`. Seconds (reading).
    pub rotation_smooth_time: Sourced<f32>,
    /// Every other three-number key of the controller whose name ends in `Offset` (`AimViewOffset`,
    /// `InAirViewOffset`, `MeleeOffset`, `DodgeOffset`, ...), by key. Same axes as `view_offset`.
    /// Which one applies in which state is the game's logic and is not in the data.
    pub other_offsets: BTreeMap<String, Sourced<[f32; 3]>>,
}

/// How the input controller tells a tap from a hold of one button (`InputController.DemuxInputs`).
///
/// For the crouch button (`PRE_CROUCH`): released before `required_hold_time` it is a tap
/// (`ACTION`, which rolls with the stick deflected and slides when fast enough); held that long it
/// sends `HOLD_CROUCH` (crouch). Pressing `JUMP` or `MELEE` while it is down cancels the tap.
#[derive(Debug, Clone, PartialEq)]
pub struct TapHold {
    /// The entity that holds the `DemuxInputs` list.
    pub entity: String,
    /// The logical input this demultiplexes (`PRE_CROUCH`: the crouch/slide/roll button).
    pub input_code: String,
    /// `TapInputCodes`: sent when the button is released before `required_hold_time`.
    pub tap_codes: Vec<String>,
    /// `HoldInputs[].inputCode`: sent once the button has been held that long.
    pub hold_codes: Vec<String>,
    /// `BypassTapInputCodes`: inputs that, pressed while the button is down, cancel the tap.
    pub bypass_tap_codes: Vec<String>,
    /// `RequiredHoldTime`. Seconds (reading).
    pub required_hold_time: Sourced<f32>,
    /// `SendHoldImmediately`: the hold code is sent the moment the button goes down.
    pub send_hold_immediately: bool,
    /// `ForceHoldWhenDirectionless`: with no stick direction at the press, the hold is sent at
    /// once, so no tap is possible (reading from the key name).
    pub force_hold_when_directionless: bool,
}

/// Everything read from Packages.bin. See the module docs for the rules.
#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    /// `TennoAvatar.Mesh`: the model of the avatar, Excalibur's skeleton.
    pub mesh: Sourced<String>,
    pub gravity: Gravity,
    pub capsule: Capsule,
    pub ground: Ground,
    pub air: Air,
    pub slide: Slide,
    pub jump: Jump,
    pub bullet_jump: BulletJump,
    pub roll: Roll,
    pub camera: Camera,
    /// The crouch / slide / roll button's tap and hold rule.
    pub crouch: TapHold,
}

/// One printable line of [`Player::rows`].
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: &'static str,
    pub value: String,
    pub unit: &'static str,
    /// The unit (or the meaning) is an interpretation, not stated by the data.
    pub reading: bool,
    /// `entity: key`
    pub source: String,
}

/// Interpretations of numbers the data gives without a unit, in one place.
pub const READINGS: &[(&str, &str)] = &[
    (
        "lengths, speeds, accelerations",
        "metres, metres per second and per second squared: the level geometry, the model heights \
         (Excalibur is 1.84 m) and the clips' root motion are in metres",
    ),
    (
        "JumpHeight",
        "the take-off speed in m/s, not a height: as a speed it gives an apex of 5^2 / (2 x 15) = \
         0.83 m against the 0.93 m the Sword stance's Jump clip rises (root bone); as a height it \
         would be 5 m",
    ),
    (
        "AdditionalGravity",
        "added to the world's gravity in the same sign convention (negative pulls down), \
         applied while falling",
    ),
    (
        "AirJumpHeightMultiplier",
        "multiplies JumpHeight, which the first jump already uses as the take-off speed (m/s); the \
         air jump takes off at 0.75 x 5 m/s",
    ),
    (
        "NumJumps",
        "total jumps between landings, the first from the ground included",
    ),
    (
        "BulletJumpSpeed",
        "a multiple of MaxSpeed (the sprint speed): the horizontal launch speed is 2.5 x 7.5 m/s",
    ),
    (
        "Height / CapsuleRadius",
        "the fields of PhysX's PxCapsuleControllerDesc: Height is the distance between the cap \
         centres, so the capsule is Height + 2 x CapsuleRadius tall",
    ),
    (
        "SlopeLimit, RunTurnRate, BaseFovOverride, PitchClamp",
        "degrees (per second for the turn rate)",
    ),
    (
        "ROOTC_ADJUST_LINEAR",
        "the roll clip's root bone moves less than the clip's recorded root motion (7.65 m against \
         8 m, which RootAdjustment.overriddenMotion repeats): the missing distance is added \
         linearly over the clip",
    ),
];

/// What the data lacks, per move. `(move, why)`. These are not approximated by this module.
pub const LEFT_OUT: &[(&str, &str)] = &[
    (
        "aim glide",
        "TennoAvatar sets AimGravityRechargeRate=1, AimGravityCoolOffRatio=0.18 and the effect \
         script, but no AimGravityTime (other avatars set it: 3 for Teshin, 0 or -1 elsewhere), and no key anywhere gives the glide's fall rate.",
    ),
    (
        "wall latch",
        "Only the effect (WallLatchAttachedEffect), a 2 s challenge (WallLatchTimedChallenges) and \
         a clip (WallRun/WallGrabSlideIdle_anim) are in the data. Nothing says how long a latch \
         lasts, what gravity or sliding does while latched, or how a wall is detected. The \
         WallSlide* keys of TennoClimbController belong to the wall slide, which \
         TennoPlayerWallSlideController disables (CanEnterWallslide=0).",
    ),
    (
        "wall run",
        "TennoClimbController has WallRunHeight/StepHeight/VerticalInitialSpeed but \
         WallRunSpeedMultiplier=0: no running along walls.",
    ),
    (
        "mantle / vault",
        "TennoPlayerWallSlideController has ContextualMantles (clip per ledge height: 0.5-1 m \
         AutoVaultUp, 1.5-2 m AutoVaultUpHighB) and TennoClimbController has AutoVault* distances, \
         but nothing says what the heights are measured from, where the character ends up \
         (UseEndPosRotForMantle=0) or how long it takes besides the clip; the 1-1.5 m and 2 m+ \
         cases have no entry.",
    ),
    (
        "double jump forward push",
        "TennoClimbController.DoubleJumpSpeed=0.35 has no unit or axis in the data (the Operator's \
         is 2, the horse's 0). Only NumJumps and AirJumpHeightMultiplier define the air jump here.",
    ),
    (
        "bullet jump: height, gravity, direction, minimum speed",
        "Only BulletJumpSpeed=2.5 and BulletJumpGravityMultiplierDuration=0.8 are set. No key gives \
         the gravity multiplier, a launch height, the direction or a minimum speed (the keys named \
         *GravityMultiplier in the data belong to projectiles).",
    ),
    (
        "air dodge",
        "NumAirDodges=1 is in TennoAvatar, but nothing names the air dodge's clip, speed or trigger.",
    ),
    (
        "stamina",
        "MaxStamina=3 and recharge values exist, but Excalibur has no JumpStaminaCosts and \
         TennoAvatar no DodgeStaminaCost, so jumps and rolls cost nothing; \
         LowStaminaAirJumpHeightMultiplier=0 never applies.",
    ),
    (
        "which clip plays when",
        "The animation trees and state machines choose clips; they are not decoded here. ROLL_CLIP \
         and BULLET_JUMP_CLIP are named by this module, not by the data.",
    ),
];

// ---------------------------------------------------------------------------
// Reading

fn fetch<'a>(ty: &'a TypeInfo, key: &str) -> Result<Sourced<&'a Value>> {
    let (value, entity) = ty
        .props
        .get(key)
        .ok_or_else(|| anyhow!("{} has no {key}", ty.path))?;
    Ok(Sourced {
        value,
        entity: entity.clone(),
        key: key.to_string(),
    })
}

fn number(ty: &TypeInfo, key: &str) -> Result<Sourced<f32>> {
    let s = fetch(ty, key)?;
    let value = s
        .value
        .float()
        .with_context(|| format!("{}: {key} is not a number", ty.path))?;
    Ok(Sourced {
        value,
        entity: s.entity,
        key: s.key,
    })
}

fn vector<const N: usize>(ty: &TypeInfo, key: &str) -> Result<Sourced<[f32; N]>> {
    let s = fetch(ty, key)?;
    let v = s
        .value
        .floats()
        .with_context(|| format!("{}: {key} is not a list of numbers", ty.path))?;
    let value: [f32; N] = v
        .try_into()
        .map_err(|_| anyhow!("{}: {key} does not have {N} numbers", ty.path))?;
    Ok(Sourced {
        value,
        entity: s.entity,
        key: s.key,
    })
}

fn text(ty: &TypeInfo, key: &str) -> Result<Sourced<String>> {
    let s = fetch(ty, key)?;
    let value = s
        .value
        .text()
        .with_context(|| format!("{}: {key} is not text", ty.path))?
        .trim()
        .to_string();
    Ok(Sourced {
        value,
        entity: s.entity,
        key: s.key,
    })
}

/// The path an entity name in `ty`'s text points at. Names in an entity's text are relative to
/// that entity's folder, and for an inherited key the entity is the ancestor that sets it, not
/// the type being read: `PhysicsBehavior=TennoWalk` set on `/Lotus/Types/Player/TennoAvatar` means
/// `/Lotus/Types/Player/TennoWalk` for every avatar that derives from it, wherever it lives.
fn named_path(ty: &TypeInfo, key: &str) -> Result<String> {
    let s = text(ty, key)?;
    Ok(resolve_path(folder(&s.entity), &s.value))
}

fn flag_of(ty: &TypeInfo, key: &str) -> Result<Sourced<bool>> {
    let t = text(ty, key)?;
    Ok(Sourced {
        value: t.value != "0" && !t.value.is_empty(),
        entity: t.entity,
        key: t.key,
    })
}

fn flag(v: Option<&Value>) -> bool {
    v.and_then(Value::text).is_some_and(|t| t.trim() == "1")
}

fn strings(v: &Value) -> Vec<String> {
    v.items()
        .iter()
        .filter_map(|x| x.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// The demultiplexer entry for one logical input of an input controller type.
pub fn tap_hold(input: &TypeInfo, code: &str) -> Result<TapHold> {
    let (list, entity) = input
        .props
        .get("DemuxInputs")
        .ok_or_else(|| anyhow!("{} has no DemuxInputs", input.path))?;
    let item = list
        .items()
        .into_iter()
        .find(|i| i.get("InputCode").and_then(Value::text).map(str::trim) == Some(code))
        .ok_or_else(|| anyhow!("{} has no DemuxInputs entry for {code}", input.path))?;
    let required = item
        .get("RequiredHoldTime")
        .and_then(Value::float)
        .ok_or_else(|| anyhow!("{code}: no RequiredHoldTime"))?;
    Ok(TapHold {
        entity: entity.clone(),
        input_code: code.to_string(),
        tap_codes: item.get("TapInputCodes").map(strings).unwrap_or_default(),
        hold_codes: item
            .get("HoldInputs")
            .map(|h| {
                h.items()
                    .iter()
                    .filter_map(|x| x.get("inputCode").and_then(Value::text))
                    .map(|t| t.trim().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        bypass_tap_codes: item
            .get("BypassTapInputCodes")
            .map(strings)
            .unwrap_or_default(),
        required_hold_time: Sourced {
            value: required,
            entity: entity.clone(),
            key: format!("DemuxInputs[{code}].RequiredHoldTime"),
        },
        send_hold_immediately: flag(item.get("SendHoldImmediately")),
        force_hold_when_directionless: flag(item.get("ForceHoldWhenDirectionless")),
    })
}

fn camera(ty: &TypeInfo) -> Result<Camera> {
    let mut other_offsets = BTreeMap::new();
    for (key, (value, entity)) in &ty.props {
        if key == "ViewOffset" || !key.ends_with("Offset") {
            continue;
        }
        if let Some(v) = value.floats().and_then(|v| <[f32; 3]>::try_from(v).ok()) {
            other_offsets.insert(
                key.clone(),
                Sourced {
                    value: v,
                    entity: entity.clone(),
                    key: key.clone(),
                },
            );
        }
    }
    Ok(Camera {
        view_offset: vector::<3>(ty, "ViewOffset")?,
        view_offset_speed: number(ty, "ViewOffsetSpeed")?,
        base_fov: number(ty, "BaseFovOverride")?,
        pitch_clamp: vector::<2>(ty, "PitchClamp")?,
        collision_radius: number(ty, "CollisionRadius")?,
        smooth_time: number(ty, "CameraSmoothTime")?,
        rotation_smooth_time: number(ty, "CameraRotationSmoothTime")?,
        other_offsets,
    })
}

impl Player {
    /// Reads the values from a `Cache.Windows` folder, including the attribute text in the
    /// `H.Misc` headers of the roll and bullet jump clips.
    pub fn load(game: &Path) -> Result<Self> {
        let bin = PackagesBin::load(game)?;
        let mut types = TypeResolver::new(&bin)?;
        let mut misc = Package::open_in(game, "H.Misc")?.context("no H.Misc package")?;
        Self::read_with(&mut types, |path| {
            let Some(raw) = misc.read_path(path)? else {
                return Ok(None);
            };
            Ok(Some(CommonHeader::parse(&raw)?.attributes))
        })
    }

    /// Reads the values with a resolver the caller already has. The two clips ([`ROLL_CLIP`],
    /// [`BULLET_JUMP_CLIP`]) then have what Packages.bin says about them and nothing from their
    /// header text; [`Player::read_with`] adds that.
    pub fn read(types: &mut TypeResolver) -> Result<Self> {
        Self::read_with(types, |_| Ok(None))
    }

    /// Like [`Player::read`]; `header` gives the attribute text of a clip's `H.Misc` header for a
    /// path (`None` when it has no file), which overrides the entity's keys (see
    /// [`ClipSpec::apply_header`]).
    pub fn read_with(
        types: &mut TypeResolver,
        mut header: impl FnMut(&str) -> Result<Option<String>>,
    ) -> Result<Self> {
        let mut resolve = |path: &str| -> Result<std::rc::Rc<TypeInfo>> {
            types
                .resolve(path)?
                .ok_or_else(|| anyhow!("{path} is not in Packages.bin"))
        };
        let avatar = resolve(AVATAR)?;
        let named = named_path;
        // PhysicsBehavior=TennoWalk -> ControllerAttributes=TennoWalkAttribs
        let behavior = resolve(&named(&avatar, "PhysicsBehavior")?)?;
        let walk = resolve(&named(&behavior, "ControllerAttributes")?)?;
        let camera_ty = resolve(&named(&avatar, "CameraControllerType")?)?;
        let motion = resolve(&named(&avatar, "MotionControllerType")?)?;
        let climb = resolve(&named(&avatar, "ClimbControllerType")?)?;
        let input = resolve(&named(&avatar, "InputControllerType")?)?;
        let physics = resolve(PHYSICS_MGR)?;
        let slide = resolve(SLIDE_ACTION)?;

        let eye_offset = vector::<3>(&avatar, "EyeOffset")?;
        anyhow::ensure!(
            eye_offset.value[0] == 0.0 && eye_offset.value[2] == 0.0,
            "EyeOffset is not straight up: {:?}",
            eye_offset.value
        );
        let player = Player {
            mesh: text(&avatar, "Mesh")?,
            gravity: Gravity {
                world: vector::<3>(&physics, "Gravity")?,
                additional: number(&walk, "AdditionalGravity")?,
            },
            capsule: Capsule {
                height: number(&walk, "Height")?,
                radius: number(&walk, "CapsuleRadius")?,
                crouch_radius: number(&walk, "CapsuleCrouchRadius")?,
                step_height: number(&walk, "StepHeight")?,
                skin_width: number(&walk, "SkinWidth")?,
                slope_limit: number(&walk, "SlopeLimit")?,
                eye_offset,
                crouch_offset: vector::<3>(&avatar, "CrouchOffset")?,
            },
            ground: Ground {
                max_speed: number(&walk, "MaxSpeed")?,
                walk_fraction: number(&walk, "WalkSpeedFraction")?,
                jog_fraction: number(&walk, "JogSpeedFraction")?,
                acceleration: number(&walk, "Acceleration")?,
                accel_drag: number(&walk, "AccelDrag")?,
                decel_drag: number(&walk, "DecelDrag")?,
                run_turn_rate: number(&motion, "RunTurnRate")?,
            },
            air: Air {
                acceleration: number(&walk, "AirAcceleration")?,
                accel_drag: number(&walk, "AirAccelDrag")?,
                decel_drag: number(&walk, "AirDecelDrag")?,
            },
            slide: Slide {
                accel: number(&walk, "SlideAccel")?,
                drag: number(&walk, "SlideDrag")?,
                end_threshold: number(&motion, "SlideEndThreshold")?,
                jump_speed: number(&slide, "JumpSpeed")?,
                gravity_adjust: number(&slide, "GravityAdjust")?,
            },
            jump: Jump {
                height: number(&walk, "JumpHeight")?,
                num_jumps: number(&avatar, "NumJumps")?,
                air_jump_height_multiplier: number(&avatar, "AirJumpHeightMultiplier")?,
                double_jump_speed: number(&climb, "DoubleJumpSpeed")?,
                anim_triggered: flag_of(&avatar, "AnimTriggeredJump")?,
            },
            bullet_jump: BulletJump {
                speed: number(&climb, "BulletJumpSpeed")?,
                gravity_multiplier_duration: number(&climb, "BulletJumpGravityMultiplierDuration")?,
                clip: ClipSpec::default(),
            },
            roll: Roll {
                clip: ClipSpec::default(),
            },
            camera: camera(&camera_ty)?,
            crouch: tap_hold(&input, "PRE_CROUCH")?,
        };
        drop(resolve);
        let mut player = player;
        for (clip, path) in [
            (&mut player.bullet_jump.clip, BULLET_JUMP_CLIP),
            (&mut player.roll.clip, ROLL_CLIP),
        ] {
            *clip = clip_spec(types, path).context(path)?;
            if let Some(text) = header(path).context(path)? {
                clip.apply_header(&text).context(path)?;
            }
        }
        Ok(player)
    }

    /// The downward acceleration while the avatar falls: the world's plus the avatar's own
    /// (reading, see [`Gravity::additional`]). Metres per second squared; positive.
    pub fn fall_gravity(&self) -> f32 {
        -self.gravity.world.value[1] - self.gravity.additional.value
    }

    /// The downward acceleration while the avatar rises: the world's. Metres per second squared.
    pub fn rise_gravity(&self) -> f32 {
        -self.gravity.world.value[1]
    }

    /// Every value with its unit and source, in the order of the struct.
    pub fn rows(&self) -> Vec<Row> {
        fn row<T: std::fmt::Debug>(
            name: &'static str,
            s: &Sourced<T>,
            unit: &'static str,
            reading: bool,
        ) -> Row {
            Row {
                name,
                value: format!("{:?}", s.value),
                unit,
                reading,
                source: s.cite(),
            }
        }
        let (c, g, a, s, j, b, cam, d) = (
            &self.capsule,
            &self.ground,
            &self.air,
            &self.slide,
            &self.jump,
            &self.bullet_jump,
            &self.camera,
            &self.crouch,
        );
        let mut rows = vec![
            row("avatar mesh", &self.mesh, "path", false),
            row("gravity", &self.gravity.world, "m/s^2", true),
            row(
                "additional gravity",
                &self.gravity.additional,
                "m/s^2",
                true,
            ),
            row("capsule height", &c.height, "m", true),
            row("capsule radius", &c.radius, "m", true),
            row("capsule crouch radius", &c.crouch_radius, "m", true),
            row("step height", &c.step_height, "m", true),
            row("skin width", &c.skin_width, "m", true),
            row("slope limit", &c.slope_limit, "degrees", true),
            row("eye offset", &c.eye_offset, "m", true),
            row("crouch offset", &c.crouch_offset, "m", true),
            row("max speed", &g.max_speed, "m/s", true),
            row(
                "walk speed fraction",
                &g.walk_fraction,
                "of max speed",
                false,
            ),
            row("jog speed fraction", &g.jog_fraction, "of max speed", false),
            row("acceleration", &g.acceleration, "unit not stated", true),
            row("accel drag", &g.accel_drag, "1/s", true),
            row("decel drag", &g.decel_drag, "1/s", true),
            row("run turn rate", &g.run_turn_rate, "degrees/s", true),
            row("air acceleration", &a.acceleration, "unit not stated", true),
            row("air accel drag", &a.accel_drag, "1/s", true),
            row("air decel drag", &a.decel_drag, "1/s", true),
            row("slide accel", &s.accel, "m/s^2", true),
            row("slide drag", &s.drag, "1/s", true),
            row("slide end threshold", &s.end_threshold, "m/s", true),
            row("slide jump speed", &s.jump_speed, "m/s", true),
            row(
                "slide gravity adjust",
                &s.gravity_adjust,
                "not determined",
                true,
            ),
            row("jump height", &j.height, "m/s (take-off speed)", true),
            row("num jumps", &j.num_jumps, "jumps between landings", true),
            row(
                "air jump height multiplier",
                &j.air_jump_height_multiplier,
                "of jump height",
                false,
            ),
            row(
                "double jump speed",
                &j.double_jump_speed,
                "not stated",
                true,
            ),
            row("anim triggered jump", &j.anim_triggered, "flag", true),
            row("bullet jump speed", &b.speed, "of max speed", true),
            row(
                "bullet jump gravity multiplier duration",
                &b.gravity_multiplier_duration,
                "s",
                true,
            ),
            row(
                "camera view offset",
                &cam.view_offset,
                "m (right, up, forward)",
                true,
            ),
            row(
                "camera view offset speed",
                &cam.view_offset_speed,
                "m/s",
                true,
            ),
            row("camera base fov", &cam.base_fov, "degrees", true),
            row("camera pitch clamp", &cam.pitch_clamp, "degrees", true),
            row("camera collision radius", &cam.collision_radius, "m", true),
            row("camera smooth time", &cam.smooth_time, "s", true),
            row(
                "camera rotation smooth time",
                &cam.rotation_smooth_time,
                "s",
                true,
            ),
            row("crouch button hold time", &d.required_hold_time, "s", true),
        ];
        let tap = |name: &'static str, value: String, key: &str| Row {
            name,
            value,
            unit: "input codes",
            reading: false,
            source: format!("{}: DemuxInputs[{}].{key}", d.entity, d.input_code),
        };
        rows.push(tap(
            "crouch button tap sends",
            format!("{:?}", d.tap_codes),
            "TapInputCodes",
        ));
        rows.push(tap(
            "crouch button hold sends",
            format!("{:?}", d.hold_codes),
            "HoldInputs",
        ));
        rows.push(tap(
            "crouch button tap cancelled by",
            format!("{:?}", d.bypass_tap_codes),
            "BypassTapInputCodes",
        ));
        rows.push(Row {
            name: "crouch button hold sent when down",
            value: d.send_hold_immediately.to_string(),
            unit: "flag",
            reading: false,
            source: format!(
                "{}: DemuxInputs[{}].SendHoldImmediately",
                d.entity, d.input_code
            ),
        });
        rows.push(Row {
            name: "crouch button hold at once with no direction",
            value: d.force_hold_when_directionless.to_string(),
            unit: "flag",
            reading: true,
            source: format!(
                "{}: DemuxInputs[{}].ForceHoldWhenDirectionless",
                d.entity, d.input_code
            ),
        });
        for (name, spec) in [
            ("roll", &self.roll.clip),
            ("bullet jump clip", &self.bullet_jump.clip),
        ] {
            let src = |key: &str| {
                spec.sources
                    .get(key)
                    .map_or(spec.path.clone(), |e| format!("{e}: {key}"))
            };
            if let Some(m) = spec.root_override {
                rows.push(Row {
                    name: if name == "roll" {
                        "roll distance"
                    } else {
                        "bullet jump clip root motion"
                    },
                    value: format!("{m:?}"),
                    unit: "m",
                    reading: true,
                    source: src("RootAdjustment"),
                });
            }
            if let Some(c) = &spec.root_conditioning {
                rows.push(Row {
                    name: if name == "roll" {
                        "roll root conditioning"
                    } else {
                        "bullet jump clip root conditioning"
                    },
                    value: c.clone(),
                    unit: "mode",
                    reading: true,
                    source: src("RootConditioning"),
                });
            }
            if !spec.events.is_empty() {
                rows.push(Row {
                    name: if name == "roll" {
                        "roll events"
                    } else {
                        "bullet jump clip events"
                    },
                    value: spec
                        .events
                        .iter()
                        .map(|e| format!("{}@{}", e.name, e.phase))
                        .collect::<Vec<_>>()
                        .join(" "),
                    unit: "name@fraction of the clip",
                    reading: false,
                    source: src("Events"),
                });
            }
        }
        for (k, v) in &cam.other_offsets {
            rows.push(Row {
                name: "camera offset",
                value: format!("{k} = {:?}", v.value),
                unit: "m (right, up, forward)",
                reading: true,
                source: v.cite(),
            });
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "/Lotus/Types/Player/";
    const M: &str = "/Lotus/Animations/Player/Movement/";
    const PHYS: &str = "/EE/Types/Physics/";

    type Entity = (
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
    );

    /// The entities the loader follows, under their real paths with the install's values (split
    /// over parents, as a text of the test builder is under 128 bytes). `skip` leaves one out.
    fn entities(skip: &str) -> Vec<Entity> {
        let all: Vec<Entity> = vec![
            (P, "TennoAvatar", "AvatarBase", Some("Mesh=/Lotus/M/X_skel.fbx\nPhysicsBehavior=TennoWalk\nEyeOffset={0,1.6,0}\nCrouchOffset={0,1,0}\n")),
            (P, "AvatarBase", "AvatarBase2", Some("NumJumps=2\nAirJumpHeightMultiplier=0.75\nAnimTriggeredJump=1\nClimbControllerType=TennoClimbController\n")),
            (P, "AvatarBase2", "", Some("CameraControllerType=CameraController\nMotionControllerType=TennoMotion\nInputControllerType=InputController\n")),
            (P, "TennoWalk", "", Some("ControllerAttributes=TennoWalkAttribs\n")),
            (PHYS, "ControllerAttribs", "", Some("SlopeLimit=45\n")),
            (P, "TennoWalkAttribs", "WalkB", Some("MaxSpeed=7.5\nWalkSpeedFraction=0.4\nJogSpeedFraction=0.8\nAcceleration=12\nAccelDrag=8.5\nDecelDrag=10\n")),
            (P, "WalkD", "WalkC", Some("JumpHeight=5\nAdditionalGravity=-9\n")),
            (P, "WalkB", "WalkD", Some("Height=1.5\nCapsuleRadius=0.45\nCapsuleCrouchRadius=0.4\nStepHeight=0.45\nSkinWidth=0.02\nAirAcceleration=10\n")),
            (P, "WalkC", "/EE/Types/Physics/ControllerAttribs", Some("AirAccelDrag=0.05\nAirDecelDrag=0.2\nSlideAccel=6\nSlideDrag=0.7\n")),
            (P, "TennoMotion", "", Some("RunTurnRate=180\nSlideEndThreshold=1.66\n")),
            (P, "TennoClimbController", "", Some("BulletJumpSpeed=2.5\nBulletJumpGravityMultiplierDuration=0.8\nDoubleJumpSpeed=0.35\n")),
            (P, "CameraController", "Cam2", Some("ViewOffset={0.5,-0.1,-1.5}\nViewOffsetSpeed=1\nBaseFovOverride=65\nPitchClamp={-89,89}\nCollisionRadius=0.25\n")),
            (P, "Cam2", "", Some("CameraSmoothTime=0.25\nCameraRotationSmoothTime=0.15\nAimViewOffset={0.5,-0.1,-1}\nRumble=3\n")),
            (P, "InputController", "", Some("DemuxInputs={\n{\nInputCode=PRE_CROUCH\nTapInputCodes={ACTION}\nBypassTapInputCodes={JUMP,MELEE}\nRequiredHoldTime=0.15\n}\n}\n")),
            (PHYS, "PhysicsMgrDesc", "", Some("Gravity={0,-15,0}\n")),
            (PHYS, "AsyncPhysicsMgrDesc", "PhysicsMgrDesc", None),
            ("/EE/Types/Game/", "SlideAction", "", Some("JumpSpeed=5\nGravityAdjust=-35\n")),
            (M, "RollForward_anim.fbx", "", Some("RootAdjustment={\nuseExistingMotion=0\noverriddenMotion={0,0,8}\n}\n")),
            (M, "RollForwardMelee", "RollForward_anim.fbx", None),
            (M, "JumpRollForwardB_anim.fbx", "", Some("Events={\n{\nTime=0.01\nEvent=BulletJumpStart\n}\n}\n")),
        ];
        all.into_iter().filter(|e| e.1 != skip).collect()
    }

    fn read(skip: &str) -> Result<Player> {
        let bin = PackagesBin::parse(&crate::packages::tests::build(&entities(skip))).unwrap();
        let mut types = TypeResolver::new(&bin).unwrap();
        Player::read(&mut types)
    }

    #[test]
    fn reads_every_value_and_cites_where_it_came_from() {
        let p = read("").unwrap();
        assert_eq!(p.ground.max_speed.value, 7.5);
        assert_eq!(
            p.ground.max_speed.cite(),
            "/Lotus/Types/Player/TennoWalkAttribs: MaxSpeed"
        );
        // A value set several parents up names the parent
        assert_eq!(p.capsule.height.entity, "/Lotus/Types/Player/WalkB");
        assert_eq!(p.slide.drag.entity, "/Lotus/Types/Player/WalkC");
        assert_eq!(
            p.capsule.slope_limit.entity,
            "/EE/Types/Physics/ControllerAttribs"
        );
        assert_eq!(p.capsule.slope_limit.value, 45.0);
        assert_eq!(p.jump.num_jumps.entity, "/Lotus/Types/Player/AvatarBase");
        assert_eq!(p.gravity.world.value, [0.0, -15.0, 0.0]);
        assert_eq!(p.gravity.world.entity, "/EE/Types/Physics/PhysicsMgrDesc");
        assert_eq!((p.fall_gravity(), p.rise_gravity()), (24.0, 15.0));
        assert!(p.jump.anim_triggered.value);
        assert_eq!(p.bullet_jump.speed.value, 2.5);
        assert_eq!(p.camera.view_offset.value, [0.5, -0.1, -1.5]);
        assert_eq!(p.camera.pitch_clamp.value, [-89.0, 89.0]);
        assert_eq!(
            p.camera.other_offsets.keys().collect::<Vec<_>>(),
            ["AimViewOffset"]
        );
        assert_eq!(p.crouch.tap_codes, ["ACTION"]);
        assert!(p.crouch.hold_codes.is_empty());
        assert_eq!(p.crouch.bypass_tap_codes, ["JUMP", "MELEE"]);
        assert!(!p.crouch.force_hold_when_directionless);
        // The roll's distance is set by the entity the clip derives from
        assert_eq!(p.roll.clip.root_override, Some([0.0, 0.0, 8.0]));
        assert_eq!(
            p.roll.clip.sources["RootAdjustment"],
            "/Lotus/Animations/Player/Movement/RollForward_anim.fbx"
        );
        assert_eq!(p.bullet_jump.clip.events[0].name, "BulletJumpStart");
        // Every row names a source
        let rows = p.rows();
        assert!(
            rows.len() > 40
                && rows
                    .iter()
                    .all(|r| r.source.contains(':') || r.source.starts_with('/'))
        );
    }

    #[test]
    fn names_inherited_from_a_parent_resolve_in_the_parents_folder() {
        // The keys that name the controllers are set on AvatarBase2, which lives elsewhere than
        // the avatar; the controllers are next to it, and not in the avatar's folder
        const S: &str = "/Lotus/Types/Shared/";
        let mut e: Vec<Entity> = Vec::new();
        for (folder, name, parent, text) in entities("") {
            match name {
                "AvatarBase" => e.push((folder, name, "/Lotus/Types/Shared/AvatarBase2", text)),
                "AvatarBase2" | "CameraController" | "Cam2" | "TennoMotion" | "InputController" => {
                    e.push((S, name, parent, text))
                }
                _ => e.push((folder, name, parent, text)),
            }
        }
        let bin = PackagesBin::parse(&crate::packages::tests::build(&e)).unwrap();
        let mut types = TypeResolver::new(&bin).unwrap();
        let p = Player::read(&mut types).unwrap();
        assert_eq!(
            p.ground.run_turn_rate.entity,
            "/Lotus/Types/Shared/TennoMotion"
        );
        assert_eq!(p.camera.view_offset.value, [0.5, -0.1, -1.5]);
        assert_eq!(p.crouch.tap_codes, ["ACTION"]);
        // The climb controller is named by AvatarBase, in the avatar's folder, as before
        assert_eq!(
            p.bullet_jump.speed.entity,
            "/Lotus/Types/Player/TennoClimbController"
        );
    }

    /// In the game's data: an avatar that derives from TennoAvatar from another folder.
    #[test]
    fn the_games_inherited_names_point_at_entities_that_exist() {
        let Some(dir) = std::env::var_os("WF_GAME_DIR") else {
            return;
        };
        let bin = PackagesBin::load(std::path::Path::new(&dir)).unwrap();
        let mut types = TypeResolver::new(&bin).unwrap();
        let ghost = types
            .resolve("/Lotus/Types/GameRules/LotusGhostAvatar")
            .unwrap()
            .unwrap();
        assert!(ghost.chain.iter().any(|c| c == AVATAR), "{:?}", ghost.chain);
        for (key, want) in [
            ("PhysicsBehavior", "/Lotus/Types/Player/TennoWalk"),
            ("MotionControllerType", "/Lotus/Types/Player/TennoMotion"),
            (
                "CameraControllerType",
                "/Lotus/Types/Player/CameraController",
            ),
        ] {
            let path = named_path(&ghost, key).unwrap();
            assert_eq!(path, want, "{key}");
            assert!(bin.contains(&path), "{path}");
        }
    }

    #[test]
    fn a_missing_value_is_an_error_not_a_default() {
        let err = format!("{:#}", read("TennoMotion").unwrap_err());
        assert!(err.contains("TennoMotion is not in Packages.bin"), "{err}");
        let err = format!("{:#}", read("TennoClimbController").unwrap_err());
        assert!(
            err.contains("TennoClimbController is not in Packages.bin"),
            "{err}"
        );
        // An entity that exists but lacks a key
        let mut e = entities("");
        e.retain(|x| x.1 != "TennoMotion");
        e.push((P, "TennoMotion", "", Some("RunTurnRate=180\n")));
        let bin = PackagesBin::parse(&crate::packages::tests::build(&e)).unwrap();
        let mut types = TypeResolver::new(&bin).unwrap();
        let err = Player::read(&mut types).unwrap_err().to_string();
        assert_eq!(
            err,
            "/Lotus/Types/Player/TennoMotion has no SlideEndThreshold"
        );
    }

    #[test]
    fn an_eye_offset_off_the_vertical_is_refused() {
        let mut e = entities("");
        e.retain(|x| x.1 != "TennoAvatar");
        e.push((P, "TennoAvatar", "AvatarBase", Some("Mesh=/M/X.fbx\nPhysicsBehavior=TennoWalk\nEyeOffset={0.5,1.6,0}\nCrouchOffset={0,1,0}\n")));
        let bin = PackagesBin::parse(&crate::packages::tests::build(&e)).unwrap();
        let mut types = TypeResolver::new(&bin).unwrap();
        assert!(Player::read(&mut types)
            .unwrap_err()
            .to_string()
            .contains("not straight up"));
    }

    #[test]
    fn tap_hold_picks_its_entry_from_the_demux_list() {
        let bin = PackagesBin::parse(&crate::packages::tests::build(&[(
            "/P/",
            "Input",
            "",
            Some("DemuxInputs={\n{\nInputCode=A\n},\n{\nInputCode=B\nRequiredHoldTime=0.5\nSendHoldImmediately=1\nHoldInputs={{inputCode=H}}\n}\n}\n"),
        )]))
        .unwrap();
        let mut types = TypeResolver::new(&bin).unwrap();
        let input = types.resolve("/P/Input").unwrap().unwrap();
        let d = tap_hold(&input, "B").unwrap();
        assert!((d.required_hold_time.value - 0.5).abs() < 1e-6);
        assert_eq!(d.required_hold_time.key, "DemuxInputs[B].RequiredHoldTime");
        assert!(d.send_hold_immediately && !d.force_hold_when_directionless);
        assert_eq!(d.hold_codes, ["H"]);
        assert!(d.tap_codes.is_empty());
        assert!(tap_hold(&input, "NOPE").is_err());
    }
}
