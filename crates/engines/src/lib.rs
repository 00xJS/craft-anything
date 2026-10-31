//! Adapters for optional external engines (Ghidra, adb, Chromium, Frida, JADX…).
//!
//! Every engine is an entry in [`ENGINES`] (see [`table`]). Each is found, in order: the path the
//! user set in Settings → Engines, the managed install in the engine store, `PATH`, then the
//! engine's standard install folders. Nothing here is required for Clean room analysis.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod adb;
pub mod assets;
pub mod cdp;
pub mod cutter;
pub mod frida;
pub mod game;
pub mod gdb;
pub mod ghidra;
pub mod install;
pub mod jadx;
pub mod mcp_client;
pub mod rizin;
pub mod table;
#[cfg(feature = "wasm-plugins")]
pub mod wasm_plugin;

pub use table::{ArchiveFormat, ENGINES, ExternalEngine, Install, Pin};

use std::path::{Path, PathBuf};

/// User overrides, mirrored from the engine settings so this crate doesn't depend on the engine.
pub trait ToolPathLookup {
    fn override_for(&self, id: &str) -> Option<PathBuf>;
}

/// The table entry for an engine id.
pub fn engine(id: &str) -> Option<&'static ExternalEngine> {
    ENGINES.iter().find(|e| e.id == id)
}

/// The ids that have a path override in Settings → Engines, in table order.
pub fn path_setting_ids() -> impl Iterator<Item = &'static str> {
    ENGINES.iter().filter(|e| e.path_setting).map(|e| e.id)
}

pub fn display_name(id: &str) -> &'static str {
    engine(id).map(|e| e.name).unwrap_or("an external tool")
}

/// Find an engine: user override, then the managed store (`<store>/<id>/bin/<exe>`), then PATH,
/// then its standard install folders.
pub fn locate(id: &str, overrides: &dyn ToolPathLookup, store: &Path) -> Option<PathBuf> {
    if let Some(p) = overrides.override_for(id)
        && p.exists()
    {
        return Some(p);
    }
    let e = engine(id)?;
    let exes = e.host_exes();
    for exe in exes {
        for candidate in [store.join(id).join("bin").join(exe), store.join(id).join(exe)] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // A single-file install (a .jar) is found by the name its pin saved it under.
    for pin in e.pins() {
        if let ArchiveFormat::File { name } = pin.format
            && store.join(id).join(name).is_file()
        {
            return Some(store.join(id).join(name));
        }
    }
    find_on_path(exes).or_else(|| e.known_install_paths().into_iter().find(|p| p.is_file()))
}

/// Standard install locations for programs that don't put themselves on PATH (browsers on
/// Windows and macOS), most common first.
pub fn well_known(id: &str) -> Vec<PathBuf> {
    engine(id).map(ExternalEngine::known_install_paths).unwrap_or_default()
}

pub(crate) fn find_on_path(exes: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for exe in exes {
            let p = dir.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoOverrides;
    impl ToolPathLookup for NoOverrides {
        fn override_for(&self, _: &str) -> Option<PathBuf> {
            None
        }
    }

    #[test]
    fn unknown_engine_is_none() {
        assert!(locate("definitely-not-an-engine", &NoOverrides, Path::new("/nonexistent")).is_none());
        assert_eq!(display_name("adb"), "Android platform tools (adb)");
    }

    #[test]
    fn browsers_have_well_known_locations_off_path() {
        assert!(well_known("adb").is_empty());
        let paths = well_known("chromium");
        if cfg!(any(windows, target_os = "macos")) {
            assert!(paths.iter().any(|p| p.to_string_lossy().contains("Chrome")));
            assert!(paths.iter().any(|p| p.to_string_lossy().contains("Edge")));
        }
    }
}
