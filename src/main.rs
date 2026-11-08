//! Flipper One KVM: plug the Flipper into a computer, and use that computer from a browser.
//!
//! - The computer's screen comes in through an HDMI-to-USB capture dongle in the
//!   Flipper's USB-A port (V4L2, MJPEG or YUYV).
//! - Its keyboard, mouse and a virtual USB drive are the Flipper itself, on USB-C1, as a
//!   USB gadget.
//! - All of it is served over HTTPS (port 8443 by default). A browser pairs with the
//!   six-digit code shown on the Flipper's screen; a code works once.
//!
//! The panel shows the code, the address, what is connected and the certificate's
//! fingerprint. Soft keys: Close, Code (a new code), Kick (end every session), Setup
//! (install the USB gadget, when it is missing), and the video source (Auto or Demo).
//!
//! `flipper-kvm --headless` runs the server alone, printing the code, for testing
//! without the panel.

mod audit;
mod config;
mod demo;
mod drive;
mod hid;
mod keymap;
mod mjpeg;
mod server;
mod setup;
mod state;
mod tls;
mod video;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use flipctl_app::{heading, pair, rule, text, App, Key, Page, Row, Ui};

use state::{HidState, Setup, Shared, Source};

struct Kvm {
    shared: Arc<Shared>,
}

impl Kvm {
    fn start() -> Self {
        let (shared, input) = Shared::new(config::Config::load());
        video::spawn(shared.clone());
        let net = shared.clone();
        std::thread::Builder::new()
            .name("server".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
                rt.block_on(async move {
                    tokio::spawn(hid::Hid::new(net.clone()).run(input));
                    if let Err(e) = server::serve(net.clone()).await {
                        net.audit.log(format!("server stopped: {e}"));
                        std::process::exit(1);
                    }
                });
            })
            .expect("server thread");

        if std::env::args().any(|a| a == "--headless") {
            headless(&shared);
        }
        Kvm { shared }
    }

    fn needs_setup(&self) -> bool {
        *self.shared.hid.lock().unwrap() == HidState::NotSetUp || !drive::available()
    }
}

/// No panel: say the code and the status on stdout whenever they change.
fn headless(shared: &Shared) -> ! {
    let mut seen = u64::MAX;
    loop {
        let g = shared.generation.load(Ordering::Relaxed);
        if g != seen {
            seen = g;
            println!(
                "code {}  web https://{}  video {}  usb {}  clients {}  cert {}",
                state::spaced(&shared.code()),
                address(shared.config.port),
                shared.video.lock().unwrap().label,
                shared.hid.lock().unwrap().label(),
                shared.clients.load(Ordering::Relaxed),
                shared.fingerprint.lock().unwrap(),
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Where to point a browser: the first wired, then wireless, then any other address.
fn address(port: u16) -> String {
    let mut addrs: Vec<(u8, String)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback() && i.ip().is_ipv4())
        .map(|i| {
            let rank = match i.name.as_str() {
                n if n.starts_with("end") || n.starts_with("eth") || n.starts_with("enp") => 0,
                n if n.starts_with("wl") => 1,
                _ => 2,
            };
            (rank, format!("{}:{port}", i.ip()))
        })
        .collect();
    addrs.sort();
    addrs.into_iter().next().map(|a| a.1).unwrap_or_else(|| "no network".into())
}

/// Text in panel-width lines, as rows.
fn wrapped(s: &str, width: usize, dim: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            rows.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        rows.push(line);
    }
    rows.into_iter().map(|l| if dim { text(l).dim() } else { text(l) }).collect()
}

impl App for Kvm {
    fn page(&self) -> Page {
        let s = &self.shared;
        if *s.setup.lock().unwrap() == Setup::Confirm {
            let mut rows = vec![heading("Set up USB", "")];
            rows.extend(wrapped(
                "Installs the USB keyboard, mouse and drive (needs sudo). USB networking drops for a moment. Press Setup again to go ahead, Code to cancel.",
                34,
                false,
            ));
            return Page::detail(rows).id("confirm");
        }
        let clients = match s.clients.load(Ordering::Relaxed) {
            0 => "None".to_string(),
            n => {
                let from = s.last_addr.lock().unwrap().map(|a| format!(" ({a})")).unwrap_or_default();
                if n == 1 { format!("1{from}") } else { format!("{n}{from}") }
            }
        };
        let drive_row = match drive::mounted() {
            Some((name, _)) => name,
            None if drive::available() => "Empty".into(),
            None => "Not set up".into(),
        };
        let mut rows = vec![
            heading("Code", state::spaced(&s.code())),
            pair("https://", address(s.config.port)),
            rule(),
            pair("Video", s.video.lock().unwrap().label.clone()),
            pair("Keyboard", s.hid.lock().unwrap().label()),
            pair("Drive", drive_row),
            pair("Browsers", clients),
        ];
        match s.setup.lock().unwrap().clone() {
            Setup::Confirm => {}
            Setup::Running => rows.push(text("Setting up USB...")),
            Setup::Done(Ok(())) => rows.push(text("USB set up.")),
            Setup::Done(Err(e)) => rows.extend(wrapped(&format!("Setup failed: {e}"), 34, false)),
            Setup::Idle if self.needs_setup() => rows.extend(wrapped("Press Setup to install the USB keyboard, mouse and drive.", 34, true)),
            Setup::Idle => {}
        }
        let fp = s.fingerprint.lock().unwrap().clone();
        if !fp.is_empty() {
            rows.push(rule());
            rows.push(pair("Certificate", "SHA-256").dim());
            // Eight bytes a line, to compare with what the browser shows.
            for chunk in fp.split(':').collect::<Vec<_>>().chunks(8) {
                rows.push(text(chunk.join(":")).dim());
            }
        }
        Page::detail(rows)
    }

    fn buttons(&self) -> [&str; 5] {
        let source = if *self.shared.source.lock().unwrap() == Source::Demo { "Auto" } else { "Demo" };
        let setup = match *self.shared.setup.lock().unwrap() {
            Setup::Running => "",
            Setup::Confirm => "Setup",
            _ if self.needs_setup() => "Setup",
            _ => "",
        };
        ["Close", "Code", "Kick", setup, source]
    }

    fn key(&mut self, key: Key, _: &mut Ui) -> bool {
        let s = &self.shared;
        match key {
            Key::View if *s.setup.lock().unwrap() == Setup::Confirm => *s.setup.lock().unwrap() = Setup::Idle,
            Key::View => {
                s.rotate_code();
                s.audit.log("new code from the panel");
            }
            Key::Power => s.kick_all(),
            Key::Edit => {
                let mut setup = s.setup.lock().unwrap();
                match *setup {
                    Setup::Confirm => {
                        *setup = Setup::Running;
                        let shared = s.clone();
                        std::thread::spawn(move || {
                            let result = setup::install();
                            shared.audit.log(match &result {
                                Ok(()) => "USB gadget installed from the panel".to_string(),
                                Err(e) => format!("USB gadget setup failed: {e}"),
                            });
                            *shared.setup.lock().unwrap() = Setup::Done(result);
                            shared.changed();
                            flipctl_app::redraw();
                        });
                    }
                    Setup::Running => {}
                    _ if self.needs_setup() => *setup = Setup::Confirm,
                    _ => return false,
                }
            }
            Key::Run => {
                let mut src = s.source.lock().unwrap();
                *src = if *src == Source::Demo { Source::Auto } else { Source::Demo };
            }
            _ => return false,
        }
        s.changed();
        true
    }

    fn every(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }

    fn tick(&mut self, _: &mut Ui) {
        // A confirmation not acted on goes away by itself.
        let mut setup = self.shared.setup.lock().unwrap();
        if matches!(*setup, Setup::Done(Ok(()))) && !self.needs_setup() {
            *setup = Setup::Idle;
        }
    }
}

flipctl_app::main!(Kvm::start());

#[cfg(test)]
mod tests {
    use super::*;
    use flipctl_app::Status;

    /// The panel page renders, and with `KVM_RENDER=1` is saved to look at.
    #[test]
    fn panel_renders() {
        let (shared, _rx) = state::test_shared();
        *shared.fingerprint.lock().unwrap() = vec!["AB"; 32].join(":");
        let grey = flipctl_pages::shot(Kvm { shared }, Status::default());
        assert_eq!(grey.len(), 256 * 144);
        assert!(grey.iter().any(|&p| p < 128) && grey.iter().any(|&p| p > 128), "something is drawn");
        if std::env::var_os("KVM_RENDER").is_some() {
            std::fs::create_dir_all("target/render").unwrap();
            std::fs::write("target/render/panel.gray", &grey).unwrap();
        }
    }

    #[test]
    fn wrapping_keeps_lines_short() {
        let rows = wrapped("Press Setup again to install the USB keyboard, mouse and drive.", 20, false);
        assert!(rows.len() >= 3);
        assert!(rows.iter().all(|r| r.label.len() <= 20));
    }
}
