//! What the AI has used: each project's building time, tokens and reported cost (saved by its
//! runs in `usage.json`, so they add up across restarts), and the session's total since the app
//! opened. Claude Code prices each run; other agents give tokens only.

use std::path::Path;
use std::time::{Duration, Instant};

use craft_anything_engine::workspace::ProjectUsage;

use crate::i18n::{tr, trf};

/// A project's usage as the app shows it: what its runs saved, plus the time since the run going
/// now last saved (it saves at each agent turn and when it ends).
#[derive(Debug, Clone, Default)]
pub struct ProjectClock {
    pub saved: ProjectUsage,
    /// A run is working: since when its time isn't in `saved` yet.
    pub since: Option<Instant>,
}

impl ProjectClock {
    pub fn load(root: &Path) -> ProjectClock {
        ProjectClock { saved: ProjectUsage::load(root), since: None }
    }

    /// The run saved its usage (an agent turn ended, or the run did): read it again.
    pub fn saved_again(&mut self, root: &Path, still_working: bool) {
        self.saved = ProjectUsage::load(root);
        self.since = still_working.then(Instant::now);
    }

    pub fn working(&self) -> bool {
        self.since.is_some()
    }

    /// Building time so far, the run going now included.
    pub fn elapsed(&self) -> Duration {
        Duration::from_millis(self.saved.work_ms) + self.since.map_or(Duration::ZERO, |s| s.elapsed())
    }
}

/// "about $3.20" (agents report an estimate at API prices).
pub fn cost_words(usd: f64) -> String {
    trf("about ${n}", &[("n", &format!("{usd:.2}"))])
}

/// "830", "12k", "1.2M".
pub fn tokens_short(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// "830 tokens", "12k tokens", "1.2M tokens".
pub fn tokens_words(n: u64) -> String {
    trf("{n} tokens", &[("n", &tokens_short(n))])
}

/// "45s", "12m 05s", "1h 04m".
pub fn duration_words(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// The tooltip for usage: what each number is, and what it counts (`counts`, e.g. "Every build
/// of this project." or "Since Craft Anything opened.").
pub fn explain(u: &ProjectUsage, counts: &str) -> String {
    let mut lines = vec![trf("{in} in, {out} out.", &[("in", &tokens_words(u.input_tokens)), ("out", &tokens_words(u.output_tokens))])];
    if u.priced {
        lines.push(tr("The cost is the coding agent's own estimate at API prices; a subscription may cover it."));
    }
    lines.push(tr(counts));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_projects_clock_adds_the_running_time_to_what_was_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ProjectClock::load(dir.path());
        assert!(!c.working() && c.elapsed().is_zero());
        ProjectUsage::update(dir.path(), |u| u.work_ms = 725_000).unwrap();
        c.saved_again(dir.path(), true);
        assert!(c.working() && c.elapsed() >= Duration::from_secs(725));
        c.saved_again(dir.path(), false);
        assert_eq!(c.elapsed(), Duration::from_secs(725));
    }

    #[test]
    fn plain_words_for_numbers() {
        assert_eq!(tokens_words(830), "830 tokens");
        assert_eq!(tokens_short(1_180_000), "1.2M");
        assert_eq!(cost_words(3.21), "about $3.21");
        assert_eq!(duration_words(Duration::from_secs(45)), "45s");
        assert_eq!(duration_words(Duration::from_secs(725)), "12m 05s");
        assert_eq!(duration_words(Duration::from_secs(3840)), "1h 04m");
    }
}
