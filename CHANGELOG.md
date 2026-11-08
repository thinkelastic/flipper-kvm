# Changelog

## 0.1.0 — 2026-11-07

First release. Built and tested in the Flipper One VM; not yet tried on a real Flipper
One (see *Status* in the README).

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

Found in the pre-release review and fixed before tagging:

- Closing the app now lets go of every key and mouse button on the target first.
- A capture device that is present but fails to open or stream shows *Capture error*
  (panel, toolbar and picture) and is retried every 2 s, instead of retrying silently
  with a stale status.
- When the HTTPS port cannot be bound, the panel says so instead of the app quitting.
- Paste: Escape no longer types the text after an earlier *Type it*.
- A mouse drag released outside the picture now releases the button on the target.
- The TLS private key is created with mode 0600 (no world-readable window).
- Uploads: two uploads of one name can no longer write into the same file, and `.part`
  leftovers from interrupted uploads are removed at start.
- `port = 0` and out-of-range `max_image_mib` in the settings file are corrected.
