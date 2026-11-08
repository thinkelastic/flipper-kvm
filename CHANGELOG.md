# Changelog

v0.1:
First release (2026-11-07). Built and tested in the Flipper One VM; not yet tried on a
real Flipper One (see *Status* in the README).

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
- Settings file at ~/.config/flipper-kvm/config.toml.
- Found in the pre-release review and fixed before tagging: closing the app lets go of
  every key and mouse button on the target; a capture device that is present but fails
  shows *Capture error* and is retried every 2 s; a port that cannot be bound is shown
  on the panel instead of the app quitting; Paste no longer types after Escape; a mouse
  drag released outside the picture releases the button; the TLS key is created with
  mode 0600; duplicate and interrupted uploads are handled; port 0 and out-of-range
  image limits in the settings file are corrected.
