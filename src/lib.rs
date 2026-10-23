//! Readers for Warframe's Evolution Engine packages (`.toc` + `.cache`) and the assets in them.

pub mod animation;
#[cfg(feature = "bridge")]
pub mod bridge;
#[cfg(feature = "bridge")]
pub mod bridge_anim;
#[cfg(feature = "bridge")]
pub mod bridge_scene;
pub mod bytes;
pub mod cache;
pub mod clipspec;
pub mod collision;
pub mod ee;
pub mod gltf;
pub mod header;
pub mod level;
pub mod level_export;
pub mod material;
pub mod model;
pub mod packages;
pub mod pbr;
pub mod physx;
pub mod player;
pub mod scene;
pub mod texture;
pub mod toc;
