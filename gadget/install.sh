#!/bin/bash
# Install (or remove) the KVM's USB keyboard, mouse and drive on a Flipper One. As root:
#   sudo gadget/install.sh            install, enable at boot, start now
#   sudo gadget/install.sh --remove   stop, disable, remove
# KVM_USER (default: the user who ran sudo, else "user") gets the devices.
# The app's panel Setup key runs this same script through sudo.
set -euo pipefail
cd "$(dirname "$0")"
[ "$(id -u)" = 0 ] || { echo "run as root: sudo $0 $*" >&2; exit 1; }
USER_NAME=${KVM_USER:-${SUDO_USER:-user}}
id "$USER_NAME" >/dev/null 2>&1 || { echo "no such user: $USER_NAME" >&2; exit 1; }
RULES=/etc/udev/rules.d/70-flipper-kvm.rules

if [ "${1:-}" = --remove ]; then
	systemctl disable --now flipper-kvm-gadget.service 2>/dev/null || true
	rm -f /etc/systemd/system/flipper-kvm-gadget.service /usr/local/sbin/flipper-kvm-gadget "$RULES"
	systemctl daemon-reload
	udevadm control --reload 2>/dev/null || true
	echo "removed"
	exit 0
fi

# The kernel makes the /dev/hidg* report devices anew each time the gadget is bound, which
# happens again when the app switches the drive between CD-ROM and disk: owned by root
# unless a rule says otherwise.
echo "SUBSYSTEM==\"hidg\", KERNEL==\"hidg[0-9]*\", OWNER=\"$USER_NAME\", MODE=\"0600\"" > "$RULES"
udevadm control --reload 2>/dev/null || true

install -m 0755 flipper-kvm-gadget.sh /usr/local/sbin/flipper-kvm-gadget
sed "s/^Environment=KVM_USER=.*/Environment=KVM_USER=$USER_NAME/" flipper-kvm-gadget.service \
	> /etc/systemd/system/flipper-kvm-gadget.service
systemctl daemon-reload
systemctl enable flipper-kvm-gadget.service >/dev/null 2>&1
systemctl restart flipper-kvm-gadget.service
/usr/local/sbin/flipper-kvm-gadget status
