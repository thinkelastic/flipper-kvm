# Flipper One KVM

Use a computer from your browser through a Flipper One. The Flipper captures the
computer's screen, acts as its USB keyboard, mouse and USB drive, and serves all of it
over HTTPS, so you can reach a machine that has no network, no remote-desktop software,
or no working OS yet (BIOS, installers, a crashed system).

```
 computer HDMI out ──► HDMI-to-USB capture dongle ──► Flipper USB-A    (video)
 computer USB port ◄────── USB-C cable ──────────────► Flipper USB-C1  (keyboard, mouse, drive)
                                                       Flipper network ──► https://<flipper>:8443
```

## Features

- **Screen** from any UVC HDMI capture dongle. MJPEG passes straight through; YUYV is
  encoded on the Flipper. Shows *No signal* when the computer stops sending, and picks a
  dongle up again when it is replugged.
- **Low-latency video** over a WebSocket with per-frame flow control, so a slow link
  gets fewer frames rather than older ones; live fps, bandwidth and latency in the
  toolbar. An MJPEG stream remains as a fallback (and for VLC).
- **Keyboard and absolute mouse**: a click lands exactly where you clicked, whatever the
  target's mouse acceleration. Full-screen keyboard lock in Chrome passes Esc, Alt+Tab
  and the like through.
- **Key combinations** (Ctrl+Alt+Del, Alt+Tab, Win+R, consoles…) and **sticky Ctrl, Alt,
  Shift, Win** for touch screens and shortcuts the browser would take.
- **Paste text** typed one key at a time for a **US, UK, German, French or Spanish**
  target layout; any key or *Release* stops it.
- **Virtual USB drive**: upload an ISO or disk image in the browser and insert it as a
  **CD-ROM** (for installers, Windows included), a **USB stick**, or a read-only stick;
  create blank disks; download images back.
- **Phones and tablets**: tap to click, long-press to right-click, two fingers to
  scroll, and the phone's own keyboard.
- **The Flipper's screen** shows the pairing code, the address, what is connected and the
  certificate fingerprint. **Setup** installs the USB side from the panel.

## What you need

- A Flipper One (FlipperOS, firmware build 2484 or later).
- A UVC HDMI-to-USB capture dongle. The common MacroSilicon MS2109/MS2130 sticks work
  with Linux out of the box; MJPEG output at 1080p is ideal.
- A USB-C cable from the Flipper's **USB-C1** port to the computer.
- The Flipper on your network (Ethernet or Wi-Fi), and a current browser.

## Install

1. Download `flipper-kvm-aarch64.fap.AppImage` from the latest release (its `.sha256`
   is beside it) and copy it into `~/Apps/Network/` on the Flipper (over MTP, or
   `scp`), then `chmod 755` it. Once the app is in Flipper's catalogue it will also be
   under Apps › App Manager.
2. Open **Apps › Network › KVM**. The first time, the panel says the USB side is not set
   up: press **Setup**, then **Setup** again to confirm. This installs a small root
   helper (`/usr/local/sbin/flipper-kvm-gadget`), a systemd unit and a udev rule, using
   FlipperOS's passwordless sudo. USB networking drops for a moment.
3. Connect the dongle and the USB-C cable.

Without sudo, run `sudo gadget/install.sh` from this repository on the Flipper instead.

## Use

1. On the Flipper, open **KVM**. It shows a six-digit **code** and the address.
2. Browse to `https://<address>:8443` (plain `http://` is redirected).
3. Your browser warns about the certificate: the KVM makes its own. To be sure you are
   talking to your Flipper, compare the certificate's **SHA-256 fingerprint** in the
   browser with the one at the bottom of the Flipper's screen (scroll down), then accept.
4. Enter the code. Each code works once; press **Code** for a new one.
5. Click the picture to control the computer (the orange outline means your keys and
   mouse go to it). Click outside, or press *Release*, to let go.

Panel keys: **Close**, **Code** (new code), **Kick** (end every session), **Setup**
(when the USB side is missing), **Demo/Auto** (picture source).

With no dongle, the picture is a **demo screen** that draws what is typed and where the
pointer is, fed by the same USB reports: handy for checking input without a capture.

## Settings

`~/.config/flipper-kvm/config.toml` is written with the defaults on first run:

| Setting | Default | |
|---|---|---|
| `port` | 8443 | HTTPS port; plain HTTP on it is redirected |
| `session_hours` | 12 | a session ends this long after pairing |
| `idle_minutes` | 60 | …or after this long with nothing open |
| `max_fps` | 30 | frame-rate cap (also in the web Settings) |
| `quality` | 80 | JPEG quality for frames encoded on the Flipper |
| `layout` | us | default target layout for Paste: us, uk, de, fr, es |
| `lockout_after` | 10 | wrong codes per address in 10 minutes before a 10-minute lockout |
| `max_image_mib` | 16384 | largest drive image the browser may upload |

Images live in `~/.local/share/flipper-kvm/images/`, with the certificate and
`audit.log` (who paired, sessions, kicks, drive changes) beside them.

## Security

- HTTPS only, with a certificate made on the Flipper and kept; its fingerprint is on the
  panel so you can check it.
- Pairing needs the code on the Flipper's screen: six digits, single use, replaced after
  5 wrong tries, a delay on every wrong try, and a lockout per address.
- Sessions are `HttpOnly`, `Secure`, `SameSite=Strict` cookies held in memory: closing
  the app ends them all, as does **Kick**. They also expire (see Settings).
- Every state-changing request and both websockets must come from the KVM's own page
  (Origin checked); a strict Content-Security-Policy and the usual security headers are
  sent; the page cannot be framed.
- The only root part is the USB helper. The app itself runs as the Flipper's user.

See [SECURITY.md](SECURITY.md) for the threat model and how to report a problem.

## Troubleshooting

| Panel or toolbar says | Meaning |
|---|---|
| Keyboard: Not set up | Press **Setup** on the panel. |
| Keyboard: No computer | The USB-C cable is not in a computer, or the computer is off or asleep. |
| No signal | The dongle is there but the computer sends no picture (asleep, HDMI unplugged, wrong output). |
| No capture, demo | No dongle found; plug one into USB-A. It is picked up within seconds. |

- A **CD-ROM** and a **USB disk** are different devices to the computer, so switching an
  image between them replugs the KVM's USB for a second (keyboard and mouse included).
- Some BIOSes only see USB devices present at power-on: insert the image, then reboot.
- To remove the USB side: `sudo /usr/local/sbin/flipper-kvm-gadget uninstall`. To remove
  everything else: delete the AppImage, `~/.config/flipper-kvm` and
  `~/.local/share/flipper-kvm`.

## Status

Built and tested end to end in the Flipper One VM (QEMU) with the virtual USB port
`dummy_hcd` as the "computer" and `vivid` as the capture card: pairing, video, keyboard,
mouse, paste, CD-ROM and disk images, all verified on the receiving side. **Not yet tried
on a real Flipper One**: a real capture dongle, the gadget next to Flipper's own USB
network/storage functions on the dwc3 port, and Windows or macOS as the target. Reports
welcome.

Not there yet: hardware H.264 (the RK3576 has an encoder; MJPEG is used today), audio,
a USB-monitor mode without a dongle (GUD gadget; Linux hosts only), ATX power control.

## Developing

See [docs/DEVELOPING.md](docs/DEVELOPING.md): the code layout, building in the Flipper
One VM with `./dev.sh`, the tests (`./dev.sh test`, `tests/e2e.py`), and releasing.

## License

Source: MIT ([LICENSE](LICENSE)). The distributed binary links Slint (GPL-3.0-only
option, through Flipper's flipctl-app) and is GPL-3.0-only as a whole, like Flipper's
own apps; [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) lists every linked crate.
The JPEG encoder is based in part on the work of the Independent JPEG Group.
An independent project, not affiliated with Flipper Devices.
