//! Animation state machines (`.animfsm`, type 383) and animation trees (`.animtree`,
//! type 388), and what Excalibur's controller makes of them.
//!
//! Both are the same container: a graph of nodes. `H.Misc` holds the node data of an
//! `.animfsm` and `H.AnimRetarget` that of an `.animtree`; the edges are in the same
//! package under `B.`.
//!
//! # Container
//!
//! The `H.` entry is a [`CommonHeader`] followed by (little-endian):
//!
//! ```text
//! [8]  01 b8 21 61 19 cd 25 02     signature
//! u8   57                          version
//! str  parent graph                path of the graph this one derives from, or empty
//! u32  node_count
//! u32  edge_count                  entries in the B part
//! u32  type_count, then type_count x str   node type paths (/EE/Types/AnimationTreeNodes/...)
//! u32  node_count, node_count x u8         the type of each node, an index into the list
//! node bodies, one after the other, in node order
//! ```
//!
//! The `B.` entry holds the edges of the graph, `node_count` lists of child node numbers:
//!
//! ```text
//! u32 0
//! u32 len_0
//! (node_count - 1) x { u32 start_i, u32 len_i }
//! edge_count x i32                 child node number, -1 for an empty slot
//! ```
//!
//! Node `i`'s children are `entries[start_i .. start_i + len_i]` (`start_0` is 0). What a
//! child means depends on the node's type: a `Select` has one slot per value of its variable, a
//! state machine node one per exposed state, a blend two inputs.
//!
//! # State machines (`.animfsm`)
//!
//! Every node is an `AnimationStateNode`. Its body:
//!
//! ```text
//! str  name
//! i32  child                       which child of the FsmAnimNode plays in this state, -1 for none
//! str  tag                         usually empty ("move" on the walking states)
//! u32  transition_count            equals the length of the node's edge list
//! u32  condition_count
//! condition_count x Cond           the conditions of the state
//! transition_count x { f32 blend, u32 mode, u32 n, n x Cond }
//! u32  m, m x Cond                 more conditions
//! 4 x { str event, u32 value }     events of the state
//! u32  k, k x u32                  variable numbers
//! ```
//!
//! A condition has two layouts. All but the last transition and the first list use
//! `Cond::Full`: `u32 variable, u32 a, u32 op, str text, f32 value`. The last transition, the
//! second list, and the first list of a state without transitions use `Cond::Short`:
//! `u32 variable, str text, f32 value, i32 int_value`. The edge list of a state has the target
//! of the *last* transition first, then those of the others in order.
//!
//! The file ends with the exposed states (`u32 k, k x { str name, u8 flag }`, the children of the
//! `FsmAnimNode` that runs this machine, in that order), `i32` the state it starts in, `u32 n`
//! and n texts (compile errors the editor wrote into the file).
//!
//! A condition compares a variable, known only by number (`102`...), with a value that the
//! data names: `v108 == JOG` has the text `JOG` and the value 1.0. `op` 0 is `==` and 1 is
//! `!=` (every use agrees); 2, 3, 4, 5 are `<`, `<=`, `>`, `>=` as used by the speed checks
//! (`v126` is the movement speed: `Moving` goes to `Idle` on `v126 op2 0.05`); other codes are kept raw.
//!
//! # Trees (`.animtree`)
//!
//! After the node bodies come four pools, each `u32 n, n x str`: clips, state machines, sub-trees
//! and errors. Nodes refer to the pools by index. Node bodies are described on [`Node`]; a clip
//! is chosen by a table (`Pick`) that maps the values of up to two variables to a clip index.
//!
//! Variables are numbers in the data; their names are in code. What the data gives are the
//! texts that conditions compare them with (see [`variable_hints`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

use crate::bytes::Reader;
use crate::cache::Package;
use crate::header::CommonHeader;

pub const KIND_ANIMFSM: u16 = 383;
pub const KIND_ANIMTREE: u16 = 388;

const SIGNATURE: [u8; 8] = [0x01, 0xb8, 0x21, 0x61, 0x19, 0xcd, 0x25, 0x02];
const MAX_STR: usize = 4096;

// ---------------------------------------------------------------------------
// Container

#[derive(Debug, Clone)]
pub struct Graph {
    pub common: CommonHeader,
    pub parent: String,
    /// Node type paths.
    pub types: Vec<String>,
    /// For each node, an index into `types`.
    pub node_types: Vec<u8>,
    /// Children of each node (`-1` is an empty slot).
    pub edges: Vec<Vec<i32>>,
    /// Where the node bodies start in the `H.` entry.
    pub body_start: usize,
}

impl Graph {
    /// Parses the `H.` and `B.` parts up to the start of the node bodies.
    pub fn parse(head: &[u8], body: &[u8], kind: u16) -> Result<Self> {
        let common = CommonHeader::parse(head).context("common header")?;
        ensure!(
            common.kind == kind,
            "type {} is not a graph of type {kind}",
            common.kind
        );
        let mut r = Reader::at(head, common.len)?;
        ensure!(r.bytes(8)? == SIGNATURE, "unknown graph signature");
        let version = r.u8()?;
        ensure!(version == 57, "unsupported graph version {version}");
        let parent = r.str32(MAX_STR)?;
        let n = r.u32()? as usize;
        let edge_count = r.u32()? as usize;
        let type_count = r.u32()? as usize;
        ensure!(n < 100_000 && type_count < 1000, "implausible graph size");
        let types = (0..type_count)
            .map(|_| r.str32(MAX_STR))
            .collect::<Result<Vec<_>>>()?;
        ensure!(r.u32()? as usize == n, "node count repeated differently");
        let node_types = r.bytes(n)?.to_vec();
        ensure!(
            node_types.iter().all(|&t| (t as usize) < types.len()),
            "node type index out of range"
        );
        let edges = parse_edges(body, n, edge_count)?;
        Ok(Graph {
            common,
            parent,
            types,
            node_types,
            edges,
            body_start: r.pos(),
        })
    }

    /// Short name of a node's type, e.g. `Select`.
    pub fn type_name(&self, node: usize) -> &str {
        let t = &self.types[self.node_types[node] as usize];
        t.rsplit('/').next().unwrap_or(t)
    }
}

fn parse_edges(b: &[u8], n: usize, edge_count: usize) -> Result<Vec<Vec<i32>>> {
    let mut r = Reader::new(b);
    ensure!(r.u32()? == 0, "unexpected first word of an edge list");
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut spans = Vec::with_capacity(n);
    spans.push((0usize, r.u32()? as usize));
    for _ in 1..n {
        let start = r.u32()? as usize;
        let len = r.u32()? as usize;
        spans.push((start, len));
    }
    let entries = (0..edge_count)
        .map(|_| r.u32().map(|v| v as i32))
        .collect::<Result<Vec<_>>>()?;
    ensure!(r.rest().is_empty(), "bytes left after the edge list");
    spans
        .into_iter()
        .map(|(s, l)| {
            entries
                .get(s..s + l)
                .map(<[i32]>::to_vec)
                .context("edge list out of range")
        })
        .collect()
}

// ---------------------------------------------------------------------------
// State machines

/// A comparison of a variable with a value the data names.
#[derive(Debug, Clone, PartialEq)]
pub struct Cond {
    /// Variable number (names are in code).
    pub var: u32,
    /// Operator code of the full layout; `None` in the short layout, which has no operator.
    pub op: Option<u32>,
    /// The second word of the full layout (0 in every file but the cover-pose tables).
    pub a: Option<u32>,
    /// The value as written: an enum name (`JOG`), a number (`0.05`) or an expression fragment.
    pub text: String,
    pub value: f32,
    /// The short layout also stores the value as an integer (-1 for a non-integer).
    pub int_value: Option<i32>,
}

impl Cond {
    /// The operator for display; the short layout has none, so it reads `?`.
    pub fn op_symbol(&self) -> String {
        match self.op {
            Some(0) => "==".into(),
            Some(1) => "!=".into(),
            Some(2) => "<".into(),
            Some(3) => "<=".into(),
            Some(4) => ">".into(),
            Some(5) => ">=".into(),
            Some(n) => format!("op{n}"),
            None => "?".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    /// Blend time in seconds.
    pub blend: f32,
    /// A small enum (0, 1, 2, 5 seen); its meaning is in code.
    pub mode: u32,
    pub conditions: Vec<Cond>,
    /// Target state, -1 for none.
    pub target: i32,
    /// The last transition of the state, stored in a different layout and first in the edges.
    pub last: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StateEvent {
    pub name: String,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub name: String,
    /// Child of the `FsmAnimNode` that plays in this state (index into the exposed states), -1 for none.
    pub child: i32,
    pub tag: String,
    pub conditions: Vec<Cond>,
    pub transitions: Vec<Transition>,
    pub more_conditions: Vec<Cond>,
    pub events: [StateEvent; 4],
    pub variables: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Exposed {
    pub name: String,
    pub flag: u8,
}

#[derive(Debug, Clone)]
pub struct Fsm {
    pub parent: String,
    pub states: Vec<State>,
    pub exposed: Vec<Exposed>,
    pub start: i32,
    pub errors: Vec<String>,
}

fn full_cond(r: &mut Reader) -> Result<Cond> {
    let var = r.u32()?;
    let a = r.u32()?;
    let op = r.u32()?;
    let text = r.str32(MAX_STR)?;
    let value = r.f32()?;
    ensure!(
        var < 100_000 && a < 1000 && op < 1000,
        "implausible condition"
    );
    Ok(Cond {
        var,
        op: Some(op),
        a: Some(a),
        text,
        value,
        int_value: None,
    })
}

fn short_cond(r: &mut Reader) -> Result<Cond> {
    let var = r.u32()?;
    let text = r.str32(MAX_STR)?;
    let value = r.f32()?;
    let int_value = r.u32()? as i32;
    ensure!(var < 100_000, "implausible condition");
    Ok(Cond {
        var,
        op: None,
        a: None,
        text,
        value,
        int_value: Some(int_value),
    })
}

fn cond_list(r: &mut Reader, short: bool) -> Result<Vec<Cond>> {
    let n = r.u32()? as usize;
    ensure!(n <= 64, "implausible condition count {n}");
    (0..n)
        .map(|_| if short { short_cond(r) } else { full_cond(r) })
        .collect()
}

impl Fsm {
    pub fn parse(head: &[u8], body: &[u8]) -> Result<Self> {
        let g = Graph::parse(head, body, KIND_ANIMFSM)?;
        ensure!(
            g.types.len() == 1 && g.types[0].ends_with("/AnimationStateNode"),
            "state machine with other node types: {:?}",
            g.types
        );
        let mut r = Reader::at(head, g.body_start)?;
        let mut states = Vec::with_capacity(g.edges.len());
        for (i, edges) in g.edges.iter().enumerate() {
            states.push(parse_state(&mut r, edges).with_context(|| format!("state {i}"))?);
        }
        let k = r.u32()? as usize;
        ensure!(k < 1000, "implausible exposed state count {k}");
        let mut exposed = Vec::with_capacity(k);
        for _ in 0..k {
            let name = r.str32(MAX_STR)?;
            exposed.push(Exposed {
                name,
                flag: r.u8()?,
            });
        }
        let start = r.u32()? as i32;
        let errors_n = r.u32()? as usize;
        ensure!(errors_n < 10_000, "implausible error count");
        let errors = (0..errors_n)
            .map(|_| r.str32(1 << 16))
            .collect::<Result<Vec<_>>>()?;
        ensure!(r.rest().is_empty(), "bytes left after the state machine");
        Ok(Fsm {
            parent: g.parent,
            states,
            exposed,
            start,
            errors,
        })
    }

    /// Name of the child slot a state plays, if it has one.
    pub fn child_name(&self, state: &State) -> Option<&str> {
        usize::try_from(state.child)
            .ok()
            .and_then(|c| self.exposed.get(c))
            .map(|e| e.name.as_str())
    }

    /// Index of the exposed state called `name`.
    pub fn exposed_index(&self, name: &str) -> Option<usize> {
        self.exposed.iter().position(|e| e.name == name)
    }
}

fn parse_state(r: &mut Reader, edges: &[i32]) -> Result<State> {
    let name = r.str32(MAX_STR)?;
    let child = r.u32()? as i32;
    let tag = r.str32(MAX_STR)?;
    let c = r.u32()? as usize;
    ensure!(
        c == edges.len(),
        "{c} transitions but {} edges",
        edges.len()
    );
    let conditions = cond_list(r, c == 0)?;
    let mut transitions = Vec::with_capacity(c);
    for t in 0..c {
        let blend = r.f32()?;
        let mode = r.u32()?;
        ensure!(
            blend.is_finite() && (-1.0..=1000.0).contains(&blend),
            "blend {blend}"
        );
        ensure!(mode < 1000, "implausible transition mode {mode}");
        let last = t + 1 == c;
        let conds = cond_list(r, last)?;
        // The edge list starts with the last transition's target
        let target = if last { edges[0] } else { edges[t + 1] };
        transitions.push(Transition {
            blend,
            mode,
            conditions: conds,
            target,
            last,
        });
    }
    let more_conditions = cond_list(r, true)?;
    let mut events = Vec::with_capacity(4);
    for _ in 0..4 {
        events.push(StateEvent {
            name: r.str32(MAX_STR)?,
            value: r.u32()?,
        });
    }
    let nv = r.u32()? as usize;
    ensure!(nv < 64, "implausible variable list");
    let variables = (0..nv).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
    Ok(State {
        name,
        child,
        tag,
        conditions,
        transitions,
        more_conditions,
        events: events.try_into().unwrap(),
        variables,
    })
}

// ---------------------------------------------------------------------------
// Trees

/// Maps the values of up to two variables to an index (a clip, in animation nodes). With
/// one variable the table has one entry per value; with two, `rows x columns` entries, the
/// first variable selecting the row. Without variables the table has the single entry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pick {
    pub vars: Vec<u32>,
    pub table: Vec<i32>,
}

impl Pick {
    fn parse(r: &mut Reader) -> Result<Self> {
        let n = r.u32()? as usize;
        ensure!(n <= 4, "implausible selector variable count {n}");
        let vars = (0..n).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
        let m = r.u32()? as usize;
        ensure!(m <= 5000, "implausible table size {m}");
        let table = (0..m)
            .map(|_| r.u32().map(|v| v as i32))
            .collect::<Result<Vec<_>>>()?;
        Ok(Pick { vars, table })
    }

    /// The distinct clip indices of the table.
    pub fn clips(&self) -> BTreeSet<i32> {
        self.table.iter().copied().filter(|&c| c >= 0).collect()
    }

    /// The values of the variables that select table entry `index`: the first variable
    /// varies slowest. `cards` gives the number of values of each variable (see
    /// [`Tree::cardinalities`]); with two or more variables the last may be unknown, and
    /// is then the quotient of the table size.
    pub fn values_of(&self, index: usize, cards: &BTreeMap<u32, usize>) -> Option<Vec<usize>> {
        let mut dims: Vec<Option<usize>> =
            self.vars.iter().map(|v| cards.get(v).copied()).collect();
        let known: usize = dims.iter().flatten().product();
        match dims.iter().filter(|d| d.is_none()).count() {
            0 => {}
            1 if known > 0 && self.table.len().is_multiple_of(known) => {
                let missing = self.table.len() / known;
                dims.iter_mut()
                    .filter(|d| d.is_none())
                    .for_each(|d| *d = Some(missing));
            }
            _ => return None,
        }
        if dims.iter().flatten().product::<usize>() != self.table.len() || index >= self.table.len()
        {
            return None;
        }
        let mut rest = index;
        let mut out = vec![0; dims.len()];
        for (i, d) in dims.iter().enumerate().rev() {
            let d = d.unwrap();
            out[i] = rest % d;
            rest /= d;
        }
        Some(out)
    }
}

/// Plays a clip chosen by `pick`.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimNode {
    pub pick: Pick,
    pub flag_a: u8,
    /// A small enum next to the blend time (0, 1, 2).
    pub mode: u32,
    /// Seconds to blend in.
    pub blend: f32,
    pub flag_b: u8,
    pub tag: String,
    /// `NamedAnimationNode` only.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListenerNode {
    pub pick: Pick,
    pub flag_a: u8,
    pub mode: u32,
    pub blend: f32,
    pub flag_b: u8,
    pub tag: String,
    /// The request number it waits for (set by game code through the controller's actions).
    pub request: u32,
}

/// The common start of the blend nodes (`AdditiveBlend`, `ProceduralBlend`, the partial-body
/// blends...): a weight driven by a variable, and an optional condition on another.
#[derive(Debug, Clone, PartialEq)]
pub struct Blend {
    /// The variable that drives the weight (0 for none).
    pub var: u32,
    pub a: f32,
    pub b: f32,
    pub c: f32,
    /// A second variable and the text it must equal (empty for none).
    pub cond_var: u32,
    pub cond_text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assign {
    pub var: u32,
    pub mode: u32,
    /// A piecewise curve of (input, output) points.
    pub curve: Vec<(f32, f32)>,
    pub flag: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AxisPoint {
    pub pos: f32,
    pub name: String,
    pub w0: f32,
    pub w1: f32,
    pub partner: String,
    pub partner_index: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeedPoint {
    /// Clip pool index.
    pub clip: u32,
    /// Speed at which the clip plays at its own rate (m/s).
    pub speed: f32,
    /// Playback rate multiplier at that speed.
    pub rate: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecoratorEntry {
    pub name: String,
    pub values: (u32, f32, u32, f32),
    pub first: Vec<(u32, f32, f32)>,
    pub second: Vec<(u32, f32, f32)>,
}

/// Parameters of a node type this module reads only to get past it.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Animation(AnimNode),
    NamedAnimation(AnimNode),
    CinematicAnimation(AnimNode),
    Listener(ListenerNode),
    /// Runs a state machine; the children are its exposed states.
    Fsm {
        fsm: u32,
    },
    SubTree {
        path: String,
    },
    /// Names a movement set (type 14), aim-pose set (type 19) or clip.
    Parametric {
        path: String,
    },
    ModifyVariable(Vec<Assign>),
    /// Picks a random child.
    Random {
        count: u32,
        flags: [u8; 6],
        blend: f32,
    },
    /// One child per value of `var`.
    Select {
        var: u32,
        count: u32,
        flags: [u8; 6],
        blend: f32,
    },
    Sequence {
        count: u32,
        blend: f32,
    },
    MovementMode {
        a: u32,
        b: u32,
        flags: [u8; 3],
    },
    Rate {
        min: f32,
        max: f32,
        var: u32,
        scale: f32,
        flag: u32,
    },
    Additive(Blend),
    Procedural(Blend),
    AdditiveViseme(Blend),
    CinematicBlend(Blend),
    PartialBody {
        blend: Blend,
        /// 1: a mask asset, 0: the root bone of a branch.
        by_mask: bool,
        mask: String,
        fade: (f32, f32),
    },
    /// A partial-body blend whose mask depends on a variable.
    PartialBodyMasks {
        blend: Blend,
        var: u32,
        /// Mask asset per value of `var` (empty string for none).
        masks: Vec<String>,
        timings: usize,
        fade: [f32; 3],
        /// Without `var`: the single mask.
        single_mask: String,
        single_by_mask: bool,
    },
    SingleAxis {
        var: u32,
        names: Vec<String>,
        points: Vec<AxisPoint>,
    },
    SpeedBlend {
        lo: f32,
        hi: f32,
        var: u32,
        points: Vec<SpeedPoint>,
    },
    Slice {
        from: f32,
        to: f32,
    },
    Cutoff {
        var: u32,
        value: u32,
    },
    ProceduralTurning {
        var: u32,
        min: f32,
        max: f32,
        rate: f32,
        curve: Vec<(f32, f32)>,
    },
    AnimEvents(Vec<(f32, String)>),
    Decorator {
        entries: Vec<DecoratorEntry>,
    },
    ParametricDelta {
        name: String,
        var: u32,
        curve: Vec<(f32, f32)>,
        clip: u32,
    },
    /// Aim offsets: poses at (yaw, pitch) angles.
    AimPoses {
        var: u32,
        bone: String,
        poses: Vec<(u32, f32, f32)>,
    },
    MotionBlend {
        blend: Blend,
        melee: bool,
        masks: Vec<String>,
        name: String,
        var: u32,
    },
    Posture {
        var: u32,
    },
    ReachIk {
        bone: String,
    },
    FootIk {
        root: String,
        legs: usize,
        pose_data: String,
    },
    LookIk {
        bones: Vec<String>,
    },
    NamedSubTree {
        path: String,
        name: String,
    },
    SubTreeListener {
        path: String,
        request: u32,
    },
}

impl Node {
    /// The clip pool indices this node plays, with a label for each.
    pub fn clip_refs(&self) -> Vec<u32> {
        match self {
            Node::Animation(a) | Node::NamedAnimation(a) | Node::CinematicAnimation(a) => {
                a.pick.clips().into_iter().map(|c| c as u32).collect()
            }
            Node::SpeedBlend { points, .. } => points.iter().map(|p| p.clip).collect(),
            Node::Listener(l) => l.pick.clips().into_iter().map(|c| c as u32).collect(),
            Node::ParametricDelta { clip, .. } => vec![*clip],
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Tree {
    pub graph: Graph,
    pub nodes: Vec<Node>,
    pub clips: Vec<String>,
    pub fsms: Vec<String>,
    pub subtrees: Vec<String>,
    pub errors: Vec<String>,
}

fn pool(r: &mut Reader) -> Result<Vec<String>> {
    let n = r.u32()? as usize;
    ensure!(n < 100_000, "implausible pool size {n}");
    (0..n).map(|_| r.str32(MAX_STR)).collect()
}

fn pairs(r: &mut Reader, max: usize) -> Result<Vec<(f32, f32)>> {
    let n = r.u32()? as usize;
    ensure!(n <= max, "implausible curve length {n}");
    (0..n).map(|_| Ok((r.f32()?, r.f32()?))).collect()
}

impl Tree {
    pub fn parse(head: &[u8], body: &[u8]) -> Result<Self> {
        let g = Graph::parse(head, body, KIND_ANIMTREE)?;
        let mut r = Reader::at(head, g.body_start)?;
        let mut nodes = Vec::with_capacity(g.edges.len());
        for i in 0..g.edges.len() {
            let name = g.type_name(i);
            let start = r.pos();
            let node = parse_node(&mut r, name)
                .with_context(|| format!("node {i} ({name}) at offset {start}"))?;
            nodes.push(node);
        }
        let clips = pool(&mut r).context("clip pool")?;
        let fsms = pool(&mut r).context("state machine pool")?;
        let subtrees = pool(&mut r).context("sub-tree pool")?;
        let errors = pool(&mut r).context("error pool")?;
        ensure!(r.rest().is_empty(), "bytes left after the pools");
        Ok(Tree {
            graph: g,
            nodes,
            clips,
            fsms,
            subtrees,
            errors,
        })
    }

    pub fn children(&self, node: usize) -> &[i32] {
        &self.graph.edges[node]
    }

    /// The path of a clip pool entry, if the index is valid.
    pub fn clip(&self, index: i32) -> Option<&str> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.clips.get(i))
            .map(String::as_str)
    }
}

fn blend_base(r: &mut Reader) -> Result<Blend> {
    Ok(Blend {
        var: r.u32()?,
        a: r.f32()?,
        b: r.f32()?,
        c: r.f32()?,
        cond_var: r.u32()?,
        cond_text: r.str32(MAX_STR)?,
    })
}

fn printable_str_ahead(r: &Reader) -> bool {
    let rest = r.peek();
    if rest.len() < 4 {
        return false;
    }
    let len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
    (1..=200).contains(&len)
        && rest.len() >= 4 + len
        && rest[4..4 + len].iter().all(|&b| (32..127).contains(&b))
}

fn printable_or_empty_str_at(r: &Reader, off: usize) -> bool {
    let rest = r.peek();
    if rest.len() < off + 4 {
        return false;
    }
    let len = u32::from_le_bytes(rest[off..off + 4].try_into().unwrap()) as usize;
    len <= 200
        && rest.len() >= off + 4 + len
        && rest[off + 4..off + 4 + len]
            .iter()
            .all(|&b| (32..127).contains(&b))
}

fn anim_node(r: &mut Reader) -> Result<AnimNode> {
    let pick = Pick::parse(r)?;
    let flag_a = r.u8()?;
    let mode = r.u32()?;
    let blend = r.f32()?;
    let flag_b = r.u8()?;
    let _zero = r.u32()?;
    let tag = r.str32(MAX_STR)?;
    Ok(AnimNode {
        pick,
        flag_a,
        mode,
        blend,
        flag_b,
        tag,
        name: String::new(),
    })
}

fn flags<const N: usize>(r: &mut Reader) -> Result<[u8; N]> {
    Ok(r.bytes(N)?.try_into().unwrap())
}

fn parse_node(r: &mut Reader, kind: &str) -> Result<Node> {
    Ok(match kind {
        "AnimationNode" => Node::Animation(anim_node(r)?),
        "CinematicAnimationNode" => Node::CinematicAnimation(anim_node(r)?),
        "NamedAnimationNode" => {
            let mut a = anim_node(r)?;
            a.name = r.str32(MAX_STR)?;
            Node::NamedAnimation(a)
        }
        "AnimationRequestListenerNode" => {
            let a = anim_node(r)?;
            let request = r.u32()?;
            Node::Listener(ListenerNode {
                pick: a.pick,
                flag_a: a.flag_a,
                mode: a.mode,
                blend: a.blend,
                flag_b: a.flag_b,
                tag: a.tag,
                request,
            })
        }
        "FsmAnimNode" => {
            ensure!(
                r.u32()? == 0,
                "unexpected first word of a state machine node"
            );
            Node::Fsm { fsm: r.u32()? }
        }
        "SubTreeNode" => Node::SubTree {
            path: r.str32(MAX_STR)?,
        },
        "ParametricAnimationNode" => Node::Parametric {
            path: r.str32(MAX_STR)?,
        },
        "ModifyVariable" => {
            let n = r.u32()? as usize;
            ensure!(n <= 64, "implausible assignment count");
            let mut out = Vec::new();
            for _ in 0..n {
                let var = r.u32()?;
                let mode = r.u32()?;
                let curve = pairs(r, 64)?;
                let flag = r.u8()?;
                out.push(Assign {
                    var,
                    mode,
                    curve,
                    flag,
                });
            }
            Node::ModifyVariable(out)
        }
        "Random" => Node::Random {
            count: r.u32()?,
            flags: flags::<6>(r)?,
            blend: r.f32()?,
        },
        "Select" => {
            let var = r.u32()?;
            let count = r.u32()?;
            let f = flags::<6>(r)?;
            let blend = r.f32()?;
            ensure!(
                count <= 64 && f.iter().all(|&b| b <= 2),
                "implausible select node"
            );
            Node::Select {
                var,
                count,
                flags: f,
                blend,
            }
        }
        "Sequence" => {
            let count = r.u32()?;
            let _z = r.u32()?;
            let _one = r.u32()?;
            Node::Sequence {
                count,
                blend: r.f32()?,
            }
        }
        "MovementMode" => Node::MovementMode {
            a: r.u32()?,
            b: r.u32()?,
            flags: flags::<3>(r)?,
        },
        "RateNode" => {
            let min = r.f32()?;
            let max = r.f32()?;
            let var = r.u32()?;
            let scale = r.f32()?;
            let flag = r.u32()?;
            let _b = r.u8()?;
            Node::Rate {
                min,
                max,
                var,
                scale,
                flag,
            }
        }
        "AdditiveBlend" => Node::Additive(blend_base(r)?),
        "ProceduralBlend" => Node::Procedural(blend_base(r)?),
        "AdditiveVisemeBlend" => Node::AdditiveViseme(blend_base(r)?),
        "CinematicBlendNode" => Node::CinematicBlend(blend_base(r)?),
        "PartialBodyBlend" => {
            let blend = blend_base(r)?;
            let by_mask = r.u8()? == 1;
            let mask = r.str32(MAX_STR)?;
            let fade = if by_mask {
                (r.f32()?, r.f32()?)
            } else {
                r.skip(13)?;
                (0.0, 0.0)
            };
            Node::PartialBody {
                blend,
                by_mask,
                mask,
                fade,
            }
        }
        "PartialBodyBlendVariableMasks" => parse_masks(r)?,
        "SingleAxisBlend" => {
            let var = r.u32()?;
            r.skip(8)?;
            r.skip(2)?;
            let _neg = r.f32()?;
            r.skip(2)?;
            let n = r.u32()? as usize;
            ensure!(n <= 200, "implausible axis size");
            let names = (0..n)
                .map(|_| r.str32(MAX_STR))
                .collect::<Result<Vec<_>>>()?;
            let ne = r.u32()? as usize;
            ensure!(ne <= 200, "implausible axis size");
            let mut points = Vec::new();
            for _ in 0..ne {
                let pos = r.f32()?;
                let name = r.str32(MAX_STR)?;
                let _idx = r.u32()?;
                let w0 = r.f32()?;
                let w1 = r.f32()?;
                let partner = r.str32(MAX_STR)?;
                let partner_index = r.u32()? as i32;
                r.skip(4 + 4 + 4 + 4)?;
                points.push(AxisPoint {
                    pos,
                    name,
                    w0,
                    w1,
                    partner,
                    partner_index,
                });
            }
            Node::SingleAxis { var, names, points }
        }
        "SpeedBlendNode" => {
            ensure!(
                r.u32()? == 0 && r.u32()? == 0,
                "unexpected speed blend header"
            );
            let lo = r.f32()?;
            let hi = r.f32()?;
            let var = r.u32()?;
            let n = r.u32()? as usize;
            ensure!(n <= 64, "implausible speed blend size");
            let points = (0..n)
                .map(|_| {
                    Ok(SpeedPoint {
                        clip: r.u32()?,
                        speed: r.f32()?,
                        rate: r.f32()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Node::SpeedBlend {
                lo,
                hi,
                var,
                points,
            }
        }
        "Slice" => Node::Slice {
            from: r.f32()?,
            to: r.f32()?,
        },
        "Cutoff" => {
            let _n = r.u32()?;
            let _z = r.u32()?;
            Node::Cutoff {
                var: r.u32()?,
                value: r.u32()?,
            }
        }
        "AddProceduralTurning" => {
            let var = r.u32()?;
            let _z = r.u32()?;
            let min = r.f32()?;
            let max = r.f32()?;
            let rate = r.f32()?;
            Node::ProceduralTurning {
                var,
                min,
                max,
                rate,
                curve: pairs(r, 64)?,
            }
        }
        "AnimEventNode" => {
            let n = r.u32()? as usize;
            ensure!(n <= 200, "implausible event count");
            let mut out = Vec::new();
            for _ in 0..n {
                let t = r.f32()?;
                let s = r.str32(MAX_STR)?;
                r.skip(8)?;
                out.push((t, s));
            }
            Node::AnimEvents(out)
        }
        "DecoratorNode" => {
            let n = r.u32()? as usize;
            ensure!(n <= 64, "implausible decorator count");
            let mut entries = Vec::new();
            for _ in 0..n {
                let name = r.str32(MAX_STR)?;
                let values = (r.u32()?, r.f32()?, r.u32()?, r.f32()?);
                let mut lists = Vec::new();
                for _ in 0..2 {
                    let k = r.u32()? as usize;
                    ensure!(k <= 16, "implausible decorator condition count");
                    lists.push(
                        (0..k)
                            .map(|_| Ok((r.u32()?, r.f32()?, r.f32()?)))
                            .collect::<Result<Vec<_>>>()?,
                    );
                }
                let second = lists.pop().unwrap();
                let first = lists.pop().unwrap();
                entries.push(DecoratorEntry {
                    name,
                    values,
                    first,
                    second,
                });
            }
            r.skip(5)?;
            Node::Decorator { entries }
        }
        "ParametricDeltaNode" => {
            ensure!(r.u32()? == 0, "unexpected first word of a delta node");
            let name = r.str32(MAX_STR)?;
            let var = r.u32()?;
            let curve = pairs(r, 64)?;
            let clip = r.u32()?;
            Node::ParametricDelta {
                name,
                var,
                curve,
                clip,
            }
        }
        "AimPoses" => {
            let var = r.u32()?;
            let _var2 = r.u32()?;
            let _f = r.f32()?;
            let bone = r.str32(MAX_STR)?;
            r.skip(12)?;
            let _base = r.u32()?;
            let n = r.u32()? as usize;
            ensure!(n <= 200, "implausible pose count");
            let poses = (0..n)
                .map(|_| Ok((r.u32()?, r.f32()?, r.f32()?)))
                .collect::<Result<Vec<_>>>()?;
            Node::AimPoses { var, bone, poses }
        }
        "MotionBlendNode" | "MeleeBlendNode" => {
            let melee = kind == "MeleeBlendNode";
            let blend = blend_base(r)?;
            let _t0 = r.f32()?;
            let _t1 = r.f32()?;
            let mask_a = r.str32(MAX_STR)?;
            let mask_b = r.str32(MAX_STR)?;
            let _blend = r.f32()?;
            let name = r.str32(MAX_STR)?;
            let n = r.u32()? as usize;
            ensure!(n <= 64, "implausible id count");
            for _ in 0..n {
                r.u32()?;
            }
            let var = r.u32()?;
            let mut masks = vec![mask_a, mask_b];
            if var == 0 {
                masks.push(r.str32(MAX_STR)?);
            } else {
                ensure!(!melee, "variable masks in a melee blend node");
                while printable_str_ahead(r) {
                    masks.push(r.str32(MAX_STR)?);
                }
            }
            if melee {
                let _extra = r.str32(MAX_STR)?;
                let k = r.u32()? as usize;
                ensure!(k <= 64, "implausible id count");
                for _ in 0..k {
                    r.u32()?;
                }
            }
            Node::MotionBlend {
                blend,
                melee,
                masks,
                name,
                var,
            }
        }
        "PostureModifierFilter" => {
            let var = r.u32()?;
            r.skip(1 + 4 + 4)?;
            Node::Posture { var }
        }
        "ReachIK" => {
            r.skip(4 + 4 + 4 + 4 + 4)?;
            let bone = r.str32(MAX_STR)?;
            r.skip(16 + 4 + 4 + 1)?;
            Node::ReachIk { bone }
        }
        "FootIKEx" => {
            r.skip(16)?;
            let root = r.str32(MAX_STR)?;
            let n = r.u32()? as usize;
            ensure!(n <= 16, "implausible leg count");
            for _ in 0..n {
                r.str32(MAX_STR)?;
                r.str32(MAX_STR)?;
                r.skip(12 + 20 + 4)?;
            }
            r.skip(57)?;
            // The foot pose data (a Packages.bin entity), empty in most trees
            let pose_data = r.str32(MAX_STR)?;
            Node::FootIk {
                root,
                legs: n,
                pose_data,
            }
        }
        "LookIKEx" => {
            let bones = (0..4)
                .map(|_| r.str32(MAX_STR))
                .collect::<Result<Vec<_>>>()?;
            r.skip(32 + 16)?;
            Node::LookIk { bones }
        }
        "NamedSubTreeNode" => {
            let path = r.str32(MAX_STR)?;
            r.skip(1 + 4 + 4)?;
            Node::NamedSubTree {
                path,
                name: r.str32(MAX_STR)?,
            }
        }
        "SubTreeRequestListenerNode" => {
            let path = r.str32(MAX_STR)?;
            r.skip(1 + 4 + 4)?;
            Node::SubTreeListener {
                path,
                request: r.u32()?,
            }
        }
        other => bail!("node type {other} is not decoded"),
    })
}

fn parse_masks(r: &mut Reader) -> Result<Node> {
    let blend = blend_base(r)?;
    let var = r.u32()?;
    if var == 0 {
        let by_mask = r.u8()? == 1;
        let single_mask = r.str32(MAX_STR)?;
        let fade = if by_mask {
            [r.f32()?, r.f32()?, 0.0]
        } else {
            r.skip(13)?;
            [0.0; 3]
        };
        return Ok(Node::PartialBodyMasks {
            blend,
            var,
            masks: Vec::new(),
            timings: 0,
            fade,
            single_mask,
            single_by_mask: by_mask,
        });
    }
    let mut masks = Vec::new();
    let mut timings = 0;
    loop {
        let rest = r.peek();
        if rest.first() == Some(&1) && printable_or_empty_str_at(r, 1) {
            r.u8()?;
            masks.push(r.str32(MAX_STR)?);
        } else if rest.len() >= 10 && rest[9] == 1 && rest[4] == 0 {
            r.skip(10)?;
            timings += 1;
        } else {
            break;
        }
    }
    let _z = r.u32()?;
    let fade = [r.f32()?, r.f32()?, r.f32()?];
    Ok(Node::PartialBodyMasks {
        blend,
        var,
        masks,
        timings,
        fade,
        single_mask: String::new(),
        single_by_mask: false,
    })
}

// ---------------------------------------------------------------------------
// Movement sets and aim-pose sets

pub const KIND_MOVEMENT_SET: u16 = 14;
pub const KIND_AIM_POSE_SET: u16 = 19;

/// The numbers that follow the clip names of a movement set.
#[derive(Debug, Clone, PartialEq)]
pub struct MovementParams {
    /// 0 in the sprint sets, 1 in the aim-walk sets.
    pub a: u32,
    /// 12 in the sprint sets, 5 in the aim-walk sets.
    pub b: f32,
    /// 180 or 1080.
    pub angle: f32,
    /// 360 or 720 (degrees per second, by the size).
    pub turn: f32,
    pub c: f32,
    /// 10 or 30.
    pub d: f32,
    pub flags: [u8; 4],
}

/// A movement set (type 14: the clips of a directional locomotion, e.g. forward, back, left and
/// right) or an aim-pose set (type 19: the clip and the bone it aims). The tables in the `B.`
/// part are not read.
#[derive(Debug, Clone, PartialEq)]
pub struct MovementSet {
    pub kind: u16,
    /// The files the set was built from.
    pub sources: Vec<String>,
    /// The names in the body, in the order the set uses them (a file name, or a path).
    pub names: Vec<String>,
    /// Type 19: the bone it aims.
    pub bone: Option<String>,
    pub params: Option<MovementParams>,
}

impl MovementSet {
    pub fn parse(head: &[u8]) -> Result<Self> {
        let common = CommonHeader::parse(head).context("common header")?;
        let mut r = Reader::at(head, common.len)?;
        match common.kind {
            KIND_MOVEMENT_SET => {
                let n = r.u32()? as usize;
                ensure!(n <= 64, "implausible clip count {n}");
                let names = (0..n)
                    .map(|_| r.str32(MAX_STR))
                    .collect::<Result<Vec<_>>>()?;
                let params = MovementParams {
                    a: r.u32()?,
                    b: r.f32()?,
                    angle: r.f32()?,
                    turn: r.f32()?,
                    c: r.f32()?,
                    d: r.f32()?,
                    flags: r.bytes(4)?.try_into().unwrap(),
                };
                Ok(MovementSet {
                    kind: common.kind,
                    sources: common.sources,
                    names,
                    bone: None,
                    params: Some(params),
                })
            }
            KIND_AIM_POSE_SET => {
                let name = r.str32(MAX_STR)?;
                let bone = r.str32(MAX_STR)?;
                Ok(MovementSet {
                    kind: common.kind,
                    sources: common.sources,
                    names: vec![name],
                    bone: Some(bone),
                    params: None,
                })
            }
            k => bail!("type {k} is not a movement or aim-pose set"),
        }
    }
}

// ---------------------------------------------------------------------------
// Reading from the packages

/// `H.Misc`/`B.Misc` (state machines, movement sets) and `H.AnimRetarget`/`B.AnimRetarget`
/// (trees), opened to read one entry at a time.
pub struct GraphPackages {
    misc_head: Package,
    misc_body: Package,
    tree_head: Package,
    tree_body: Package,
}

impl GraphPackages {
    pub fn open(dir: &Path) -> Result<Self> {
        let open = |name: &str| {
            Package::open_in(dir, name)?
                .with_context(|| format!("{name}.toc is not in {}", dir.display()))
        };
        Ok(GraphPackages {
            misc_head: open("H.Misc")?,
            misc_body: open("B.Misc")?,
            tree_head: open("H.AnimRetarget")?,
            tree_body: open("B.AnimRetarget")?,
        })
    }

    /// Paths of every state machine.
    pub fn fsm_paths(&self) -> Vec<String> {
        self.misc_head
            .toc
            .entries
            .iter()
            .filter(|e| e.path.ends_with(".animfsm"))
            .map(|e| e.path.clone())
            .collect()
    }

    /// Paths of every tree.
    pub fn tree_paths(&self) -> Vec<String> {
        self.tree_head
            .toc
            .entries
            .iter()
            .filter(|e| e.path.ends_with(".animtree"))
            .map(|e| e.path.clone())
            .collect()
    }

    pub fn set_nocache(&self) {
        self.misc_head.set_nocache();
        self.misc_body.set_nocache();
        self.tree_head.set_nocache();
        self.tree_body.set_nocache();
    }

    /// `None` when the path is not in the packages.
    pub fn fsm(&mut self, path: &str) -> Result<Option<Fsm>> {
        let Some(head) = self.misc_head.read_path(path)? else {
            return Ok(None);
        };
        let body = self
            .misc_body
            .read_path(path)?
            .with_context(|| format!("{path} has no B.Misc part"))?;
        Fsm::parse(&head, &body)
            .with_context(|| path.to_string())
            .map(Some)
    }

    /// A movement set or aim-pose set; `None` when the path is not in `H.Misc`.
    pub fn movement_set(&mut self, path: &str) -> Result<Option<MovementSet>> {
        let Some(head) = self.misc_head.read_path(path)? else {
            return Ok(None);
        };
        MovementSet::parse(&head)
            .with_context(|| path.to_string())
            .map(Some)
    }

    /// `None` when the path is not in the packages.
    pub fn tree(&mut self, path: &str) -> Result<Option<Tree>> {
        let Some(head) = self.tree_head.read_path(path)? else {
            return Ok(None);
        };
        let body = self
            .tree_body
            .read_path(path)?
            .with_context(|| format!("{path} has no B.AnimRetarget part"))?;
        Tree::parse(&head, &body)
            .with_context(|| path.to_string())
            .map(Some)
    }
}

// ---------------------------------------------------------------------------
// From a suit to its locomotion

/// The controller `Player` entities use when a suit does not name one.
pub const DEFAULT_CONTROLLER: &str = "/Lotus/Types/Player/TennoAnimController";

/// What Packages.bin says about a suit's animation.
#[derive(Debug, Clone)]
pub struct SuitAnimation {
    pub suit: String,
    pub mesh: Option<String>,
    /// The suit's `AnimationSet` item and the sub-trees it replaces by name.
    pub animation_set: Option<String>,
    pub subtree_overrides: Vec<(String, String)>,
    pub controller: String,
    /// The suit has no `DefaultAnimControllerOverride`, so [`DEFAULT_CONTROLLER`] stands.
    pub controller_is_default: bool,
    /// Absolute path of the controller's `AnimTree`.
    pub anim_tree: String,
    /// (variable name, value) pairs of the controller's actions, e.g. `IV_JUMP`, `JS_JUMPING`.
    pub controller_variables: Vec<(String, String)>,
}

fn collect_variables(v: &crate::ee::Value, out: &mut Vec<(String, String)>) {
    use crate::ee::Value;
    match v {
        Value::Table(items) => {
            if let (Some(var), Some(val)) = (v.get("Variable"), v.get("Value")) {
                if let (Some(var), Some(val)) = (var.text(), val.text()) {
                    out.push((var.to_string(), val.to_string()));
                }
            }
            for (_, x) in items {
                collect_variables(x, out);
            }
        }
        Value::List(items) => items.iter().for_each(|x| collect_variables(x, out)),
        Value::Text(_) => {}
    }
}

/// Reads a suit's animation set and controller from Packages.bin.
pub fn suit_animation(types: &mut crate::level::TypeResolver, suit: &str) -> Result<SuitAnimation> {
    use crate::material::{folder, resolve_path};
    let info = types
        .resolve(suit)?
        .with_context(|| format!("{suit} is not in Packages.bin"))?;
    let text = |key: &str| -> Option<String> {
        let (v, owner) = info.props.get(key)?;
        let t = v.text()?.trim();
        (!t.is_empty()).then(|| resolve_path(folder(owner), t))
    };
    let mesh = text("Mesh");
    // The suit's customization names the set it wears by default
    let animation_set = info
        .props
        .get("DefaultCustomization")
        .and_then(|(v, _)| v.get("AnimationSet"))
        .or_else(|| info.props.get("AnimationSet").map(|(v, _)| v))
        .and_then(|v| v.get("ItemType"))
        .and_then(|v| v.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let mut subtree_overrides = Vec::new();
    if let Some(set) = &animation_set {
        if let Some(item) = types.resolve(set)? {
            if let Some((crate::ee::Value::Table(items), owner)) =
                item.props.get("AnimationSubTrees")
            {
                for (k, v) in items {
                    if let Some(t) = v.text() {
                        subtree_overrides.push((k.clone(), resolve_path(folder(owner), t.trim())));
                    }
                }
            }
        }
    }
    let over = text("DefaultAnimControllerOverride");
    let controller_is_default = over.is_none();
    let controller = over.unwrap_or_else(|| DEFAULT_CONTROLLER.to_string());
    let ctl = types
        .resolve(&controller)?
        .with_context(|| format!("controller {controller} is not in Packages.bin"))?;
    let (tree, owner) = ctl
        .props
        .get("AnimTree")
        .with_context(|| format!("controller {controller} has no AnimTree"))?;
    let anim_tree = resolve_path(
        folder(owner),
        tree.text().context("AnimTree is not a path")?.trim(),
    );
    let mut controller_variables = Vec::new();
    if let Some((actions, _)) = ctl.props.get("Actions") {
        collect_variables(actions, &mut controller_variables);
    }
    Ok(SuitAnimation {
        suit: suit.to_string(),
        mesh,
        animation_set,
        subtree_overrides,
        controller,
        controller_is_default,
        anim_tree,
        controller_variables,
    })
}

/// What the data says about one variable number.
#[derive(Debug, Clone, Default)]
pub struct VarInfo {
    /// Enum names conditions compare it with, and their values.
    pub values: BTreeMap<String, f32>,
    /// A name written in an expression a condition kept (`Turn Angle`, `Action`).
    pub expression_name: Option<String>,
    /// The controller variable (`IV_JUMP`) whose values are among `values`.
    pub controller_name: Option<String>,
}

impl VarInfo {
    /// A short label: the controller name, else the expression name, else empty.
    pub fn label(&self) -> &str {
        self.controller_name
            .as_deref()
            .or(self.expression_name.as_deref())
            .unwrap_or("")
    }
}

fn is_number(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && t.parse::<f32>().is_ok()
}

/// The name in the fragment of an expression a condition kept: `18 || Action == 19` comes from
/// `Action == 18 || Action == 19`, so the pieces after the first connector start with the name.
fn expression_name(text: &str) -> Option<String> {
    let mut pieces = vec![text];
    for sep in [" || ", " and ", " or "] {
        pieces = pieces.into_iter().flat_map(|p| p.split(sep)).collect();
    }
    for piece in pieces.iter().skip(1) {
        let cut = ["==", "!=", "<=", ">=", "<", ">"]
            .iter()
            .filter_map(|op| piece.find(op))
            .min()?;
        let name = piece[..cut].trim();
        if !name.is_empty() && name.chars().any(|c| c.is_alphabetic()) && name.len() <= 40 {
            return Some(name.to_string());
        }
    }
    None
}

/// Collects, for every variable number the state machines use, the texts they compare it with,
/// and tries to name it: by an expression fragment, or by a controller variable whose values
/// appear among its enum names.
pub fn variable_hints<'a>(
    fsms: impl IntoIterator<Item = &'a Fsm>,
    controller_variables: &[(String, String)],
) -> BTreeMap<u32, VarInfo> {
    let mut out: BTreeMap<u32, VarInfo> = BTreeMap::new();
    for fsm in fsms {
        for state in &fsm.states {
            let conds = state
                .conditions
                .iter()
                .chain(&state.more_conditions)
                .chain(state.transitions.iter().flat_map(|t| &t.conditions));
            for c in conds {
                let info = out.entry(c.var).or_default();
                if c.text.is_empty() {
                    continue;
                }
                if is_number(&c.text) {
                    continue;
                }
                if let Some(n) = expression_name(&c.text) {
                    info.expression_name.get_or_insert(n);
                } else if !c.text.contains(' ') {
                    info.values.entry(c.text.clone()).or_insert(c.value);
                }
            }
        }
    }
    // A controller variable names a number when exactly one number has its values
    let mut by_iv: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (var, val) in controller_variables {
        if !val.is_empty() && !is_number(val) {
            by_iv.entry(var).or_default().insert(val);
        }
    }
    for (iv, vals) in by_iv {
        let matches: Vec<u32> = out
            .iter()
            .filter(|(_, info)| vals.iter().any(|v| info.values.contains_key(*v)))
            .map(|(k, _)| *k)
            .collect();
        if let [only] = matches[..] {
            out.get_mut(&only).unwrap().controller_name = Some(iv.to_string());
        }
    }
    out
}

impl Tree {
    /// The number of values of each variable, from the animation tables that have only that
    /// variable (a table has one entry per value).
    pub fn cardinalities(&self) -> BTreeMap<u32, usize> {
        let mut out = BTreeMap::new();
        for n in &self.nodes {
            if let Node::Animation(a) | Node::NamedAnimation(a) | Node::CinematicAnimation(a) = n {
                if let [v] = a.pick.vars[..] {
                    out.entry(v).or_insert(a.pick.table.len());
                }
            }
            if let Node::Listener(l) = n {
                if let [v] = l.pick.vars[..] {
                    out.entry(v).or_insert(l.pick.table.len());
                }
            }
        }
        out
    }

    /// For every node, the nodes that list it as a child.
    pub fn parents(&self) -> Vec<Vec<usize>> {
        let mut out = vec![Vec::new(); self.nodes.len()];
        for (i, ch) in self.graph.edges.iter().enumerate() {
            for &c in ch {
                if let Some(p) = usize::try_from(c).ok().and_then(|c| out.get_mut(c)) {
                    if !p.contains(&i) {
                        p.push(i);
                    }
                }
            }
        }
        out
    }

    /// The node nothing points to.
    pub fn root(&self) -> Option<usize> {
        let parents = self.parents();
        (0..self.nodes.len()).find(|&i| parents[i].is_empty())
    }

    /// The first state machine node at or under `start`, depth first, whose machine satisfies `want`.
    pub fn find_fsm(
        &self,
        start: usize,
        fsms: &[Option<Fsm>],
        want: &dyn Fn(&Fsm) -> bool,
    ) -> Option<usize> {
        let mut seen = vec![false; self.nodes.len()];
        let mut stack = vec![start];
        while let Some(n) = stack.pop() {
            if std::mem::replace(&mut seen[n], true) {
                continue;
            }
            if let Node::Fsm { fsm } = &self.nodes[n] {
                if let Some(Some(f)) = fsms.get(*fsm as usize) {
                    if want(f) {
                        return Some(n);
                    }
                }
            }
            for &c in self.graph.edges[n].iter().rev() {
                if let Ok(c) = usize::try_from(c) {
                    stack.push(c);
                }
            }
        }
        None
    }
}

/// How the path to a leaf passes a node: which slot of which node is taken, and the variable
/// value that takes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub node: usize,
    pub kind: String,
    /// The variable the node switches on, when it switches on one.
    pub var: Option<u32>,
    /// Child slot taken; for a `Select` it is the value of `var`.
    pub slot: usize,
}

/// A clip a leaf node plays.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipUse {
    pub clip: String,
    /// Speed (m/s) at which a speed blend plays the clip at its own rate.
    pub speed: Option<f32>,
    /// Playback rate multiplier a speed blend gives it.
    pub rate: Option<f32>,
    /// Variable values that choose it in an animation table.
    pub when: Vec<(u32, usize)>,
}

/// A node that plays clips, and the first path found to it from a start node.
#[derive(Debug, Clone, PartialEq)]
pub struct Leaf {
    pub node: usize,
    pub kind: String,
    pub path: Vec<Selection>,
    /// Seconds the node blends in, for animation nodes.
    pub blend: Option<f32>,
    pub clips: Vec<ClipUse>,
}

fn matches_folder(path: &str, folder_name: &str) -> bool {
    let (dir, _) = path.rsplit_once('/').unwrap_or(("", path));
    dir.split('/').any(|s| s.eq_ignore_ascii_case(folder_name))
}

impl Tree {
    /// Every node under `start` that plays clips, each once, with the way to it. With `stance`
    /// (a folder name such as `Sword` or `Unarmed`), only leaves with a clip in a folder of that
    /// name, and only those clips.
    pub fn leaves(&self, start: usize, stance: Option<&str>) -> Vec<Leaf> {
        let cards = self.cardinalities();
        let mut seen = vec![false; self.nodes.len()];
        let mut out = Vec::new();
        let mut path = Vec::new();
        self.collect_leaves(start, stance, &cards, &mut seen, &mut path, &mut out);
        out
    }

    fn collect_leaves(
        &self,
        node: usize,
        stance: Option<&str>,
        cards: &BTreeMap<u32, usize>,
        seen: &mut Vec<bool>,
        path: &mut Vec<Selection>,
        out: &mut Vec<Leaf>,
    ) {
        if std::mem::replace(&mut seen[node], true) {
            return;
        }
        let keep = |p: &str| stance.is_none_or(|s| matches_folder(p, s));
        let mut clips = Vec::new();
        let mut blend = None;
        match &self.nodes[node] {
            Node::Animation(a) | Node::NamedAnimation(a) | Node::CinematicAnimation(a) => {
                blend = Some(a.blend);
                for (i, &c) in a.pick.table.iter().enumerate() {
                    let Some(p) = self.clip(c) else { continue };
                    if keep(p) {
                        let when = a.pick.values_of(i, cards).map_or_else(Vec::new, |vals| {
                            a.pick.vars.iter().copied().zip(vals).collect()
                        });
                        clips.push(ClipUse {
                            clip: p.to_string(),
                            speed: None,
                            rate: None,
                            when,
                        });
                    }
                }
            }
            Node::SpeedBlend { points, .. } => {
                for p in points {
                    if let Some(c) = self.clips.get(p.clip as usize).filter(|c| keep(c)) {
                        clips.push(ClipUse {
                            clip: c.clone(),
                            speed: Some(p.speed),
                            rate: Some(p.rate),
                            when: Vec::new(),
                        });
                    }
                }
            }
            Node::Parametric { path } if keep(path) => clips.push(ClipUse {
                clip: path.clone(),
                speed: None,
                rate: None,
                when: Vec::new(),
            }),
            _ => {}
        }
        if !clips.is_empty() {
            out.push(Leaf {
                node,
                kind: self.graph.type_name(node).to_string(),
                path: path.clone(),
                blend,
                clips,
            });
        }
        let var = match &self.nodes[node] {
            Node::Select { var, .. } | Node::SingleAxis { var, .. } => Some(*var),
            _ => None,
        };
        for (slot, &c) in self.graph.edges[node].iter().enumerate() {
            let Ok(c) = usize::try_from(c) else { continue };
            path.push(Selection {
                node,
                kind: self.graph.type_name(node).to_string(),
                var,
                slot,
            });
            self.collect_leaves(c, stance, cards, seen, path, out);
            path.pop();
        }
    }
}

/// The weight of a bone in a blend mask.
#[derive(Debug, Clone, PartialEq)]
pub struct BoneWeight {
    pub bone: String,
    pub weight: f32,
    pub additive: bool,
}

/// Reads a mask entity (`Masks={{Name=GAME_C1_SPINE1 Weight=0.75 Additive=0}...}`) from Packages.bin.
pub fn bone_mask(
    types: &mut crate::level::TypeResolver,
    path: &str,
) -> Result<Option<Vec<BoneWeight>>> {
    let Some(info) = types.resolve(path)? else {
        return Ok(None);
    };
    let Some((masks, _)) = info.props.get("Masks") else {
        return Ok(None);
    };
    let mut out = Vec::new();
    for m in masks.items() {
        let Some(bone) = m.get("Name").and_then(|v| v.text()) else {
            continue;
        };
        out.push(BoneWeight {
            bone: bone.to_string(),
            weight: m.get("Weight").and_then(|v| v.float()).unwrap_or(0.0),
            additive: m
                .get("Additive")
                .and_then(|v| v.text())
                .is_some_and(|t| t.trim() == "1"),
        });
    }
    Ok(Some(out))
}

/// Where a tree's locomotion state machine is.
#[derive(Debug, Clone, Copy)]
pub struct Locomotion {
    /// The state machine at the root of the tree (`FsmRoot`).
    pub root_fsm: usize,
    /// Its child that plays the `Locomotion` state.
    pub locomotion_child: usize,
    /// The locomotion state machine node (the first under that child with an `Idle` state).
    pub fsm_node: usize,
}

/// Finds the locomotion state machine: the root machine's `Locomotion` child leads, through
/// blends, to the machine whose exposed states include `Idle`.
pub fn find_locomotion(tree: &Tree, fsms: &[Option<Fsm>]) -> Option<Locomotion> {
    let root = tree.root()?;
    let root_fsm = tree.find_fsm(root, fsms, &|f| f.exposed_index("Locomotion").is_some())?;
    let Node::Fsm { fsm } = &tree.nodes[root_fsm] else {
        return None;
    };
    let f = fsms.get(*fsm as usize)?.as_ref()?;
    let slot = f.exposed_index("Locomotion")?;
    let child = usize::try_from(*tree.graph.edges[root_fsm].get(slot)?).ok()?;
    let fsm_node = tree.find_fsm(child, fsms, &|f| f.exposed_index("Idle").is_some())?;
    Some(Locomotion {
        root_fsm,
        locomotion_child: child,
        fsm_node,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::PackagesBin;

    fn u32le(out: &mut Vec<u8>, v: u32) {
        out.extend(v.to_le_bytes());
    }

    fn f32le(out: &mut Vec<u8>, v: f32) {
        out.extend(v.to_le_bytes());
    }

    fn str32(out: &mut Vec<u8>, s: &str) {
        u32le(out, s.len() as u32);
        out.extend(s.as_bytes());
    }

    /// An `H.` entry: common header, graph header, then the node bodies.
    fn head(kind: u32, types: &[&str], node_types: &[u8], edge_count: u32, body: &[u8]) -> Vec<u8> {
        let mut h = crate::header::tests::build(&["/Lotus/x"], "", kind);
        h.extend(SIGNATURE);
        h.push(57);
        str32(&mut h, "");
        u32le(&mut h, node_types.len() as u32);
        u32le(&mut h, edge_count);
        u32le(&mut h, types.len() as u32);
        for t in types {
            str32(&mut h, t);
        }
        u32le(&mut h, node_types.len() as u32);
        h.extend(node_types);
        h.extend(body);
        h
    }

    /// A `B.` entry for the children of each node.
    fn edges(lists: &[&[i32]]) -> Vec<u8> {
        let mut b = Vec::new();
        u32le(&mut b, 0);
        let mut start = 0;
        for (i, l) in lists.iter().enumerate() {
            if i > 0 {
                u32le(&mut b, start);
            }
            u32le(&mut b, l.len() as u32);
            start += l.len() as u32;
        }
        for l in lists {
            for &c in *l {
                b.extend(c.to_le_bytes());
            }
        }
        b
    }

    fn full_cond(out: &mut Vec<u8>, var: u32, a: u32, op: u32, text: &str, value: f32) {
        u32le(out, var);
        u32le(out, a);
        u32le(out, op);
        str32(out, text);
        f32le(out, value);
    }

    fn short_cond(out: &mut Vec<u8>, var: u32, text: &str, value: f32, int: i32) {
        u32le(out, var);
        str32(out, text);
        f32le(out, value);
        out.extend(int.to_le_bytes());
    }

    /// State A: one full condition, a transition with a full condition, a last transition
    /// with a short one. State B: no transitions, so its list is short too.
    fn fsm_bytes() -> (Vec<u8>, Vec<u8>) {
        let mut body = Vec::new();
        // state A
        str32(&mut body, "A");
        u32le(&mut body, 0); // child
        str32(&mut body, "move");
        u32le(&mut body, 2); // transitions
        u32le(&mut body, 1);
        full_cond(&mut body, 108, 0, 1, "JOG", 1.0);
        f32le(&mut body, 0.3);
        u32le(&mut body, 1);
        u32le(&mut body, 1);
        full_cond(&mut body, 126, 0, 2, "0.05", 0.05);
        f32le(&mut body, 0.2);
        u32le(&mut body, 0);
        u32le(&mut body, 1);
        short_cond(&mut body, 77, "JS_JUMPING", 2.0, 2);
        u32le(&mut body, 0); // more conditions
        for (name, v) in [("Start", 0), ("", 0), ("Stop", 1.0f32.to_bits()), ("", 0)] {
            str32(&mut body, name);
            u32le(&mut body, v);
        }
        u32le(&mut body, 0); // variables
                             // state B
        str32(&mut body, "");
        body.extend((-1i32).to_le_bytes());
        str32(&mut body, "");
        u32le(&mut body, 0);
        u32le(&mut body, 1);
        short_cond(&mut body, 0, "", 0.0, -1);
        u32le(&mut body, 0);
        for _ in 0..4 {
            str32(&mut body, "");
            u32le(&mut body, 0);
        }
        u32le(&mut body, 2);
        u32le(&mut body, 7);
        u32le(&mut body, 9);
        // exposed states, start, errors
        u32le(&mut body, 1);
        str32(&mut body, "A");
        body.push(1);
        u32le(&mut body, 0);
        u32le(&mut body, 1);
        str32(&mut body, "Anim [Error]: x");
        let h = head(
            KIND_ANIMFSM as u32,
            &["/EE/Types/AnimationTreeNodes/AnimationStateNode"],
            &[0, 0],
            2,
            &body,
        );
        // Edge list of A: the last transition's target first, then the first transition's
        (h, edges(&[&[-1, 1], &[]]))
    }

    #[test]
    fn state_machine() {
        let (h, b) = fsm_bytes();
        let f = Fsm::parse(&h, &b).unwrap();
        assert_eq!(f.states.len(), 2);
        let a = &f.states[0];
        assert_eq!((a.name.as_str(), a.child, a.tag.as_str()), ("A", 0, "move"));
        assert_eq!(f.child_name(a), Some("A"));
        assert_eq!(a.conditions.len(), 1);
        assert_eq!(a.conditions[0].op_symbol(), "!=");
        assert_eq!(a.conditions[0].text, "JOG");
        let (t0, t1) = (&a.transitions[0], &a.transitions[1]);
        assert_eq!((t0.blend, t0.mode, t0.target, t0.last), (0.3, 1, 1, false));
        assert_eq!(t0.conditions[0].op_symbol(), "<");
        // The last transition's target is the first edge
        assert_eq!((t1.blend, t1.target, t1.last), (0.2, -1, true));
        assert_eq!(t1.conditions[0].int_value, Some(2));
        assert_eq!(t1.conditions[0].op, None);
        assert_eq!(a.events[0].name, "Start");
        assert_eq!(f32::from_bits(a.events[2].value), 1.0);
        // A state without transitions has a short first list
        let b = &f.states[1];
        assert_eq!(b.conditions[0].int_value, Some(-1));
        assert_eq!(b.variables, [7, 9]);
        assert_eq!(
            f.exposed,
            [Exposed {
                name: "A".into(),
                flag: 1
            }]
        );
        assert_eq!(f.errors, ["Anim [Error]: x"]);
    }

    #[test]
    fn state_machine_rejects_damage() {
        let (h, b) = fsm_bytes();
        // Edge list of the wrong length for state A
        assert!(Fsm::parse(&h, &edges(&[&[-1], &[]])).is_err());
        // Truncated
        assert!(Fsm::parse(&h[..h.len() - 3], &b).is_err());
        // Another type
        let mut wrong = h.clone();
        let at = wrong
            .windows(4)
            .position(|w| w == 383u32.to_le_bytes())
            .unwrap();
        wrong[at..at + 4].copy_from_slice(&388u32.to_le_bytes());
        assert!(Fsm::parse(&wrong, &b).is_err());
    }

    fn tree_bytes() -> (Vec<u8>, Vec<u8>) {
        let types = [
            "/EE/Types/AnimationTreeNodes/Select",
            "/EE/Types/AnimationTreeNodes/AnimationNode",
            "/EE/Types/AnimationTreeNodes/SpeedBlendNode",
            "/EE/Types/AnimationTreeNodes/FsmAnimNode",
        ];
        let mut body = Vec::new();
        // 0: Select on v158, slots for values 0..2
        u32le(&mut body, 158);
        u32le(&mut body, 2);
        body.extend([0, 1, 1, 0, 0, 0]);
        f32le(&mut body, 0.3);
        // 1: an animation node picking clip 1 by v100 (values 0 and 1 -> clips 1 and 0)
        u32le(&mut body, 1);
        u32le(&mut body, 100);
        u32le(&mut body, 2);
        u32le(&mut body, 1);
        u32le(&mut body, 0);
        body.push(0);
        u32le(&mut body, 2);
        f32le(&mut body, 0.25);
        body.push(1);
        u32le(&mut body, 0);
        str32(&mut body, "");
        // 2: speed blend over v126 with two clips
        u32le(&mut body, 0);
        u32le(&mut body, 0);
        f32le(&mut body, 10.0);
        f32le(&mut body, 10.0);
        u32le(&mut body, 126);
        u32le(&mut body, 2);
        for (clip, speed, rate) in [(0u32, 0.0f32, 0.5f32), (1, 4.2, 1.2)] {
            u32le(&mut body, clip);
            f32le(&mut body, speed);
            f32le(&mut body, rate);
        }
        // 3: runs state machine 0
        u32le(&mut body, 0);
        u32le(&mut body, 0);
        // pools
        for pool in [
            &[
                "/Lotus/Animations/Tenno/Movement/Unarmed/Idle_anim.fbx",
                "/Lotus/Animations/Tenno/Movement/Unarmed/JogForward_anim.fbx",
            ][..],
            &["/Lotus/Types/X.animfsm"][..],
            &[][..],
            &[][..],
        ] {
            u32le(&mut body, pool.len() as u32);
            for p in pool {
                str32(&mut body, p);
            }
        }
        let h = head(KIND_ANIMTREE as u32, &types, &[0, 1, 2, 3], 4, &body);
        (h, edges(&[&[1, -1, 2], &[], &[], &[0]]))
    }

    #[test]
    fn tree_nodes_and_pools() {
        let (h, b) = tree_bytes();
        let t = Tree::parse(&h, &b).unwrap();
        assert_eq!(t.nodes.len(), 4);
        assert_eq!(t.graph.type_name(2), "SpeedBlendNode");
        let Node::Select {
            var, flags, blend, ..
        } = &t.nodes[0]
        else {
            panic!()
        };
        assert_eq!((*var, flags[1], *blend), (158, 1, 0.3));
        assert_eq!(t.children(0), [1, -1, 2]);
        let Node::Animation(a) = &t.nodes[1] else {
            panic!()
        };
        assert_eq!(a.pick.vars, [100]);
        assert_eq!(a.pick.table, [1, 0]);
        assert_eq!((a.mode, a.blend, a.flag_b), (2, 0.25, 1));
        assert_eq!(
            t.clip(a.pick.table[0]),
            Some("/Lotus/Animations/Tenno/Movement/Unarmed/JogForward_anim.fbx")
        );
        let Node::SpeedBlend { var, points, .. } = &t.nodes[2] else {
            panic!()
        };
        assert_eq!(*var, 126);
        assert_eq!(
            points[1],
            SpeedPoint {
                clip: 1,
                speed: 4.2,
                rate: 1.2
            }
        );
        assert_eq!(t.nodes[2].clip_refs(), [0, 1]);
        assert_eq!(t.nodes[3], Node::Fsm { fsm: 0 });
        assert_eq!((t.clips.len(), t.fsms.len(), t.subtrees.len()), (2, 1, 0));
        // Only node 3 (which lists node 0 as its child) has no parent
        assert_eq!(t.root(), Some(3));
        assert_eq!(t.parents()[2], [0]);
        assert_eq!(t.parents()[0], [3]);
    }

    #[test]
    fn tree_with_a_type_nobody_decodes() {
        let (mut h, b) = tree_bytes();
        // Rename the Select type to one this module does not know, keeping the length
        let at = h.windows(6).position(|w| w == b"Select").unwrap();
        h[at..at + 6].copy_from_slice(b"Sel3ct");
        let e = format!("{:#}", Tree::parse(&h, &b).unwrap_err());
        assert!(e.contains("Sel3ct is not decoded"), "{e}");
        // Bytes left over after the pools
        let (mut h, b) = tree_bytes();
        h.push(0);
        assert!(Tree::parse(&h, &b).is_err());
    }

    fn fsm_with(exposed: &[&str], conds: &[Cond]) -> Fsm {
        let state = State {
            name: String::new(),
            child: -1,
            tag: String::new(),
            conditions: conds.to_vec(),
            transitions: Vec::new(),
            more_conditions: Vec::new(),
            events: std::array::from_fn(|_| StateEvent {
                name: String::new(),
                value: 0,
            }),
            variables: Vec::new(),
        };
        Fsm {
            parent: String::new(),
            states: vec![state],
            exposed: exposed
                .iter()
                .map(|n| Exposed {
                    name: n.to_string(),
                    flag: 0,
                })
                .collect(),
            start: 0,
            errors: Vec::new(),
        }
    }

    fn cond(var: u32, text: &str, value: f32) -> Cond {
        Cond {
            var,
            op: Some(0),
            a: Some(0),
            text: text.into(),
            value,
            int_value: None,
        }
    }

    #[test]
    fn names_from_expressions_and_controller() {
        assert_eq!(
            expression_name("18 || Action == 19").as_deref(),
            Some("Action")
        );
        assert_eq!(
            expression_name("10 || Turn Angle < -10").as_deref(),
            Some("Turn Angle")
        );
        assert_eq!(
            expression_name("0.05 and Delta > 0.38 and Delta < 0.45").as_deref(),
            Some("Delta")
        );
        assert_eq!(expression_name("JOG"), None);
        assert_eq!(expression_name("0.05"), None);
        let f = fsm_with(
            &["Idle"],
            &[
                cond(77, "JS_JUMPING", 2.0),
                cond(77, "JS_FALLING", 3.0),
                cond(2, "18 || Action == 19", 18.0),
                cond(108, "JOG", 1.0),
                cond(126, "0.05", 0.05),
            ],
        );
        let ctl = vec![
            ("IV_JUMP".to_string(), "JS_JUMPING".to_string()),
            ("IV_ACTION".to_string(), "4".to_string()),
            ("IV_MOVE".to_string(), "NOT_USED".to_string()),
        ];
        let h = variable_hints([&f], &ctl);
        assert_eq!(h[&77].label(), "IV_JUMP");
        assert_eq!(h[&77].values["JS_FALLING"], 3.0);
        assert_eq!(h[&2].label(), "Action");
        assert_eq!(h[&108].label(), "");
        assert_eq!(h[&108].values["JOG"], 1.0);
        // Numbers are not enum names
        assert!(h[&126].values.is_empty());
        assert_eq!(f.exposed_index("Idle"), Some(0));
    }

    #[test]
    fn suit_animation_from_packages_bin() {
        let bin = crate::packages::tests::build(&[
            (
                "/Lotus/Types/Player/",
                "TennoAnimController",
                "",
                Some("AnimTree=Animation/Tree.animtree\nActions={\n{\nAction=Jump\nSetToBegin={\n{\nVariable=IV_JUMP\nValue=JS_JUMPING\n}\n}\n}\n}\n"),
            ),
            (
                "/Lotus/Powersuits/X/",
                "XSuit",
                "",
                Some("Mesh=X_skel.fbx\nDefaultCustomization={\nChest={\nMAIN_HAND={\n{\nScale=1\n}\n}\n}\nAnimationSet={\nItemType=/Lotus/Upgrades/XAnims\n}\n}\n"),
            ),
            (
                "/Lotus/Powersuits/X/",
                "YSuit",
                "/Lotus/Powersuits/X/XSuit",
                Some("DefaultAnimControllerOverride=/Lotus/Types/Player/Other\n"),
            ),
            (
                "/Lotus/Types/Player/",
                "Other",
                "",
                Some("AnimTree=/Lotus/Elsewhere/Other.animtree\n"),
            ),
            (
                "/Lotus/Upgrades/",
                "XAnims",
                "",
                Some("AnimationSubTrees={\nPRN_IDLE_OVERRIDE=/Lotus/Types/XIdle.animtree\n}\n"),
            ),
        ]);
        let bin = PackagesBin::parse(&bin).unwrap();
        let mut types = crate::level::TypeResolver::new(&bin).unwrap();
        let s = suit_animation(&mut types, "/Lotus/Powersuits/X/XSuit").unwrap();
        assert_eq!(s.mesh.as_deref(), Some("/Lotus/Powersuits/X/X_skel.fbx"));
        assert_eq!(s.animation_set.as_deref(), Some("/Lotus/Upgrades/XAnims"));
        assert_eq!(
            s.subtree_overrides,
            [(
                "PRN_IDLE_OVERRIDE".to_string(),
                "/Lotus/Types/XIdle.animtree".to_string()
            )]
        );
        assert!(s.controller_is_default);
        assert_eq!(s.controller, DEFAULT_CONTROLLER);
        assert_eq!(s.anim_tree, "/Lotus/Types/Player/Animation/Tree.animtree");
        assert_eq!(
            s.controller_variables,
            [("IV_JUMP".to_string(), "JS_JUMPING".to_string())]
        );
        // A child suit inherits the set and replaces the controller
        let y = suit_animation(&mut types, "/Lotus/Powersuits/X/YSuit").unwrap();
        assert!(!y.controller_is_default);
        assert_eq!(y.anim_tree, "/Lotus/Elsewhere/Other.animtree");
        assert_eq!(y.animation_set, s.animation_set);
        assert!(suit_animation(&mut types, "/Lotus/Nope").is_err());
    }

    #[test]
    fn locomotion_is_found_under_the_roots_locomotion_child() {
        use crate::animtree::Node::*;
        let mk = |exposed: &[&str]| Some(fsm_with(exposed, &[]));
        let fsms = vec![
            mk(&["Injury", "Locomotion"]),
            mk(&["Idle", "Moving"]),
            mk(&["Idle"]),
        ];
        // 0 root machine; 1 Injury child; 2 Locomotion child (a blend); 3 locomotion machine
        let nodes = vec![
            Fsm { fsm: 0 },
            Slice { from: 0.0, to: 1.0 },
            Slice { from: 0.0, to: 1.0 },
            Fsm { fsm: 1 },
        ];
        let edges = vec![vec![1, 2], vec![], vec![3], vec![-1, -1]];
        let common = CommonHeader {
            hash: [0; 16],
            sources: Vec::new(),
            attributes: String::new(),
            kind: KIND_ANIMTREE,
            kind_high: 0,
            len: 0,
        };
        let tree = Tree {
            graph: Graph {
                common,
                parent: String::new(),
                types: vec!["x".into()],
                node_types: vec![0; 4],
                edges,
                body_start: 0,
            },
            nodes,
            clips: Vec::new(),
            fsms: Vec::new(),
            subtrees: Vec::new(),
            errors: Vec::new(),
        };
        let l = find_locomotion(&tree, &fsms).unwrap();
        assert_eq!((l.root_fsm, l.locomotion_child, l.fsm_node), (0, 2, 3));
        assert_eq!(tree.root(), Some(0));
    }
}
