//! AtlasOS Launcher's core, with no Qt: the app catalogue, matching and
//! scoring, the calculator and unit converter, the Settings index, the recent
//! files list, the usage and pins stores, commands and web search URLs.
//! See docs/DESIGN.md, "Search".

/// The core's version, shown in logs next to the app's.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::VERSION.is_empty());
    }
}
