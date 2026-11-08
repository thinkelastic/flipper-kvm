#!/bin/bash
# flipper-kvm-gadget {start|stop|restart|status|uninstall} - the USB devices flipper-kvm
# drives on the target computer.
#
# Adds three functions to Flipper One's USB gadget on USB-C1:
#   - a boot keyboard (8-byte reports, LED output),
#   - an absolute pointer (0..32767 on both axes, so a click lands where it was made in
#     the browser, whatever the host's mouse acceleration),
#   - a removable mass-storage drive, empty until the app loads a disk image into it.
# The KVM app runs as the desktop user and only writes reports and image paths, so this
# is the one part that needs root.
#
# One UDC can only be held by one gadget. If Flipper's own composite gadget ("flipper",
# from usb-gadget-setup.sh: NCM + storage + MTP) exists, these functions join it, which
# rebinds it once (a USB Ethernet blip). Otherwise a gadget of our own is made.
#
# Links in /run/flipper-kvm: keyboard and mouse (the /dev/hidg* report devices) and
# drive (the drive's LUN directory). All of it belongs to $KVM_USER.
#
# With no UDC at all (the Flipper One VM), dummy_hcd is loaded: a virtual device port
# whose host side is this same machine, so the KVM's keyboard, mouse and drive appear on
# the VM's own desktop. That is for testing; a real Flipper One has the dwc3 UDC.
set -euo pipefail

CFG=/sys/kernel/config/usb_gadget
SHARED=flipper
OWN=flipper-kvm
KBD=hid.kvm-kbd
MOUSE=hid.kvm-mouse
DRIVE=mass_storage.kvm
FUNCS=("$KBD" "$MOUSE" "$DRIVE")
RUN=/run/flipper-kvm
: "${KVM_USER:=user}"

# Boot keyboard: modifiers, reserved, 6 keys; 5 LED bits out. The standard descriptor.
KBD_DESC="05010906a101050719e029e71500250175019508810295017508810395057501050819012905910295017503910395067508150025650507190029658100c0"
# Absolute pointer: 5 buttons, X and Y 0..32767, vertical and horizontal wheel.
MOUSE_DESC="05010902a1010901a1000509190129051500250195057501810295017503810105010930093116000026ff7f75109502810209381581257f750895018106050c0a38021581257f750895018106c0c0"

hex() { local h=${1// /}; printf '%b' "$(sed 's/../\\x&/g' <<<"$h")"; }
log() { echo "flipper-kvm-gadget: $*" >&2; }

gadget() {
	if [ -d "$CFG/$SHARED" ] && [ ! -d "$CFG/$OWN" ]; then echo "$CFG/$SHARED"; else echo "$CFG/$OWN"; fi
}

pick_udc() {
	local udc
	udc=$(ls /sys/class/udc 2>/dev/null | grep -v vudc | head -n1 || true)
	if [ -z "$udc" ]; then
		log "no UDC; loading dummy_hcd (a loopback port, for the VM)"
		modprobe dummy_hcd 2>/dev/null || true
		udc=$(ls /sys/class/udc 2>/dev/null | head -n1 || true)
	fi
	[ -n "$udc" ] || { log "no USB device controller available"; exit 1; }
	echo "$udc"
}

make_own() {
	local g=$1
	mkdir -p "$g"
	echo 0x1d6b > "$g/idVendor"     # Linux Foundation
	echo 0x0104 > "$g/idProduct"    # Multifunction Composite Gadget
	echo 0x0100 > "$g/bcdDevice"
	echo 0x0200 > "$g/bcdUSB"
	# Several functions: the IAD composite class, so every host binds each one.
	echo 0xEF > "$g/bDeviceClass"
	echo 0x02 > "$g/bDeviceSubClass"
	echo 0x01 > "$g/bDeviceProtocol"
	mkdir -p "$g/strings/0x409"
	tr -d '\n' < /etc/machine-id | cut -c1-16 > "$g/strings/0x409/serialnumber"
	echo "Flipper Devices" > "$g/strings/0x409/manufacturer"
	echo "Flipper One KVM" > "$g/strings/0x409/product"
	mkdir -p "$g/configs/c.1/strings/0x409"
	echo "Keyboard, mouse and drive" > "$g/configs/c.1/strings/0x409/configuration"
	echo 100 > "$g/configs/c.1/MaxPower"
}

add_hid() {
	local g=$1 name=$2 protocol=$3 subclass=$4 len=$5 desc=$6
	[ -d "$g/functions/$name" ] && return 0
	mkdir "$g/functions/$name"
	echo "$protocol" > "$g/functions/$name/protocol"
	echo "$subclass" > "$g/functions/$name/subclass"
	echo "$len" > "$g/functions/$name/report_length"
	# configfs keeps only the last write(), and printf flushes at a 0x0a byte, so the
	# descriptor is built in a file and copied over in one write.
	local tmp; tmp=$(mktemp)
	hex "$desc" > "$tmp"
	cat "$tmp" > "$g/functions/$name/report_desc"
	rm -f "$tmp"
	[ "$(wc -c < "$g/functions/$name/report_desc")" -eq $(( ${#desc} / 2 )) ] ||
		{ log "$name: report descriptor did not take"; exit 1; }
	ln -s "$g/functions/$name" "$g/configs/c.1/"
}

add_drive() {
	local g=$1 l
	modprobe usb_f_mass_storage 2>/dev/null || true
	if [ ! -d "$g/functions/$DRIVE" ]; then
		mkdir "$g/functions/$DRIVE"
		l=$g/functions/$DRIVE/lun.0
		echo 1 > "$l/removable"
		echo "Flipper KVM Drive" > "$l/inquiry_string"
		ln -s "$g/functions/$DRIVE" "$g/configs/c.1/"
	fi
	l=$g/functions/$DRIVE/lun.0
	# The app loads and ejects images itself.
	chown "$KVM_USER:" "$l/file" "$l/ro" "$l/cdrom"
	[ -e "$l/forced_eject" ] && chown "$KVM_USER:" "$l/forced_eject"
	ln -sfn "$l" "$RUN/drive"
}

# /dev/hidgN for a function, by the major:minor it was given.
devnode() {
	local want n
	want=$(cat "$1/functions/$2/dev")
	for n in /sys/class/hidg/hidg*; do
		[ "$(cat "$n/dev")" = "$want" ] && { echo "/dev/$(basename "$n")"; return; }
	done
}

start() {
	modprobe libcomposite
	local g udc was
	g=$(gadget)
	[ -d "$g" ] || make_own "$g"
	mkdir -p "$RUN"
	was=$(cat "$g/UDC" 2>/dev/null || true)
	[ -n "$was" ] && echo "" > "$g/UDC"          # functions can only be added unbound
	add_hid "$g" "$KBD" 1 1 8 "$KBD_DESC"
	add_hid "$g" "$MOUSE" 0 0 7 "$MOUSE_DESC"
	add_drive "$g"
	udc=${was:-$(pick_udc)}
	echo "$udc" > "$g/UDC"
	udevadm settle 2>/dev/null || sleep 0.5
	local k m
	k=$(devnode "$g" "$KBD"); m=$(devnode "$g" "$MOUSE")
	chown "$KVM_USER:" "$k" "$m"; chmod 0600 "$k" "$m"
	chown "$KVM_USER:" "$RUN"
	ln -sfn "$k" "$RUN/keyboard"; ln -sfn "$m" "$RUN/mouse"
	# A host reads whether the drive is a CD-ROM or a disk only when the device is
	# plugged in, so switching between the two needs a re-plug: the app may rebind.
	chown "$KVM_USER:" "$g/UDC"
	ln -sfn "$g/UDC" "$RUN/udc"
	echo "$udc" > "$RUN/udc-name"
	log "keyboard $k, mouse $m, drive on $(basename "$g") via $udc"
}

stop() {
	local g udc f any=0
	g=$(gadget)
	rm -f "$RUN/keyboard" "$RUN/mouse" "$RUN/drive" "$RUN/udc" "$RUN/udc-name"
	for f in "${FUNCS[@]}"; do [ -d "$g/functions/$f" ] && any=1; done
	[ "$any" = 1 ] || return 0
	udc=$(cat "$g/UDC" 2>/dev/null || true)
	[ -n "$udc" ] && echo "" > "$g/UDC"
	[ -e "$g/functions/$DRIVE/lun.0/file" ] && echo "" > "$g/functions/$DRIVE/lun.0/file" 2>/dev/null || true
	for f in "${FUNCS[@]}"; do
		rm -f "$g/configs/c.1/$f"
		[ -d "$g/functions/$f" ] && rmdir "$g/functions/$f"
	done
	if [ "$(basename "$g")" = "$OWN" ]; then
		rmdir "$g/configs/c.1/strings/0x409" "$g/configs/c.1" "$g/strings/0x409" "$g" 2>/dev/null || true
	elif [ -n "$udc" ]; then
		echo "$udc" > "$g/UDC"                   # give Flipper's gadget its port back
	fi
	log "removed"
}

status() {
	local g; g=$(gadget)
	if [ -e "$RUN/keyboard" ] && [ -e "$RUN/mouse" ]; then
		echo "up: $(basename "$g") on $(cat "$g/UDC" 2>/dev/null), keyboard $(readlink "$RUN/keyboard"), mouse $(readlink "$RUN/mouse"), drive $([ -e "$RUN/drive" ] && echo yes || echo no)"
	else
		echo "down"; return 3
	fi
}

uninstall() {
	systemctl disable --now flipper-kvm-gadget.service 2>/dev/null || stop
	rm -f /etc/systemd/system/flipper-kvm-gadget.service /etc/udev/rules.d/70-flipper-kvm.rules
	systemctl daemon-reload
	udevadm control --reload 2>/dev/null || true
	rm -f /usr/local/sbin/flipper-kvm-gadget
	log "uninstalled"
}

case "${1:-}" in
	start) start ;;
	stop) stop ;;
	restart) stop; start ;;
	status) status ;;
	uninstall) uninstall ;;
	*) echo "usage: $0 {start|stop|restart|status|uninstall}" >&2; exit 2 ;;
esac
