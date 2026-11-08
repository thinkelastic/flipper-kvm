//! The USB keyboard and mouse: browser input turned into HID reports on /dev/hidg*.
//!
//! The devices are made by gadget/flipper-kvm-gadget.sh and linked at
//! /run/flipper-kvm/{keyboard,mouse}. Reports follow the descriptors there: an 8-byte
//! boot keyboard report, and a 7-byte absolute pointer report (buttons, X and Y in
//! 0..32767, vertical and horizontal wheel). Every report is also handed to the demo
//! screen, so the path can be seen working without a capture device.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::keymap;
use crate::state::{HidState, Shared};

pub const KEYBOARD: &str = "/run/flipper-kvm/keyboard";
pub const MOUSE: &str = "/run/flipper-kvm/mouse";

/// Longest text Paste text types in one go.
pub const MAX_TYPE: usize = 20_000;
/// Time a typed key is held, and the gap after it.
const TYPE_STEP: Duration = Duration::from_millis(8);

/// One input event from a browser, as JSON: `{"t":"key","code":"KeyA","down":true}`.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Input {
    /// A physical key, by `KeyboardEvent.code`.
    Key { code: String, down: bool },
    /// The pointer at `x`, `y` (0..1 across the picture) with `b` buttons held
    /// (1 left, 2 right, 4 middle, 8 back, 16 forward: the same bits as `MouseEvent.buttons`).
    Mouse { x: f64, y: f64, b: u8 },
    /// Wheel notches: positive `dy` scrolls down, positive `dx` right.
    Wheel {
        #[serde(default)]
        dy: i8,
        #[serde(default)]
        dx: i8,
    },
    /// Type text on the target, for a target keyboard set to `layout` (default: config).
    Type {
        text: String,
        #[serde(default)]
        layout: Option<String>,
    },
    /// Press keys together and let go, e.g. ["ControlLeft","AltLeft","Delete"].
    Combo { keys: Vec<String> },
    /// Let go of everything and stop typing.
    Release,
}

/// The report state, kept so every report says what is held now.
#[derive(Default, Debug, PartialEq)]
pub struct Reports {
    pub modifiers: u8,
    pub keys: Vec<u8>,
    pub buttons: u8,
    pub x: u16,
    pub y: u16,
}

impl Reports {
    pub fn keyboard(&self) -> [u8; 8] {
        let mut r = [0u8; 8];
        r[0] = self.modifiers;
        for (slot, k) in r[2..].iter_mut().zip(&self.keys) {
            *slot = *k;
        }
        r
    }

    pub fn mouse(&self, dy: i8, dx: i8) -> [u8; 7] {
        let [xl, xh] = self.x.to_le_bytes();
        let [yl, yh] = self.y.to_le_bytes();
        [self.buttons, xl, xh, yl, yh, (-dy) as u8, dx as u8]
    }

    /// Apply a key; false if the code is not a key the keyboard has.
    pub fn key(&mut self, code: &str, down: bool) -> bool {
        if let Some(bit) = keymap::modifier(code) {
            if down {
                self.modifiers |= bit;
            } else {
                self.modifiers &= !bit;
            }
            return true;
        }
        let Some(u) = keymap::usage(code) else { return false };
        self.keys.retain(|&k| k != u);
        if down && self.keys.len() < 6 {
            self.keys.push(u);
        }
        true
    }

    pub fn pointer(&mut self, x: f64, y: f64, b: u8) {
        let scale = |v: f64| (v.clamp(0.0, 1.0) * 32767.0).round() as u16;
        self.x = scale(x);
        self.y = scale(y);
        self.buttons = b & 0x1f;
    }
}

/// Text to key strokes: (modifiers, usage) per character, skipping what the layout lacks.
pub fn strokes(text: &str, layout: &str) -> (VecDeque<(u8, u8)>, usize) {
    let mut out = VecDeque::new();
    let mut skipped = 0;
    for c in text.chars().filter(|&c| c != '\r').take(MAX_TYPE) {
        match keymap::char_key_in(c, layout) {
            Some((usage, mods)) => out.push_back((mods, usage)),
            None => skipped += 1,
        }
    }
    (out, skipped)
}

/// A report device, opened when it appears and reopened if it goes away.
struct Device {
    path: &'static str,
    file: Option<File>,
}

impl Device {
    fn new(path: &'static str) -> Self {
        Device { path, file: None }
    }

    fn open(&mut self) -> bool {
        if self.file.is_none() {
            self.file = OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(self.path).ok();
        }
        self.file.is_some()
    }

    /// Write a report, waiting briefly while the host has not taken the last one.
    async fn send(&mut self, report: &[u8]) -> io::Result<()> {
        if !self.open() {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        let file = self.file.as_mut().unwrap();
        for _ in 0..25 {
            match file.write(report) {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => tokio::time::sleep(Duration::from_millis(2)).await,
                Err(e) => {
                    // ESHUTDOWN: no host is reading. Anything else (ENODEV...): the gadget
                    // went away; open it again next time.
                    if e.raw_os_error() != Some(libc::ESHUTDOWN) {
                        self.file = None;
                    }
                    return Err(e);
                }
            }
        }
        Err(io::Error::from(io::ErrorKind::WouldBlock))
    }
}

pub struct Hid {
    shared: Arc<Shared>,
    keyboard: Device,
    mouse: Device,
    reports: Reports,
    /// Paste text still to type: (modifiers, usage), and whether the current one is down.
    typing: VecDeque<(u8, u8)>,
    key_down: bool,
}

impl Hid {
    pub fn new(shared: Arc<Shared>) -> Self {
        Hid {
            shared,
            keyboard: Device::new(KEYBOARD),
            mouse: Device::new(MOUSE),
            reports: Reports::default(),
            typing: VecDeque::new(),
            key_down: false,
        }
    }

    /// Handle input until the channel closes. Typing runs one key step per tick, so any
    /// other input (or Release) can stop it; the devices are checked every two seconds.
    pub async fn run(mut self, mut rx: UnboundedReceiver<Input>) {
        let mut probe = tokio::time::interval(Duration::from_secs(2));
        let mut step = tokio::time::interval(TYPE_STEP);
        step.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let typing = !self.typing.is_empty() || self.key_down;
            tokio::select! {
                input = rx.recv() => match input {
                    Some(input) => self.handle(input).await,
                    None => break,
                },
                _ = step.tick(), if typing => self.type_step().await,
                _ = probe.tick() => {
                    // The current keyboard state again: harmless, and it says whether
                    // a host is there to take it.
                    let r = self.reports.keyboard();
                    let res = self.keyboard.send(&r).await;
                    self.note(&res);
                }
            }
        }
    }

    fn note(&self, res: &io::Result<()>) {
        let state = match res {
            Ok(()) => HidState::Ready,
            Err(e) if e.kind() == io::ErrorKind::NotFound => HidState::NotSetUp,
            Err(_) => HidState::NoHost,
        };
        let mut hid = self.shared.hid.lock().unwrap();
        if *hid != state {
            *hid = state;
            drop(hid);
            self.shared.changed();
        }
    }

    async fn send_keyboard(&mut self) {
        let r = self.reports.keyboard();
        self.shared.demo.lock().unwrap().keyboard(r[0], &r[2..]);
        let res = self.keyboard.send(&r).await;
        self.note(&res);
    }

    async fn send_mouse(&mut self, dy: i8, dx: i8) {
        let r = self.reports.mouse(dy, dx);
        self.shared.demo.lock().unwrap().mouse(self.reports.x, self.reports.y, self.reports.buttons, dy);
        let res = self.mouse.send(&r).await;
        self.note(&res);
    }

    /// Half a typed character: press it, or let go of it.
    async fn type_step(&mut self) {
        if self.key_down {
            self.reports.modifiers = 0;
            self.reports.keys.clear();
            self.key_down = false;
        } else if let Some((mods, usage)) = self.typing.pop_front() {
            self.reports.modifiers = mods;
            self.reports.keys = vec![usage];
            self.key_down = true;
        } else {
            return;
        }
        self.send_keyboard().await;
    }

    async fn stop_typing(&mut self) {
        if !self.typing.is_empty() || self.key_down {
            self.typing.clear();
            self.key_down = false;
            self.reports.modifiers = 0;
            self.reports.keys.clear();
            self.send_keyboard().await;
        }
    }

    async fn handle(&mut self, input: Input) {
        match input {
            Input::Key { code, down } => {
                self.stop_typing().await;
                if self.reports.key(&code, down) {
                    self.send_keyboard().await;
                }
            }
            Input::Mouse { x, y, b } => {
                if !x.is_finite() || !y.is_finite() {
                    return;
                }
                self.reports.pointer(x, y, b);
                self.send_mouse(0, 0).await;
            }
            Input::Wheel { dy, dx } => self.send_mouse(dy.clamp(-10, 10), dx.clamp(-10, 10)).await,
            Input::Combo { keys } => {
                self.stop_typing().await;
                let keys: Vec<String> = keys.into_iter().take(8).collect();
                for k in &keys {
                    self.reports.key(k, true);
                }
                self.send_keyboard().await;
                tokio::time::sleep(Duration::from_millis(60)).await;
                for k in keys.iter().rev() {
                    self.reports.key(k, false);
                }
                self.send_keyboard().await;
            }
            Input::Type { text, layout } => {
                self.stop_typing().await;
                self.release().await;
                let layout = layout
                    .filter(|l| keymap::LAYOUTS.contains(&l.as_str()))
                    .unwrap_or_else(|| self.shared.config.layout.clone());
                let (strokes, skipped) = strokes(&text, &layout);
                if skipped > 0 {
                    eprintln!("hid: {skipped} characters have no key on the {layout} layout");
                }
                self.typing = strokes;
            }
            Input::Release => {
                self.stop_typing().await;
                self.release().await;
            }
        }
    }

    async fn release(&mut self) {
        let had_keys = self.reports.modifiers != 0 || !self.reports.keys.is_empty();
        let had_buttons = self.reports.buttons != 0;
        self.reports.modifiers = 0;
        self.reports.keys.clear();
        self.reports.buttons = 0;
        if had_keys {
            self.send_keyboard().await;
        }
        if had_buttons {
            self.send_mouse(0, 0).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_parses_from_json() {
        let k: Input = serde_json::from_str(r#"{"t":"key","code":"KeyA","down":true}"#).unwrap();
        assert_eq!(k, Input::Key { code: "KeyA".into(), down: true });
        let w: Input = serde_json::from_str(r#"{"t":"wheel","dy":-2}"#).unwrap();
        assert_eq!(w, Input::Wheel { dy: -2, dx: 0 });
        let r: Input = serde_json::from_str(r#"{"t":"release"}"#).unwrap();
        assert_eq!(r, Input::Release);
        let t: Input = serde_json::from_str(r#"{"t":"type","text":"hi","layout":"de"}"#).unwrap();
        assert_eq!(t, Input::Type { text: "hi".into(), layout: Some("de".into()) });
    }

    #[test]
    fn keyboard_report_holds_modifiers_and_keys() {
        let mut r = Reports::default();
        r.key("ShiftLeft", true);
        r.key("KeyA", true);
        r.key("KeyB", true);
        assert_eq!(r.keyboard(), [0x02, 0, 0x04, 0x05, 0, 0, 0, 0]);
        r.key("KeyA", false);
        assert_eq!(r.keyboard(), [0x02, 0, 0x05, 0, 0, 0, 0, 0]);
        r.key("ShiftLeft", false);
        r.key("KeyB", false);
        assert_eq!(r.keyboard(), [0; 8]);
    }

    #[test]
    fn at_most_six_keys() {
        let mut r = Reports::default();
        for c in ["KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG"] {
            r.key(c, true);
        }
        assert_eq!(r.keys.len(), 6);
    }

    #[test]
    fn mouse_report_is_absolute_and_little_endian() {
        let mut r = Reports::default();
        r.pointer(1.0, 0.5, 1);
        assert_eq!(r.mouse(0, 0), [1, 0xff, 0x7f, 0x00, 0x40, 0, 0]);
        r.pointer(-3.0, 9.0, 0xff);
        assert_eq!(r.mouse(1, 0), [0x1f, 0, 0, 0xff, 0x7f, 0xff, 0]);
    }

    #[test]
    fn strokes_follow_the_layout_and_skip_what_it_lacks() {
        let (s, skipped) = strokes("Az@\r\n", "de");
        assert_eq!(Vec::from(s), vec![(0x02, 0x04), (0, 0x1C), (0x40, 0x14), (0, 0x28)]);
        assert_eq!(skipped, 0);
        let (s, skipped) = strokes("é!", "us");
        assert_eq!(Vec::from(s), vec![(0x02, 0x1E)]);
        assert_eq!(skipped, 1);
    }
}
