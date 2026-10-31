//! Find programs on `PATH` (with `PATHEXT` on Windows). No subprocess is run.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The first match for `name` on `PATH`, or `name` itself when it is already a path to a file.
pub fn which(name: &str) -> Option<PathBuf> {
    which_all(name).into_iter().next()
}

/// Every match for `name` on `PATH`, in order (useful to spot duplicate installs).
pub fn which_all(name: &str) -> Vec<PathBuf> {
    which_in(name, std::env::var_os("PATH"))
}

/// Like [`which_all`] with an explicit search path.
pub fn which_in(name: &str, path: Option<OsString>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if name.is_empty() {
        return out;
    }
    let as_path = Path::new(name);
    if as_path.components().count() > 1 || as_path.is_absolute() {
        for c in candidates(as_path) {
            if is_executable(&c) {
                out.push(c);
                break;
            }
        }
        return out;
    }
    let Some(path) = path else { return out };
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for c in candidates(&dir.join(name)) {
            if is_executable(&c) && !out.contains(&c) {
                out.push(c);
                break;
            }
        }
    }
    out
}

/// File name of the command-line binary (`craft-anything-cli`, plus `.exe` on Windows).
pub fn cli_file_name() -> String {
    format!("craft-anything-cli{}", std::env::consts::EXE_SUFFIX)
}

/// Absolute path of the installed `craft-anything-cli`: this executable when it is the CLI, else
/// a sibling of this executable (the desktop app ships next to it), else the first one on `PATH`.
pub fn self_cli_path() -> Option<PathBuf> {
    let name = cli_file_name();
    let exe = std::env::current_exe().ok().map(|p| std::fs::canonicalize(&p).unwrap_or(p));
    if let Some(exe) = &exe {
        if exe.file_name().is_some_and(|n| n == name.as_str()) {
            return Some(exe.clone());
        }
        if let Some(dir) = exe.parent() {
            let sib = dir.join(&name);
            if is_executable(&sib) {
                return Some(sib);
            }
            // test binaries live in target/<profile>/deps; the CLI is one level up
            if let Some(up) = dir.parent().map(|d| d.join(&name))
                && is_executable(&up)
            {
                return Some(up);
            }
        }
    }
    which("craft-anything-cli")
}

#[cfg(windows)]
fn candidates(base: &Path) -> Vec<PathBuf> {
    let mut v = vec![base.to_path_buf()];
    if base.extension().is_none() {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        for e in exts.split(';').filter(|e| !e.is_empty()) {
            let mut s = base.as_os_str().to_os_string();
            s.push(e);
            v.push(PathBuf::from(s));
        }
    }
    v
}

#[cfg(not(windows))]
fn candidates(base: &Path) -> Vec<PathBuf> {
    vec![base.to_path_buf()]
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_sh_and_misses_nonsense() {
        #[cfg(unix)]
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-program-xyz").is_none());
        assert!(which("").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn explicit_search_path() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tool");
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        assert!(which_in("tool", Some(dir.path().as_os_str().to_owned())).is_empty());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(which_in("tool", Some(dir.path().as_os_str().to_owned())), vec![p]);
    }
}
