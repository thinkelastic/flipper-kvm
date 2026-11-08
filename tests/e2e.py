#!/usr/bin/env python3
"""End-to-end checks against a running Flipper One KVM, as a browser would use it.

    python3 tests/e2e.py --code 123456 [--host localhost] [--port 8443]

The code is the one on the Flipper's screen (it is used up). Standard library only.
Checks: plain-HTTP redirect, security headers, auth, pairing, settings, the websocket
video path with acknowledgements, the MJPEG stream, the virtual drive (create, upload,
download, mount, eject, delete), cross-origin refusal, input, logout.
"""
import argparse, base64, hashlib, http.client, json, os, socket, ssl, struct, sys, time

p = argparse.ArgumentParser()
p.add_argument("--code", required=True)
p.add_argument("--host", default="localhost")
p.add_argument("--port", type=int, default=8443)
args = p.parse_args()
H, P = args.host, args.port
ORIGIN = f"https://{H}:{P}"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE  # self-signed; the fingerprint is checked on the panel
cookie = None
failures = 0


def check(name, ok, detail=""):
    global failures
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail else ""))
    if not ok:
        failures += 1


def req(method, path, body=None, headers=None, raw=False):
    c = http.client.HTTPSConnection(H, P, context=CTX, timeout=30)
    h = {"Origin": ORIGIN}
    if cookie:
        h["Cookie"] = cookie
    if isinstance(body, (dict, list)):
        body = json.dumps(body)
        h["Content-Type"] = "application/json"
    h.update(headers or {})
    c.request(method, path, body=body, headers=h)
    r = c.getresponse()
    data = r.read()
    c.close()
    if raw:
        return r, data
    try:
        return r, json.loads(data or b"null")
    except ValueError:
        return r, data


def ws_connect(path):
    s = CTX.wrap_socket(socket.create_connection((H, P), timeout=15), server_hostname=H)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall((f"GET {path} HTTP/1.1\r\nHost: {H}:{P}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
               f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nOrigin: {ORIGIN}\r\n"
               f"Cookie: {cookie}\r\n\r\n").encode())
    head = b""
    while b"\r\n\r\n" not in head:
        head += s.recv(1)
    return s, head.split(b"\r\n")[0].decode()


def ws_send(s, data, opcode=1):
    data = data.encode() if isinstance(data, str) else data
    mask = os.urandom(4)
    n = len(data)
    hdr = bytes([0x80 | opcode]) + (bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + struct.pack(">H", n) if n < 65536 else bytes([0x80 | 127]) + struct.pack(">Q", n))
    s.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))


def recv_exact(s, n):
    buf = b""
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            raise EOFError
        buf += chunk
    return buf


def ws_recv(s):
    b0, b1 = recv_exact(s, 2)
    n = b1 & 0x7F
    if n == 126:
        n = struct.unpack(">H", recv_exact(s, 2))[0]
    elif n == 127:
        n = struct.unpack(">Q", recv_exact(s, 8))[0]
    return b0 & 0x0F, recv_exact(s, n)


# 1. Plain HTTP on the HTTPS port is redirected.
s = socket.create_connection((H, P), timeout=10)
s.sendall(f"GET /some/path HTTP/1.1\r\nHost: {H}:{P}\r\n\r\n".encode())
resp = s.recv(4096).decode(errors="replace")
s.close()
check("plain HTTP is redirected to HTTPS", " 301 " in resp.split("\r\n")[0] and f"Location: https://{H}:{P}/some/path" in resp, resp.split("\r\n")[0])

# 2. The page and its headers.
r, _ = req("GET", "/", raw=True)
csp = r.getheader("Content-Security-Policy") or ""
check("page served with a strict CSP", r.status == 200 and "default-src 'self'" in csp and "frame-ancestors 'none'" in csp)
check("security headers", r.getheader("X-Content-Type-Options") == "nosniff" and r.getheader("X-Frame-Options") == "DENY" and r.getheader("Referrer-Policy") == "no-referrer")

# 3. Nothing without a session.
r, _ = req("GET", "/api/state")
check("API needs a session", r.status == 401)
r, _ = req("GET", "/api/drive")
check("drive API needs a session", r.status == 401)

# 4. A cross-origin post is refused before anything else.
r, _ = req("POST", "/api/pair", {"code": args.code}, headers={"Origin": "https://evil.example"})
check("cross-origin post refused", r.status == 403)

# 5. Pairing.
r, d = req("POST", "/api/pair", {"code": args.code})
check("pairing with the panel's code", r.status == 200, str(d))
if r.status != 200:
    sys.exit("cannot go on without a session")
set_cookie = r.getheader("Set-Cookie")
cookie = set_cookie.split(";")[0]
check("session cookie is HttpOnly, Secure, SameSite=Strict", all(x in set_cookie for x in ("HttpOnly", "Secure", "SameSite=Strict")))
r, d = req("POST", "/api/pair", {"code": args.code})
check("a code works once", r.status == 403, d.get("error", "") if isinstance(d, dict) else "")

# 6. State and settings.
r, st = req("GET", "/api/state")
check("state", r.status == 200 and "video" in st and "drive" in st and len(st.get("fingerprint", "")) == 95, st.get("video", {}).get("label", ""))
r, _ = req("POST", "/api/settings", {"max_fps": 12, "quality": 60})
r, st = req("GET", "/api/state")
check("settings apply", st["settings"]["max_fps"] == 12 and st["settings"]["quality"] == 60, str(st["settings"]))

# 7. Video over the websocket: frames come one per acknowledgement.
s, status = ws_connect("/ws/video")
check("video websocket opens", " 101 " in status, status)
n, sizes, start, jpeg_ok = 0, set(), time.time(), True
while n < 15:
    op, payload = ws_recv(s)
    if op != 2:
        continue
    seq, w, h, age, rtt = struct.unpack("<IHHHH", payload[:12])
    sizes.add((w, h))
    jpeg_ok &= payload[12:14] == b"\xff\xd8"
    n += 1
    ws_send(s, "a")
fps = n / (time.time() - start)
s.close()
check("video frames are JPEGs with a size header", jpeg_ok and all(w > 0 and h > 0 for w, h in sizes), f"{sizes}, {fps:.1f} fps, rtt {rtt} ms")
check("frame rate respects the setting", fps <= 12 * 1.5 + 1, f"{fps:.1f} fps for max 12")
req("POST", "/api/settings", {"max_fps": 30, "quality": 80})

# 8. The MJPEG fallback.
c = http.client.HTTPSConnection(H, P, context=CTX, timeout=15)
c.request("GET", "/stream", headers={"Cookie": cookie})
r = c.getresponse()
first = r.read(65536)
c.close()
check("MJPEG stream", r.status == 200 and b"--frame" in first and b"\xff\xd8" in first, r.getheader("Content-Type"))

# 9. The virtual drive.
r, d = req("GET", "/api/drive")
check("drive state", r.status == 200 and "images" in d, f"available={d.get('available')}")
available = d.get("available")
for name in ("e2e-blank.img", "e2e-upload.img"):
    req("POST", "/api/drive/eject", {})
    req("DELETE", f"/api/drive/images/{name}")
r, d = req("POST", "/api/drive/create", {"name": "e2e-blank.img", "mib": 16})
check("create a blank disk", r.status == 200, str(d))
blob = os.urandom(3 * 1024 * 1024 + 17)
r, d = req("PUT", "/api/drive/images/e2e-upload.img", blob, headers={"Content-Type": "application/octet-stream"})
check("upload an image", r.status == 200 and d.get("size") == len(blob), str(d))
r, d = req("PUT", "/api/drive/images/..%2Fescape.img", b"x")
check("upload refuses a path", r.status in (400, 404), str(r.status))
r, got = req("GET", "/api/drive/images/e2e-upload.img", raw=True)
check("download gives the same bytes", hashlib.sha256(got).digest() == hashlib.sha256(blob).digest())
r, d = req("GET", "/api/drive")
names = {i["name"]: i["size"] for i in d["images"]}
check("both images listed with sizes", names.get("e2e-blank.img") == 16 * 1024 * 1024 and names.get("e2e-upload.img") == len(blob), str(names))
if available:
    r, d = req("POST", "/api/drive/mount", {"name": "e2e-upload.img", "mode": "cdrom"})
    r, st = req("GET", "/api/drive")
    check("mount as CD-ROM", st.get("mounted") == {"name": "e2e-upload.img", "mode": "cdrom"}, str(st.get("mounted")))
    r, d = req("POST", "/api/drive/mount", {"name": "e2e-blank.img", "mode": "disk"})
    r, st = req("GET", "/api/drive")
    check("switch to a writable disk", st.get("mounted") == {"name": "e2e-blank.img", "mode": "disk"}, str(st.get("mounted")))
    r, d = req("DELETE", "/api/drive/images/e2e-blank.img")
    check("a mounted image cannot be deleted", r.status == 409, str(d))
    r, d = req("POST", "/api/drive/eject", {})
    r, st = req("GET", "/api/drive")
    check("eject", r.status == 200 and st.get("mounted") is None)
else:
    print("SKIP mount tests: the gadget has no drive (press Setup on the panel)")
for name in ("e2e-blank.img", "e2e-upload.img"):
    r, d = req("DELETE", f"/api/drive/images/{name}")
check("delete images", r.status == 200)

# 10. Input.
s, status = ws_connect("/ws")
check("input websocket opens", " 101 " in status, status)
ws_send(s, json.dumps({"t": "mouse", "x": 0.5, "y": 0.5, "b": 0}))
ws_send(s, json.dumps({"t": "release"}))
s.close()

# 11. Audit and logout.
r, d = req("GET", "/api/audit")
check("audit log has the pairing", r.status == 200 and any("paired" in l for l in d["lines"]))
req("POST", "/api/logout", {})
r, _ = req("GET", "/api/state")
check("logout ends the session", r.status == 401)

print(f"\n{'all passed' if failures == 0 else f'{failures} failed'}")
sys.exit(1 if failures else 0)
