//! Find programs on `PATH` (with `PATHEXT` on Windows). No subprocess is run.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The first match for `name` on `PATH`, or `name` itself when it is already a path to a file.
pub fn which(name: &str) -> Option<PathBuf> {
    which_all(name).into_iter().next()
}

/// Every match for `name` on [`search_path`], in order (useful to spot duplicate installs).
pub fn which_all(name: &str) -> Vec<PathBuf> {
    which_in(name, search_path())
}

/// Folders, under the home folder and then system-wide, where coding agents, npm, Homebrew and
/// other installers put command-line programs.
#[cfg(unix)]
const USUAL_HOME_DIRS: &[&str] = &[".local/bin", ".claude/local", ".npm-global/bin", ".bun/bin", ".volta/bin", ".cargo/bin"];
#[cfg(unix)]
const USUAL_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];
#[cfg(not(unix))]
const USUAL_HOME_DIRS: &[&str] = &[];
#[cfg(not(unix))]
const USUAL_DIRS: &[&str] = &[];

/// `PATH` plus the usual folders for command-line programs, after it so the person's own order
/// wins. An app opened from the Dock or a desktop launcher gets the system's short `PATH`, not the
/// shell's, and would miss a coding agent installed in `~/.local/bin`. Unchanged on Windows, where
/// apps get the full `PATH`.
pub fn search_path() -> Option<OsString> {
    with_usual_dirs(std::env::var_os("PATH"), std::env::var_os("HOME").map(PathBuf::from))
}

fn with_usual_dirs(path: Option<OsString>, home: Option<PathBuf>) -> Option<OsString> {
    let mut dirs: Vec<PathBuf> = path.as_ref().map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    let before = dirs.len();
    for d in home.iter().flat_map(|h| USUAL_HOME_DIRS.iter().map(move |d| h.join(d))).chain(USUAL_DIRS.iter().map(PathBuf::from)) {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    if dirs.len() == before {
        return path;
    }
    std::env::join_paths(&dirs).ok().or(path)
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
    fn usual_dirs_come_after_path() {
        let home = PathBuf::from("/home/u");
        let path = std::env::join_paths([PathBuf::from("/usr/bin"), home.join(".local/bin")]).unwrap();
        let dirs: Vec<PathBuf> = std::env::split_paths(&with_usual_dirs(Some(path), Some(home.clone())).unwrap()).collect();
        assert_eq!(dirs[..3], [PathBuf::from("/usr/bin"), home.join(".local/bin"), home.join(".claude/local")]);
        assert_eq!(dirs.len(), 1 + USUAL_HOME_DIRS.len() + USUAL_DIRS.len(), "nothing twice");
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        // No PATH and no home folder: the system-wide folders still.
        assert_eq!(with_usual_dirs(None, None), std::env::join_paths(USUAL_DIRS).ok());
    }

    #[cfg(not(unix))]
    #[test]
    fn path_is_unchanged_off_unix() {
        let path = OsString::from(r"C:\Windows;C:\tools");
        assert_eq!(with_usual_dirs(Some(path.clone()), Some(PathBuf::from(r"C:\Users\u"))), Some(path));
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
