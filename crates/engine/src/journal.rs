//! The project journal: an append-only JSONL log under `<project>/journal/YYYY-MM-DD.jsonl`.
//!
//! Every tool call (params, duration, outcome, evidence id), every model turn (backend, tokens,
//! cost, stop reason), consents, errors and user messages land here, so a later turn or another
//! agent can see exactly what happened. Writing is best effort: a journal failure never fails the
//! call that caused it.

use serde_json::{Map, Value, json};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Longest params/payload kept per entry (bytes of JSON). Longer values are replaced by a note.
pub const MAX_FIELD_BYTES: usize = 4096;

/// Seconds since the Unix epoch and the UTC civil date (`YYYY-MM-DD`) and time (`hh:mm:ss`).
pub fn now_utc() -> (u64, String, String) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (date, time) = civil(secs);
    (secs, date, time)
}

fn civil(secs: u64) -> (String, String) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (format!("{y:04}-{m:02}-{d:02}"), format!("{:02}:{:02}:{:02}", rem / 3600, (rem % 3600) / 60, rem % 60))
}

/// Keep a JSON value small enough for the journal.
pub fn bounded(v: &Value) -> Value {
    let v = redacted(v);
    let text = v.to_string();
    if text.len() <= MAX_FIELD_BYTES { v } else { json!({"omitted": "too large for the journal", "bytes": text.len()}) }
}

/// Field names whose values are secrets and never written to the journal.
const SECRET_FIELDS: &[&str] =
    &["api_key", "apikey", "secret", "password", "passphrase", "token", "access_token", "refresh_token", "authorization", "private_key", "hidden_values"];

/// `v` with the value of every secret-named field (at any depth) replaced by `"(redacted)"`.
pub fn redacted(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, val)| {
                    let secret = SECRET_FIELDS.iter().any(|s| k.eq_ignore_ascii_case(s));
                    (k.clone(), if secret { json!("(redacted)") } else { redacted(val) })
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(redacted).collect()),
        other => other.clone(),
    }
}

pub fn journal_dir(project_root: &Path) -> PathBuf {
    project_root.join("journal")
}

/// Append one entry (an object) to today's journal file. Adds `ts` and `time` fields.
pub fn append(project_root: &Path, kind: &str, entry: Value) {
    let (secs, date, time) = now_utc();
    let mut map = match entry {
        Value::Object(m) => m,
        other => {
            let mut m = Map::new();
            m.insert("value".into(), other);
            m
        }
    };
    map.insert("ts".into(), json!(secs));
    map.insert("time".into(), json!(format!("{date}T{time}Z")));
    map.insert("kind".into(), json!(kind));
    let dir = journal_dir(project_root);
    let line = match serde_json::to_string(&Value::Object(map)) {
        Ok(l) => l,
        Err(_) => return,
    };
    let result = std::fs::create_dir_all(&dir).and_then(|_| {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("{date}.jsonl")))?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")
    });
    if let Err(e) = result {
        log::warn!("journal write failed: {e}");
    }
}

/// Journal days available, oldest first (`YYYY-MM-DD`).
pub fn days(project_root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(journal_dir(project_root))
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".jsonl")).map(str::to_string))
                .filter(|n| n.len() == 10 && n.chars().all(|c| c.is_ascii_digit() || c == '-'))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Read entries of one day (unparseable lines are skipped).
pub fn read_day(project_root: &Path, day: &str) -> Vec<Value> {
    if day.len() != 10 || !day.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return Vec::new();
    }
    let text = std::fs::read_to_string(journal_dir(project_root).join(format!("{day}.jsonl"))).unwrap_or_default();
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn secrets_never_reach_the_journal() {
        let v = json!({"api_key": "sk-123", "nested": {"Token": "t"}, "list": [{"password": "p"}], "key": "auto_mode", "value": 1});
        let r = bounded(&v);
        assert_eq!(r["api_key"], "(redacted)");
        assert_eq!(r["nested"]["Token"], "(redacted)");
        assert_eq!(r["list"][0]["password"], "(redacted)");
        assert_eq!(r["key"], "auto_mode", "a setting's name isn't a secret");
        assert!(!r.to_string().contains("sk-123"));
        let named = bounded(&json!({"path": "a.har", "hidden_values": ["pat@example.test", "tok-fake"]}));
        assert_eq!(named["hidden_values"], "(redacted)", "values a caller asks to hide");
        assert_eq!(named["path"], "a.har");
    }

    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0).0, "1970-01-01");
        assert_eq!(civil(951_782_400).0, "2000-02-29");
        assert_eq!(civil(1_791_763_200).0, "2026-10-12");
    }

    #[test]
    fn append_and_read() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "tool_call", json!({"tool": "x"}));
        append(dir.path(), "note", json!("hi"));
        let days = days(dir.path());
        assert_eq!(days.len(), 1);
        let entries = read_day(dir.path(), &days[0]);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["tool"], "x");
        assert_eq!(entries[1]["value"], "hi");
        assert!(bounded(&json!("a".repeat(10_000)))["omitted"].is_string());
        assert!(read_day(dir.path(), "../../etc").is_empty());
    }
}
