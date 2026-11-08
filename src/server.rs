//! The web side: HTTPS (plain HTTP on the same port is redirected), pairing by the code
//! on the panel, the target's screen, input, video settings and the virtual drive.
//!
//! Routes (everything but the page, its assets and /api/pair needs a session):
//!   GET    /                         the page
//!   POST   /api/pair                 {"code":"123456"} -> session cookie
//!   POST   /api/logout
//!   GET    /api/state                status, settings, drive, as JSON
//!   POST   /api/settings             {"max_fps":15,"quality":70,"size":"1280x720"|"auto"}
//!   POST   /api/source               {"source":"auto"|"demo"}
//!   GET    /api/audit                recent audit lines
//!   GET    /ws/video                 frames over a websocket, one per acknowledgement
//!   GET    /stream                   the same frames as multipart MJPEG (fallback, VLC)
//!   GET    /ws                       input events, see hid::Input
//!   GET    /api/drive                images, what is mounted, free space
//!   POST   /api/drive/mount          {"name":"x.iso","mode":"cdrom"|"disk"|"disk-ro"}
//!   POST   /api/drive/eject
//!   POST   /api/drive/create         {"name":"blank.img","mib":1024}
//!   PUT    /api/drive/images/{name}  upload (the request body is the file)
//!   GET    /api/drive/images/{name}  download
//!   DELETE /api/drive/images/{name}

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::config::data_dir;
use crate::drive;
use crate::hid::Input;
use crate::state::{HidState, PairError, Shared, Source};
use crate::tls;

const COOKIE: &str = "kvm_session";

type App = State<Arc<Shared>>;

pub async fn serve(shared: Arc<Shared>) -> std::io::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (cert, key) = tls::certificate(&data_dir())?;
    *shared.fingerprint.lock().unwrap() = tls::fingerprint(&cert);
    shared.changed();
    // No upload can be in progress yet: what is left over from an interrupted one goes.
    drive::remove_partials();
    let config = tls::server_config(&cert, &key)?;
    let port = shared.config.port;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    eprintln!("server: https://0.0.0.0:{port}/");
    shared.audit.log(format!("started on port {port}"));
    axum_server::bind(addr)
        .acceptor(tls::SniffAcceptor::new(config, port))
        .serve(router(shared).into_make_service_with_connect_info::<SocketAddr>())
        .await
}

pub fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/app.js", get(|| asset("text/javascript", include_str!("../web/app.js"))))
        .route("/style.css", get(|| asset("text/css", include_str!("../web/style.css"))))
        .route("/icon.svg", get(|| asset("image/svg+xml", include_str!("../web/icon.svg"))))
        .route("/api/pair", post(pair))
        .route("/api/logout", post(logout))
        .route("/api/state", get(state))
        .route("/api/settings", post(settings))
        .route("/api/source", post(source))
        .route("/api/audit", get(audit))
        .route("/stream", get(stream))
        .route("/ws", get(input_ws))
        .route("/ws/video", get(video_ws))
        .route("/api/drive", get(drive_state))
        .route("/api/drive/mount", post(drive_mount))
        .route("/api/drive/eject", post(drive_eject))
        .route("/api/drive/create", post(drive_create))
        .route("/api/drive/images/{name}", put(drive_upload).get(drive_download).delete(drive_delete).layer(DefaultBodyLimit::disable()))
        .layer(middleware::from_fn(guard))
        .with_state(shared)
}

/// On every response: security headers. On every state-changing request: the Origin, if
/// the browser sent one, must be this server, so another site cannot post with the cookie.
async fn guard(mut req: Request, next: Next) -> Response {
    // Over HTTP/2 the host arrives as :authority, which lands in the URI and not in a
    // Host header; put it there, so the Origin check and the page's policy see it.
    if !req.headers().contains_key(header::HOST) {
        if let Some(v) = req.uri().authority().and_then(|a| HeaderValue::from_str(a.as_str()).ok()) {
            req.headers_mut().insert(header::HOST, v);
        }
    }
    if !matches!(*req.method(), Method::GET | Method::HEAD) && req.headers().contains_key(header::ORIGIN) && !same_origin(req.headers()) {
        return (StatusCode::FORBIDDEN, "cross-origin request refused").into_response();
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("same-origin"));
    h.insert("permissions-policy", HeaderValue::from_static("camera=(), microphone=(), geolocation=()"));
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

async fn asset(kind: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, "no-cache")], body).into_response()
}

/// The Host header, if it is made of host characters only.
fn host(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c)))
}

async fn page(headers: HeaderMap) -> Response {
    let mut res = asset("text/html; charset=utf-8", include_str!("../web/index.html")).await;
    // Only its own scripts and styles; websockets only back to this host (named, for
    // browsers that do not count wss: as 'self'); never framed.
    let connect = host(&headers).map(|h| format!(" wss://{h}")).unwrap_or_default();
    let csp = format!(
        "default-src 'self'; img-src 'self' blob: data:; connect-src 'self'{connect}; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"
    );
    if let Ok(v) = HeaderValue::from_str(&csp) {
        res.headers_mut().insert(header::CONTENT_SECURITY_POLICY, v);
    }
    res
}

/// The session token from the cookie, if it is a live one.
fn session(app: &Shared, headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    let token = cookies.split(';').find_map(|c| c.trim().strip_prefix(&format!("{COOKIE}=")))?;
    app.authorized(token).then(|| token.to_string())
}

fn unauthorized() -> Response {
    error(StatusCode::UNAUTHORIZED, "Enter the code shown on the Flipper's screen.")
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

macro_rules! require {
    ($app:expr, $headers:expr) => {
        match session(&$app, &$headers) {
            Some(token) => token,
            None => return unauthorized(),
        }
    };
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
}

async fn pair(State(app): App, ConnectInfo(peer): ConnectInfo<SocketAddr>, Json(body): Json<PairBody>) -> Response {
    match app.pair(&body.code, peer.ip()) {
        Ok(token) => {
            let max_age = app.config.session_hours * 3600;
            let cookie = format!("{COOKIE}={token}; Path=/; Max-Age={max_age}; Secure; HttpOnly; SameSite=Strict");
            ([(header::SET_COOKIE, cookie)], Json(json!({"ok": true}))).into_response()
        }
        Err(e) => {
            // Slow guessing down.
            tokio::time::sleep(Duration::from_millis(800)).await;
            match e {
                PairError::Wrong(left) => error(StatusCode::FORBIDDEN, format!("Wrong code. {left} tries left before it changes.")),
                PairError::Replaced => error(StatusCode::FORBIDDEN, "Too many wrong codes; the Flipper shows a new one now."),
                PairError::Locked(secs) => error(
                    StatusCode::TOO_MANY_REQUESTS,
                    format!("Too many wrong codes from this computer. Try again in {} minutes.", secs.div_ceil(60)),
                ),
            }
        }
    }
}

async fn logout(State(app): App, headers: HeaderMap) -> Response {
    if let Some(token) = session(&app, &headers) {
        app.logout(&token);
    }
    let cookie = format!("{COOKIE}=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0");
    ([(header::SET_COOKIE, cookie)], Json(json!({"ok": true}))).into_response()
}

fn size_text((w, h): (u32, u32)) -> String {
    format!("{w}x{h}")
}

async fn state(State(app): App, headers: HeaderMap) -> Response {
    require!(app, headers);
    let video = app.video.lock().unwrap().clone();
    let settings = app.settings.lock().unwrap().clone();
    let hid = app.hid.lock().unwrap().clone();
    let source = *app.source.lock().unwrap();
    let mounted = drive::mounted();
    Json(json!({
        "video": {
            "label": video.label, "device": video.device, "width": video.width, "height": video.height,
            "fps": video.fps, "signal": video.signal,
            "sizes": video.sizes.iter().copied().map(size_text).collect::<Vec<_>>(),
        },
        "settings": {
            "max_fps": settings.max_fps, "quality": settings.quality,
            "size": settings.size.map(size_text).unwrap_or_else(|| "auto".into()),
        },
        "hid": {"label": hid.label(), "ready": hid == HidState::Ready},
        "source": if source == Source::Demo { "demo" } else { "auto" },
        "clients": app.clients.load(Ordering::Relaxed),
        "viewers": app.viewers.load(Ordering::Relaxed),
        "drive": {
            "available": drive::available(),
            "mounted": mounted.map(|(name, mode)| json!({"name": name, "mode": mode})),
        },
        "layout": app.config.layout,
        "layouts": crate::keymap::LAYOUTS,
        "fingerprint": *app.fingerprint.lock().unwrap(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct SettingsBody {
    max_fps: Option<u32>,
    quality: Option<u8>,
    size: Option<String>,
}

async fn settings(State(app): App, headers: HeaderMap, Json(body): Json<SettingsBody>) -> Response {
    require!(app, headers);
    let mut s = app.settings.lock().unwrap().clone();
    if let Some(f) = body.max_fps {
        s.max_fps = f.clamp(1, 60);
    }
    if let Some(q) = body.quality {
        s.quality = q.clamp(30, 95);
    }
    if let Some(size) = body.size {
        s.size = match size.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))) {
            Some((w, h)) if (320..=3840).contains(&w) && (240..=2160).contains(&h) => Some((w, h)),
            _ => None,
        };
    }
    app.audit.log(format!("video settings {} fps, quality {}, size {:?}", s.max_fps, s.quality, s.size));
    *app.settings.lock().unwrap() = s;
    app.changed();
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct SourceBody {
    source: String,
}

async fn source(State(app): App, headers: HeaderMap, Json(body): Json<SourceBody>) -> Response {
    require!(app, headers);
    *app.source.lock().unwrap() = if body.source == "demo" { Source::Demo } else { Source::Auto };
    app.changed();
    Json(json!({"ok": true})).into_response()
}

async fn audit(State(app): App, headers: HeaderMap) -> Response {
    require!(app, headers);
    Json(json!({"lines": app.audit.recent(50)})).into_response()
}

/// Counts a viewer or a client, and keeps the session from going idle, while held.
struct Held {
    app: Arc<Shared>,
    which: fn(&Shared) -> &AtomicUsize,
    token: String,
}

impl Held {
    fn new(app: Arc<Shared>, which: fn(&Shared) -> &AtomicUsize, token: &str) -> Self {
        which(&app).fetch_add(1, Ordering::Relaxed);
        app.connection(token, 1);
        app.changed();
        Held { app, which, token: token.to_string() }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        (self.which)(&self.app).fetch_sub(1, Ordering::Relaxed);
        self.app.connection(&self.token, -1);
        self.app.changed();
    }
}

async fn stream(State(app): App, headers: HeaderMap) -> Response {
    let token = require!(app, headers);
    let rx = app.frames.subscribe();
    let held = Held::new(app.clone(), |s| &s.viewers, &token);
    let body = futures_util::stream::unfold((rx, held, None::<u64>), |(mut rx, held, last)| async move {
        loop {
            let next = rx.borrow_and_update().clone().filter(|f| Some(f.seq) != last);
            let Some(frame) = next else {
                rx.changed().await.ok()?;
                continue;
            };
            // A kicked or expired session's stream ends with its next frame.
            if !held.app.authorized(&held.token) {
                return None;
            }
            let head = format!("--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n", frame.jpeg.len());
            let mut part = Vec::with_capacity(head.len() + frame.jpeg.len() + 2);
            part.extend_from_slice(head.as_bytes());
            part.extend_from_slice(&frame.jpeg);
            part.extend_from_slice(b"\r\n");
            return Some((Ok::<Bytes, std::io::Error>(Bytes::from(part)), (rx, held, Some(frame.seq))));
        }
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "multipart/x-mixed-replace; boundary=frame")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(body))
        .unwrap()
}

/// Only the page this server sent may open a websocket or post: a websocket is not held
/// to the same-origin rule, so another site could otherwise drive it with the cookie.
fn same_origin(headers: &HeaderMap) -> bool {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    match (origin, host(headers)) {
        (Some(o), Some(h)) => o.strip_prefix("https://") == Some(h),
        _ => false,
    }
}

async fn input_ws(State(app): App, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Response {
    let token = require!(app, headers);
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin websocket refused");
    }
    upgrade.max_message_size(256 * 1024).on_upgrade(move |socket| input(app, token, socket))
}

async fn input(app: Arc<Shared>, token: String, mut socket: WebSocket) {
    let _held = Held::new(app.clone(), |s| &s.clients, &token);
    while let Some(Ok(msg)) = socket.recv().await {
        if !app.authorized(&token) {
            let _ = socket.send(Message::Close(None)).await;
            break;
        }
        match msg {
            Message::Text(text) => match serde_json::from_str::<Input>(text.as_str()) {
                Ok(event) => {
                    let _ = app.input.send(event);
                }
                Err(e) => eprintln!("ws: bad input {e}"),
            },
            Message::Close(_) => break,
            _ => {}
        }
    }
    // Never leave a key or a button held down on the target.
    let _ = app.input.send(Input::Release);
}

async fn video_ws(State(app): App, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Response {
    let token = require!(app, headers);
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin websocket refused");
    }
    upgrade.max_message_size(1024).on_upgrade(move |socket| video(app, token, socket))
}

/// One frame, then wait for the browser to say it has drawn it, then the newest frame.
/// A slow link or a slow browser therefore gets fewer frames, never older ones.
///
/// Each binary message: seq (u32), width (u16), height (u16), age of the frame when sent
/// in ms (u16), last round trip in ms (u16), all little-endian, then the JPEG.
async fn video(app: Arc<Shared>, token: String, mut socket: WebSocket) {
    let _held = Held::new(app.clone(), |s| &s.viewers, &token);
    let mut rx = app.frames.subscribe();
    let mut last: Option<u64> = None;
    let mut rtt: u16 = 0;
    'outer: loop {
        let frame = loop {
            if let Some(f) = rx.borrow_and_update().clone().filter(|f| Some(f.seq) != last) {
                break f;
            }
            tokio::select! {
                changed = rx.changed() => if changed.is_err() { break 'outer },
                msg = socket.recv() => match msg {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break 'outer,
                    _ => {}
                },
            }
        };
        if !app.authorized(&token) {
            let _ = socket.send(Message::Close(None)).await;
            break;
        }
        let mut msg = Vec::with_capacity(12 + frame.jpeg.len());
        msg.extend_from_slice(&(frame.seq as u32).to_le_bytes());
        msg.extend_from_slice(&(frame.width.min(65535) as u16).to_le_bytes());
        msg.extend_from_slice(&(frame.height.min(65535) as u16).to_le_bytes());
        msg.extend_from_slice(&(frame.captured.elapsed().as_millis().min(65535) as u16).to_le_bytes());
        msg.extend_from_slice(&rtt.to_le_bytes());
        msg.extend_from_slice(&frame.jpeg);
        let sent = Instant::now();
        if socket.send(Message::Binary(Bytes::from(msg))).await.is_err() {
            break;
        }
        last = Some(frame.seq);
        // The acknowledgement; anything else from the browser is ignored.
        let ack = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                match socket.recv().await {
                    Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) => return true,
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return false,
                    _ => {}
                }
            }
        })
        .await;
        match ack {
            Ok(true) => rtt = sent.elapsed().as_millis().min(65535) as u16,
            _ => break,
        }
    }
}

// ---- The virtual drive ------------------------------------------------------

async fn drive_state(State(app): App, headers: HeaderMap) -> Response {
    require!(app, headers);
    let (images, free, mounted) =
        tokio::task::spawn_blocking(|| (drive::list(), drive::free_bytes(), drive::mounted())).await.unwrap_or_default();
    Json(json!({
        "available": drive::available(),
        "images": images,
        "mounted": mounted.map(|(name, mode)| json!({"name": name, "mode": mode})),
        "free": free,
        "max_upload": app.config.max_image_mib * 1024 * 1024,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct MountBody {
    name: String,
    mode: String,
}

async fn drive_mount(State(app): App, headers: HeaderMap, Json(body): Json<MountBody>) -> Response {
    require!(app, headers);
    let Some(mode) = drive::Mode::parse(&body.mode) else { return error(StatusCode::BAD_REQUEST, "unknown mode") };
    let name = body.name.clone();
    match tokio::task::spawn_blocking(move || drive::mount(&name, mode)).await.unwrap() {
        Ok(()) => {
            app.audit.log(format!("drive: mounted {} as {}", body.name, mode.name()));
            app.changed();
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => error(StatusCode::CONFLICT, e.to_string()),
    }
}

async fn drive_eject(State(app): App, headers: HeaderMap) -> Response {
    require!(app, headers);
    match tokio::task::spawn_blocking(drive::eject).await.unwrap() {
        Ok(()) => {
            app.audit.log("drive: ejected");
            app.changed();
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => error(StatusCode::CONFLICT, e.to_string()),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    mib: u64,
}

async fn drive_create(State(app): App, headers: HeaderMap, Json(body): Json<CreateBody>) -> Response {
    require!(app, headers);
    if body.mib == 0 || body.mib > app.config.max_image_mib || body.mib * 1024 * 1024 > drive::free_bytes() {
        return error(StatusCode::BAD_REQUEST, "size must be between 1 MiB and the free space (and the configured maximum)");
    }
    let name = body.name.clone();
    match tokio::task::spawn_blocking(move || drive::create_blank(&name, body.mib)).await.unwrap() {
        Ok(()) => {
            app.audit.log(format!("drive: created {} ({} MiB)", body.name, body.mib));
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => error(StatusCode::CONFLICT, e.to_string()),
    }
}

async fn drive_delete(State(app): App, headers: HeaderMap, Path(name): Path<String>) -> Response {
    require!(app, headers);
    let n = name.clone();
    match tokio::task::spawn_blocking(move || drive::delete(&n)).await.unwrap() {
        Ok(()) => {
            app.audit.log(format!("drive: deleted {name}"));
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => error(StatusCode::CONFLICT, e.to_string()),
    }
}

/// The request body is written to `<name>.part` and renamed when complete, so a broken
/// upload never leaves a half image under the real name. The part file is created
/// exclusively, so two uploads of the same name cannot write into one file.
async fn drive_upload(State(app): App, headers: HeaderMap, Path(name): Path<String>, body: Body) -> Response {
    require!(app, headers);
    let Some(path) = drive::path_of(&name) else { return error(StatusCode::BAD_REQUEST, "bad image name") };
    if name.ends_with(".part") {
        return error(StatusCode::BAD_REQUEST, "bad image name");
    }
    let max = app.config.max_image_mib * 1024 * 1024;
    let len: Option<u64> = headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
    if let Some(len) = len {
        if len > max {
            return error(StatusCode::PAYLOAD_TOO_LARGE, format!("larger than the {} MiB limit", app.config.max_image_mib));
        }
        if len + 64 * 1024 * 1024 > drive::free_bytes() {
            return error(StatusCode::INSUFFICIENT_STORAGE, "not enough free space on the Flipper");
        }
    }
    if path.exists() {
        return error(StatusCode::CONFLICT, "an image with that name exists; delete it first");
    }
    let _ = tokio::fs::create_dir_all(drive::images_dir()).await;
    let part = drive::partial_path(&path);
    let mut file = match tokio::fs::OpenOptions::new().write(true).create_new(true).open(&part).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return error(StatusCode::CONFLICT, "an upload with that name is already in progress");
        }
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();
    let result: Result<(), String> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| e.to_string())?;
            written += chunk.len() as u64;
            if written > max {
                return Err(format!("larger than the {} MiB limit", app.config.max_image_mib));
            }
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        file.flush().await.map_err(|e| e.to_string())?;
        file.sync_all().await.map_err(|e| e.to_string())
    }
    .await;
    drop(file);
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return error(StatusCode::BAD_REQUEST, format!("upload failed: {e}"));
    }
    if let Err(e) = tokio::fs::rename(&part, &path).await {
        let _ = tokio::fs::remove_file(&part).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    app.audit.log(format!("drive: uploaded {name} ({written} bytes)"));
    Json(json!({"ok": true, "size": written})).into_response()
}

async fn drive_download(State(app): App, headers: HeaderMap, Path(name): Path<String>) -> Response {
    require!(app, headers);
    let Some(path) = drive::path_of(&name) else { return error(StatusCode::BAD_REQUEST, "bad image name") };
    let Ok(file) = tokio::fs::File::open(&path).await else { return error(StatusCode::NOT_FOUND, "no such image") };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let body = futures_util::stream::unfold(file, |mut file| async move {
        let mut buf = vec![0u8; 256 * 1024];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<Bytes, std::io::Error>(Bytes::from(buf)), file))
            }
            Err(e) => Some((Err(e), file)),
        }
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, len)
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))
        .body(Body::from_stream(body))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(k.clone(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn websocket_origin_must_match() {
        assert!(same_origin(&headers(&[(header::ORIGIN, "https://10.0.0.5:8443"), (header::HOST, "10.0.0.5:8443")])));
        assert!(!same_origin(&headers(&[(header::ORIGIN, "https://evil.example"), (header::HOST, "10.0.0.5:8443")])));
        assert!(!same_origin(&headers(&[(header::ORIGIN, "http://10.0.0.5:8443"), (header::HOST, "10.0.0.5:8443")])));
        assert!(!same_origin(&headers(&[(header::HOST, "10.0.0.5:8443")])));
    }

    #[test]
    fn session_comes_from_the_cookie() {
        let (s, _rx) = crate::state::test_shared();
        let token = s.pair(&s.code(), [10, 0, 0, 1].into()).unwrap();
        let ok = headers(&[(header::COOKIE, &format!("theme=dark; {COOKIE}={token}"))]);
        assert_eq!(session(&s, &ok), Some(token));
        let bad = headers(&[(header::COOKIE, &format!("{COOKIE}=nope"))]);
        assert_eq!(session(&s, &bad), None);
    }

    fn request(method: &str, uri: &str, body: &str, extra: &[(&str, &str)]) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder().method(method).uri(uri).header("host", "10.0.0.5:8443");
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        let mut req = b.body(Body::from(body.to_string())).unwrap();
        req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 9], 5555))));
        req
    }

    async fn body_json(res: Response) -> serde_json::Value {
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    #[tokio::test]
    async fn pairing_then_using_the_api() {
        let (s, _rx) = crate::state::test_shared();
        let app = router(s.clone());

        let res = app.clone().oneshot(request("GET", "/api/state", "", &[])).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let body = format!(r#"{{"code":"{}"}}"#, s.code());
        let res = app
            .clone()
            .oneshot(request("POST", "/api/pair", &body, &[("content-type", "application/json"), ("origin", "https://10.0.0.5:8443")]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let cookie = res.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
        assert!(cookie.contains("HttpOnly") && cookie.contains("Secure") && cookie.contains("SameSite=Strict"));
        let token_cookie = cookie.split(';').next().unwrap().to_string();

        let res = app.clone().oneshot(request("GET", "/api/state", "", &[("cookie", &token_cookie)])).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        let state = body_json(res).await;
        assert_eq!(state["source"], "auto");
        assert_eq!(state["layouts"][0], "us");
    }

    #[tokio::test]
    async fn cross_origin_posts_are_refused() {
        let (s, _rx) = crate::state::test_shared();
        let body = format!(r#"{{"code":"{}"}}"#, s.code());
        let res = router(s.clone())
            .oneshot(request("POST", "/api/pair", &body, &[("content-type", "application/json"), ("origin", "https://evil.example")]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    /// HTTP/2 sends the authority in the URI and no Host header; the request must still
    /// pass the Origin check and reach the handler.
    #[tokio::test]
    async fn a_request_without_a_host_header_uses_the_uri_authority() {
        let (s, _rx) = crate::state::test_shared();
        let body = format!(r#"{{"code":"{}"}}"#, s.code());
        let mut req = HttpRequest::builder()
            .method("POST")
            .uri("https://10.0.0.5:8443/api/pair")
            .header("content-type", "application/json")
            .header("origin", "https://10.0.0.5:8443")
            .body(Body::from(body))
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 9], 5555))));
        let res = router(s).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn wrong_codes_say_how_many_tries_are_left() {
        let (s, _rx) = crate::state::test_shared();
        let wrong = if s.code() == "000000" { "111111" } else { "000000" };
        let res = router(s.clone())
            .oneshot(request("POST", "/api/pair", &format!(r#"{{"code":"{wrong}"}}"#), &[("content-type", "application/json")]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert!(body_json(res).await["error"].as_str().unwrap().contains("4 tries left"));
    }

    #[tokio::test]
    async fn the_page_has_a_strict_policy() {
        let (s, _rx) = crate::state::test_shared();
        let res = router(s).oneshot(request("GET", "/", "", &[])).await.unwrap();
        let csp = res.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().to_string();
        assert!(csp.contains("default-src 'self'") && csp.contains("wss://10.0.0.5:8443") && csp.contains("frame-ancestors 'none'"));
        assert_eq!(res.headers()[header::X_FRAME_OPTIONS], "DENY");
    }
}
