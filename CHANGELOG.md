# Changelog

## 0.1.0 (unreleased)

First release.

- Capture from UVC HDMI dongles (MJPEG pass-through with Huffman tables restored, YUYV
  encoded), with a no-signal watchdog, hot-plug and a demo screen.
- Video over a flow-controlled WebSocket with live stats; MJPEG fallback. Frame rate,
  quality and capture size from the browser.
- USB keyboard and absolute mouse; key combinations; sticky modifiers; Paste text for
  US, UK, DE, FR, ES targets, cancellable.
- Virtual USB drive: upload, download, create; insert as CD-ROM, USB disk or read-only
  disk.
- Touch and phone keyboard support.
- Pairing by single-use code on the panel; HTTPS with fingerprint on the panel; HTTP
  redirect; sessions with expiry; per-address lockout; Origin checks; CSP and security
  headers; audit log.
- Panel page with Setup (installs the USB gadget), new code, kick, source.
- Settings file at `~/.config/flipper-kvm/config.toml`.
