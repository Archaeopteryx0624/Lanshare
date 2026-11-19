#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use axum::{
    body::Body,
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use futures_util::StreamExt;
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

const PORT: u16 = 8765;
const INDEX: &str = include_str!("../../../shared/index.html");

#[derive(Clone)]
struct Peer {
    id: String,
    name: String,
    ip: String,
    port: u16,
    pin: String,
}

struct Inner {
    name: String,
    pin: String,
    root: PathBuf,
    peers: Mutex<Vec<Peer>>,
    http: reqwest::Client,
    done: AtomicU64,
    total: AtomicU64,
}
type App = Arc<Inner>;

#[derive(Deserialize, Default)]
struct Q {
    #[serde(default)]
    path: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    peer: String,
}

// ---------- helpers ----------
fn err(code: StatusCode, msg: impl ToString) -> Response {
    (code, msg.to_string()).into_response()
}
fn bad(msg: impl ToString) -> Response {
    err(StatusCode::BAD_REQUEST, msg)
}
fn authed(a: &Inner, h: &HeaderMap) -> bool {
    h.get("x-token").and_then(|v| v.to_str().ok()) == Some(a.pin.as_str())
}
fn is_local(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Joins a relative path onto root, refusing "..", drive letters and absolute paths.
fn resolve(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut p = root.to_path_buf();
    for c in Path::new(rel.trim_start_matches(['/', '\\'])).components() {
        match c {
            Component::Normal(s) => p.push(s),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(p)
}

fn unique(p: PathBuf) -> PathBuf {
    if !p.exists() {
        return p;
    }
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut i = 1;
    loop {
        let c = dir.join(format!("{stem} ({i}){ext}"));
        if !c.exists() {
            return c;
        }
        i += 1;
    }
}

fn list_dir(root: &Path, rel: &str) -> Result<Value, String> {
    let dir = resolve(root, rel).ok_or("bad path")?;
    let mut items: Vec<(bool, String, u64, u64)> = vec![];
    for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let Ok(md) = e.metadata() else { continue };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        items.push((md.is_dir(), e.file_name().to_string_lossy().into_owned(), md.len(), mtime));
    }
    items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let entries: Vec<Value> = items
        .into_iter()
        .map(|(dir, name, size, mtime)| json!({"name": name, "dir": dir, "size": size, "mtime": mtime}))
        .collect();
    Ok(json!({"path": rel, "entries": entries}))
}

fn add_peer(a: &Inner, name: String, ip: String, port: u16, pin: String) -> Value {
    let id = format!("{ip}:{port}");
    let mut ps = a.peers.lock().unwrap();
    ps.retain(|p| p.id != id);
    ps.push(Peer { id: id.clone(), name: name.clone(), ip: ip.clone(), port, pin });
    json!({"id": id, "name": name, "ip": ip})
}
fn find_peer(a: &Inner, id: &str) -> Option<Peer> {
    a.peers.lock().unwrap().iter().find(|p| p.id == id).cloned()
}

// ---------- peer API (needs X-Token = this device's PIN) ----------
async fn index() -> Html<&'static str> {
    Html(INDEX)
}
async fn api_info(State(a): State<App>) -> Json<Value> {
    Json(json!({"name": a.name, "platform": "windows"}))
}
async fn api_list(State(a): State<App>, h: HeaderMap, Query(q): Query<Q>) -> Response {
    if !authed(&a, &h) {
        return err(StatusCode::UNAUTHORIZED, "wrong pin");
    }
    match list_dir(&a.root, &q.path) {
        Ok(v) => Json(v).into_response(),
        Err(e) => bad(e),
    }
}
async fn api_download(State(a): State<App>, h: HeaderMap, Query(q): Query<Q>) -> Response {
    if !authed(&a, &h) {
        return err(StatusCode::UNAUTHORIZED, "wrong pin");
    }
    let Some(p) = resolve(&a.root, &q.path) else { return bad("bad path") };
    let Ok(f) = tokio::fs::File::open(&p).await else { return err(StatusCode::NOT_FOUND, "no such file") };
    let len = f.metadata().await.map(|m| m.len()).unwrap_or(0);
    Response::builder()
        .header("content-type", "application/octet-stream")
        .header("content-length", len)
        .body(Body::from_stream(ReaderStream::new(f)))
        .unwrap()
}
async fn api_upload(State(a): State<App>, h: HeaderMap, Query(q): Query<Q>, body: Body) -> Response {
    if !authed(&a, &h) {
        return err(StatusCode::UNAUTHORIZED, "wrong pin");
    }
    let Some(dir) = resolve(&a.root, &q.path) else { return bad("bad path") };
    let Some(fname) = Path::new(&q.name).file_name().map(|s| s.to_owned()) else { return bad("bad name") };
    let dest = unique(dir.join(fname));
    let mut f = match tokio::fs::File::create(&dest).await {
        Ok(f) => f,
        Err(e) => return bad(e),
    };
    let mut s = body.into_data_stream();
    while let Some(c) = s.next().await {
        let c = match c {
            Ok(c) => c,
            Err(e) => return bad(e),
        };
        if let Err(e) = f.write_all(&c).await {
            return bad(e);
        }
    }
    Json(json!({"saved": dest.file_name().map(|s| s.to_string_lossy().into_owned())})).into_response()
}
#[derive(Deserialize)]
struct Hello {
    name: String,
    port: u16,
    pin: String,
}
async fn api_hello(
    State(a): State<App>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    h: HeaderMap,
    Json(b): Json<Hello>,
) -> Response {
    if !authed(&a, &h) {
        return err(StatusCode::UNAUTHORIZED, "wrong pin");
    }
    add_peer(&a, b.name, addr.ip().to_string(), b.port, b.pin);
    StatusCode::OK.into_response()
}

// ---------- local UI API (loopback only) ----------
async fn ui_state(State(a): State<App>, ConnectInfo(addr): ConnectInfo<SocketAddr>) -> Response {
    if !is_local(&addr) {
        return err(StatusCode::FORBIDDEN, "local only");
    }
    let ip = local_ip_address::local_ip().map(|i| i.to_string()).unwrap_or_default();
    let peers: Vec<Value> = a
        .peers
        .lock()
        .unwrap()
        .iter()
        .map(|p| json!({"id": p.id, "name": p.name, "ip": p.ip}))
        .collect();
    Json(json!({"name": a.name, "pin": a.pin, "ip": ip, "port": PORT, "peers": peers})).into_response()
}
async fn ui_list(State(a): State<App>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Query(q): Query<Q>) -> Response {
    if !is_local(&addr) {
        return err(StatusCode::FORBIDDEN, "local only");
    }
    match list_dir(&a.root, &q.path) {
        Ok(v) => Json(v).into_response(),
        Err(e) => bad(e),
    }
}
async fn ui_rlist(State(a): State<App>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Query(q): Query<Q>) -> Response {
    if !is_local(&addr) {
        return err(StatusCode::FORBIDDEN, "local only");
    }
    let Some(p) = find_peer(&a, &q.peer) else { return bad("Unknown device") };
    let r = a
        .http
        .get(format!("http://{}:{}/api/list", p.ip, p.port))
        .query(&[("path", q.path.as_str())])
        .header("x-token", &p.pin)
        .timeout(Duration::from_secs(8))
        .send()
        .await;
    match r {
        Ok(r) if r.status().is_success() => {
            let t = r.text().await.unwrap_or_default();
            ([("content-type", "application/json")], t).into_response()
        }
        Ok(r) => bad(format!("Device answered {}", r.status())),
        Err(_) => bad("Cannot reach that device. Is it still running and on the same wifi?"),
    }
}
#[derive(Deserialize)]
struct Conn {
    ip: String,
    pin: String,
}
async fn ui_connect(State(a): State<App>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Json(b): Json<Conn>) -> Response {
    if !is_local(&addr) {
        return err(StatusCode::FORBIDDEN, "local only");
    }
    let host = b.ip.trim().to_string();
    let (ip, port) = match host.split_once(':') {
        Some((i, p)) => (i.to_string(), p.parse().unwrap_or(PORT)),
        None => (host.clone(), PORT),
    };
    let base = format!("http://{ip}:{port}");
    let t = Duration::from_secs(5);
    match a.http.get(format!("{base}/api/list")).header("x-token", &b.pin).timeout(t).send().await {
        Ok(r) if r.status().as_u16() == 401 => return bad("Wrong PIN"),
        Ok(r) if r.status().is_success() => {}
        Ok(r) => return bad(format!("Device answered {}", r.status())),
        Err(_) => return bad("Cannot reach that device. Check the IP and that both are on the same wifi."),
    }
    let name = match a.http.get(format!("{base}/api/info")).timeout(t).send().await {
        Ok(r) => r
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["name"].as_str().map(String::from))
            .unwrap_or_else(|| ip.clone()),
        Err(_) => ip.clone(),
    };
    // Tell the other device about us so it can browse this one too.
    let _ = a
        .http
        .post(format!("{base}/api/hello"))
        .header("x-token", &b.pin)
        .json(&json!({"name": a.name, "port": PORT, "pin": a.pin}))
        .timeout(t)
        .send()
        .await;
    Json(add_peer(&a, name, ip, port, b.pin)).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pull {
    peer: String,
    remote_path: String,
    local_dir: String,
}
async fn pull(a: &Inner, b: Pull) -> Result<Value, String> {
    let p = find_peer(a, &b.peer).ok_or("Unknown device")?;
    let dir = resolve(&a.root, &b.local_dir).ok_or("Bad folder")?;
    let fname = Path::new(&b.remote_path).file_name().ok_or("Bad file")?.to_owned();
    let r = a
        .http
        .get(format!("http://{}:{}/api/download", p.ip, p.port))
        .query(&[("path", b.remote_path.as_str())])
        .header("x-token", &p.pin)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("Device answered {}", r.status()));
    }
    let dest = unique(dir.join(fname));
    let mut f = tokio::fs::File::create(&dest).await.map_err(|e| e.to_string())?;
    a.total.store(r.content_length().unwrap_or(0), Ordering::Relaxed);
    a.done.store(0, Ordering::Relaxed);
    let mut s = r.bytes_stream();
    while let Some(c) = s.next().await {
        let c = c.map_err(|e| e.to_string())?;
        f.write_all(&c).await.map_err(|e| e.to_string())?;
        a.done.fetch_add(c.len() as u64, Ordering::Relaxed);
    }
    Ok(json!({"saved": dest.file_name().map(|s| s.to_string_lossy().into_owned())}))
}
async fn ui_pull(State(a): State<App>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Json(b): Json<Pull>) -> Response {
    if !is_local(&addr) {
        return err(StatusCode::FORBIDDEN, "local only");
    }
    match pull(&a, b).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Push {
    peer: String,
    local_path: String,
    remote_dir: String,
}
async fn push(a: App, b: Push) -> Result<Value, String> {
    let p = find_peer(&a, &b.peer).ok_or("Unknown device")?;
    let src = resolve(&a.root, &b.local_path).ok_or("Bad file")?;
    let name = src.file_name().ok_or("Bad file")?.to_string_lossy().into_owned();
    let f = tokio::fs::File::open(&src).await.map_err(|e| e.to_string())?;
    let len = f.metadata().await.map_err(|e| e.to_string())?.len();
    a.total.store(len, Ordering::Relaxed);
    a.done.store(0, Ordering::Relaxed);
    let a2 = a.clone();
    let r = a
        .http
        .put(format!("http://{}:{}/api/upload", p.ip, p.port))
        .query(&[("path", b.remote_dir.as_str()), ("name", name.as_str())])
        .header("x-token", &p.pin)
        .header("content-length", len)
        .body(reqwest::Body::wrap_stream(ReaderStream::new(f).inspect(move |c| {
            if let Ok(c) = c {
                a2.done.fetch_add(c.len() as u64, Ordering::Relaxed);
            }
        })))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("Device answered {}", r.status()));
    }
    Ok(json!({"sent": name}))
}
async fn ui_push(State(a): State<App>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Json(b): Json<Push>) -> Response {
    if !is_local(&addr) {
        return err(StatusCode::FORBIDDEN, "local only");
    }
    match push(a.clone(), b).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => bad(e),
    }
}

// ---------- folders & progress ----------
async fn api_mkdir(State(a): State<App>, h: HeaderMap, Query(q): Query<Q>) -> Response {
    if !authed(&a, &h) {
        return err(StatusCode::UNAUTHORIZED, "wrong pin");
    }
    let Some(d) = resolve(&a.root, &q.path) else { return bad("bad path") };
    match std::fs::create_dir_all(&d) {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => bad(e),
    }
}
#[derive(Deserialize)]
struct Mk {
    path: String,
}
async fn ui_mkdir(State(a): State<App>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Json(b): Json<Mk>) -> Response {
    if !is_local(&addr) {
        return err(StatusCode::FORBIDDEN, "local only");
    }
    let Some(d) = resolve(&a.root, &b.path) else { return bad("Bad folder") };
    match std::fs::create_dir_all(&d) {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => bad(e),
    }
}
#[derive(Deserialize)]
struct RMk {
    peer: String,
    path: String,
}
async fn ui_rmkdir(State(a): State<App>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Json(b): Json<RMk>) -> Response {
    if !is_local(&addr) {
        return err(StatusCode::FORBIDDEN, "local only");
    }
    let Some(p) = find_peer(&a, &b.peer) else { return bad("Unknown device") };
    let r = a
        .http
        .post(format!("http://{}:{}/api/mkdir", p.ip, p.port))
        .query(&[("path", b.path.as_str())])
        .header("x-token", &p.pin)
        .timeout(Duration::from_secs(8))
        .send()
        .await;
    match r {
        Ok(r) if r.status().is_success() => StatusCode::OK.into_response(),
        Ok(r) => bad(format!("Device answered {}", r.status())),
        Err(_) => bad("Cannot reach that device. Is it still running and on the same wifi?"),
    }
}
async fn ui_progress(State(a): State<App>, ConnectInfo(addr): ConnectInfo<SocketAddr>) -> Response {
    if !is_local(&addr) {
        return err(StatusCode::FORBIDDEN, "local only");
    }
    Json(json!({"done": a.done.load(Ordering::Relaxed), "total": a.total.load(Ordering::Relaxed)})).into_response()
}

// ---------- startup ----------
fn load_pin() -> String {
    let dir = std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
    let f = dir.join("lanshare-pin.txt");
    if let Ok(s) = std::fs::read_to_string(&f) {
        let s = s.trim().to_string();
        if s.len() == 6 {
            return s;
        }
    }
    let pin = rand::thread_rng().gen_range(100000..1000000).to_string();
    let _ = std::fs::write(&f, &pin);
    pin
}

fn router(app: App) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/info", get(api_info))
        .route("/api/list", get(api_list))
        .route("/api/download", get(api_download))
        .route("/api/upload", put(api_upload))
        .route("/api/hello", post(api_hello))
        .route("/api/mkdir", post(api_mkdir))
        .route("/ui/mkdir", post(ui_mkdir))
        .route("/ui/rmkdir", post(ui_rmkdir))
        .route("/ui/progress", get(ui_progress))
        .route("/ui/state", get(ui_state))
        .route("/ui/list", get(ui_list))
        .route("/ui/rlist", get(ui_rlist))
        .route("/ui/connect", post(ui_connect))
        .route("/ui/pull", post(ui_pull))
        .route("/ui/push", post(ui_push))
        .with_state(app)
}

fn main() {
    let app: App = Arc::new(Inner {
        name: hostname::get().map(|h| h.to_string_lossy().into_owned()).unwrap_or_else(|_| "Windows PC".into()),
        pin: load_pin(),
        root: std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(".")),
        peers: Mutex::new(vec![]),
        http: reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).build().unwrap(),
        done: AtomicU64::new(0),
        total: AtomicU64::new(0),
    });

    // Bind first so the window never loads before the server is listening.
    let listener = std::net::TcpListener::bind(("0.0.0.0", PORT)).expect("port 8765 is already in use");
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
            let l = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(l, router(app).into_make_service_with_connect_info::<SocketAddr>())
                .await
                .unwrap();
        });
    });

    tauri::Builder::default()
        .setup(|app| {
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External("http://127.0.0.1:8765/".parse().unwrap()),
            )
            .title("LAN Share")
            .inner_size(1100.0, 720.0)
            .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running LAN Share");
}
