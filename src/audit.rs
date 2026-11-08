//! A record of who connected and what they did: pairing, sessions, kicks, drive changes.
//! Kept in ~/.local/share/flipper-kvm/audit.log (rotated at 1 MiB), and the recent lines
//! in memory for the panel and the web page.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const KEEP: usize = 100;
const ROTATE_AT: u64 = 1 << 20;

pub struct Audit {
    path: Option<PathBuf>,
    recent: Mutex<VecDeque<String>>,
}

impl Audit {
    pub fn new(path: Option<PathBuf>) -> Self {
        Audit { path, recent: Mutex::new(VecDeque::new()) }
    }

    pub fn log(&self, what: impl AsRef<str>) {
        let line = format!("{} {}", timestamp(SystemTime::now()), what.as_ref());
        eprintln!("audit: {line}");
        if let Some(path) = &self.path {
            if std::fs::metadata(path).map(|m| m.len() > ROTATE_AT).unwrap_or(false) {
                let _ = std::fs::rename(path, path.with_extension("log.1"));
            }
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(f, "{line}");
            }
        }
        let mut recent = self.recent.lock().unwrap();
        recent.push_back(line);
        while recent.len() > KEEP {
            recent.pop_front();
        }
    }

    /// The newest lines, newest last.
    pub fn recent(&self, n: usize) -> Vec<String> {
        let recent = self.recent.lock().unwrap();
        recent.iter().skip(recent.len().saturating_sub(n)).cloned().collect()
    }
}

/// UTC, ISO 8601, to the second: `2026-11-07T18:22:01Z`.
pub fn timestamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00Z");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_794_075_721)), "2026-11-07T18:22:01Z");
    }

    #[test]
    fn keeps_the_recent_lines() {
        let a = Audit::new(None);
        for i in 0..150 {
            a.log(format!("event {i}"));
        }
        let r = a.recent(3);
        assert_eq!(r.len(), 3);
        assert!(r[2].ends_with("event 149"));
    }
}
