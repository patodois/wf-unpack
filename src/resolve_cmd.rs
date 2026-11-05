//! `wf-unpack resolve <uniqueName>`: which model asset ids a `/Lotus/...` name reaches.

use std::path::Path;

use anyhow::Result;
use rewrite_bridge::AssetKind;

use wf_unpack::bridge::{Warframe, ANIMATIONS_SUFFIX};
use wf_unpack::build::Strictness;
use wf_unpack::packages::PackagesBin;
use wf_unpack::resolve::{resolve, Outcome};

/// How many level paths are listed before the rest is only counted.
const LEVELS_SHOWN: usize = 10;

/// Exit status when the name reaches no model: 0 is a model or an id, 1 an error, 2 a usage error.
pub const NO_MODEL: i32 = 3;

pub fn resolve_cmd(game: &Path, name: &str, strict: bool) -> Result<i32> {
    let strictness = if strict {
        Strictness::Strict
    } else {
        Strictness::Lenient
    };
    let wf = Warframe::open_with(game, strictness)?;
    let bin = PackagesBin::load_with(game, strictness)?;
    let r = resolve(&wf, &bin, name)?;
    println!("{}", r.explain());
    if !r.lineage.is_empty() {
        let short: Vec<&str> = r
            .lineage
            .iter()
            .map(|e| e.rsplit('/').next().unwrap_or(e))
            .collect();
        println!("lineage: {}", short.join(" <- "));
        if let Some(again) = &r.loops_at {
            println!("  (the parent chain comes back to {again})");
        }
    }
    if let Some(a) = &r.asset {
        println!("  kind {:?}", a.kind);
    }
    for hit in &r.models {
        let m = &hit.mention;
        println!("{}={}", m.key, m.path);
        println!(
            "    set by {} ({})",
            m.entity,
            if m.generation == 0 {
                "the entity itself".to_string()
            } else {
                format!("{} up the lineage", m.generation)
            }
        );
        match &hit.asset {
            Some(a) => println!("    asset id: {}   (listed as {:?})", a.id, a.kind),
            None => {
                println!("    not an asset id: H.Misc has no static or skinned model at that path")
            }
        }
    }
    let ids = r.asset_ids();
    if !ids.is_empty() {
        println!("asset ids (provider \"Warframe\"):");
        for id in &ids {
            println!("  {id}");
        }
        if r.models
            .iter()
            .any(|m| matches!(m.asset.as_ref().map(|a| a.kind), Some(AssetKind::Character)))
        {
            println!("  A skinned model plays no clip unless its id ends {ANIMATIONS_SUFFIX}<filter>; `wf-unpack anim <id>` lists the clips that fit.");
        }
    }
    if r.models.iter().filter(|m| m.asset.is_some()).count() > 1 {
        println!(
            "note: several models are named, nearest entity first. The first is not always the one the entity is drawn \
             as (a sub-object's mesh can come before the character's); compare the kinds and check the model."
        );
    }
    if r.models.iter().any(|m| {
        matches!(m.asset.as_ref().map(|a| a.kind), Some(AssetKind::Prop))
            && m.mention.path.starts_with("/Lotus/Characters/")
    }) {
        println!(
            "note: a model under /Lotus/Characters/ listed as a Prop is a part of a character (a body, helmet, armour), \
             not a whole character; this command does not assemble the parts."
        );
    }
    if !r.levels.is_empty() {
        println!("levels named ({}):", r.levels.len());
        for (m, listed) in r.levels.iter().take(LEVELS_SHOWN) {
            println!(
                "  {}={}   ({})",
                m.key,
                m.path,
                if *listed {
                    "a listed level"
                } else {
                    "not a level the provider lists"
                }
            );
        }
        if r.levels.len() > LEVELS_SHOWN {
            println!("  and {} more", r.levels.len() - LEVELS_SHOWN);
        }
        println!("  Some are pieces a mission level is assembled from at run time; only a listed level loads as a scene.");
    }
    Ok(match r.outcome {
        Outcome::Models | Outcome::AlreadyAnAssetId => 0,
        _ => NO_MODEL,
    })
}
