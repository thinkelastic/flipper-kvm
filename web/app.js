"use strict";
// Flipper One KVM, the browser side. Pair with the code on the panel, draw the target's
// screen, send keyboard, mouse and touch input over a websocket (src/hid.rs has the
// messages), and manage video settings, sessions and the virtual drive.

const $ = (s, el = document) => el.querySelector(s);
const $$ = (s, el = document) => [...el.querySelectorAll(s)];
const stage = $("#stage"), canvas = $("#screen"), ctx = canvas.getContext("2d"), mjpeg = $("#mjpeg"), hint = $("#hint");
const touchDevice = matchMedia("(pointer: coarse)").matches;
const pref = {
  get: (k, d) => { try { return localStorage.getItem("kvm." + k) ?? d; } catch (_) { return d; } },
  set: (k, v) => { try { localStorage.setItem("kvm." + k, v); } catch (_) {} },
};

let ws = null, vws = null, statusTimer = null, statsTimer = null, controlling = false, active = false;
let transport = pref.get("transport", "ws");
let state = null;
const held = new Set(); // sticky modifiers

async function api(path, body, method) {
  const res = await fetch(path, {
    method: method || (body === undefined ? "GET" : "POST"),
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    credentials: "same-origin",
  });
  let data = {};
  try { data = await res.json(); } catch (_) {}
  if (res.status === 401 && active) showPair("Your session ended. Enter the new code from the Flipper.");
  return { ok: res.ok, status: res.status, data };
}

function fmtBytes(n) {
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i ? 1 : 0)} ${u[i]}`;
}
function fmtSpan(secs) {
  if (secs < 60) return `${Math.round(secs)} s`;
  if (secs < 3600) return `${Math.round(secs / 60)} min`;
  const h = Math.floor(secs / 3600), m = Math.round((secs % 3600) / 60);
  return m ? `${h} h ${m} min` : `${h} h`;
}
function el(tag, props, ...children) {
  const e = Object.assign(document.createElement(tag), props || {});
  for (const c of children) if (c != null) e.append(c);
  return e;
}
function icon(path) {
  const s = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  s.setAttribute("viewBox", "0 0 24 24");
  s.setAttribute("aria-hidden", "true");
  const p = document.createElementNS("http://www.w3.org/2000/svg", "path");
  p.setAttribute("d", path);
  s.append(p);
  return s;
}
const ICON_DOWN = "M12 3v12M7 10l5 5 5-5M4 19h16", ICON_TRASH = "M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3";

// ---- Pairing ------------------------------------------------------------

$("#pairhost").textContent = location.host;
$("#lcdhost").textContent = location.host;
$("#devhost").textContent = location.host;

function showPair(message) {
  stopKvm();
  $("#kvm").hidden = true;
  $("#pair").hidden = false;
  $("#pairerr").textContent = message || "";
  $("#code").value = "";
  $("#code").focus();
}

$("#pairform").addEventListener("submit", async (e) => {
  e.preventDefault();
  const btn = $("#pairform button");
  btn.disabled = true;
  $("#pairerr").textContent = "";
  let r;
  try {
    r = await api("/api/pair", { code: $("#code").value });
  } catch (_) {
    r = { ok: false, data: { error: "The Flipper did not answer. Is it on the network?" } };
  }
  btn.disabled = false;
  if (r.ok) showKvm();
  else {
    $("#pairerr").textContent = r.data.error || `Could not pair (HTTP ${r.status}).`;
    $("#code").select();
  }
});

// ---- The KVM view ---------------------------------------------------------

function showKvm() {
  active = true;
  $("#pair").hidden = true;
  $("#kvm").hidden = false;
  startVideo();
  connect();
  refresh();
  statusTimer = setInterval(refresh, 2000);
  statsTimer = setInterval(stats, 1000);
}

function stopKvm() {
  active = false;
  clearInterval(statusTimer);
  clearInterval(statsTimer);
  stopVideo();
  if (ws) { ws.onclose = null; ws.close(); ws = null; }
  setControlling(false);
  closeSheets();
  $("#keyspop").hidden = true;
  $("#keysbtn").setAttribute("aria-expanded", "false");
}

async function refresh() {
  const r = await api("/api/state");
  if (!r.ok) return;
  state = r.data;
  const s = state;
  chip("#video", s.video.label, !s.video.signal ? "bad" : s.video.device === "demo" ? "warn" : "ok");
  chip("#hid", s.hid.ready ? "USB ready" : "USB: " + s.hid.label, s.hid.ready ? "ok" : "warn");
  const left = s.session && s.session.expires_in;
  $("#expires").textContent = left ? `Session ends in ${fmtSpan(left)}` : "";
  const nosig = !s.video.signal && s.source !== "demo";
  $("#nosignal").hidden = !nosig;
  if (nosig) $("#nosigwhy").textContent = `${s.video.device} is connected but the computer is not sending a picture. Asleep, HDMI unplugged, or the wrong output?`;
  if (!$("#settingssheet").hidden) fillSettings();
}

function chip(sel, text, kind) {
  const e = $(sel);
  e.textContent = text;
  e.className = "chip " + kind;
}

function connect() {
  ws = new WebSocket(`wss://${location.host}/ws`);
  chip("#link", "Connecting", "warn");
  $("#devdot").className = "dot";
  ws.onopen = () => {
    chip("#link", "Input on", "ok");
    $("#devdot").className = "dot ok";
    for (const m of held) send({ t: "key", code: m, down: true });
  };
  ws.onclose = () => {
    chip("#link", "Reconnecting", "bad");
    $("#devdot").className = "dot bad";
    setTimeout(async () => {
      if (!active) return;
      const r = await api("/api/state");
      if (r.ok) connect();
    }, 1500);
  };
}

function send(msg) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
}

// ---- Video ----------------------------------------------------------------

let frames = 0, bytes = 0, age = 0, rtt = 0, lastStats = performance.now(), videoFailures = 0;
const fpsHist = [];

function setAspect(w, h) {
  if (w > 0 && h > 0) { stage.style.setProperty("--w", w); stage.style.setProperty("--h", h); }
}

function startVideo() {
  stopVideo();
  $("#hud").hidden = false;
  if (transport === "mjpeg") {
    canvas.hidden = true;
    mjpeg.hidden = false;
    mjpeg.src = "/stream?" + Date.now();
    return;
  }
  mjpeg.hidden = true;
  canvas.hidden = false;
  vws = new WebSocket(`wss://${location.host}/ws/video`);
  vws.binaryType = "arraybuffer";
  let opened = false;
  vws.onopen = () => { opened = true; videoFailures = 0; };
  vws.onmessage = async (e) => {
    const dv = new DataView(e.data);
    const w = dv.getUint16(4, true), h = dv.getUint16(6, true);
    age = dv.getUint16(8, true);
    rtt = dv.getUint16(10, true);
    try {
      const bmp = await createImageBitmap(new Blob([new Uint8Array(e.data, 12)], { type: "image/jpeg" }));
      if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; setAspect(w, h); }
      ctx.drawImage(bmp, 0, 0);
      bmp.close();
    } catch (_) {}
    frames++;
    bytes += e.data.byteLength;
    if (vws && vws.readyState === WebSocket.OPEN) vws.send("a"); // drawn: the next one, please
  };
  vws.onclose = () => {
    vws = null;
    if (!active) return;
    if (!opened && ++videoFailures >= 3) {
      // A proxy or browser that will not carry websockets: fall back to MJPEG.
      transport = "mjpeg";
      syncSeg("transport", transport);
    }
    setTimeout(() => active && startVideo(), 1500);
  };
}

function stopVideo() {
  if (vws) { vws.onclose = null; vws.close(); vws = null; }
  mjpeg.removeAttribute("src");
  $("#hud").hidden = true;
}

mjpeg.addEventListener("load", () => setAspect(mjpeg.naturalWidth, mjpeg.naturalHeight));
mjpeg.addEventListener("error", () => {
  setTimeout(async () => {
    if (!active || transport !== "mjpeg") return;
    const r = await api("/api/state");
    if (r.ok) mjpeg.src = "/stream?" + Date.now();
  }, 1500);
});

function stats() {
  const now = performance.now(), dt = (now - lastStats) / 1000;
  lastStats = now;
  if (transport === "mjpeg") {
    $("#stats").textContent = "MJPEG stream";
    $("#m-fps").textContent = "–"; $("#m-mbps").textContent = "–"; $("#m-lat").textContent = "–";
    return;
  }
  const fps = frames / dt, mbps = (bytes * 8) / dt / 1e6, lat = Math.round(age + rtt / 2);
  frames = 0;
  bytes = 0;
  fpsHist.push(fps);
  if (fpsHist.length > 15) fpsHist.shift();
  const max = Math.max(1, ...fpsHist);
  $("#sparkline").setAttribute("points", fpsHist.map((v, i) => `${(i * 56) / 14},${16 - (v / max) * 14}`).join(" "));
  $("#stats").textContent = `${fps.toFixed(0)} fps · ${mbps.toFixed(1)} Mbit/s · ${lat} ms`;
  $("#m-fps").textContent = fps.toFixed(0);
  $("#m-mbps").textContent = mbps.toFixed(1);
  $("#m-lat").textContent = String(lat);
}

$("#wake").onclick = () => send({ t: "combo", keys: ["ShiftLeft"] });
$("#todemo").onclick = () => api("/api/source", { source: "demo" }).then(refresh);

// ---- Keyboard -----------------------------------------------------------

function setControlling(on) {
  controlling = on;
  stage.classList.toggle("active", on);
  $("#control").hidden = !on;
  $("#ctlhint").hidden = !on || touchDevice || pref.get("release", "ctrl-alt-r") === "none";
  if (!touchDevice) hint.hidden = on;
  if (!on) {
    send({ t: "release" });
    for (const m of held) send({ t: "key", code: m, down: true }); // sticky ones stay held
  }
}

stage.addEventListener("focus", () => setControlling(true));
stage.addEventListener("blur", () => setControlling(false));

// The shortcut that hands the keyboard back to the browser instead of reaching the target.
let lastRightCtrl = 0;
function releaseShortcut(e) {
  const mode = pref.get("release", "ctrl-alt-r");
  if (mode === "ctrl-alt-r") return e.ctrlKey && e.altKey && e.code === "KeyR";
  if (mode === "rctrl2" && e.code === "ControlRight") {
    const now = performance.now(), twice = now - lastRightCtrl < 450;
    lastRightCtrl = now;
    return twice;
  }
  return false;
}

function onKey(e, down) {
  if (!controlling || e.target !== stage) return;
  e.preventDefault();
  e.stopPropagation();
  if (e.repeat) return; // the target repeats a held key itself
  if (down && releaseShortcut(e)) { stage.blur(); return; }
  send({ t: "key", code: e.code, down });
}
stage.addEventListener("keydown", (e) => onKey(e, true));
stage.addEventListener("keyup", (e) => onKey(e, false));

// Sticky modifiers: held until clicked again, for touch screens and for shortcuts the
// browser or the OS would otherwise take.
for (const b of $$(".toggle")) {
  b.addEventListener("click", () => {
    const m = b.dataset.mod, on = !held.has(m);
    on ? held.add(m) : held.delete(m);
    b.setAttribute("aria-pressed", on);
    send({ t: "key", code: m, down: on });
  });
}

function releaseAll() {
  held.clear();
  for (const b of $$(".toggle")) b.setAttribute("aria-pressed", "false");
  send({ t: "release" });
}

// Key combinations, in a popover above the dock.
const keyspop = $("#keyspop"), keysbtn = $("#keysbtn");
function showKeys(on) {
  keyspop.hidden = !on;
  keysbtn.setAttribute("aria-expanded", on);
}
keysbtn.onclick = () => showKeys(keyspop.hidden);
for (const b of $$("[data-combo]")) {
  b.addEventListener("click", () => {
    send({ t: "combo", keys: b.dataset.combo.split(" ") });
    showKeys(false);
    stage.focus();
  });
}
document.addEventListener("click", (e) => {
  if (!keyspop.hidden && !keyspop.contains(e.target) && !keysbtn.contains(e.target)) showKeys(false);
});

// The phone's own keyboard, through a hidden input: text arrives as text, so it is typed
// with the target layout. With a sticky modifier held, a letter or digit is sent as that
// key instead, so Ctrl+C and the like work from a phone.
const kbd = $("#kbd");
$("#kbdbtn").addEventListener("click", () => { kbd.value = ""; kbd.focus(); });
function charCode(c) {
  if (/^[a-z]$/i.test(c)) return "Key" + c.toUpperCase();
  if (/^[0-9]$/.test(c)) return "Digit" + c;
  return null;
}
kbd.addEventListener("beforeinput", (e) => {
  if (e.inputType === "deleteContentBackward") { e.preventDefault(); send({ t: "combo", keys: ["Backspace"] }); }
  else if (e.inputType === "insertLineBreak") { e.preventDefault(); send({ t: "combo", keys: ["Enter"] }); }
});
kbd.addEventListener("input", () => {
  const text = kbd.value;
  kbd.value = "";
  if (!text) return;
  const code = text.length === 1 && held.size ? charCode(text) : null;
  if (code) send({ t: "combo", keys: [code] });
  else send({ t: "type", text, layout: layout() });
});
kbd.addEventListener("keydown", (e) => {
  if (["Enter", "Tab", "Escape", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(e.key)) {
    e.preventDefault();
    send({ t: "combo", keys: [e.code || e.key] });
  }
});

// ---- Mouse ----------------------------------------------------------------

// Where a point is on the picture, 0..1 each way, allowing for letterboxing.
function position(clientX, clientY) {
  const e = transport === "mjpeg" ? mjpeg : canvas;
  const r = e.getBoundingClientRect();
  const nw = (e.naturalWidth || e.width) || 16, nh = (e.naturalHeight || e.height) || 9;
  const scale = Math.min(r.width / nw, r.height / nh);
  const w = nw * scale, h = nh * scale;
  const left = r.left + (r.width - w) / 2, top = r.top + (r.height - h) / 2;
  return { x: (clientX - left) / w, y: (clientY - top) / h };
}
const clamp = (v) => Math.min(1, Math.max(0, v));

let pending = null;
function flushMove() {
  if (pending) send(pending);
  pending = null;
}
function moveTo(p, b) {
  if (p.x < 0 || p.x > 1 || p.y < 0 || p.y > 1) return;
  if (!pending) requestAnimationFrame(flushMove);
  pending = { t: "mouse", x: p.x, y: p.y, b };
}

stage.addEventListener("mousemove", (e) => {
  if (controlling) moveTo(position(e.clientX, e.clientY), e.buttons & 31);
});

function onButton(e) {
  if (!controlling) { stage.focus(); return; }
  e.preventDefault();
  const p = position(e.clientX, e.clientY);
  pending = null;
  send({ t: "mouse", x: clamp(p.x), y: clamp(p.y), b: e.buttons & 31 });
}
stage.addEventListener("mousedown", onButton);
stage.addEventListener("mouseup", onButton);
// A drag that ends outside the picture still lets go of the button on the target.
window.addEventListener("mouseup", (e) => {
  if (controlling && e.target !== stage && !stage.contains(e.target)) onButton(e);
});
stage.addEventListener("contextmenu", (e) => e.preventDefault());

let wheelY = 0, wheelX = 0;
function wheel(dyPx, dxPx) {
  wheelY += dyPx;
  wheelX += dxPx;
  const dy = Math.trunc(wheelY / 60), dx = Math.trunc(wheelX / 60);
  if (dy || dx) {
    wheelY -= dy * 60;
    wheelX -= dx * 60;
    send({ t: "wheel", dy: Math.max(-10, Math.min(10, dy)), dx: Math.max(-10, Math.min(10, dx)) });
  }
}
stage.addEventListener("wheel", (e) => {
  if (!controlling) return;
  e.preventDefault();
  const unit = e.deltaMode === 1 ? 33 : e.deltaMode === 2 ? 400 : 1;
  wheel(e.deltaY * unit, e.deltaX * unit);
}, { passive: false });

// ---- Touch ----------------------------------------------------------------
// One finger moves the pointer to where it is; a tap clicks there; a long press is a
// right click; two fingers scroll.

let touch = null;
stage.addEventListener("touchstart", (e) => {
  if (e.target === kbd) return;
  e.preventDefault();
  if (e.touches.length === 1) {
    const t = e.touches[0], p = position(t.clientX, t.clientY);
    touch = { x: t.clientX, y: t.clientY, at: performance.now(), moved: false, long: null, longDone: false, scrollY: null };
    moveTo(p, 0);
    touch.long = setTimeout(() => {
      if (touch && !touch.moved) {
        send({ t: "mouse", x: clamp(p.x), y: clamp(p.y), b: 2 });
        send({ t: "mouse", x: clamp(p.x), y: clamp(p.y), b: 0 });
        touch.longDone = true;
        if (navigator.vibrate) navigator.vibrate(20);
      }
    }, 600);
  } else if (e.touches.length === 2 && touch) {
    clearTimeout(touch.long);
    touch.scrollY = (e.touches[0].clientY + e.touches[1].clientY) / 2;
    touch.moved = true;
  }
}, { passive: false });

stage.addEventListener("touchmove", (e) => {
  if (!touch) return;
  e.preventDefault();
  if (e.touches.length === 2 && touch.scrollY !== null) {
    const y = (e.touches[0].clientY + e.touches[1].clientY) / 2;
    wheel((touch.scrollY - y) * 2, 0);
    touch.scrollY = y;
    return;
  }
  const t = e.touches[0];
  if (Math.hypot(t.clientX - touch.x, t.clientY - touch.y) > 10) {
    touch.moved = true;
    clearTimeout(touch.long);
  }
  moveTo(position(t.clientX, t.clientY), 0);
}, { passive: false });

stage.addEventListener("touchend", (e) => {
  if (!touch) return;
  e.preventDefault();
  clearTimeout(touch.long);
  if (!touch.moved && !touch.longDone && e.touches.length === 0) {
    const p = position(touch.x, touch.y);
    flushMove();
    send({ t: "mouse", x: clamp(p.x), y: clamp(p.y), b: 1 });
    send({ t: "mouse", x: clamp(p.x), y: clamp(p.y), b: 0 });
  }
  if (e.touches.length === 0) touch = null;
}, { passive: false });

// ---- Paste --------------------------------------------------------------

function layout() {
  return pref.get("layout", null) || (state && state.layout) || "us";
}
function fillLayouts(sel) {
  sel.replaceChildren(...((state && state.layouts) || ["us"]).map((l) => new Option(l.toUpperCase(), l)));
  sel.value = layout();
}
$("#paste").onclick = () => {
  fillLayouts($("#pastelayout"));
  $("#pastetext").value = "";
  $("#pastedlg").returnValue = ""; // it keeps the last close's value; Escape must not type
  $("#pastedlg").showModal();
};
for (const sel of [$("#pastelayout"), $("#layout")]) {
  sel.addEventListener("change", (e) => pref.set("layout", e.target.value));
}
$("#pastedlg").addEventListener("close", () => {
  const text = $("#pastetext").value;
  if ($("#pastedlg").returnValue === "ok" && text) send({ t: "type", text, layout: $("#pastelayout").value });
  stage.focus();
});

// ---- Sheets (drive, settings) ----------------------------------------------

function closeSheets() {
  for (const id of ["drivesheet", "settingssheet"]) {
    $("#" + id).hidden = true;
    $(`[aria-controls="${id}"]`).setAttribute("aria-expanded", "false");
  }
}
function toggleSheet(id, onOpen) {
  const sheet = $("#" + id), open = sheet.hidden;
  closeSheets();
  if (open) {
    sheet.hidden = false;
    $(`[aria-controls="${id}"]`).setAttribute("aria-expanded", "true");
    onOpen();
  } else {
    stage.focus();
  }
}
for (const b of $$("[data-close]")) b.onclick = () => { closeSheets(); stage.focus(); };
for (const sheet of $$(".sheet")) {
  sheet.addEventListener("keydown", (e) => { if (e.key === "Escape") { closeSheets(); stage.focus(); } });
}

// ---- Drive ----------------------------------------------------------------

async function loadDrive() {
  const r = await api("/api/drive");
  if (!r.ok) return;
  const d = r.data;
  $("#driveerr").textContent = "";
  const status = $("#drivestatus");
  status.replaceChildren();
  status.className = "";
  if (!d.available) {
    status.className = "note";
    status.textContent = "The USB drive is not set up: press Setup on the Flipper.";
  } else if (d.mounted) {
    status.className = "inserted";
    const eject = el("button", { type: "button", textContent: "Eject" });
    eject.onclick = () => driveAction("/api/drive/eject", {});
    status.append(
      icon("M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zm0 6.5a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5zM12 3v3M12 18v3"),
      el("div", { className: "grow" }, el("div", { className: "ell b", textContent: d.mounted.name }), el("div", { className: "what", textContent: `Inserted as ${modeName(d.mounted.mode)}` })),
      eject,
    );
    status.querySelector("svg").setAttribute("width", "28");
    status.querySelector("svg").setAttribute("height", "28");
    status.querySelector("svg").style.cssText = "fill:none;stroke:currentColor;stroke-width:1.6;stroke-linecap:round;stroke-linejoin:round";
  } else {
    status.className = "note";
    status.textContent = "Empty. Insert an image below: the computer then sees a drive with it.";
  }
  $("#images").replaceChildren(...d.images.map((img) => imageRow(img, d)));
  $("#drivecount").textContent = String(d.images.length);
  $("#driveempty").hidden = d.images.length > 0;
  $("#drivefree").textContent = `${fmtBytes(d.free)} free`;
  const used = d.images.reduce((n, i) => n + i.size, 0);
  $("#spaceused").style.width = `${Math.min(100, (used / Math.max(1, used + d.free)) * 100)}%`;
  $("#spacetext").textContent = `${fmtBytes(used)} in images · ${fmtBytes(d.free)} free on the Flipper`;
  $("#spacemax").textContent = `largest upload ${fmtBytes(d.max_upload)}`;
}

function modeName(m) {
  return { cdrom: "CD-ROM, the computer sees a DVD drive", disk: "a USB disk, writable", "disk-ro": "a USB disk, read-only" }[m] || m;
}

function imageRow(img, d) {
  const isMounted = d.mounted && d.mounted.name === img.name;
  const li = el("li", { className: isMounted ? "mounted" : "" });
  li.append(el("div", { className: "row" }, el("span", { className: "name", textContent: img.name }), el("span", { className: "mono small dim", textContent: fmtBytes(img.size) })));
  const acts = el("div", { className: "acts" });
  const seg = el("span", { className: "seg", role: "group" });
  seg.setAttribute("aria-label", "Insert as");
  for (const [mode, label, title] of [["cdrom", "CD-ROM", "Insert as a read-only CD/DVD (installers)"], ["disk", "USB", "Insert as a writable USB stick"], ["disk-ro", "USB read-only", "Insert as a read-only USB stick"]]) {
    const b = el("button", { type: "button", textContent: label, title, disabled: !d.available });
    b.setAttribute("aria-pressed", isMounted && d.mounted.mode === mode);
    b.onclick = () => driveAction("/api/drive/mount", { name: img.name, mode });
    seg.append(b);
  }
  const dl = el("a", { href: "/api/drive/images/" + encodeURIComponent(img.name), className: "button", title: "Download" });
  dl.setAttribute("download", img.name);
  dl.setAttribute("aria-label", "Download " + img.name);
  dl.append(icon(ICON_DOWN));
  const del = el("button", { type: "button", className: "iconbtn", title: "Delete" });
  del.setAttribute("aria-label", "Delete " + img.name);
  del.append(icon(ICON_TRASH));
  del.onclick = async () => {
    if (confirm(`Delete ${img.name} from the Flipper?`)) await driveAction("/api/drive/images/" + encodeURIComponent(img.name), undefined, "DELETE");
  };
  acts.append(seg, el("span", { className: "grow" }), dl, del);
  li.append(acts);
  return li;
}

async function driveAction(path, body, method) {
  const r = await api(path, body, method);
  if (!r.ok) $("#driveerr").textContent = r.data.error || "Failed.";
  await loadDrive();
  if (r.ok) $("#driveerr").textContent = "";
}

$("#drive").onclick = () => toggleSheet("drivesheet", loadDrive);

let upload = null;
function uploadFile(f) {
  if (!f || upload) return;
  const name = f.name.replace(/[^A-Za-z0-9 ._()-]/g, "_").replace(/^\.+/, "").slice(0, 128) || "image.img";
  const xhr = new XMLHttpRequest();
  upload = xhr;
  xhr.open("PUT", "/api/drive/images/" + encodeURIComponent(name));
  const card = $("#upcard"), started = performance.now();
  card.hidden = false;
  $("#upname").textContent = name;
  $("#upbytes").textContent = fmtBytes(f.size);
  $("#upprog").value = 0;
  $("#upmsg").textContent = "Starting…";
  $("#upeta").textContent = "";
  xhr.upload.onprogress = (e) => {
    if (!e.lengthComputable) return;
    const secs = (performance.now() - started) / 1000, rate = e.loaded / Math.max(0.5, secs);
    $("#upprog").value = e.loaded / e.total;
    $("#upbytes").textContent = `${fmtBytes(e.loaded)} of ${fmtBytes(e.total)}`;
    $("#upmsg").textContent = `${Math.round((e.loaded / e.total) * 100)}% · ${fmtBytes(rate)}/s`;
    $("#upeta").textContent = rate > 0 ? `about ${fmtSpan((e.total - e.loaded) / rate)} left` : "";
  };
  xhr.onloadend = async () => {
    upload = null;
    card.hidden = true;
    $("#upfile").value = "";
    if (xhr.status === 0) $("#driveerr").textContent = "Upload cancelled.";
    else if (xhr.status !== 200) {
      let msg = "Upload failed.";
      try { msg = JSON.parse(xhr.responseText).error || msg; } catch (_) {}
      $("#driveerr").textContent = msg;
    }
    await loadDrive();
  };
  xhr.send(f);
}
$("#upfile").onchange = () => uploadFile($("#upfile").files[0]);
$("#upcancel").onclick = () => { if (upload) upload.abort(); };
// Dropping a file anywhere on the picture uploads it.
stage.addEventListener("dragover", (e) => { if (e.dataTransfer.types.includes("Files")) { e.preventDefault(); e.dataTransfer.dropEffect = "copy"; } });
stage.addEventListener("drop", (e) => {
  if (!e.dataTransfer.files.length) return;
  e.preventDefault();
  if ($("#drivesheet").hidden) toggleSheet("drivesheet", loadDrive);
  uploadFile(e.dataTransfer.files[0]);
});

$("#newform").addEventListener("submit", async (e) => {
  e.preventDefault();
  const name = $("#newname").value.trim() || "disk.img";
  await driveAction("/api/drive/create", { name, mib: Math.max(1, Number($("#newsize").value)) * 1024 });
});

// ---- Settings -------------------------------------------------------------

function syncSeg(kind, value) {
  for (const b of $$(`[data-${kind}]`)) b.setAttribute("aria-pressed", b.dataset[kind] === value);
}

function fillSettings() {
  const s = state;
  if (!s) return;
  syncSeg("source", s.source);
  syncSeg("transport", transport);
  const size = $("#size");
  size.replaceChildren(new Option("Automatic", "auto"), ...s.video.sizes.map((v) => new Option(v, v)));
  size.value = s.settings.size;
  size.disabled = s.video.sizes.length === 0;
  if (document.activeElement !== $("#fps")) { $("#fps").value = s.settings.max_fps; $("#fpsval").textContent = `${s.settings.max_fps} fps`; }
  if (document.activeElement !== $("#quality")) { $("#quality").value = s.settings.quality; $("#qval").textContent = s.settings.quality; }
  fillLayouts($("#layout"));
  $("#releasekey").value = pref.get("release", "ctrl-alt-r");
  $("#kblock").checked = pref.get("kblock", "1") === "1";
  const groups = (s.fingerprint || "").split(":");
  $("#fp").replaceChildren(...Array.from({ length: Math.ceil(groups.length / 4) }, (_, i) => el("span", { textContent: groups.slice(i * 4, i * 4 + 4).join(":") })));
  $("#version").textContent = s.version ? `Flipper One KVM ${s.version}` : "";
}

async function loadSessions() {
  const r = await api("/api/sessions");
  if (!r.ok) return;
  $("#sessions").replaceChildren(...r.data.sessions.map((s) => {
    const li = el("li", { className: s.me ? "me" : "" });
    const what = s.me ? "this browser" : s.open > 0 ? "connected" : "idle";
    li.append(
      el("span", { className: "dot " + (s.open > 0 || s.me ? "ok" : "") }),
      el("span", { className: "who" }, el("b", { textContent: s.addr }), el("span", { className: "small dim", textContent: ` · paired ${fmtSpan(s.age_secs)} ago · ${what}` })),
    );
    if (!s.me) {
      const kick = el("button", { type: "button", className: "small", textContent: "Kick" });
      kick.onclick = async () => { await api("/api/sessions/kick", { id: s.id }); loadSessions(); };
      li.append(kick);
    }
    return li;
  }));
}

async function openSettings() {
  await refresh();
  fillSettings();
  loadSessions();
  const a = await api("/api/audit");
  $("#audit").textContent = a.ok ? a.data.lines.slice().reverse().join("\n") : "";
}
$("#settings").onclick = () => toggleSheet("settingssheet", openSettings);
for (const b of $$("[data-source]")) b.onclick = () => api("/api/source", { source: b.dataset.source }).then(refresh).then(fillSettings);
for (const b of $$("[data-transport]")) b.onclick = () => {
  transport = b.dataset.transport;
  pref.set("transport", transport);
  syncSeg("transport", transport);
  startVideo();
};
$("#size").onchange = (e) => api("/api/settings", { size: e.target.value });
$("#fps").oninput = (e) => ($("#fpsval").textContent = `${e.target.value} fps`);
$("#fps").onchange = (e) => api("/api/settings", { max_fps: Number(e.target.value) });
$("#quality").oninput = (e) => ($("#qval").textContent = e.target.value);
$("#quality").onchange = (e) => api("/api/settings", { quality: Number(e.target.value) });
$("#releasekey").onchange = (e) => {
  pref.set("release", e.target.value);
  $("#ctlkeys").replaceChildren(...({ "ctrl-alt-r": ["Ctrl", "Alt", "R"], rctrl2: ["Right Ctrl", "×2"], none: [] }[e.target.value] || []).map((k) => el("kbd", { textContent: k })));
};
$("#kblock").onchange = (e) => pref.set("kblock", e.target.checked ? "1" : "0");
$("#kickall").onclick = async () => {
  if (confirm("End every session, this one included?")) { await api("/api/sessions/kick", { id: "all" }); showPair("Every session was ended. Enter a new code from the Flipper."); }
};

// ---- Toolbar --------------------------------------------------------------

$("#release").onclick = releaseAll;
$("#full").onclick = async () => {
  await stage.requestFullscreen().catch(() => {});
  // Chrome can then pass Escape, Cmd/Ctrl+W and the like to the target too.
  if (pref.get("kblock", "1") === "1" && navigator.keyboard && navigator.keyboard.lock) navigator.keyboard.lock().catch(() => {});
  stage.focus();
};
document.addEventListener("fullscreenchange", () => {
  if (!document.fullscreenElement && navigator.keyboard && navigator.keyboard.unlock) navigator.keyboard.unlock();
});
$("#logout").onclick = async () => { await api("/api/logout", {}); showPair(""); };
$("#pastedlg").addEventListener("close", () => stage.focus());

// ---- Start ----------------------------------------------------------------

if (touchDevice) {
  hint.textContent = "Tap to click, long-press to right-click, two fingers to scroll. Keyboard opens your keyboard.";
  setTimeout(() => (hint.hidden = true), 6000);
}
$("#releasekey").value = pref.get("release", "ctrl-alt-r");
$("#releasekey").dispatchEvent(new Event("change"));

(async () => {
  let ok = false;
  try { ok = (await fetch("/api/state", { credentials: "same-origin" })).ok; } catch (_) {}
  if (ok) showKvm();
  else showPair("");
})();
