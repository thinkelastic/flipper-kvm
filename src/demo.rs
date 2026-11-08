//! The demo screen: a stand-in for the computer on the other end, used when there is no
//! capture device. It is fed the same HID reports the USB gadget gets, so typing and
//! pointing in the browser show up in the picture whether or not a computer is plugged in.

use font8x8::{UnicodeFonts, BASIC_FONTS};

pub const W: usize = 1280;
pub const H: usize = 720;

const BG: [u8; 3] = [0x14, 0x17, 0x1c];
const PANEL: [u8; 3] = [0x22, 0x27, 0x30];
const ORANGE: [u8; 3] = [0xff, 0x82, 0x00];
const TEXT: [u8; 3] = [0xe8, 0xe8, 0xe8];
const DIM: [u8; 3] = [0x8a, 0x90, 0x99];

#[derive(Default)]
pub struct DemoTarget {
    /// What has been typed, as a US keyboard would make it.
    pub typed: String,
    held: Vec<u8>,
    pub x: u16,
    pub y: u16,
    pub buttons: u8,
    pub clicks: u32,
    pub scroll: i32,
    pub reports: u64,
    pub why: String,
}

impl DemoTarget {
    /// A keyboard report: the keys that just went down are typed.
    pub fn keyboard(&mut self, modifiers: u8, keys: &[u8]) {
        self.reports += 1;
        let shift = modifiers & 0x22 != 0;
        for &k in keys.iter().filter(|&&k| k != 0) {
            if self.held.contains(&k) {
                continue;
            }
            match k {
                0x2A => {
                    self.typed.pop();
                }
                _ if modifiers & 0xDD != 0 => {} // Ctrl, Alt or Meta: a shortcut, not text
                _ => {
                    if let Some(c) = crate::keymap::key_char(k, shift) {
                        self.typed.push(c);
                    }
                }
            }
        }
        self.held = keys.iter().copied().filter(|&k| k != 0).collect();
        // Keep the last few lines.
        let lines: Vec<&str> = self.typed.split('\n').collect();
        if lines.len() > 12 {
            self.typed = lines[lines.len() - 12..].join("\n");
        }
    }

    pub fn mouse(&mut self, x: u16, y: u16, buttons: u8, dy: i8) {
        self.reports += 1;
        if buttons & !self.buttons & 1 != 0 {
            self.clicks += 1;
        }
        self.x = x;
        self.y = y;
        self.buttons = buttons;
        self.scroll += i32::from(dy);
    }

    /// The screen as RGB, W x H.
    pub fn render(&self, frame: u64) -> Vec<u8> {
        let mut c = Canvas { px: vec![0; W * H * 3] };
        c.fill(0, 0, W, H, BG);
        c.fill(0, 0, W, 56, PANEL);
        c.text(24, 16, 3, "Flipper One KVM", ORANGE);
        c.text(420, 24, 2, "demo screen - no capture device connected", DIM);
        if !self.why.is_empty() {
            c.text(24, 72, 2, &self.why, DIM);
        }

        // What has been typed, in a box, with a cursor that blinks.
        c.fill(24, 110, W - 48, 380, PANEL);
        c.text(40, 122, 2, "Keyboard: type in the browser window", DIM);
        let mut y = 160;
        let lines: Vec<&str> = self.typed.split('\n').collect();
        for (i, line) in lines.iter().enumerate() {
            let tail: String = line.chars().rev().take(70).collect::<Vec<_>>().into_iter().rev().collect();
            c.text(40, y, 2, &tail, TEXT);
            if i == lines.len() - 1 && (frame / 5) % 2 == 0 {
                c.fill(40 + tail.chars().count() * 16, y, 12, 16, ORANGE);
            }
            y += 26;
        }

        // The pointer, the buttons and the wheel.
        let px = usize::from(self.x) * (W - 1) / 32767;
        let py = usize::from(self.y) * (H - 1) / 32767;
        let status = format!(
            "Mouse: x {:5} y {:5}  buttons {}{}{}  clicks {}  wheel {}   reports {}",
            self.x,
            self.y,
            if self.buttons & 1 != 0 { "L" } else { "-" },
            if self.buttons & 4 != 0 { "M" } else { "-" },
            if self.buttons & 2 != 0 { "R" } else { "-" },
            self.clicks,
            self.scroll,
            self.reports
        );
        c.text(24, 520, 2, &status, TEXT);
        let secs = frame / 10;
        c.text(24, H - 40, 2, &format!("frame {frame}  up {}:{:02}", secs / 60, secs % 60), DIM);
        c.cursor(px, py, if self.buttons != 0 { ORANGE } else { TEXT });
        c.px
    }
}

/// A plain screen with a message, W x H RGB: "No signal" and the like.
pub fn message(title: &str, line: &str, frame: u64) -> Vec<u8> {
    let mut c = Canvas { px: vec![0; W * H * 3] };
    c.fill(0, 0, W, H, BG);
    let tx = W.saturating_sub(title.chars().count() * 32) / 2;
    c.text(tx, H / 2 - 60, 4, title, ORANGE);
    let lx = W.saturating_sub(line.chars().count() * 16) / 2;
    c.text(lx, H / 2 + 10, 2, line, DIM);
    // A dot that moves, so a frozen picture can be told from a live "no signal".
    let x = (frame as usize * 8) % (W - 40) + 20;
    c.fill(x, H - 30, 8, 8, DIM);
    c.px
}

struct Canvas {
    px: Vec<u8>,
}

impl Canvas {
    fn put(&mut self, x: usize, y: usize, rgb: [u8; 3]) {
        if x < W && y < H {
            let i = (y * W + x) * 3;
            self.px[i..i + 3].copy_from_slice(&rgb);
        }
    }

    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, rgb: [u8; 3]) {
        for yy in y..(y + h).min(H) {
            for xx in x..(x + w).min(W) {
                self.put(xx, yy, rgb);
            }
        }
    }

    fn text(&mut self, x: usize, y: usize, scale: usize, s: &str, rgb: [u8; 3]) {
        for (i, ch) in s.chars().enumerate() {
            let glyph = BASIC_FONTS.get(ch).or_else(|| BASIC_FONTS.get('?')).unwrap();
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits & (1 << col) != 0 {
                        self.fill(x + (i * 8 + col) * scale, y + row * scale, scale, scale, rgb);
                    }
                }
            }
        }
    }

    /// An arrow pointer with its tip at x, y.
    fn cursor(&mut self, x: usize, y: usize, rgb: [u8; 3]) {
        for row in 0..22usize {
            let width = (row * 2 / 3).min(12);
            for col in 0..=width {
                let edge = col == 0 || col == width || row == 21;
                self.put(x + col, y + row, if edge { [0, 0, 0] } else { rgb });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_backspace() {
        let mut d = DemoTarget::default();
        d.keyboard(0x02, &[0x0B]); // H
        d.keyboard(0, &[]);
        d.keyboard(0, &[0x0C]); // i
        d.keyboard(0, &[0x0C, 0x1E]); // i held, 1 pressed: only 1 is new
        d.keyboard(0, &[]);
        assert_eq!(d.typed, "Hi1");
        d.keyboard(0, &[0x2A]);
        assert_eq!(d.typed, "Hi");
        d.keyboard(0x01, &[0x06]); // Ctrl+C is not text
        assert_eq!(d.typed, "Hi");
    }

    #[test]
    fn clicks_count_on_press() {
        let mut d = DemoTarget::default();
        d.mouse(0, 0, 1, 0);
        d.mouse(10, 10, 1, 0);
        d.mouse(10, 10, 0, 0);
        assert_eq!(d.clicks, 1);
    }

    #[test]
    fn renders_a_full_frame() {
        let d = DemoTarget { typed: "hello\nworld".into(), x: 32767, y: 32767, ..Default::default() };
        assert_eq!(d.render(0).len(), W * H * 3);
    }
}
