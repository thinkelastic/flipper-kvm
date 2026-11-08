//! Installing the USB gadget from the panel.
//!
//! The gadget (keyboard, mouse and drive on USB-C1) needs root once: a script in
//! /usr/local/sbin and a systemd unit that runs it at boot. FlipperOS lets the device
//! user run sudo without a password, which flipctl itself relies on, so the app can do
//! this when the owner presses Setup on the panel. The files are carried in the binary.

use std::io::Write;
use std::process::{Command, Stdio};

const SCRIPT: &str = include_str!("../gadget/flipper-kvm-gadget.sh");
const UNIT: &str = include_str!("../gadget/flipper-kvm-gadget.service");
const INSTALL: &str = include_str!("../gadget/install.sh");

/// Whether sudo works without a password here.
pub fn can_sudo() -> bool {
    Command::new("sudo").args(["-n", "true"]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Write the gadget files to a private temporary folder and run the installer there as
/// root. Blocks for the few seconds the install and the gadget's first start take.
pub fn install() -> Result<(), String> {
    if !can_sudo() {
        return Err("sudo needs a password here; run gadget/install.sh by hand".into());
    }
    let dir = std::env::temp_dir().join(format!("flipper-kvm-setup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    for (name, body) in [("flipper-kvm-gadget.sh", SCRIPT), ("flipper-kvm-gadget.service", UNIT), ("install.sh", INSTALL)] {
        let mut f = std::fs::File::create(dir.join(name)).map_err(|e| e.to_string())?;
        f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    }
    let user = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_else(|_| "user".into());
    let out = Command::new("sudo")
        .args(["-n", "env", &format!("KVM_USER={user}"), "bash"])
        .arg(dir.join("install.sh"))
        .output()
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_dir_all(&dir);
    let out = out?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().last().unwrap_or("install failed").to_string())
    }
}
