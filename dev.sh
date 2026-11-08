#!/bin/bash
# dev.sh - build and run flipper-kvm on a Flipper One (or the Flipper One VM) from here.
#
#   ./dev.sh sync       copy the source to ~/src/flipper-kvm on the device
#   ./dev.sh build      sync, then cargo build --release on the device
#   ./dev.sh test       sync, then cargo test on the device
#   ./dev.sh bundle     build the .fap.AppImage with flipctl-slint's bundler (native, on the device)
#   ./dev.sh run        bundle, install into ~/Apps/Network and open it on the panel
#   ./dev.sh headless   build and run the server without the panel UI (Ctrl-C stops)
#   ./dev.sh gadget     install the USB keyboard, mouse and drive (what the panel's Setup does)
#   ./dev.sh licenses   regenerate THIRD-PARTY-LICENSES.md with cargo-about
#   ./dev.sh e2e CODE   run tests/e2e.py against the device with the code on its screen
#   ./dev.sh ssh [cmd]  shell on the device
#
# The build happens on the device: it is arm64 like the target, and flipctl-slint is
# cloned there at the revision the firmware ships (~/src/flipctl-slint).
#
# Env: FLIPPER_HOST (127.0.0.1 = the VM), FLIPPER_PORT (2222), FLIPPER_PASS (user)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
HOST=${FLIPPER_HOST:-127.0.0.1}
PORT=${FLIPPER_PORT:-2222}
FLIPCTL_REV=99d8ef816a190026a9ff8eec95f86fa95228d238

ASKPASS="$ROOT/.askpass"
printf '#!/bin/sh\necho "%s"\n' "${FLIPPER_PASS:-user}" > "$ASKPASS"; chmod 700 "$ASKPASS"
export SSH_ASKPASS="$ASKPASS" SSH_ASKPASS_REQUIRE=force
SSHO=(-p "$PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR
      -o ControlMaster=auto -o ControlPath="/tmp/fkvm-%C" -o ControlPersist=120)
dev() { ssh "${SSHO[@]}" "user@$HOST" "$@"; }
SUDO='echo "${FLIPPER_PASS:-user}" | sudo -S -p ""'

sync() {
	COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata -C "$ROOT" \
		--exclude target --exclude .git --exclude '.ssh-*' --exclude .askpass -cf - . |
		dev "set -e; mkdir -p ~/src/flipper-kvm && tar -xf - -C ~/src/flipper-kvm
		     if [ ! -d ~/src/flipctl-slint ]; then
		         git clone -q https://github.com/flipperdevices/flipctl-slint ~/src/flipctl-slint
		         git -C ~/src/flipctl-slint checkout -q $FLIPCTL_REV
		     fi
		     ln -sfn ~/src/flipper-kvm ~/src/flipctl-slint/apps/flipper-kvm"
}

# Cargo.lock is made on the device; keep the copy here in step so it can be committed.
pull_lock() { dev "cat ~/src/flipper-kvm/Cargo.lock" > "$ROOT/Cargo.lock.new" && mv "$ROOT/Cargo.lock.new" "$ROOT/Cargo.lock"; }

# A framework app may link only libc, libm and libgcc_s; the bundler refuses anything
# else. Debian's gcc adds -fstack-protector by default, and on arm64 the canary it reads,
# __stack_chk_guard, lives in ld-linux-aarch64.so.1, which would then be needed by name.
# Two things bring it in, and both are avoided here:
#  - Debian's packaged rustc, whose libcompiler_builtins has C built that way: so the
#    official rustup toolchain in ~/.cargo/bin is used (installed if missing).
#  - ring's own C (TLS): built with -fno-stack-protector.
# Same environment for every build, so nothing rebuilds needlessly.
ENV='[ -x ~/.cargo/bin/rustup ] || { curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal --no-modify-path >/dev/null; };
     export PATH=~/.cargo/bin:$PATH CFLAGS="-O2 -fno-stack-protector";'

build() { sync; dev "$ENV cd ~/src/flipper-kvm && cargo build --release 2>&1 | grep -vE '^\s+(Compiling|Downloaded|Downloading)' "; pull_lock; }

bundle() {
	build
	dev "$ENV cd ~/src/flipctl-slint && tools/appimage/build.sh --native apps/flipper-kvm 2>&1 | grep -E '^==|error|Error|links more|^lib|^ld' || true
	     ls -l target/appimage/flipper-kvm-aarch64.fap.AppImage"
}

cmd=${1:-help}; [ $# -gt 0 ] && shift
case "$cmd" in
	sync) sync ;;
	build) build ;;
	test) sync; dev "$ENV cd ~/src/flipper-kvm && cargo test --release $* 2>&1 | grep -vE '^\s+(Compiling|Downloaded|Downloading)'"; pull_lock ;;
	bundle) bundle ;;
	run)
		bundle
		dev "set -e; mkdir -p ~/Apps/Network
		     cp ~/src/flipctl-slint/target/appimage/flipper-kvm-aarch64.fap.AppImage ~/Apps/Network/
		     chmod 755 ~/Apps/Network/flipper-kvm-aarch64.fap.AppImage
		     XDG_RUNTIME_DIR=/run/user/\$(id -u) flipctl open ~/Apps/Network/flipper-kvm-aarch64.fap.AppImage"
		echo "open https://localhost:8443/ (VM) and enter the code shown on the panel" ;;
	headless) build; dev -t "$ENV cd ~/src/flipper-kvm && ./target/release/flipper-kvm --headless $*" ;;
	gadget) sync; dev -t "cd ~/src/flipper-kvm && $SUDO ./gadget/install.sh $*" ;;
	licenses)
		sync
		dev "$ENV command -v cargo-about >/dev/null || cargo install -q cargo-about --features cli --locked
		     cd ~/src/flipper-kvm && cargo about generate --fail about.hbs -o THIRD-PARTY-LICENSES.md"
		dev "cat ~/src/flipper-kvm/THIRD-PARTY-LICENSES.md" > "$ROOT/THIRD-PARTY-LICENSES.md"
		echo "THIRD-PARTY-LICENSES.md: $(wc -l < "$ROOT/THIRD-PARTY-LICENSES.md") lines" ;;
	e2e)
		# The VM forwards 8443 to localhost; a real Flipper is reached at its own address.
		host=$([ "$HOST" = 127.0.0.1 ] && echo localhost || echo "$HOST")
		python3 -I "$ROOT/tests/e2e.py" --host "$host" --code "${1:?usage: ./dev.sh e2e CODE}" ;;
	ssh) dev -t "$@" ;;
	*) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//' ;;
esac
