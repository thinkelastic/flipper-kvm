//! Settings, from ~/.config/flipper-kvm/config.toml. Written with the defaults and their
//! explanations the first time the app runs, so there is always a file to edit.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// HTTPS port. Plain HTTP on the same port is redirected to HTTPS.
    pub port: u16,
    /// A session ends this many hours after pairing, however active.
    pub session_hours: u64,
    /// A session ends after this many minutes with no request and no open connection.
    pub idle_minutes: u64,
    /// Frames per second sent to a browser at most.
    pub max_fps: u32,
    /// JPEG quality (30..95) for frames encoded here (YUYV capture, demo screen).
    pub quality: u8,
    /// Keyboard layout of the target, for Paste text: us, uk, de, fr, es.
    pub layout: String,
    /// Wrong codes from one address within ten minutes before it is locked out.
    pub lockout_after: u32,
    /// Largest disk image the browser may upload, in MiB.
    pub max_image_mib: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            port: 8443,
            session_hours: 12,
            idle_minutes: 60,
            max_fps: 30,
            quality: 80,
            layout: "us".into(),
            lockout_after: 10,
            max_image_mib: 16 * 1024,
        }
    }
}

const TEMPLATE: &str = r#"# Flipper One KVM settings. Delete a line to use its default.

# HTTPS port. Plain HTTP on the same port is redirected to HTTPS.
port = 8443

# A session ends this many hours after pairing, however active.
session_hours = 12

# A session ends after this many minutes with no request and no open connection.
idle_minutes = 60

# Frames per second sent to a browser at most.
max_fps = 30

# JPEG quality (30..95) for frames encoded on the Flipper (YUYV capture, demo screen).
# MJPEG from a capture dongle is passed through as it comes.
quality = 80

# Keyboard layout of the target computer, used by Paste text: us, uk, de, fr, es.
layout = "us"

# Wrong codes from one address within ten minutes before it is locked out for ten minutes.
lockout_after = 10

# Largest disk image the browser may upload to the virtual drive, in MiB.
max_image_mib = 16384
"#;

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("flipper-kvm")
}

pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FLIPPER_KVM_DATA") {
        return d.into();
    }
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("flipper-kvm")
}

impl Config {
    pub fn load() -> Self {
        let path = config_dir().join("config.toml");
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                eprintln!("config: {}: {e}; using defaults", path.display());
                Config::default()
            }),
            Err(_) => {
                let _ = std::fs::create_dir_all(config_dir());
                let _ = std::fs::write(&path, TEMPLATE);
                Config::default()
            }
        };
        if let Some(p) = std::env::var("FLIPPER_KVM_PORT").ok().and_then(|p| p.parse().ok()) {
            cfg.port = p;
        }
        cfg.sanitize()
    }

    fn sanitize(mut self) -> Self {
        if self.port == 0 {
            self.port = 8443;
        }
        self.quality = self.quality.clamp(30, 95);
        self.max_fps = self.max_fps.clamp(1, 60);
        self.session_hours = self.session_hours.clamp(1, 24 * 30);
        self.idle_minutes = self.idle_minutes.clamp(1, 24 * 60);
        self.lockout_after = self.lockout_after.clamp(3, 1000);
        // Up to 1 TiB; keeps `mib * 1024 * 1024` well within u64.
        self.max_image_mib = self.max_image_mib.clamp(1, 1 << 20);
        if !crate::keymap::LAYOUTS.contains(&self.layout.as_str()) {
            self.layout = "us".into();
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_is_the_defaults() {
        let parsed: Config = toml::from_str(TEMPLATE).unwrap();
        let d = Config::default();
        assert_eq!(
            (parsed.port, parsed.session_hours, parsed.idle_minutes, parsed.max_fps, parsed.quality),
            (d.port, d.session_hours, d.idle_minutes, d.max_fps, d.quality)
        );
        assert_eq!((parsed.layout, parsed.lockout_after, parsed.max_image_mib), (d.layout, d.lockout_after, d.max_image_mib));
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let c: Config = toml::from_str("quality = 5\nmax_fps = 500\nlayout = \"klingon\"\nport = 0\nmax_image_mib = 0").unwrap();
        let c = c.sanitize();
        assert_eq!((c.quality, c.max_fps, c.layout.as_str()), (30, 60, "us"));
        assert_eq!((c.port, c.max_image_mib), (8443, 1));
    }
}
