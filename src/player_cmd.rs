//! The `player` command: the Tenno's movement values with their units and sources.

use std::path::Path;

use anyhow::Result;

use wf_unpack::player::{Player, LEFT_OUT, READINGS};

/// Prints every value of [`Player`] with its unit and the `entity: key` it was read from.
/// A `*` after the unit marks a unit or meaning that is an interpretation.
pub fn player_cmd(game: &Path, notes: bool) -> Result<()> {
    let player = Player::load(game)?;
    let rows = player.rows();
    let w = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let v = rows
        .iter()
        .map(|r| r.value.len())
        .max()
        .unwrap_or(0)
        .min(40);
    println!(
        "The Tenno's movement values (Packages.bin); * marks a unit that is a reading, not data\n"
    );
    for r in &rows {
        let unit = format!("{}{}", r.unit, if r.reading { " *" } else { "" });
        println!("{:w$}  {:v$}  {:24}  {}", r.name, r.value, unit, r.source);
    }
    println!(
        "\nfalling gravity {} m/s^2 (world {} + additional {}), rising gravity {} m/s^2 *",
        player.fall_gravity(),
        -player.gravity.world.value[1],
        -player.gravity.additional.value,
        player.rise_gravity()
    );
    if notes {
        println!("\nReadings of numbers the data gives without a unit:");
        for (what, how) in READINGS {
            println!("  {what}: {how}");
        }
        println!("\nLeft out because the data does not provide it:");
        for (what, why) in LEFT_OUT {
            println!("  {what}: {why}");
        }
    }
    Ok(())
}
