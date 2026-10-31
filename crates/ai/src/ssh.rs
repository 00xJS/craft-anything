//! Remote backends over the system `ssh` (never interactive: `BatchMode=yes`, connect timeout).

use std::path::{Path, PathBuf};

/// Options every ssh run gets.
pub const SSH_OPTIONS: &[&str] = &["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=4"];

/// `ssh` argv (without the program) that runs `remote` on `host`. Each remote argument is quoted
/// for the remote POSIX shell.
pub fn ssh_args(host: &str, remote: &[String]) -> Vec<String> {
    let mut v: Vec<String> = SSH_OPTIONS.iter().map(|s| s.to_string()).collect();
    v.push("-T".into());
    v.push(host.to_string());
    v.push("--".into());
    v.push(remote.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "));
    v
}

/// Quote one word for a POSIX shell.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn ssh_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh"))
}

/// Host aliases from `~/.ssh/config` (and files it includes), without wildcard patterns.
pub fn ssh_hosts() -> Vec<String> {
    match ssh_dir() {
        Some(d) => hosts_from_file(&d.join("config"), &d, 0),
        None => Vec::new(),
    }
}

fn hosts_from_file(path: &Path, base: &Path, depth: u32) -> Vec<String> {
    if depth > 4 {
        return Vec::new();
    }
    let text = match std::fs::metadata(path) {
        Ok(m) if m.len() <= 1024 * 1024 => std::fs::read_to_string(path).unwrap_or_default(),
        _ => return Vec::new(),
    };
    let mut out = parse_hosts(&text);
    for inc in includes(&text) {
        let p = if inc.starts_with('/') {
            PathBuf::from(&inc)
        } else if let Some(rest) = inc.strip_prefix("~/") {
            dirs::home_dir().map(|h| h.join(rest)).unwrap_or_default()
        } else {
            base.join(&inc)
        };
        // Simple support for a trailing `*` in the file name (e.g. `config.d/*`).
        let files: Vec<PathBuf> = if p.file_name().is_some_and(|n| n.to_string_lossy().contains('*')) {
            p.parent().and_then(|d| std::fs::read_dir(d).ok()).map(|rd| rd.flatten().map(|e| e.path()).filter(|f| f.is_file()).collect()).unwrap_or_default()
        } else {
            vec![p]
        };
        for f in files {
            for h in hosts_from_file(&f, base, depth + 1) {
                if !out.contains(&h) {
                    out.push(h);
                }
            }
        }
    }
    out
}

fn includes(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (k, v) = l.split_once(|c: char| c.is_whitespace() || c == '=')?;
            k.eq_ignore_ascii_case("include").then(|| v.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        })
        .flatten()
        .collect()
}

/// Servers reached over SSH only to push and pull code. An alias for one (`github.com-work`, to
/// use a second account's key) is no computer to run an agent on.
const GIT_HOSTS: &[&str] = &[
    "github.com",
    "ssh.github.com",
    "gitlab.com",
    "altssh.gitlab.com",
    "bitbucket.org",
    "altssh.bitbucket.org",
    "codeberg.org",
    "git.sr.ht",
    "ssh.dev.azure.com",
    "vs-ssh.visualstudio.com",
];

/// `name` is a known git server, or an alias made from one (`github.com-work`, `GitHub.com.alt`).
fn is_git_host(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    GIT_HOSTS.iter().any(|g| name.strip_prefix(g).is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '.', '_'])))
}

/// Concrete `Host` names in an ssh config text, without git servers: a host named after one, or
/// whose block has a git server's `HostName` or `User git`.
pub fn parse_hosts(text: &str) -> Vec<String> {
    // One entry per `Host` (or `Match`) line: its names, and whether it points at a git server.
    let mut blocks: Vec<(Vec<String>, bool)> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('#') {
            continue;
        }
        let Some((k, v)) = l.split_once(|c: char| c.is_whitespace() || c == '=') else { continue };
        let v = v.trim_start().trim_start_matches('=').trim();
        if k.eq_ignore_ascii_case("host") {
            let names: Vec<String> =
                v.split_whitespace().map(|n| n.trim_matches('"')).filter(|n| !n.is_empty() && !n.contains(['*', '?', '!'])).map(str::to_string).collect();
            let git = names.iter().any(|n| is_git_host(n));
            blocks.push((names, git));
        } else if k.eq_ignore_ascii_case("match") {
            blocks.push((Vec::new(), false));
        } else if let Some((_, git)) = blocks.last_mut() {
            let v = v.trim_matches('"');
            *git |= (k.eq_ignore_ascii_case("hostname") && is_git_host(v)) || (k.eq_ignore_ascii_case("user") && v == "git");
        }
    }
    let git: Vec<&String> = blocks.iter().filter(|b| b.1).flat_map(|b| &b.0).collect();
    let mut out: Vec<String> = Vec::new();
    for name in blocks.iter().filter(|b| !b.1).flat_map(|b| &b.0) {
        if !git.contains(&name) && !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

/// A local port forwarded to a server on a remote host (`ssh -N -L`), owned and killed on drop.
pub struct Tunnel {
    _proc: craft_anything_platform::RunningProcess,
    pub local_port: u16,
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tunnel(:{})", self.local_port)
    }
}

/// Forward a free local port to `remote_host:remote_port` as seen from `ssh_host`. Waits (at most
/// 12 s) until the forward accepts connections.
pub fn open_tunnel(ssh_host: &str, remote_host: &str, remote_port: u16) -> Result<Tunnel, craft_anything_evidence::ToolError> {
    use craft_anything_evidence::{ErrorCode, ToolError};
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| ToolError::new(ErrorCode::OverLimit, format!("No free local port: {e}")))?;
    let mut args: Vec<String> = SSH_OPTIONS.iter().map(|s| s.to_string()).collect();
    args.extend([
        "-N".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-L".into(),
        format!("127.0.0.1:{port}:{remote_host}:{remote_port}"),
        ssh_host.to_string(),
    ]);
    let mut proc = craft_anything_platform::Process::new("ssh")
        .args(&args)
        .timeout(std::time::Duration::from_secs(24 * 3600))
        .label("ssh")
        .remediation("Install OpenSSH and make sure `ssh <host>` works without a password prompt (use a key or agent).")
        .spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
    let url = format!("http://127.0.0.1:{port}");
    while std::time::Instant::now() < deadline {
        if crate::detect::port_open(&url, std::time::Duration::from_millis(200)) {
            return Ok(Tunnel { _proc: proc, local_port: port });
        }
        if let Some(r) = proc.try_result() {
            let msg = match r {
                Ok(out) => out.stderr_text().trim().to_string(),
                Err(e) => e.message.clone(),
            };
            return Err(ToolError::new(ErrorCode::EngineMissing, format!("ssh to {ssh_host} failed: {msg}"))
                .with_remediation("Check that `ssh <host>` works without a password prompt."));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(ToolError::new(ErrorCode::TimedOut, format!("The ssh tunnel to {ssh_host} didn't come up in time.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts_and_skips_patterns() {
        let cfg = "# c\nHost studio-mac gpu\n  HostName 10.0.0.2\nHost *\n  User x\nhost=lab !bad *.corp\nMatch host foo\nHost gpu\n";
        assert_eq!(parse_hosts(cfg), vec!["studio-mac", "gpu", "lab"]);
        assert_eq!(includes("Include config.d/*\ninclude a b\n"), vec!["config.d/*", "a", "b"]);
    }

    #[test]
    fn git_servers_are_not_hosts() {
        let cfg = "Host github.com-second\n  HostName github.com\n  IdentityFile ~/.ssh/k\nHost work-gh\n  HostName github.com\n\
                   Host forge\n  HostName git.example.org\n  User git\nHost GitHub.com.alt gitlab.com\nHost box\n  HostName 10.0.0.3\n  User git2\n\
                   Host = spaced\nHost github.community\nHost work-gh\n";
        assert_eq!(parse_hosts(cfg), vec!["box", "spaced", "github.community"]);
    }

    #[test]
    fn includes_are_followed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("config.d")).unwrap();
        std::fs::write(dir.path().join("config.d").join("work"), "Host work-box\n").unwrap();
        std::fs::write(dir.path().join("config"), "Include config.d/*\nHost home\n").unwrap();
        assert_eq!(hosts_from_file(&dir.path().join("config"), dir.path(), 0), vec!["home", "work-box"]);
    }

    #[test]
    fn args_are_batch_mode_and_quoted() {
        let a = ssh_args("box", &["claude".into(), "-p".into(), "it's a test".into()]);
        assert!(a.windows(2).any(|w| w[0] == "-o" && w[1] == "BatchMode=yes"));
        assert!(a.iter().any(|s| s == "ConnectTimeout=10"));
        assert_eq!(a.last().unwrap(), "claude -p 'it'\\''s a test'");
    }
}
