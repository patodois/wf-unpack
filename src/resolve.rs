//! From a `uniqueName` (the `/Lotus/...` name of a game entity: a Warframe, a weapon, an enemy) to the
//! model asset ids the Warframe provider lists.
//!
//! A uniqueName is **not** an asset id. It names an entity of `/Packages.bin`; an asset id is the path of a
//! file (`.fbx` model, `.level`). The link between them is data in the entity's property text: a line
//! `Mesh=<path>` or `PickUpMesh=<path>` at the top level of the text of the entity or of one it derives from
//! (see [`MODEL_KEYS`]). This module reads those lines along the entity's lineage and checks each path
//! against what the provider lists, so the answer is either the ids a caller can hand to
//! [`Warframe::load`](rewrite_bridge::Provider::load), or the reason there are none.
//!
//! What it does **not** do: pick the character out of several models, assemble a Warframe from its body,
//! helmet and weapons, or read keys other than [`MODEL_KEYS`]. The answer lists every model the lineage
//! names, nearest entity first, with the entity that set each one, and leaves the choice to the caller.

use anyhow::Result;
use rewrite_bridge::{AssetInfo, AssetKind};

use crate::bridge::{split_animated_id, Warframe};
use crate::packages::PackagesBin;

/// The property keys, at the top level of an entity's text, whose value is the entity's own model path.
pub const MODEL_KEYS: [&str; 2] = ["Mesh", "PickUpMesh"];
/// Keys (at any depth) whose value may be a `.level` path.
pub const LEVEL_KEYS: [&str; 3] = ["BaseLevel", "Diorama", "Prefab"];

/// One path found in the property text of the lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub key: String,
    pub path: String,
    /// The entity whose text has the line
    pub entity: String,
    /// 0 for the entity asked about, 1 for its parent, and so on
    pub generation: usize,
}

/// A model path found, and what the provider says about it.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelHit {
    pub mention: Mention,
    /// `None`: the path is not a model the provider lists (not in `H.Misc`, or not a static or skinned model)
    pub asset: Option<AssetInfo>,
}

/// What a uniqueName reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The name is already an id the provider lists (with or without a `?animations=` suffix).
    AlreadyAnAssetId,
    /// Not an entity of Packages.bin.
    NotAnEntity,
    /// An entity, but no `Mesh` or `PickUpMesh` line in it or in its parents.
    NoModelProperty,
    /// Model paths are named, but none is a model the provider lists.
    ModelsNotListed,
    /// At least one listed model.
    Models,
}

#[derive(Debug, Clone)]
pub struct Resolution {
    pub unique_name: String,
    pub outcome: Outcome,
    /// The entity and the entities it derives from, nearest first
    pub lineage: Vec<String>,
    /// An ancestor the lineage came back to (a loop in the data), if any
    pub loops_at: Option<String>,
    pub models: Vec<ModelHit>,
    /// `.level` paths named by `BaseLevel`, `Diorama` or `Prefab`, with whether the provider lists them
    pub levels: Vec<(Mention, bool)>,
    /// Set when the name itself is a listed asset
    pub asset: Option<AssetInfo>,
}

impl Resolution {
    /// The ids the caller can pass to the provider: listed models, nearest entity first, each once.
    pub fn asset_ids(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        if let Some(a) = &self.asset {
            out.push(&a.id);
        }
        for hit in &self.models {
            if let Some(a) = &hit.asset {
                if !out.contains(&a.id.as_str()) {
                    out.push(&a.id);
                }
            }
        }
        out
    }

    /// Why there is no model, or what the answer is, in one sentence.
    pub fn explain(&self) -> String {
        match self.outcome {
            Outcome::AlreadyAnAssetId => format!("{} is already an asset id the provider lists", self.unique_name),
            Outcome::NotAnEntity => format!(
                "{} does not reach a model: it is not an entity of Packages.bin (language tags, node ids and \
                 server-side names are not)",
                self.unique_name
            ),
            Outcome::NoModelProperty => format!(
                "{} does not reach a model: it is an entity, but neither it nor the {} entit{} it derives from \
                 has a {} line",
                self.unique_name,
                self.lineage.len().saturating_sub(1),
                if self.lineage.len() == 2 { "y" } else { "ies" },
                MODEL_KEYS.join(" or ")
            ),
            Outcome::ModelsNotListed => format!(
                "{} does not reach a model the provider lists: its lineage names {} model path(s), and none is a \
                 static or skinned model of H.Misc",
                self.unique_name,
                self.models.len()
            ),
            Outcome::Models => format!(
                "{} reaches {} model(s)",
                self.unique_name,
                self.models.iter().filter(|m| m.asset.is_some()).count()
            ),
        }
    }
}

/// The `Key=value` lines of an entity's property text: `(depth, key, value)`, where depth counts the
/// unclosed `{` before the line.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str, &str)> {
    let mut depth = 0usize;
    text.lines().filter_map(move |line| {
        let line = line.trim();
        let at = depth;
        let opens = line.matches('{').count();
        let closes = line.matches('}').count();
        depth = (depth + opens).saturating_sub(closes);
        let (key, value) = line.split_once('=')?;
        // A key is a bare word; a line like `{0,0}=...` or a text with `=` inside a list is not one
        (!key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b':'))
        .then_some(())?;
        Some((at, key, value.trim()))
    })
}

/// The model and level paths named along a lineage. `chain` is `(entity, property text)`, nearest first.
/// Each path is reported once, at the nearest entity that names it.
pub fn mentions(chain: &[(String, String)]) -> (Vec<Mention>, Vec<Mention>) {
    let (mut models, mut levels): (Vec<Mention>, Vec<Mention>) = (Vec::new(), Vec::new());
    for (generation, (entity, text)) in chain.iter().enumerate() {
        for (depth, key, value) in lines(text) {
            let mention = |v: &str| Mention {
                key: key.to_string(),
                path: v.to_string(),
                entity: entity.clone(),
                generation,
            };
            if depth == 0 && MODEL_KEYS.contains(&key) && value.ends_with(".fbx") {
                if !models.iter().any(|m| m.path == value) {
                    models.push(mention(value));
                }
            } else if LEVEL_KEYS.contains(&key)
                && value.ends_with(".level")
                && !levels.iter().any(|m| m.path == value)
            {
                levels.push(mention(value));
            }
        }
    }
    (models, levels)
}

/// Resolves `unique_name` against Packages.bin and the provider.
pub fn resolve(wf: &Warframe, bin: &PackagesBin, unique_name: &str) -> Result<Resolution> {
    let mut out = Resolution {
        unique_name: unique_name.to_string(),
        outcome: Outcome::NotAnEntity,
        lineage: Vec::new(),
        loops_at: None,
        models: Vec::new(),
        levels: Vec::new(),
        asset: None,
    };
    // A path the provider lists is an id, whatever the entities say (a name may also carry the clip filter
    // of an animated id)
    let (bare, _) = split_animated_id(unique_name);
    if let Some(info) = wf.asset_info(bare)? {
        out.asset = Some(info);
        out.outcome = Outcome::AlreadyAnAssetId;
        return Ok(out);
    }
    if !bin.contains(unique_name) {
        return Ok(out);
    }
    let (lineage, loops_at) = bin.lineage(unique_name);
    let mut texts = bin.texts()?;
    let mut chain = Vec::with_capacity(lineage.len());
    for entity in &lineage {
        chain.push((entity.clone(), texts.text(entity)?.unwrap_or_default()));
    }
    let (models, levels) = mentions(&chain);
    for mention in models {
        let asset = wf.asset_info(&mention.path)?;
        out.models.push(ModelHit { mention, asset });
    }
    for mention in levels {
        let listed = wf
            .asset_info(&mention.path)?
            .is_some_and(|a| a.kind == AssetKind::Level);
        out.levels.push((mention, listed));
    }
    out.lineage = lineage;
    out.loops_at = loops_at;
    out.outcome = outcome_of(&out.models);
    Ok(out)
}

/// The outcome for an entity, from the model paths its lineage names.
fn outcome_of(models: &[ModelHit]) -> Outcome {
    if models.is_empty() {
        Outcome::NoModelProperty
    } else if models.iter().all(|m| m.asset.is_none()) {
        Outcome::ModelsNotListed
    } else {
        Outcome::Models
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(parts: &[(&str, &str)]) -> Vec<(String, String)> {
        parts
            .iter()
            .map(|(e, t)| (e.to_string(), t.to_string()))
            .collect()
    }

    #[test]
    fn a_model_line_is_found_on_the_entity_that_sets_it() {
        let c = chain(&[
            ("/Lotus/Powersuits/X/X", "Name=a\n"),
            ("/Lotus/Powersuits/X/XBase", "Diorama=/Lotus/Levels/Episodes/X.level\nMesh=/Lotus/Characters/Tenno/X/XBody_skel.fbx\n"),
            ("/Lotus/Types/Game/PowerSuit", "Mesh=/Lotus/Characters/Tenno/Default_skel.fbx\n"),
        ]);
        let (models, levels) = mentions(&c);
        assert_eq!(models.len(), 2);
        assert_eq!(
            models[0],
            Mention {
                key: "Mesh".into(),
                path: "/Lotus/Characters/Tenno/X/XBody_skel.fbx".into(),
                entity: "/Lotus/Powersuits/X/XBase".into(),
                generation: 1,
            }
        );
        assert_eq!(models[1].generation, 2);
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].key, "Diorama");
    }

    #[test]
    fn only_top_level_keys_and_only_fbx_values_are_models() {
        let text = "\
Behaviors={
{
Mesh=/Lotus/Nested/Inside.fbx
}
}
Resource=/Lotus/Animations/Clip_anim.fbx
PickUpMesh=/Lotus/Weapons/W/W.fbx
Mesh=/Lotus/Textures/NotAModel.png
";
        let (models, _) = mentions(&chain(&[("/E", text)]));
        let paths: Vec<_> = models
            .iter()
            .map(|m| (m.key.as_str(), m.path.as_str()))
            .collect();
        // The nested Mesh is a sub-object's, Resource is not a model key, and a .png is not a model
        assert_eq!(paths, [("PickUpMesh", "/Lotus/Weapons/W/W.fbx")]);
    }

    #[test]
    fn braces_on_one_line_do_not_change_the_depth() {
        let text = "Pos={1,2,3}\nMesh=/Lotus/A.fbx\nList={\nA=1\n}\nMesh=/Lotus/B.fbx\n";
        let (models, _) = mentions(&chain(&[("/E", text)]));
        assert_eq!(models.len(), 2, "{models:?}");
    }

    #[test]
    fn a_path_named_twice_is_reported_at_the_nearest_entity() {
        let c = chain(&[("/A", "Mesh=/Lotus/M.fbx\n"), ("/B", "Mesh=/Lotus/M.fbx\n")]);
        let (models, _) = mentions(&c);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].entity, "/A");
    }

    fn hit(path: &str, listed: bool) -> ModelHit {
        ModelHit {
            mention: Mention {
                key: "Mesh".into(),
                path: path.into(),
                entity: "/E".into(),
                generation: 0,
            },
            asset: listed.then(|| AssetInfo {
                id: path.into(),
                name: "n".into(),
                kind: AssetKind::Prop,
            }),
        }
    }

    #[test]
    fn the_outcome_tells_no_property_from_unlisted_paths_from_models() {
        assert_eq!(outcome_of(&[]), Outcome::NoModelProperty);
        assert_eq!(
            outcome_of(&[hit("/a.fbx", false), hit("/b.fbx", false)]),
            Outcome::ModelsNotListed
        );
        assert_eq!(
            outcome_of(&[hit("/a.fbx", false), hit("/b.fbx", true)]),
            Outcome::Models
        );
    }

    #[test]
    fn the_explanations_say_why() {
        let mut r = Resolution {
            unique_name: "/Lotus/Language/X".into(),
            outcome: Outcome::NotAnEntity,
            lineage: vec![],
            loops_at: None,
            models: vec![],
            levels: vec![],
            asset: None,
        };
        assert!(r.explain().contains("not an entity of Packages.bin"));
        r.outcome = Outcome::NoModelProperty;
        r.lineage = vec!["/E".into(), "/P".into()];
        assert!(r
            .explain()
            .contains("neither it nor the 1 entity it derives from has a Mesh or PickUpMesh line"));
        r.models = vec![
            hit("/a.fbx", true),
            hit("/a.fbx", true),
            hit("/b.fbx", false),
        ];
        r.outcome = Outcome::Models;
        // asset_ids lists each listed id once, nearest first
        assert_eq!(r.asset_ids(), ["/a.fbx"]);
        assert!(r.explain().contains("reaches 2 model(s)"));
    }

    #[test]
    fn levels_are_found_at_any_depth() {
        let text = "BaseLevels={\n{\nBaseLevel=/Lotus/Levels/A.level\nLikelihood=1\n},\n{\nPrefab=/Lotus/Levels/B.level\n}\n}\n";
        let (models, levels) = mentions(&chain(&[("/E", text)]));
        assert!(models.is_empty());
        let paths: Vec<_> = levels.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, ["/Lotus/Levels/A.level", "/Lotus/Levels/B.level"]);
    }
}
