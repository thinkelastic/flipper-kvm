"use strict";
// Flipper One KVM, the browser side. Pair with the code on the panel, draw the target's
// screen, send keyboard, mouse and touch input over a websocket (src/hid.rs has the
// messages), and manage video settings and the virtual drive.

const $ = (s, el = document) => el.querySelector(s);
const $$ = (s, el = document) => [...el.querySelectorAll(s)];
const stage = $("#stage"), canvas = $("#screen"), ctx = canvas.getContext("2d"), mjpeg = $("#mjpeg"), hint = $("#hint");
const touchDevice = matchMedia("(pointer: coarse)").matches;

let ws = null, vws = null, statusTimer = null, statsTimer = null, controlling = false, active = false;
let transport = localStorage.getItem("kvm.transport") || "ws";
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

// ---- Pairing ------------------------------------------------------------

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
  const r = await api("/api/pair", { code: $("#code").value });
  btn.disabled = false;
  if (r.ok) showKvm();
  else {
    $("#pairerr").textContent = r.data.error || "Could not connect.";
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
}

async function refresh() {
  const r = await api("/api/state");
  if (!r.ok) return;
  state = r.data;
  const s = state;
  chip("#video", s.video.label, !s.video.signal ? "bad" : s.video.device === "demo" ? "warn" : "ok");
  chip("#hid", s.hid.ready ? "USB ready" : "USB: " + s.hid.label, s.hid.ready ? "ok" : "warn");
}

function chip(sel, text, kind) {
  const el = $(sel);
  el.textContent = text;
  el.className = "chip " + kind;
}

function connect() {
  ws = new WebSocket(`wss://${location.host}/ws`);
  chip("#link", "Connecting", "warn");
  ws.onopen = () => {
    chip("#link", "Input on", "ok");
    for (const m of held) send({ t: "key", code: m, down: true });
  };
  ws.onclose = () => {
    chip("#link", "Reconnecting", "bad");
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

function startVideo() {
  stopVideo();
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
      if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
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
    }
    setTimeout(() => active && startVideo(), 1500);
  };
}

function stopVideo() {
  if (vws) { vws.onclose = null; vws.close(); vws = null; }
  mjpeg.removeAttribute("src");
}

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
  if (transport === "mjpeg") { chip("#stats", "MJPEG", "ok"); return; }
  const fps = frames / dt, mbps = (bytes * 8) / dt / 1e6;
  frames = 0;
  bytes = 0;
  chip("#stats", `${fps.toFixed(0)} fps · ${mbps.toFixed(1)} Mbit/s · ${Math.round(age + rtt / 2)} ms`, fps > 0 ? "ok" : "warn");
}

// ---- Keyboard -----------------------------------------------------------

function setControlling(on) {
  controlling = on;
  stage.classList.toggle("active", on);
  if (!touchDevice) hint.hidden = on;
  if (!on) {
    send({ t: "release" });
    for (const m of held) send({ t: "key", code: m, down: true }); // sticky ones stay held
  }
}

stage.addEventListener("focus", () => setControlling(true));
stage.addEventListener("blur", () => setControlling(false));

function onKey(e, down) {
  if (!controlling || e.target !== stage) return;
  e.preventDefault();
  e.stopPropagation();
  if (e.repeat) return; // the target repeats a held key itself
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

for (const b of $$("[data-combo]")) {
  b.addEventListener("click", () => {
    send({ t: "combo", keys: b.dataset.combo.split(" ") });
    $("#keysmenu").open = false;
    stage.focus();
  });
}
document.addEventListener("click", (e) => {
  if (!$("#keysmenu").contains(e.target)) $("#keysmenu").open = false;
});

// The phone's own keyboard, through a hidden input: text arrives as text, so it is typed
// with the layout chosen for Paste. With a sticky modifier held, a letter or digit is
// sent as that key instead, so Ctrl+C and the like work from a phone.
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
  const el = transport === "mjpeg" ? mjpeg : canvas;
  const r = el.getBoundingClientRect();
  const nw = (el.naturalWidth || el.width) || 16, nh = (el.naturalHeight || el.height) || 9;
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
    touch = { x: t.clientX, y: t.clientY, at: performance.now(), moved: false, long: null, scrollY: null };
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
  return localStorage.getItem("kvm.layout") || (state && state.layout) || "us";
}
$("#paste").onclick = () => {
  const sel = $("#layout");
  sel.replaceChildren(...((state && state.layouts) || ["us"]).map((l) => new Option(l.toUpperCase(), l)));
  sel.value = layout();
  $("#pastetext").value = "";
  $("#pastedlg").returnValue = ""; // it keeps the last close's value; Escape must not type
  $("#pastedlg").showModal();
};
$("#layout").addEventListener("change", (e) => localStorage.setItem("kvm.layout", e.target.value));
$("#pastedlg").addEventListener("close", () => {
  const text = $("#pastetext").value;
  if ($("#pastedlg").returnValue === "ok" && text) send({ t: "type", text, layout: $("#layout").value });
  stage.focus();
});

// ---- Drive ----------------------------------------------------------------

async function loadDrive() {
  const r = await api("/api/drive");
  if (!r.ok) return;
  const d = r.data;
  $("#driveerr").textContent = "";
  $("#drivestatus").replaceChildren();
  if (!d.available) {
    $("#drivestatus").textContent = "The USB drive is not set up: press Setup on the Flipper.";
  } else if (d.mounted) {
    const eject = Object.assign(document.createElement("button"), { type: "button", textContent: "Eject" });
    eject.onclick = async () => { await driveAction("/api/drive/eject", {}); };
    $("#drivestatus").append(`Inserted: ${d.mounted.name} (${modeName(d.mounted.mode)}) `, eject);
  } else {
    $("#drivestatus").textContent = "Empty. Insert an image below.";
  }
  const tbody = $("#images tbody");
  tbody.replaceChildren(...d.images.map((img) => imageRow(img, d)));
  $("#driveempty").hidden = d.images.length > 0;
  $("#drivefree").textContent = `${fmtBytes(d.free)} free on the Flipper. Largest upload: ${fmtBytes(d.max_upload)}.`;
}

function modeName(m) {
  return { cdrom: "CD-ROM", disk: "USB disk", "disk-ro": "USB disk, read-only" }[m] || m;
}

function imageRow(img, d) {
  const tr = document.createElement("tr");
  const name = document.createElement("td");
  name.textContent = img.name;
  const size = document.createElement("td");
  size.textContent = fmtBytes(img.size);
  const acts = document.createElement("td");
  acts.className = "acts";
  const btn = (label, fn, title) => {
    const b = Object.assign(document.createElement("button"), { type: "button", textContent: label, title: title || "" });
    b.onclick = fn;
    b.disabled = !d.available && label !== "Delete" && label !== "Download";
    acts.append(b);
  };
  const isMounted = d.mounted && d.mounted.name === img.name;
  btn("CD-ROM", () => driveAction("/api/drive/mount", { name: img.name, mode: "cdrom" }), "Insert as a read-only CD/DVD (installers)");
  btn("USB", () => driveAction("/api/drive/mount", { name: img.name, mode: "disk" }), "Insert as a writable USB stick");
  btn("USB (RO)", () => driveAction("/api/drive/mount", { name: img.name, mode: "disk-ro" }), "Insert as a read-only USB stick");
  const dl = Object.assign(document.createElement("a"), { href: "/api/drive/images/" + encodeURIComponent(img.name), textContent: "Download", className: "button" });
  dl.setAttribute("download", img.name);
  acts.append(dl);
  btn("Delete", async () => {
    if (confirm(`Delete ${img.name} from the Flipper?`)) {
      await driveAction("/api/drive/images/" + encodeURIComponent(img.name), undefined, "DELETE");
    }
  });
  if (isMounted) tr.className = "mounted";
  tr.append(name, size, acts);
  return tr;
}

async function driveAction(path, body, method) {
  const r = await api(path, body, method);
  if (!r.ok) $("#driveerr").textContent = r.data.error || "Failed.";
  await loadDrive();
  if (r.ok) $("#driveerr").textContent = "";
}

$("#drive").onclick = () => { $("#drivedlg").showModal(); loadDrive(); };

$("#upbtn").onclick = () => {
  const f = $("#upfile").files[0];
  if (!f) return;
  const name = f.name.replace(/[^A-Za-z0-9 ._()-]/g, "_").replace(/^\.+/, "").slice(0, 128) || "image.img";
  const xhr = new XMLHttpRequest();
  xhr.open("PUT", "/api/drive/images/" + encodeURIComponent(name));
  const prog = $("#upprog");
  prog.hidden = false;
  prog.value = 0;
  $("#upbtn").disabled = true;
  $("#upmsg").textContent = "Uploading…";
  xhr.upload.onprogress = (e) => {
    if (e.lengthComputable) {
      prog.value = e.loaded / e.total;
      $("#upmsg").textContent = `${fmtBytes(e.loaded)} of ${fmtBytes(e.total)}`;
    }
  };
  xhr.onloadend = async () => {
    prog.hidden = true;
    $("#upbtn").disabled = false;
    let msg = "Uploaded.";
    if (xhr.status !== 200) {
      try { msg = JSON.parse(xhr.responseText).error; } catch (_) { msg = "Upload failed."; }
    }
    $("#upmsg").textContent = msg;
    $("#upfile").value = "";
    await loadDrive();
  };
  xhr.send(f);
};

$("#newbtn").onclick = async () => {
  const name = $("#newname").value.trim() || "disk.img";
  await driveAction("/api/drive/create", { name, mib: Number($("#newsize").value) });
};

// ---- Settings -------------------------------------------------------------

async function openSettings() {
  $("#settingsdlg").showModal();
  await refresh();
  if (!state) return;
  const s = state;
  $("#source").value = s.source;
  const size = $("#size");
  size.replaceChildren(new Option("Automatic", "auto"), ...s.video.sizes.map((v) => new Option(v, v)));
  size.value = s.settings.size;
  size.disabled = s.video.sizes.length === 0;
  $("#fps").value = s.settings.max_fps;
  $("#fpsval").textContent = s.settings.max_fps;
  $("#quality").value = s.settings.quality;
  $("#qval").textContent = s.settings.quality;
  $("#transport").value = transport;
  $("#fp").textContent = s.fingerprint;
  const a = await api("/api/audit");
  $("#audit").textContent = a.ok ? a.data.lines.slice().reverse().join("\n") : "";
}
$("#settings").onclick = openSettings;
$("#source").onchange = (e) => api("/api/source", { source: e.target.value }).then(refresh);
$("#size").onchange = (e) => api("/api/settings", { size: e.target.value });
$("#fps").oninput = (e) => ($("#fpsval").textContent = e.target.value);
$("#fps").onchange = (e) => api("/api/settings", { max_fps: Number(e.target.value) });
$("#quality").oninput = (e) => ($("#qval").textContent = e.target.value);
$("#quality").onchange = (e) => api("/api/settings", { quality: Number(e.target.value) });
$("#transport").onchange = (e) => {
  transport = e.target.value;
  localStorage.setItem("kvm.transport", transport);
  startVideo();
};

// ---- Toolbar --------------------------------------------------------------

$("#release").onclick = releaseAll;
$("#full").onclick = async () => {
  await stage.requestFullscreen().catch(() => {});
  // Chrome can then pass Escape, Cmd/Ctrl+W and the like to the target too.
  if (navigator.keyboard && navigator.keyboard.lock) navigator.keyboard.lock().catch(() => {});
  stage.focus();
};
document.addEventListener("fullscreenchange", () => {
  if (!document.fullscreenElement && navigator.keyboard && navigator.keyboard.unlock) navigator.keyboard.unlock();
});
$("#logout").onclick = async () => { await api("/api/logout", {}); showPair(""); };
for (const d of $$("dialog")) d.addEventListener("close", () => { if (d.id !== "pastedlg") stage.focus(); });

// ---- Start ----------------------------------------------------------------

if (touchDevice) {
  hint.textContent = "Tap to click, long-press to right-click, two fingers to scroll. Keyboard opens your keyboard.";
  setTimeout(() => (hint.hidden = true), 6000);
}

(async () => {
  const r = await fetch("/api/state", { credentials: "same-origin" });
  if (r.ok) showKvm();
  else showPair("");
})();
