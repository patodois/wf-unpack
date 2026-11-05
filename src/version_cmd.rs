//! `wf-unpack version`: which game build an install is on, and whether this reader supports it.

use std::path::Path;

use anyhow::Result;

use wf_unpack::build::{
    unix_date, GameBuild, Strictness, Support, PACKAGES_MAX_VERIFIED, STEAM_APP_ID, VERIFIED,
};
use wf_unpack::{packages, toc};

/// Prints the report and returns the exit status: 0 when the build is supported (and, with `strict`,
/// passes the strict check), 1 when it is not.
pub fn version_cmd(game: &Path, strict: bool) -> Result<i32> {
    let b = GameBuild::inspect(game)?;
    println!("install:          {}", game.display());
    let tocs: Vec<String> = b
        .toc_versions
        .iter()
        .map(|(v, n)| format!("version {v} in {n}"))
        .collect();
    println!(
        ".toc files:       {}   (this reader reads version {} only)",
        if tocs.is_empty() {
            "none".to_string()
        } else {
            tocs.join(", ")
        },
        toc::VERSION
    );
    for bad in &b.bad_tocs {
        println!("                  not a .toc despite the name: {bad}");
    }
    match &b.packages_version {
        Ok(v) => println!(
            "Packages.bin:     version {v}   (read: {} and later; strict mode: up to {PACKAGES_MAX_VERIFIED}, the highest run here)",
            packages::MIN_VERSION
        ),
        Err(e) => println!("Packages.bin:     cannot be read: {e}"),
    }
    match (&b.file_version, &b.product_version) {
        (Some(f), None) => println!("Warframe.x64.exe: file version {f}"),
        (Some(f), Some(p)) => println!("Warframe.x64.exe: file version {f}, product version {p}"),
        _ => println!(
            "Warframe.x64.exe: not found beside the Cache.Windows folder, so no file version"
        ),
    }
    match &b.steam {
        Some(s) => println!(
            "Steam:            app {STEAM_APP_ID}, build id {}{}",
            s.build_id,
            s.last_updated.map_or(String::new(), |t| format!(", updated {}", unix_date(t)))
        ),
        None => println!("Steam:            no appmanifest_{STEAM_APP_ID}.acf found (not a Steam install, or not where Steam keeps it)"),
    }
    let known: Vec<String> = VERIFIED
        .iter()
        .map(|v| {
            format!(
                "{} (Steam build {}, file version {})",
                v.date, v.steam_build_id, v.file_version
            )
        })
        .collect();
    println!("verified builds:  {}", known.join("; "));
    let support = b.support();
    println!("support:          {support}");
    for p in b.problems() {
        println!("  problem:        {p}");
    }
    match support {
        Support::Verified => println!("  This is a build the README's check numbers were produced on."),
        Support::Readable => println!(
            "  The formats are ones this reader reads, but this build is not one it was run on: the check commands\n  \
             may report different counts (the README table is for the verified build)."
        ),
        Support::Unsupported => {}
    }
    let strict_problems = b.strict_problems();
    for p in &strict_problems {
        println!("  strict mode:    would refuse: {p}");
    }
    let refused = support == Support::Unsupported || (strict && !strict_problems.is_empty());
    if refused {
        // The same error `PackagesBin::load_with` gives in strict mode
        if let Err(e) = b.check(if strict {
            Strictness::Strict
        } else {
            Strictness::Lenient
        }) {
            eprintln!("error: {e}");
        }
    }
    Ok(i32::from(refused))
}
