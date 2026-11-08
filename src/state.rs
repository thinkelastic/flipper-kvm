//! What the panel, the web server, the capture thread and the HID task share.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use rand::Rng;
use tokio::sync::{mpsc, watch};

use crate::audit::Audit;
use crate::config::Config;
use crate::demo::DemoTarget;
use crate::hid::Input;

/// Wrong codes allowed before the code is replaced.
pub const MAX_FAILURES: u32 = 5;
/// The window an address's wrong codes are counted in, and how long a lockout lasts.
const LOCKOUT_WINDOW: Duration = Duration::from_secs(600);

/// One encoded frame of the target's screen.
#[derive(Clone)]
pub struct Frame {
    pub jpeg: Bytes,
    pub width: u32,
    pub height: u32,
    /// Counts up with every new frame, so a sender can tell a frame it has already sent.
    pub seq: u64,
    pub captured: Instant,
}

/// Where the picture comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A capture device if there is one, else the demo screen.
    Auto,
    /// The demo screen, always.
    Demo,
}

#[derive(Clone, Debug, Default)]
pub struct VideoStatus {
    /// Short, for the panel: "1920x1080 MJPEG 30fps", "Demo screen", "No signal".
    pub label: String,
    /// The device or "demo".
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Frame sizes the device offers, for the resolution picker.
    pub sizes: Vec<(u32, u32)>,
    pub signal: bool,
}

/// What a browser may change about the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoSettings {
    pub max_fps: u32,
    pub quality: u8,
    /// A capture size to ask the device for; None for the largest up to 1080p.
    pub size: Option<(u32, u32)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum HidState {
    /// The report devices are open and the host takes reports.
    Ready,
    /// The devices exist but the host is not reading (not plugged in, or asleep).
    NoHost,
    /// No /run/flipper-kvm devices: the gadget is not set up.
    #[default]
    NotSetUp,
}

impl HidState {
    pub fn label(&self) -> &'static str {
        match self {
            HidState::Ready => "USB ready",
            HidState::NoHost => "No computer",
            HidState::NotSetUp => "Not set up",
        }
    }
}

/// The panel's Setup key: installing the USB gadget through sudo.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Setup {
    #[default]
    Idle,
    /// Setup was pressed once; a second press installs.
    Confirm,
    Running,
    Done(Result<(), String>),
}

#[derive(Clone, Debug)]
pub struct Session {
    pub created: Instant,
    pub last_seen: Instant,
    pub addr: IpAddr,
    /// Open connections (input, video); a session with one open does not go idle.
    pub open: usize,
}

#[derive(Debug, PartialEq)]
pub enum PairError {
    /// Wrong code; this many tries left before it changes.
    Wrong(u32),
    /// Too many wrong codes: the code was replaced.
    Replaced,
    /// This address tried too many wrong codes; try again in this many seconds.
    Locked(u64),
}

pub struct Shared {
    pub config: Config,
    code: Mutex<String>,
    sessions: Mutex<HashMap<String, Session>>,
    failures: AtomicU32,
    by_addr: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    /// Bumped whenever something the panel shows changes.
    pub generation: AtomicU64,
    /// Open input connections.
    pub clients: AtomicUsize,
    /// Open video streams; capture skips encoding while there are none.
    pub viewers: AtomicUsize,
    pub video: Mutex<VideoStatus>,
    pub settings: Mutex<VideoSettings>,
    pub hid: Mutex<HidState>,
    pub source: Mutex<Source>,
    pub frames: watch::Sender<Option<Frame>>,
    pub input: mpsc::UnboundedSender<Input>,
    pub demo: Mutex<DemoTarget>,
    pub audit: Audit,
    /// SHA-256 of the TLS certificate, "AB:CD:...", once the server has it.
    pub fingerprint: Mutex<String>,
    pub setup: Mutex<Setup>,
    /// The image on the virtual drive and how it is shown, if one is.
    pub drive: Mutex<Option<(String, String)>>,
    /// Where the most recent browser connected from.
    pub last_addr: Mutex<Option<IpAddr>>,
}

impl Shared {
    pub fn new(config: Config) -> (Arc<Self>, mpsc::UnboundedReceiver<Input>) {
        Self::with_audit(config, Audit::new(Some(crate::config::data_dir().join("audit.log"))))
    }

    pub fn with_audit(config: Config, audit: Audit) -> (Arc<Self>, mpsc::UnboundedReceiver<Input>) {
        let (input, rx) = mpsc::unbounded_channel();
        let settings = VideoSettings { max_fps: config.max_fps, quality: config.quality, size: None };
        let shared = Shared {
            config,
            code: Mutex::new(new_code()),
            sessions: Mutex::new(HashMap::new()),
            failures: AtomicU32::new(0),
            by_addr: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            clients: AtomicUsize::new(0),
            viewers: AtomicUsize::new(0),
            video: Mutex::new(VideoStatus::default()),
            settings: Mutex::new(settings),
            hid: Mutex::new(HidState::default()),
            source: Mutex::new(Source::Auto),
            frames: watch::channel(None).0,
            input,
            demo: Mutex::new(DemoTarget::default()),
            audit,
            fingerprint: Mutex::new(String::new()),
            setup: Mutex::new(Setup::Idle),
            drive: Mutex::new(None),
            last_addr: Mutex::new(None),
        };
        (Arc::new(shared), rx)
    }

    pub fn code(&self) -> String {
        self.code.lock().unwrap().clone()
    }

    /// A fresh code; the old one stops working.
    pub fn rotate_code(&self) {
        *self.code.lock().unwrap() = new_code();
        self.failures.store(0, Ordering::Relaxed);
        self.changed();
    }

    /// Check a code typed in a browser at `addr`. A right code is used up and a session
    /// token returned. Wrong codes count against the code (5, then it is replaced) and
    /// against the address (`lockout_after` in ten minutes, then it is locked out).
    pub fn pair(&self, attempt: &str, addr: IpAddr) -> Result<String, PairError> {
        let now = Instant::now();
        {
            let mut by_addr = self.by_addr.lock().unwrap();
            by_addr.retain(|_, (_, since)| now.duration_since(*since) < LOCKOUT_WINDOW);
            if let Some((n, since)) = by_addr.get(&addr) {
                if *n >= self.config.lockout_after {
                    return Err(PairError::Locked((LOCKOUT_WINDOW - now.duration_since(*since)).as_secs().max(1)));
                }
            }
        }
        let attempt: String = attempt.chars().filter(|c| c.is_ascii_digit()).collect();
        let ok = constant_time_eq(attempt.as_bytes(), self.code.lock().unwrap().as_bytes());
        if ok {
            let token = token();
            let session = Session { created: now, last_seen: now, addr, open: 0 };
            self.sessions.lock().unwrap().insert(token.clone(), session);
            self.by_addr.lock().unwrap().remove(&addr);
            *self.last_addr.lock().unwrap() = Some(addr);
            self.audit.log(format!("paired {addr}"));
            self.rotate_code();
            return Ok(token);
        }
        {
            let mut by_addr = self.by_addr.lock().unwrap();
            let entry = by_addr.entry(addr).or_insert((0, now));
            entry.0 += 1;
            if entry.0 == self.config.lockout_after {
                self.audit.log(format!("locked out {addr} after {} wrong codes", entry.0));
            }
        }
        self.audit.log(format!("wrong code from {addr}"));
        let n = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= MAX_FAILURES {
            self.audit.log("code replaced after too many wrong tries");
            self.rotate_code();
            Err(PairError::Replaced)
        } else {
            Err(PairError::Wrong(MAX_FAILURES - n))
        }
    }

    /// Whether `token` is a live session, marking it seen. Expired sessions are dropped.
    pub fn authorized(&self, token: &str) -> bool {
        let now = Instant::now();
        let max_age = Duration::from_secs(self.config.session_hours * 3600);
        let idle = Duration::from_secs(self.config.idle_minutes * 60);
        let mut sessions = self.sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|_, s| {
            let alive = now.duration_since(s.created) < max_age && (s.open > 0 || now.duration_since(s.last_seen) < idle);
            if !alive {
                self.audit.log(format!("session from {} expired", s.addr));
            }
            alive
        });
        let changed = sessions.len() != before;
        let ok = match sessions.get_mut(token) {
            Some(s) => {
                s.last_seen = now;
                true
            }
            None => false,
        };
        drop(sessions);
        if changed {
            self.changed();
        }
        ok
    }

    /// A connection of this session opened (+1) or closed (-1).
    pub fn connection(&self, token: &str, delta: isize) {
        if let Some(s) = self.sessions.lock().unwrap().get_mut(token) {
            s.open = s.open.saturating_add_signed(delta);
            s.last_seen = Instant::now();
        }
    }

    pub fn logout(&self, token: &str) {
        if let Some(s) = self.sessions.lock().unwrap().remove(token) {
            self.audit.log(format!("logged out {}", s.addr));
        }
        self.changed();
    }

    /// Forget every session. Open connections notice on their next message or frame.
    pub fn kick_all(&self) {
        let n = {
            let mut s = self.sessions.lock().unwrap();
            let n = s.len();
            s.clear();
            n
        };
        self.audit.log(format!("kicked all ({n} sessions)"));
        self.changed();
    }

    pub fn changed(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

fn new_code() -> String {
    format!("{:06}", rand::rng().random_range(0..1_000_000u32))
}

fn token() -> String {
    let bytes: [u8; 24] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// "123456" as "123 456", easier to read off the panel.
pub fn spaced(code: &str) -> String {
    if code.len() == 6 {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

#[cfg(test)]
pub fn test_shared() -> (Arc<Shared>, mpsc::UnboundedReceiver<Input>) {
    Shared::with_audit(Config::default(), Audit::new(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, n])
    }

    #[test]
    fn a_right_code_pairs_once() {
        let (s, _rx) = test_shared();
        let code = s.code();
        let token = s.pair(&code, ip(1)).expect("right code");
        assert!(s.authorized(&token));
        assert_ne!(s.code(), code, "a used code is replaced");
        assert!(s.pair(&code, ip(1)).is_err(), "and does not work twice");
    }

    #[test]
    fn spaces_in_the_code_are_fine() {
        let (s, _rx) = test_shared();
        let code = spaced(&s.code());
        assert!(s.pair(&code, ip(1)).is_ok());
    }

    fn wrong(s: &Shared) -> &'static str {
        if s.code() == "000000" { "111111" } else { "000000" }
    }

    #[test]
    fn too_many_wrong_codes_replace_it() {
        let (s, _rx) = test_shared();
        let code = s.code();
        for left in (1..MAX_FAILURES).rev() {
            assert_eq!(s.pair(wrong(&s), ip(1)), Err(PairError::Wrong(left)));
        }
        assert_eq!(s.pair(wrong(&s), ip(1)), Err(PairError::Replaced));
        assert_ne!(s.code(), code);
    }

    #[test]
    fn an_address_with_too_many_wrong_codes_is_locked_out() {
        let (s, _rx) = test_shared();
        for _ in 0..s.config.lockout_after {
            let _ = s.pair(wrong(&s), ip(9));
        }
        assert!(matches!(s.pair(&s.code(), ip(9)), Err(PairError::Locked(_))), "even the right code");
        assert!(s.pair(&s.code(), ip(2)).is_ok(), "other addresses are not affected");
    }

    #[test]
    fn kick_all_ends_sessions() {
        let (s, _rx) = test_shared();
        let t = s.pair(&s.code(), ip(1)).unwrap();
        s.kick_all();
        assert!(!s.authorized(&t));
    }

    #[test]
    fn idle_sessions_expire_unless_a_connection_is_open() {
        let (s, _rx) = test_shared();
        let t = s.pair(&s.code(), ip(1)).unwrap();
        let long_ago = Instant::now() - Duration::from_secs(s.config.idle_minutes * 60 + 1);
        s.sessions.lock().unwrap().get_mut(&t).unwrap().last_seen = long_ago;
        s.connection(&t, 1);
        s.sessions.lock().unwrap().get_mut(&t).unwrap().last_seen = long_ago;
        assert!(s.authorized(&t), "an open connection keeps it alive");
        s.connection(&t, -1);
        s.sessions.lock().unwrap().get_mut(&t).unwrap().last_seen = long_ago;
        assert!(!s.authorized(&t), "idle with nothing open: gone");
    }
}
