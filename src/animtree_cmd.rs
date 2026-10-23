//! `animtree` and `check-animtrees`: what Excalibur's animation controller does.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};

use wf_unpack::animation::AnimationPackages;
use wf_unpack::animtree::{
    find_locomotion, suit_animation, variable_hints, Fsm, GraphPackages, Node, Tree, VarInfo,
};
use wf_unpack::header::CommonHeader;
use wf_unpack::level::TypeResolver;
use wf_unpack::material::{folder, resolve_path};
use wf_unpack::packages::PackagesBin;

/// Parses every state machine and tree and summarises what does not decode.
pub fn check_cmd(game: &Path) -> Result<()> {
    let mut pk = GraphPackages::open(game)?;
    pk.set_nocache();
    let fsms = pk.fsm_paths();
    let trees = pk.tree_paths();
    let mut fsm_ok = 0;
    let mut fsm_fail: Vec<(String, String)> = Vec::new();
    let (mut states, mut transitions) = (0usize, 0usize);
    for p in &fsms {
        match pk.fsm(p) {
            Ok(Some(f)) => {
                fsm_ok += 1;
                states += f.states.len();
                transitions += f.states.iter().map(|s| s.transitions.len()).sum::<usize>();
            }
            Ok(None) => {}
            Err(e) => fsm_fail.push((p.clone(), format!("{e:#}"))),
        }
    }
    let mut tree_ok = 0;
    let mut nodes = 0usize;
    let mut reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in &trees {
        match pk.tree(p) {
            Ok(Some(t)) => {
                tree_ok += 1;
                nodes += t.nodes.len();
            }
            Ok(None) => {}
            Err(e) => {
                let msg = format!("{e:#}");
                let key = msg
                    .split(": ")
                    .find(|s| s.contains("is not decoded"))
                    .map(str::to_string)
                    .unwrap_or_else(|| msg.clone());
                reasons.entry(key).or_default().push(p.clone());
            }
        }
    }
    println!(
        "state machines: {fsm_ok} of {} decoded ({states} states, {transitions} transitions)",
        fsms.len()
    );
    for (p, e) in fsm_fail.iter().take(10) {
        println!("  {p}: {e}");
    }
    println!(
        "trees: {tree_ok} of {} decoded ({nodes} nodes)",
        trees.len()
    );
    let mut rs: Vec<_> = reasons.into_iter().collect();
    rs.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (r, v) in rs.iter().take(15) {
        println!("  {:4}  {r}  (e.g. {})", v.len(), v[0]);
    }
    Ok(())
}

/// A clip path without the folder every clip shares and the `_anim.fbx` ending.
fn short(path: &str) -> &str {
    let p = path.strip_prefix("/Lotus/Animations/").unwrap_or(path);
    p.strip_suffix("_anim.fbx").unwrap_or(p)
}

fn matches_stance(path: &str, stance: &str) -> bool {
    let (folder, _) = path.rsplit_once('/').unwrap_or(("", path));
    folder.split('/').any(|s| s.eq_ignore_ascii_case(stance))
}

struct Ctx<'a> {
    tree: &'a Tree,
    fsms: &'a [Option<Fsm>],
    hints: &'a BTreeMap<u32, VarInfo>,
    stance: Option<&'a str>,
    max_depth: usize,
    /// Per node: does it, or something under it, play a clip of the stance?
    keep: HashMap<usize, bool>,
}

impl Ctx<'_> {
    fn var(&self, v: u32) -> String {
        match self.hints.get(&v).map(VarInfo::label) {
            Some(l) if !l.is_empty() => format!("v{v}({l})"),
            _ => format!("v{v}"),
        }
    }

    /// Name of an enum value of a variable, if a condition used one.
    fn value_name(&self, v: u32, value: i64) -> Option<&str> {
        let info = self.hints.get(&v)?;
        info.values
            .iter()
            .find(|(_, &x)| x as i64 == value)
            .map(|(n, _)| n.as_str())
    }

    fn value_label(&self, v: u32, value: i64) -> String {
        match self.value_name(v, value) {
            Some(n) => format!("v{v}={value} {n}"),
            None => format!("v{v}={value}"),
        }
    }

    fn clip_paths(&self, node: usize) -> Vec<&str> {
        let t = self.tree;
        let mut out: Vec<&str> = t.nodes[node]
            .clip_refs()
            .into_iter()
            .filter_map(|c| t.clips.get(c as usize).map(String::as_str))
            .collect();
        if let Node::Parametric { path } = &t.nodes[node] {
            out.push(path);
        }
        out
    }

    fn keeps(&mut self, node: usize) -> bool {
        let Some(stance) = self.stance else {
            return true;
        };
        if let Some(&k) = self.keep.get(&node) {
            return k;
        }
        // Mark before descending: the graph is acyclic, but shared nodes are visited often
        self.keep.insert(node, false);
        let mut k = self
            .clip_paths(node)
            .iter()
            .any(|p| matches_stance(p, stance));
        let kids: Vec<usize> = self.tree.graph.edges[node]
            .iter()
            .filter_map(|&c| usize::try_from(c).ok())
            .collect();
        for c in kids {
            if self.keeps(c) {
                k = true;
            }
        }
        self.keep.insert(node, k);
        k
    }

    fn summary(&self, node: usize) -> String {
        let t = self.tree;
        match &t.nodes[node] {
            Node::Animation(a) | Node::NamedAnimation(a) | Node::CinematicAnimation(a) => {
                let kind = t.graph.type_name(node);
                let vars: Vec<String> = a.pick.vars.iter().map(|&v| self.var(v)).collect();
                let clips = a.pick.clips();
                let mut s = format!(
                    "{kind} blend {:.2}s{}",
                    a.blend,
                    if a.tag.is_empty() {
                        String::new()
                    } else {
                        format!(" tag {:?}", a.tag)
                    }
                );
                if let Node::NamedAnimation(_) = &t.nodes[node] {
                    s += &format!(" name {:?}", a.name);
                }
                if vars.is_empty() {
                    if let Some(c) = a.pick.table.first().and_then(|&c| t.clip(c)) {
                        s += &format!(": {}", short(c));
                    }
                } else {
                    s += &format!(
                        " by {} ({} entries, {} clips)",
                        vars.join(" x "),
                        a.pick.table.len(),
                        clips.len()
                    );
                }
                s
            }
            Node::Listener(l) => format!(
                "Listener request {} blend {:.2}s ({} clips)",
                l.request,
                l.blend,
                l.pick.clips().len()
            ),
            Node::Fsm { fsm } => {
                let name = t
                    .fsms
                    .get(*fsm as usize)
                    .map_or("?", |p| p.rsplit('/').next().unwrap());
                let ex = self
                    .fsms
                    .get(*fsm as usize)
                    .and_then(|f| f.as_ref())
                    .map(|f| {
                        f.exposed
                            .iter()
                            .map(|e| e.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                format!("State machine {name} [{ex}]")
            }
            Node::SubTree { path } => format!("SubTree {path}"),
            Node::NamedSubTree { path, name } => format!("NamedSubTree {name:?} -> {path:?}"),
            Node::SubTreeListener { path, request } => {
                format!("SubTreeListener request {request} -> {path}")
            }
            Node::Parametric { path } => format!("Set {path}"),
            Node::ModifyVariable(a) => format!(
                "ModifyVariable {}",
                a.iter()
                    .map(|x| format!("{} mode {} curve {:?}", self.var(x.var), x.mode, x.curve))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Node::Random { count, blend, .. } => format!("Random of {count}, blend {blend:.2}s"),
            Node::Select { var, blend, .. } => format!(
                "Select {} ({} slots), blend {blend:.2}s",
                self.var(*var),
                self.tree.graph.edges[node].len()
            ),
            Node::Sequence { count, blend } => format!("Sequence of {count}, blend {blend:.2}s"),
            Node::MovementMode { a, b, .. } => format!("MovementMode {a} {b}"),
            Node::Rate {
                min,
                max,
                var,
                scale,
                ..
            } => {
                format!("Rate {min:.3}..{max:.3} by {} x{scale}", self.var(*var))
            }
            Node::Additive(b) => format!("Additive weight {}", self.blend(b)),
            Node::Procedural(b) => format!("Procedural weight {}", self.blend(b)),
            Node::AdditiveViseme(b) => format!("AdditiveViseme weight {}", self.blend(b)),
            Node::CinematicBlend(b) => format!("CinematicBlend weight {}", self.blend(b)),
            Node::PartialBody {
                blend,
                by_mask,
                mask,
                fade,
            } => format!(
                "PartialBody {} {} weight {} fade {:.2}/{:.2}",
                if *by_mask { "mask" } else { "from bone" },
                mask,
                self.blend(blend),
                fade.0,
                fade.1
            ),
            Node::PartialBodyMasks {
                blend,
                var,
                masks,
                single_mask,
                ..
            } => {
                if *var == 0 {
                    format!(
                        "PartialBody mask {single_mask} weight {}",
                        self.blend(blend)
                    )
                } else {
                    format!(
                        "PartialBody mask by {} ({} masks) weight {}",
                        self.var(*var),
                        masks.len(),
                        self.blend(blend)
                    )
                }
            }
            Node::SingleAxis { var, points, .. } => format!(
                "SingleAxis {} [{}]",
                self.var(*var),
                points
                    .iter()
                    .map(|p| format!("{} {}", p.pos, p.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Node::SpeedBlend {
                lo,
                hi,
                var,
                points,
            } => format!(
                "SpeedBlend {} ({lo},{hi}) [{}]",
                self.var(*var),
                points
                    .iter()
                    .map(|p| {
                        format!(
                            "{} @{:.2} x{:.3}",
                            t.clips.get(p.clip as usize).map_or("?", |c| short(c)),
                            p.speed,
                            p.rate
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Node::Slice { from, to } => format!("Slice {from:.2}..{to:.2}"),
            Node::Cutoff { var, value } => format!("Cutoff {} {value}", self.var(*var)),
            Node::ProceduralTurning {
                var,
                min,
                max,
                rate,
                ..
            } => format!(
                "ProceduralTurning {} {min}..{max} deg at {rate} deg/s",
                self.var(*var)
            ),
            Node::AnimEvents(e) => format!(
                "Events {}",
                e.iter()
                    .map(|(t, n)| format!("{n}@{t:.2}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Node::Decorator { entries } => format!(
                "Decorator {}",
                entries
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Node::ParametricDelta {
                name, var, clip, ..
            } => format!(
                "Delta {} {name:?} pose {}",
                self.var(*var),
                t.clips.get(*clip as usize).map_or("?", |c| short(c))
            ),
            Node::AimPoses { bone, poses, .. } => {
                format!("AimPoses {bone} ({} poses)", poses.len())
            }
            Node::MotionBlend {
                melee,
                name,
                var,
                masks,
                ..
            } => format!(
                "{} {name:?} masks {} by {}",
                if *melee { "MeleeBlend" } else { "MotionBlend" },
                masks.len(),
                if *var == 0 {
                    "-".into()
                } else {
                    self.var(*var)
                }
            ),
            Node::Posture { var } => format!("PostureFilter {}", self.var(*var)),
            Node::ReachIk { bone } => format!("ReachIK {bone}"),
            Node::FootIk { legs, .. } => format!("FootIK {legs} legs"),
            Node::LookIk { bones } => format!("LookIK {}", bones.join(",")),
        }
    }

    fn blend(&self, b: &wf_unpack::animtree::Blend) -> String {
        let mut s = if b.var == 0 {
            "const".to_string()
        } else {
            self.var(b.var)
        };
        if b.cond_var != 0 {
            s += &format!(" when {} == {:?}", self.var(b.cond_var), b.cond_text);
        }
        s
    }

    /// The label of a child slot of a node.
    fn slot(&self, node: usize, i: usize) -> String {
        let t = self.tree;
        match &t.nodes[node] {
            Node::Select { var, .. } => self.value_label(*var, i as i64),
            Node::Fsm { fsm } => self
                .fsms
                .get(*fsm as usize)
                .and_then(|f| f.as_ref())
                .and_then(|f| f.exposed.get(i))
                .map_or(format!("state {i}"), |e| format!("state {}", e.name)),
            Node::SingleAxis { points, .. } => points
                .get(i)
                .map_or(format!("{i}"), |p| format!("{} {}", p.pos, p.name)),
            Node::PartialBody { .. } | Node::PartialBodyMasks { .. } => {
                if i == 0 {
                    "base".into()
                } else {
                    "layer".into()
                }
            }
            Node::Additive(_) => {
                if i == 0 {
                    "base".into()
                } else {
                    "additive".into()
                }
            }
            _ => format!("{i}"),
        }
    }

    /// Lines for the clip table of a leaf: with a stance only the cells of that stance.
    fn clip_cells(&self, node: usize, out: &mut Vec<String>) {
        let t = self.tree;
        let (Node::Animation(a) | Node::NamedAnimation(a) | Node::CinematicAnimation(a)) =
            &t.nodes[node]
        else {
            return;
        };
        let cols = match a.pick.vars.len() {
            2 => a.pick.table.len() / a.pick.vars.len().max(1),
            _ => 0,
        };
        let _ = cols;
        for (i, &c) in a.pick.table.iter().enumerate() {
            let Some(path) = t.clip(c) else { continue };
            if let Some(st) = self.stance {
                if !matches_stance(path, st) {
                    continue;
                }
            }
            let label = match a.pick.vars.len() {
                0 => String::new(),
                1 => self.value_label(a.pick.vars[0], i as i64),
                _ => {
                    let rows = a.pick.table.len() / (a.pick.table.len() / 20).max(1);
                    let _ = rows;
                    format!(
                        "{} x {} cell {i}",
                        self.var(a.pick.vars[0]),
                        self.var(a.pick.vars[1])
                    )
                }
            };
            out.push(format!(
                "{label}{}{}",
                if label.is_empty() { "" } else { " -> " },
                short(path)
            ));
        }
    }

    fn print(&mut self, node: usize, depth: usize, label: &str, shown: &mut HashSet<usize>) {
        let pad = "  ".repeat(depth + 1);
        let again = !shown.insert(node);
        let lead = if label.is_empty() {
            String::new()
        } else {
            format!("{label}: ")
        };
        println!(
            "{pad}{lead}[{node}] {}{}",
            self.summary(node),
            if again { " (shown above)" } else { "" }
        );
        if again {
            return;
        }
        let mut cells = Vec::new();
        self.clip_cells(node, &mut cells);
        for c in cells.iter().take(24) {
            println!("{pad}    {c}");
        }
        if cells.len() > 24 {
            println!("{pad}    ... {} more", cells.len() - 24);
        }
        if let Some(st) = self.stance {
            // Leaves with clips of other stances are skipped
            let _ = st;
        }
        if depth >= self.max_depth {
            if !self.tree.graph.edges[node].is_empty() {
                println!("{pad}  ...");
            }
            return;
        }
        let edges = self.tree.graph.edges[node].clone();
        let mut skipped = 0;
        for (i, &c) in edges.iter().enumerate() {
            let Ok(c) = usize::try_from(c) else { continue };
            if !self.keeps(c) {
                skipped += 1;
                continue;
            }
            let l = self.slot(node, i);
            self.print(c, depth + 1, &l, shown);
        }
        if let (true, Some(st)) = (skipped > 0, self.stance) {
            println!("{pad}  ({skipped} other children play no {st} clips)");
        }
    }
}

fn describe_cond(ctx: &Ctx, c: &wf_unpack::animtree::Cond) -> String {
    let value = if c.text.is_empty() {
        format!("{}", c.value)
    } else if c.text.parse::<f32>().is_ok() {
        c.text.clone()
    } else {
        format!("{}({})", c.text, c.value)
    };
    if c.var == 0 {
        // No variable: a trigger (word a is 0, the operator says which), or a built-in
        // quantity (a = 1, 2, 4, 5) compared with the text
        return match c.a {
            Some(0) | None => format!("trigger {}", c.op_symbol()),
            Some(a) => format!("builtin{a} {} {value}", c.op_symbol()),
        };
    }
    format!("{} {} {value}", ctx.var(c.var), c.op_symbol())
}

pub fn animtree_cmd(
    game: &Path,
    suit: &str,
    stance: Option<&str>,
    max_depth: Option<usize>,
) -> Result<()> {
    let bin = PackagesBin::load(game)?;
    let mut types = TypeResolver::new(&bin)?;
    let sa = suit_animation(&mut types, suit)?;
    println!("suit          {}", sa.suit);
    if let Some(m) = &sa.mesh {
        println!("mesh          {m}");
    }
    match &sa.animation_set {
        Some(set) => {
            println!("animation set {set}");
            for (k, v) in &sa.subtree_overrides {
                println!("                {k} -> {v}");
            }
        }
        None => println!("animation set (none)"),
    }
    println!(
        "controller    {}{}",
        sa.controller,
        if sa.controller_is_default {
            " (the suit names none: the default of player entities)"
        } else {
            ""
        }
    );
    println!("anim tree     {}", sa.anim_tree);

    let mut pk = GraphPackages::open(game)?;
    let tree = pk
        .tree(&sa.anim_tree)?
        .with_context(|| format!("{} is not in H.AnimRetarget", sa.anim_tree))?;
    println!(
        "              {} nodes, {} clips, {} state machines, {} sub-trees, parent graph {:?}",
        tree.nodes.len(),
        tree.clips.len(),
        tree.fsms.len(),
        tree.subtrees.len(),
        tree.graph.parent
    );
    let tree_folder = folder(&sa.anim_tree).to_string();
    let mut fsms: Vec<Option<Fsm>> = Vec::new();
    for p in &tree.fsms {
        fsms.push(pk.fsm(&resolve_path(&tree_folder, p))?);
    }
    let hints = variable_hints(fsms.iter().flatten(), &sa.controller_variables);

    let mut ctx = Ctx {
        tree: &tree,
        fsms: &fsms,
        hints: &hints,
        stance,
        max_depth: max_depth.unwrap_or(if stance.is_some() { 64 } else { 3 }),
        keep: HashMap::new(),
    };

    let Some(loco) = find_locomotion(&tree, &fsms) else {
        println!("no locomotion state machine found");
        return Ok(());
    };
    let Node::Fsm { fsm: root_idx } = &tree.nodes[loco.root_fsm] else {
        unreachable!()
    };
    let root = fsms[*root_idx as usize].as_ref().unwrap();
    println!(
        "\nroot machine  {} (node {}): exposed {}",
        tree.fsms[*root_idx as usize].rsplit('/').next().unwrap(),
        loco.root_fsm,
        root.exposed
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let Node::Fsm { fsm: loco_idx } = &tree.nodes[loco.fsm_node] else {
        unreachable!()
    };
    let lf = fsms[*loco_idx as usize].as_ref().unwrap();
    println!(
        "locomotion    {} (node {}), starts in state #{}",
        tree.fsms[*loco_idx as usize].rsplit('/').next().unwrap(),
        loco.fsm_node,
        lf.start
    );
    print_machine(&ctx, lf);

    println!("\nvariables the machine and the tree use (numbers; names are in code)");
    let mut used: Vec<u32> = hints.keys().copied().collect();
    for n in &tree.nodes {
        collect_vars(n, &mut used);
    }
    used.sort_unstable();
    used.dedup();
    for v in used {
        let info = hints.get(&v);
        let mut line = format!("  v{v:<4}");
        if let Some(i) = info {
            if !i.label().is_empty() {
                line += &format!(" {}", i.label());
            }
            let mut vals: Vec<_> = i.values.iter().collect();
            vals.sort_by(|a, b| a.1.partial_cmp(b.1).unwrap().then(a.0.cmp(b.0)));
            if !vals.is_empty() {
                line += &format!(
                    "  {}",
                    vals.iter()
                        .map(|(n, x)| format!("{n}={x}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
        }
        println!("{line}");
    }

    // What plays in each state
    let edges = &tree.graph.edges[loco.fsm_node];
    println!(
        "\nwhat plays in each state{}",
        stance
            .map(|s| format!(" (clips of the {s} folder only)"))
            .unwrap_or_default()
    );
    for (i, e) in lf.exposed.iter().enumerate() {
        let child = edges.get(i).copied().unwrap_or(-1);
        println!(
            "\n{} {}{}",
            e.name,
            if e.flag == 1 { "(loops) " } else { "" },
            if child < 0 { "plays no child" } else { "" }
        );
        if let Ok(c) = usize::try_from(child) {
            let mut shown = HashSet::new();
            if ctx.keeps(c) {
                ctx.print(c, 0, "", &mut shown);
            } else {
                println!("  (no clips of the {} folder)", stance.unwrap_or(""));
            }
        }
    }

    // Do the clips exist, and do the ones that are clips decode?
    let mut anim = AnimationPackages::open(game)?;
    let mut refs: Vec<String> = tree.clips.clone();
    for n in &tree.nodes {
        if let Node::Parametric { path } = n {
            refs.push(path.clone());
        }
    }
    refs.sort();
    refs.dedup();
    let wanted: HashSet<&str> = refs.iter().map(String::as_str).collect();
    let found: HashMap<String, wf_unpack::toc::Entry> = anim
        .entries()
        .iter()
        .filter(|e| wanted.contains(e.path.as_str()))
        .map(|e| (e.path.clone(), e.clone()))
        .collect();
    let missing: Vec<&String> = refs.iter().filter(|p| !found.contains_key(*p)).collect();
    println!(
        "\nassets named by the tree: {}; found in H.Misc: {}; not found: {}",
        refs.len(),
        found.len(),
        missing.len()
    );
    for m in missing.iter().take(10) {
        println!("  missing {m}");
    }
    let mut kinds: BTreeMap<u16, usize> = BTreeMap::new();
    let (mut decoded, mut failed) = (0usize, Vec::new());
    for p in &refs {
        let Some(e) = found.get(p) else { continue };
        let raw = anim.read_header(e)?;
        let Ok(h) = CommonHeader::parse(&raw) else {
            continue;
        };
        *kinds.entry(h.kind).or_default() += 1;
        if h.kind == wf_unpack::animation::KIND_ANIMATION {
            match anim.decode_with(p, &raw) {
                Ok(_) => decoded += 1,
                Err(err) => failed.push(format!("{p}: {err:#}")),
            }
        }
    }
    println!(
        "  by asset type: {} (14 movement sets, 19 aim-pose sets, 132 clips)",
        kinds
            .iter()
            .map(|(k, n)| format!("type {k}: {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  clips (type 132) decoded: {decoded}, not decoded: {}",
        failed.len()
    );
    for f in failed.iter().take(5) {
        println!("    {f}");
    }
    if !tree.errors.is_empty() {
        println!("  the tree holds {} compile errors", tree.errors.len());
    }
    Ok(())
}

fn collect_vars(n: &Node, out: &mut Vec<u32>) {
    match n {
        Node::Animation(a) | Node::NamedAnimation(a) | Node::CinematicAnimation(a) => {
            out.extend(&a.pick.vars)
        }
        Node::Listener(l) => out.extend(&l.pick.vars),
        Node::Select { var, .. } | Node::SingleAxis { var, .. } | Node::SpeedBlend { var, .. } => {
            out.push(*var)
        }
        Node::Rate { var, .. } => out.push(*var),
        Node::Additive(b)
        | Node::Procedural(b)
        | Node::AdditiveViseme(b)
        | Node::CinematicBlend(b) => out.extend([b.var, b.cond_var]),
        Node::PartialBody { blend, .. } => out.extend([blend.var, blend.cond_var]),
        Node::PartialBodyMasks { blend, var, .. } => out.extend([blend.var, blend.cond_var, *var]),
        _ => {}
    }
}

fn print_machine(ctx: &Ctx, f: &Fsm) {
    println!(
        "exposed states: {}",
        f.exposed
            .iter()
            .enumerate()
            .map(|(i, e)| format!("{i}:{}{}", e.name, if e.flag == 1 { "*" } else { "" }))
            .collect::<Vec<_>>()
            .join(" ")
    );
    for (i, s) in f.states.iter().enumerate() {
        let name = if s.name.is_empty() {
            "(unnamed)"
        } else {
            &s.name
        };
        let plays = f
            .child_name(s)
            .map(|n| format!(" plays {n}"))
            .unwrap_or_default();
        println!(
            "\n  #{i} {name}{plays}{}",
            if s.tag.is_empty() {
                String::new()
            } else {
                format!(" [{}]", s.tag)
            }
        );
        if !s.conditions.is_empty() {
            println!(
                "      conditions: {}",
                s.conditions
                    .iter()
                    .map(|c| describe_cond(ctx, c))
                    .collect::<Vec<_>>()
                    .join(" and ")
            );
        }
        for (j, t) in s.transitions.iter().enumerate() {
            let target = match usize::try_from(t.target).ok().and_then(|x| f.states.get(x)) {
                Some(ts) => format!(
                    "#{} {}",
                    t.target,
                    if ts.name.is_empty() {
                        "(unnamed)"
                    } else {
                        &ts.name
                    }
                ),
                None => "none".into(),
            };
            let when = if t.conditions.is_empty() {
                "no condition".to_string()
            } else {
                t.conditions
                    .iter()
                    .map(|c| describe_cond(ctx, c))
                    .collect::<Vec<_>>()
                    .join(" and ")
            };
            println!(
                "      T{j}{} -> {target}  blend {:.2}s  mode {}  when {when}",
                if t.last { " (last)" } else { "" },
                t.blend,
                t.mode
            );
        }
        if !s.more_conditions.is_empty() {
            println!(
                "      more conditions: {}",
                s.more_conditions
                    .iter()
                    .map(|c| describe_cond(ctx, c))
                    .collect::<Vec<_>>()
                    .join(" and ")
            );
        }
        let ev: Vec<String> = s
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.name.is_empty() || e.value != 0)
            .map(|(k, e)| format!("slot{k} {:?}={}", e.name, f32::from_bits(e.value)))
            .collect();
        if !ev.is_empty() {
            println!("      events: {}", ev.join(", "));
        }
    }
}
