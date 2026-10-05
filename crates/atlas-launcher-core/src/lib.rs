//! AtlasOS Launcher's core, with no Qt: the app catalogue, matching and
//! scoring, the calculator and unit converter, the Settings index, the recent
//! files list, the usage and pins stores, commands and web search URLs, the
//! checks on late results, and the query engine that ranks and merges them.
//! See docs/DESIGN.md, "Search".

pub mod calc;
pub mod catalog;
pub mod commands;
pub mod engine;
pub mod fsutil;
pub mod late;
pub mod pins;
pub mod query;
pub mod recent;
pub mod result;
pub mod settings_index;
pub mod text;
pub mod usage;
pub mod web;

/// The core's version, shown in logs next to the app's.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
