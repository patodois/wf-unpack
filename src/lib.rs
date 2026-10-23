//! Readers for Warframe's Evolution Engine packages (`.toc` + `.cache`) and the assets in them.

pub mod animation;
#[cfg(feature = "bridge")]
pub mod bridge;
pub mod bytes;
pub mod cache;
pub mod gltf;
pub mod header;
pub mod material;
pub mod model;
pub mod packages;
pub mod pbr;
pub mod scene;
pub mod texture;
pub mod toc;
