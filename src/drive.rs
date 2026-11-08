//! The virtual USB drive: disk images on the Flipper shown to the target as a USB stick
//! or a CD-ROM, through a mass-storage function in the KVM's USB gadget.
//!
//! The gadget script links the function's LUN at /run/flipper-kvm/drive and gives its
//! `file`, `ro` and `cdrom` attributes to the app's user, so mounting needs no root:
//! writing a path to `file` loads it, an empty line ejects it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub const LUN: &str = "/run/flipper-kvm/drive";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A read-only CD/DVD: for installer ISOs.
    Cdrom,
    /// A writable USB stick.
    Disk,
    /// A read-only USB stick.
    DiskReadOnly,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "cdrom" => Some(Mode::Cdrom),
            "disk" => Some(Mode::Disk),
            "disk-ro" => Some(Mode::DiskReadOnly),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Cdrom => "cdrom",
            Mode::Disk => "disk",
            Mode::DiskReadOnly => "disk-ro",
        }
    }
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Image {
    pub name: String,
    pub size: u64,
}

pub fn images_dir() -> PathBuf {
    crate::config::data_dir().join("images")
}

/// Whether the gadget has a drive to load images into.
pub fn available() -> bool {
    Path::new(LUN).join("file").exists()
}

/// A file name a browser may use: letters, digits, space, dot, dash, underscore,
/// parentheses; no leading dot; at most 128 characters. Never a path.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || " ._-()".contains(c))
}

pub fn path_of(name: &str) -> Option<PathBuf> {
    valid_name(name).then(|| images_dir().join(name))
}

pub fn list() -> Vec<Image> {
    let mut images: Vec<Image> = fs::read_dir(images_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let meta = e.metadata().ok()?;
            (meta.is_file() && valid_name(&name) && !name.ends_with(".part")).then_some(Image { name, size: meta.len() })
        })
        .collect();
    images.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    images
}

/// Free space where images are kept, in bytes.
pub fn free_bytes() -> u64 {
    let dir = images_dir();
    let _ = fs::create_dir_all(&dir);
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()) else { return 0 };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return 0;
    }
    st.f_bavail as u64 * st.f_frsize as u64
}

fn write_attr(name: &str, value: &str) -> io::Result<()> {
    fs::write(Path::new(LUN).join(name), format!("{value}\n"))
}

/// What is loaded now, from the LUN itself: (image name, mode).
pub fn mounted() -> Option<(String, String)> {
    let file = fs::read_to_string(Path::new(LUN).join("file")).ok()?;
    let file = file.trim();
    if file.is_empty() {
        return None;
    }
    let name = Path::new(file).file_name()?.to_string_lossy().into_owned();
    let read = |a: &str| fs::read_to_string(Path::new(LUN).join(a)).map(|s| s.trim() == "1").unwrap_or(false);
    let mode = if read("cdrom") { Mode::Cdrom } else if read("ro") { Mode::DiskReadOnly } else { Mode::Disk };
    Some((name, mode.name().to_string()))
}

/// Take the medium out. The host sees the drive empty, as if a disc were ejected.
/// Hosts lock a CD-ROM's "door" while they use it, which refuses a plain eject with
/// EBUSY; the forced eject the kernel offers for that case is used then.
pub fn eject() -> io::Result<()> {
    match write_attr("file", "") {
        Err(e) if e.raw_os_error() == Some(libc::EBUSY) && Path::new(LUN).join("forced_eject").exists() => {
            write_attr("forced_eject", "1")
        }
        other => other,
    }
}

/// Load `name` as `mode`, ejecting whatever was there.
pub fn mount(name: &str, mode: Mode) -> io::Result<()> {
    let path = path_of(name).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad image name"))?;
    if !path.is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no such image"));
    }
    if !available() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "the USB gadget has no drive; set it up from the panel"));
    }
    let was_cdrom = fs::read_to_string(Path::new(LUN).join("cdrom")).map(|s| s.trim() == "1").unwrap_or(false);
    eject()?;
    // cdrom and ro can only change with no medium loaded.
    write_attr("cdrom", if mode == Mode::Cdrom { "1" } else { "0" })?;
    write_attr("ro", if mode == Mode::Disk { "0" } else { "1" })?;
    write_attr("file", &path.to_string_lossy())?;
    if was_cdrom != (mode == Mode::Cdrom) {
        replug()?;
    }
    Ok(())
}

/// Unplug and plug the USB device again, so the host reads the drive's type (CD-ROM or
/// disk) afresh. The keyboard and mouse drop out for the second this takes.
pub fn replug() -> io::Result<()> {
    let name = fs::read_to_string("/run/flipper-kvm/udc-name")?;
    let name = name.trim();
    if name.is_empty() {
        return Ok(());
    }
    fs::write("/run/flipper-kvm/udc", "\n")?;
    std::thread::sleep(std::time::Duration::from_millis(400));
    fs::write("/run/flipper-kvm/udc", format!("{name}\n"))
}

/// A new empty disk image of `mib` MiB (sparse), for the target to format and fill.
pub fn create_blank(name: &str, mib: u64) -> io::Result<()> {
    let path = path_of(name).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad image name"))?;
    if path.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "an image with that name exists"));
    }
    fs::create_dir_all(images_dir())?;
    let f = fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
    f.set_len(mib * 1024 * 1024)
}

pub fn delete(name: &str) -> io::Result<()> {
    let path = path_of(name).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad image name"))?;
    if mounted().is_some_and(|(m, _)| m == name) {
        return Err(io::Error::new(io::ErrorKind::ResourceBusy, "eject it first"));
    }
    fs::remove_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_plain_file_names() {
        for ok in ["debian-13.iso", "Win 11 (x64).iso", "disk_1.img"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", ".hidden", "../etc/passwd", "a/b.iso", "x\0.iso", "café.iso", &"a".repeat(129)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn modes_round_trip() {
        for m in [Mode::Cdrom, Mode::Disk, Mode::DiskReadOnly] {
            assert_eq!(Mode::parse(m.name()), Some(m));
        }
        assert_eq!(Mode::parse("floppy"), None);
    }
}
