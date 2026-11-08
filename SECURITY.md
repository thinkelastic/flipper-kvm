# Security

A KVM is full control of a computer, keyboard and boot media included, so the bar is
the same as for remote administration tools.

## Threat model

**Protected against**

- *Someone on the same network* reaching the web page: they need the code on the
  Flipper's screen, which is six digits, single use, replaced after 5 wrong tries, slowed
  by a delay per try, and locked out per address after `lockout_after` wrong tries.
- *Eavesdropping* on the LAN: HTTPS only. Plain HTTP is answered with a redirect and
  nothing else.
- *Man in the middle* on first contact: the certificate is self-signed, so the browser
  cannot vouch for it; the panel shows its SHA-256 fingerprint to compare with the
  browser's. Without that check, a first connection can be intercepted.
- *Other websites* the operator visits: state-changing requests and both websockets must
  carry this server's Origin; cookies are `SameSite=Strict`; a strict CSP and
  `frame-ancestors 'none'` stop injection and framing.
- *Forgotten sessions*: sessions expire (`session_hours`, `idle_minutes`), end when the app
  closes, and **Kick** on the panel ends them all at once.
- *Path tricks* on the drive API: image names are plain file names only (no `/`, no
  leading dot, a fixed character set).

**Not protected against**

- Anyone who can hold the Flipper: the code is on its screen, and the device user has
  passwordless sudo (a FlipperOS default).
- A compromised browser or operator machine.
- The target computer itself: it sees a keyboard, a mouse and a drive, like any KVM.

## What runs as root

Only `/usr/local/sbin/flipper-kvm-gadget` (from `gadget/`), at boot through
`flipper-kvm-gadget.service`, to create the USB functions and hand their device files to
the Flipper's user, plus a udev rule for the same. The app runs as that user. Setup on
the panel installs these through `sudo`.

## Records

`~/.local/share/flipper-kvm/audit.log` records pairing (with the address), wrong codes,
lockouts, logouts, expiries, kicks, setting changes and drive changes, with UTC
timestamps. The last lines are also under Settings › Activity in the web page.

## Reporting

Please report vulnerabilities privately (a GitHub security advisory on this repository)
rather than in a public issue, with steps to reproduce. Expect an answer within a week.
