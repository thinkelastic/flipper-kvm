# Developing Flipper One KVM

## Layout

| Path | What |
|---|---|
| `src/main.rs` | The panel page (flipctl-app) and startup; `--headless` runs without the panel |
| `src/server.rs` | axum over rustls: pairing, sessions, video (websocket and MJPEG), input, settings, drive API, security middleware |
| `src/tls.rs` | Certificate, fingerprint, plain-HTTP redirect on the TLS port |
| `src/state.rs` | Code, sessions, lockouts, status, shared by the threads |
| `src/video.rs` | V4L2 capture with a poll() watchdog, demo fallback, hot-plug, size/fps/quality |
| `src/mjpeg.rs` | Adds the Huffman tables UVC frames leave out; JPEG encoding; YUYV conversion |
| `src/hid.rs`, `src/keymap.rs` | Browser events to keyboard/mouse reports; cancellable typing; target layouts |
| `src/drive.rs` | Virtual drive: images, mount/eject, CD-ROM/disk replug |
| `src/setup.rs` | The panel's Setup: runs `gadget/install.sh` through sudo |
| `src/config.rs`, `src/audit.rs`, `src/demo.rs` | Settings file, audit log, demo screen |
| `web/` | The page, embedded in the binary |
| `gadget/` | Root helper, systemd unit and installer for the USB functions |
| `tests/e2e.py` | End-to-end checks against a running KVM |

## Building

Everything builds on an arm64 Linux machine; the Flipper One VM
(`../flipper-one-vm`, QEMU with HVF on Apple Silicon) is the convenient one. `./dev.sh`
syncs this folder there and runs the build, with flipctl-slint cloned at the revision the
firmware ships so Flipper's bundler can make the `.fap.AppImage`:

```
./dev.sh test        # unit and router tests
./dev.sh run         # bundle, install into ~/Apps/Network, open on the panel
./dev.sh headless    # the server alone; the code is printed
./dev.sh gadget      # install the USB side by hand (the panel's Setup does the same)
./dev.sh licenses    # regenerate THIRD-PARTY-LICENSES.md (cargo-about)
./dev.sh e2e CODE    # tests/e2e.py against the device, with the code on its screen
```

`FLIPPER_HOST`/`FLIPPER_PORT` point it at a real Flipper instead of the VM.

Two build details matter, because the bundler only accepts binaries that link libc,
libm and libgcc_s:

- Use the rustup toolchain, not Debian's `rustc`: Debian's `libcompiler_builtins` is
  built with the stack protector, whose arm64 canary lives in the dynamic loader.
- Build ring's C with `CFLAGS=-fno-stack-protector`, for the same reason.

`dev.sh` does both.

### Testing in the VM

- The gadget script loads `dummy_hcd` when there is no UDC, so the KVM's keyboard, mouse
  and drive appear on the VM's own desktop: type into a terminal there and check
  `lsblk` for the drive.
- `sudo modprobe vivid` adds a virtual capture card (YUYV) for the V4L2 path.
- The VM forwards port 8443 to the Mac (`run.sh`), so the page is
  `https://localhost:8443`.

## Releasing

1. Bump `version` in `Cargo.toml`, add the changes to `CHANGELOG.md`.
2. `./dev.sh licenses` if dependencies changed; commit `Cargo.lock` and
   `THIRD-PARTY-LICENSES.md`.
3. Tag `vX.Y.Z` and push: CI (`.github/workflows/ci.yml`) tests, bundles on an arm64
   runner, and attaches the AppImage and its SHA-256 to a GitHub release.

### App Manager catalogue

Flipper's App Manager installs from `apps.toml` on the `dev` branch of
`flipperdevices/flipctl-apps`, with the files as release assets on its `apps` tag. To
propose KVM there:

1. Build the bundle (CI artifact or `./dev.sh bundle`).
2. In a flipctl-slint checkout with the app linked under `apps/`, run
   `tools/appimage/stage.sh /tmp/stage`; it writes the `apps.toml` entry (name, path
   `Network/flipper-kvm-aarch64.fap.AppImage`, size, sha256).
3. Open a pull request on `flipperdevices/flipctl-apps` with that entry, linking the
   release asset and this repository. Their maintainers upload assets to the `apps` tag.
