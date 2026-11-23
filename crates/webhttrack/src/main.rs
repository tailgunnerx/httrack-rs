use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::{
    extract::State,
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use tower_http::services::ServeDir;
use url::Url;

use httrack_core::{
    BrowserProfile, CrawlAction, CrawlStats, CrawlerEngine, DirectionScope, FilterEngine,
    HostScope, HtsOptions, LinkRewriteMode, StructureType,
};

#[derive(Clone)]
struct AppState {
    stats: Arc<Mutex<CrawlStats>>,
    stop_flag: Arc<AtomicBool>,
    last_options: Arc<Mutex<HtsOptions>>,
    #[allow(dead_code)]
    assets_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct StartMirrorRequest {
    pub project_name: Option<String>,
    pub base_path: Option<String>,
    pub urls: Vec<String>,
    pub action: Option<String>,
    pub max_depth: Option<usize>,
    pub sockets: Option<usize>,
    pub near_files: Option<bool>,
    pub generate_warc: Option<bool>,
    pub generate_changes: Option<bool>,
    pub single_file: Option<bool>,
    pub use_sitemap: Option<bool>,
    pub robots_mode: Option<u8>,
    pub structure_preset: Option<u32>,
    pub link_mode: Option<u8>,
    pub host_scope: Option<String>,
    pub dir_scope: Option<String>,
    pub browser_profile: Option<String>,
    pub auth_header: Option<String>,
    pub cookies_raw: Option<String>,
    pub exclude_mimes: Option<Vec<String>>,
    pub include_mimes: Option<Vec<String>>,
    pub exclude_extensions: Option<Vec<String>>,
    pub include_extensions: Option<Vec<String>>,
    pub filters: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct TestRuleRequest {
    pub test_url: String,
    pub seed_urls: Vec<String>,
    pub near_files: Option<bool>,
    pub host_scope: Option<String>,
    pub dir_scope: Option<String>,
    pub exclude_mimes: Option<Vec<String>>,
    pub include_mimes: Option<Vec<String>>,
    pub exclude_extensions: Option<Vec<String>>,
    pub include_extensions: Option<Vec<String>>,
    pub filters: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct ApiResponse {
    pub ok: bool,
    pub message: String,
    pub output_dir: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    let mut port: u16 = 8080;
    let mut i = 1;
    while i < args.len() {
        if (args[i] == "--port" || args[i] == "-p") && i + 1 < args.len() {
            if let Ok(p) = args[i + 1].parse::<u16>() {
                port = p;
            }
            i += 2;
            continue;
        }
        i += 1;
    }

    let assets_dir = find_assets_dir();

    let state = AppState {
        stats: Arc::new(Mutex::new(CrawlStats::default())),
        stop_flag: Arc::new(AtomicBool::new(false)),
        last_options: Arc::new(Mutex::new(HtsOptions::default())),
        assets_dir: assets_dir.clone(),
    };

    let server_html_dir = assets_dir.join("html").join("server");

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/status", get(status_handler))
        .route("/api/start", post(start_handler))
        .route("/api/stop", post(stop_handler))
        .route("/api/test-filter", post(test_filter_handler))
        .nest_service("/legacy", ServeDir::new(&server_html_dir))
        .nest_service("/images", ServeDir::new(server_html_dir.join("images")))
        .nest_service("/themes", ServeDir::new(server_html_dir.join("themes")))
        .nest_service("/div", ServeDir::new(server_html_dir.join("div")))
        .with_state(state);

    let bind_addr = format!("127.0.0.1:{}", port);
    println!("================================================================");
    println!(" WebHTTrack Website Copier 3.51-0 (Modernized Rust GUI Server)");
    println!(" Listening on: http://{}", bind_addr);
    println!("================================================================");

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn find_assets_dir() -> PathBuf {
    let mut candidates = vec![
        PathBuf::from("assets"),
        PathBuf::from("../assets"),
        PathBuf::from("../../assets"),
    ];
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidates.push(exe_dir.join("assets"));
            candidates.push(exe_dir.join("../assets"));
            candidates.push(exe_dir.join("../../assets"));
        }
    }
    for c in candidates {
        if c.exists() {
            return c;
        }
    }
    PathBuf::from("assets")
}

async fn status_handler(State(state): State<AppState>) -> impl IntoResponse {
    let st = state.stats.lock().unwrap().clone();
    let out_dir = state
        .last_options
        .lock()
        .unwrap()
        .output_dir
        .to_string_lossy()
        .to_string();
    Json(serde_json::json!({
        "stats": st,
        "output_dir": out_dir,
    }))
}

async fn stop_handler(State(state): State<AppState>) -> impl IntoResponse {
    state.stop_flag.store(true, Ordering::Relaxed);
    Json(serde_json::json!({
        "ok": true,
        "message": "Stop signal sent to crawler"
    }))
}

/// Interactive rule tester endpoint (`/api/test-filter`) so users can test any URL against their
/// MIME exclusions, extension filters, and scope rules BEFORE running a crawl.
async fn test_filter_handler(Json(req): Json<TestRuleRequest>) -> impl IntoResponse {
    let mut opts = HtsOptions::default();
    opts.exclude_mimes = req.exclude_mimes.unwrap_or_default();
    opts.include_mimes = req.include_mimes.unwrap_or_default();
    opts.exclude_extensions = req.exclude_extensions.unwrap_or_default();
    opts.include_extensions = req.include_extensions.unwrap_or_default();
    opts.filters = req.filters.unwrap_or_default();
    opts.near_files = req.near_files.unwrap_or(false);

    if let Some(hs) = req.host_scope.as_deref() {
        opts.host_scope = match hs {
            "domain" => HostScope::SameDomain,
            "tld" => HostScope::SameTld,
            "everywhere" => HostScope::Everywhere,
            _ => HostScope::SameAddress,
        };
    }
    if let Some(ds) = req.dir_scope.as_deref() {
        opts.dir_scope = match ds {
            "up" => DirectionScope::UpOnly,
            "both" => DirectionScope::Both,
            "same" => DirectionScope::SameDir,
            _ => DirectionScope::DownOnly,
        };
    }

    let target = match Url::parse(req.test_url.trim())
        .or_else(|_| Url::parse(&format!("https://{}", req.test_url.trim())))
    {
        Ok(u) => u,
        Err(e) => {
            return Json(serde_json::json!({
                "allowed": false,
                "reason": format!("Invalid URL: {}", e)
            }))
        }
    };

    let seeds: Vec<Url> = req
        .seed_urls
        .iter()
        .filter_map(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Url::parse(t).or_else(|_| Url::parse(&format!("https://{}", t))).ok()
            }
        })
        .collect();

    let engine = FilterEngine::from_options(&opts);
    let is_asset = httrack_core::guess_mime_from_url(&target)
        .map(|m| m != "text/html")
        .unwrap_or(false);

    let (allowed, reason) = engine.evaluate_url(
        &target,
        &seeds,
        opts.host_scope,
        opts.dir_scope,
        opts.near_files,
        is_asset,
    );

    Json(serde_json::json!({
        "allowed": allowed,
        "reason": reason,
        "inferred_mime": httrack_core::guess_mime_from_url(&target).unwrap_or("unknown (checked on HTTP response)"),
    }))
}

async fn start_handler(
    State(state): State<AppState>,
    Json(req): Json<StartMirrorRequest>,
) -> impl IntoResponse {
    {
        let st = state.stats.lock().unwrap();
        if st.is_running {
            return Json(ApiResponse {
                ok: false,
                message: "A mirror job is already running.".to_string(),
                output_dir: String::new(),
            });
        }
    }

    let mut opts = HtsOptions::default();
    opts.urls = req
        .urls
        .into_iter()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .collect();

    if opts.urls.is_empty() {
        return Json(ApiResponse {
            ok: false,
            message: "Please provide at least one URL to mirror.".to_string(),
            output_dir: String::new(),
        });
    }

    let mut out_dir = PathBuf::from(
        req.base_path
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("./mirrors"),
    );
    if let Some(proj) = req.project_name.as_deref().filter(|s| !s.trim().is_empty()) {
        out_dir.push(proj.trim());
    }
    opts.output_dir = out_dir.clone();

    if let Some(act) = req.action.as_deref() {
        opts.action = match act {
            "get" => CrawlAction::GetFiles,
            "spider" => CrawlAction::Spider,
            "skeleton" => CrawlAction::Skeleton,
            "update" => CrawlAction::Update,
            "continue" => CrawlAction::Continue,
            _ => CrawlAction::Mirror,
        };
    }
    if let Some(d) = req.max_depth {
        opts.max_depth = d;
    }
    if let Some(c) = req.sockets {
        opts.sockets = c.max(1);
    }
    if let Some(n) = req.near_files {
        opts.near_files = n;
    }
    if let Some(w) = req.generate_warc {
        opts.generate_warc = w;
    }
    if let Some(ch) = req.generate_changes {
        opts.generate_changes = ch;
    }
    if let Some(sf) = req.single_file {
        opts.single_file = sf;
    }
    if let Some(sm) = req.use_sitemap {
        opts.use_sitemap = sm;
    }
    if let Some(rm) = req.robots_mode {
        opts.robots_mode = rm;
    }
    if let Some(sp) = req.structure_preset {
        opts.structure = StructureType::Preset(sp);
    }
    if let Some(lm) = req.link_mode {
        opts.link_mode = match lm {
            1 => LinkRewriteMode::AbsoluteUrl,
            3 => LinkRewriteMode::AbsoluteUri,
            4 => LinkRewriteMode::Original,
            5 => LinkRewriteMode::TransparentProxy,
            _ => LinkRewriteMode::Relative,
        };
    }
    if let Some(hs) = req.host_scope.as_deref() {
        opts.host_scope = match hs {
            "domain" => HostScope::SameDomain,
            "tld" => HostScope::SameTld,
            "everywhere" => HostScope::Everywhere,
            _ => HostScope::SameAddress,
        };
    }
    if let Some(ds) = req.dir_scope.as_deref() {
        opts.dir_scope = match ds {
            "up" => DirectionScope::UpOnly,
            "both" => DirectionScope::Both,
            "same" => DirectionScope::SameDir,
            _ => DirectionScope::DownOnly,
        };
    }
    if let Some(bp) = req.browser_profile.as_deref() {
        opts.browser_profile = BrowserProfile::from_name(bp);
    }
    if let Some(auth) = req.auth_header.filter(|s| !s.trim().is_empty()) {
        opts.auth_header = Some(auth);
    }
    if let Some(cookies) = req.cookies_raw.filter(|s| !s.trim().is_empty()) {
        opts.cookies_raw = Some(cookies);
    }
    if let Some(em) = req.exclude_mimes {
        opts.exclude_mimes = em.into_iter().filter(|s| !s.trim().is_empty()).collect();
    }
    if let Some(im) = req.include_mimes {
        opts.include_mimes = im.into_iter().filter(|s| !s.trim().is_empty()).collect();
    }
    if let Some(ee) = req.exclude_extensions {
        opts.exclude_extensions = ee.into_iter().filter(|s| !s.trim().is_empty()).collect();
    }
    if let Some(ie) = req.include_extensions {
        opts.include_extensions = ie.into_iter().filter(|s| !s.trim().is_empty()).collect();
    }
    if let Some(fl) = req.filters {
        opts.filters = fl
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    opts.quiet = true;
    *state.last_options.lock().unwrap() = opts.clone();
    state.stop_flag.store(false, Ordering::Relaxed);

    let stats_clone = state.stats.clone();
    let stop_clone = state.stop_flag.clone();
    tokio::spawn(async move {
        let engine = CrawlerEngine::with_shared_state(opts, stats_clone, stop_clone);
        let _ = engine.run().await;
    });

    Json(ApiResponse {
        ok: true,
        message: "Mirror started".to_string(),
        output_dir: out_dir.to_string_lossy().to_string(),
    })
}

async fn index_handler(State(_state): State<AppState>) -> Html<&'static str> {
    Html(WEB_UI_HTML)
}

const WEB_UI_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>WebHTTrack 3.51-0 (Modernized Rust Edition)</title>
  <link rel="icon" href="/div/32x32/httrack.png" />
  <style>
    :root {
      --bg: #0f172a;
      --panel: #1e293b;
      --panel-alt: #0b1120;
      --border: #334155;
      --accent: #38bdf8;
      --accent-hover: #0284c7;
      --text: #f8fafc;
      --muted: #94a3b8;
      --success: #22c55e;
      --danger: #ef4444;
      --warn: #f59e0b;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      font-family: system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
      background: var(--bg) url('/images/bg_rings.svg') no-repeat top right;
      color: var(--text);
      min-height: 100vh;
    }
    header {
      background: rgba(15, 23, 42, 0.92);
      border-bottom: 1px solid var(--border);
      padding: 14px 28px;
      display: flex;
      align-items: center;
      justify-content: space-between;
    }
    .brand { display: flex; align-items: center; gap: 14px; }
    .brand img.logo { width: 36px; height: 36px; }
    .brand img.wordmark { height: 26px; filter: brightness(1.8); }
    .badge {
      background: rgba(56, 189, 248, 0.15);
      color: var(--accent);
      border: 1px solid rgba(56, 189, 248, 0.4);
      padding: 3px 10px;
      border-radius: 999px;
      font-size: 12px;
      font-weight: 600;
    }
    main {
      max-width: 1240px;
      margin: 24px auto;
      padding: 0 20px;
      display: grid;
      grid-template-columns: 1.35fr 0.85fr;
      gap: 22px;
    }
    @media (max-width: 960px) { main { grid-template-columns: 1fr; } }
    .card {
      background: var(--panel);
      border: 1px solid var(--border);
      border-radius: 12px;
      padding: 22px;
      box-shadow: 0 10px 25px rgba(0,0,0,0.25);
      margin-bottom: 20px;
    }
    h2 {
      margin: 0 0 14px 0;
      font-size: 17px;
      display: flex;
      align-items: center;
      justify-content: space-between;
      border-bottom: 1px solid var(--border);
      padding-bottom: 10px;
    }
    h3 {
      margin: 18px 0 8px 0;
      font-size: 14px;
      color: var(--accent);
      text-transform: uppercase;
      letter-spacing: 0.04em;
    }
    .grid-2 { display: grid; grid-template-columns: 1fr 1fr; gap: 14px; }
    .field { margin-bottom: 14px; }
    label {
      display: block;
      font-size: 13px;
      color: var(--text);
      margin-bottom: 4px;
      font-weight: 600;
    }
    .help-text {
      font-size: 12px;
      color: var(--muted);
      margin-top: 4px;
      line-height: 1.4;
    }
    input[type="text"], input[type="number"], textarea, select {
      width: 100%;
      background: var(--panel-alt);
      border: 1px solid var(--border);
      color: var(--text);
      padding: 9px 12px;
      border-radius: 7px;
      font-size: 13.5px;
    }
    textarea { resize: vertical; min-height: 68px; font-family: monospace; }
    .chip-group {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      margin: 8px 0 10px 0;
    }
    .chip {
      background: var(--panel-alt);
      border: 1px solid var(--border);
      color: var(--text);
      padding: 6px 12px;
      border-radius: 999px;
      font-size: 12.5px;
      cursor: pointer;
      user-select: none;
      display: flex;
      align-items: center;
      gap: 6px;
      transition: all 0.15s;
    }
    .chip input { margin: 0; }
    .chip:hover { border-color: var(--accent); }
    .option-card-list {
      display: grid;
      grid-template-columns: 1fr;
      gap: 9px;
      margin: 10px 0 16px 0;
    }
    .option-item {
      background: var(--panel-alt);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 10px 12px;
      display: flex;
      align-items: flex-start;
      gap: 10px;
      cursor: pointer;
    }
    .option-item input { margin-top: 3px; }
    .option-item .title { font-size: 13.5px; font-weight: 600; color: var(--text); }
    .option-item .desc { font-size: 12px; color: var(--muted); margin-top: 2px; line-height: 1.35; }
    .btns { display: flex; gap: 12px; margin-top: 14px; }
    button {
      background: var(--accent);
      color: #0f172a;
      border: none;
      padding: 11px 20px;
      border-radius: 8px;
      font-weight: 700;
      cursor: pointer;
      font-size: 14px;
      transition: background 0.15s;
    }
    button:hover { background: var(--accent-hover); color: #fff; }
    button.secondary { background: #334155; color: #f8fafc; font-weight: 600; padding: 9px 14px; font-size: 13px; }
    button.stop { background: var(--danger); color: #fff; }
    .stat-grid {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 10px;
      margin-bottom: 16px;
    }
    .stat-box {
      background: var(--panel-alt);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 11px;
    }
    .stat-box .num { font-size: 20px; font-weight: 700; color: var(--accent); }
    .stat-box .lbl { font-size: 11.5px; color: var(--muted); }
    .log-box {
      background: var(--panel-alt);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 12px;
      font-family: monospace;
      font-size: 12px;
      height: 240px;
      overflow-y: auto;
      color: #cbd5e1;
      white-space: pre-wrap;
    }
    .status-pill {
      padding: 3px 10px;
      border-radius: 999px;
      font-size: 12px;
      font-weight: 600;
    }
    .status-idle { background: #334155; color: #cbd5e1; }
    .status-running { background: rgba(56,189,248,0.2); color: #38bdf8; }
    .status-done { background: rgba(34,197,94,0.2); color: #22c55e; }
    .verdict-box {
      margin-top: 10px;
      padding: 10px 12px;
      border-radius: 8px;
      font-size: 12.5px;
      font-family: monospace;
      display: none;
    }
    .verdict-ok { background: rgba(34,197,94,0.15); border: 1px solid var(--success); color: #86efac; }
    .verdict-no { background: rgba(239,68,68,0.15); border: 1px solid var(--danger); color: #fca5a5; }
  </style>
</head>
<body>
  <header>
    <div class="brand">
      <img class="logo" src="/div/48x48/httrack.png" alt="HTTrack" />
      <img class="wordmark" src="/images/wordmark.svg" alt="HTTrack" />
      <span class="badge">3.51-0 Modernized Rust Engine</span>
    </div>
    <div style="font-size:13px; color:var(--muted);">
      Two-Stage Pre-Flight MIME Filter &bull; Browser Impersonation &bull; Live Rule Tester
    </div>
  </header>

  <main>
    <div>
      <div class="card">
        <h2>
          <span>1. Project &amp; Target Website</span>
          <span style="font-size:12px; color:var(--muted); font-weight:normal;">Offline Mirror Setup</span>
        </h2>

        <div class="grid-2">
          <div class="field">
            <label>Project Name</label>
            <input type="text" id="project_name" value="my_site" />
            <div class="help-text">Subfolder created inside Base Path to store this mirror.</div>
          </div>
          <div class="field">
            <label>Base Path (`-O`)</label>
            <input type="text" id="base_path" value="./mirrors" />
            <div class="help-text">Root directory where projects and `hts-cache/` are saved.</div>
          </div>
        </div>

        <div class="field">
          <label>Starting Web Addresses (URLs) — one per line</label>
          <textarea id="urls" placeholder="https://example.com/docs/"></textarea>
          <div class="help-text">The crawler starts at these URLs and recursively discovers pages &amp; assets from them.</div>
        </div>

        <div class="grid-2">
          <div class="field">
            <label>Crawl Mode (Action)</label>
            <select id="action">
              <option value="mirror">Full Offline Mirror — Save pages + assets &amp; rewrite links (-w)</option>
              <option value="skeleton">HTML Pages Only — Skip all images, CSS, JS, and media (--skeleton)</option>
              <option value="get">Single URL Grab — Only download the exact URLs listed above (-g)</option>
              <option value="spider">Broken Link Checker — Test URLs without saving files (--spider)</option>
              <option value="update">Incremental Update — Refresh an existing mirror folder (--update)</option>
            </select>
          </div>
          <div class="field">
            <label>Maximum Click Depth (`-rN`)</label>
            <input type="number" id="max_depth" value="9999" min="0" />
            <div class="help-text">`1` = only start page + links directly on it. `9999` = entire site.</div>
          </div>
        </div>

        <h3>2. Reliable File Type &amp; MIME Filtering (Two-Stage Pre-Flight + Header Check)</h3>
        <div class="help-text" style="margin-bottom:8px;">
          Unlike original HTTrack (which often downloaded files before checking their MIME type), checking any category below blocks matching file extensions <b>before opening a connection</b> AND blocks dynamic endpoints whose HTTP `Content-Type` header matches.
        </div>

        <label>Quick Skip Categories (`--exclude-mime`)</label>
        <div class="chip-group">
          <label class="chip"><input type="checkbox" class="mime-chip" value="video/*" /> Skip Videos (`video/*`, .mp4, .webm)</label>
          <label class="chip"><input type="checkbox" class="mime-chip" value="audio/*" /> Skip Audio (`audio/*`, .mp3, .wav)</label>
          <label class="chip"><input type="checkbox" class="mime-chip" value="image/*" /> Skip Images (`image/*`, .png, .jpg)</label>
          <label class="chip"><input type="checkbox" class="mime-chip" value="font/*" /> Skip Web Fonts (`font/*`, .woff2)</label>
          <label class="chip"><input type="checkbox" class="mime-chip" value="application/pdf" /> Skip PDFs (`application/pdf`)</label>
          <label class="chip"><input type="checkbox" class="mime-chip" value="application/*zip*" /> Skip Archives (`.zip`, `.tar.gz`)</label>
        </div>

        <div class="grid-2">
          <div class="field">
            <label>Custom Excluded MIME Types (`--exclude-mime`)</label>
            <input type="text" id="exclude_mimes" placeholder="e.g. video/*, application/octet-stream" />
            <div class="help-text">Comma-separated MIME wildcards to block from downloading.</div>
          </div>
          <div class="field">
            <label>Excluded File Extensions (`--exclude-ext`)</label>
            <input type="text" id="exclude_extensions" placeholder="e.g. mp4, zip, exe, dmg, iso" />
            <div class="help-text">Immediately skips any URL ending with these extensions.</div>
          </div>
        </div>

        <div class="grid-2">
          <div class="field">
            <label>Only Allow MIME Types (`--include-mime` whitelist)</label>
            <input type="text" id="include_mimes" placeholder="Leave blank for all, or e.g. image/*, text/css" />
            <div class="help-text">If set, only assets matching these MIME types are saved (HTML is still scanned for links).</div>
          </div>
          <div class="field">
            <label>Advanced URL Wildcard Rules (`+pattern` / `-pattern`)</label>
            <input type="text" id="filters" placeholder="e.g. -*/logout* -*/admin/* +*.png" />
            <div class="help-text">Prefix with `+` to force-allow or `-` to block matching URL paths.</div>
          </div>
        </div>

        <h3>3. Where Can the Crawler Go? (Domain &amp; Folder Boundaries)</h3>
        <div class="grid-2">
          <div class="field">
            <label>Domain Boundary</label>
            <select id="host_scope">
              <option value="address">Exact Hostname Only (-a) — e.g. stay strictly on docs.example.com</option>
              <option value="domain">Same Principal Domain (-d) — allow any subdomain (*.example.com)</option>
              <option value="tld">Same Top-Level Domain (-l) — allow any site with same .com/.org</option>
              <option value="everywhere">Anywhere on the Web (-e) — follow external site links (use with low depth!)</option>
            </select>
          </div>
          <div class="field">
            <label>Directory Boundary</label>
            <select id="dir_scope">
              <option value="down">Subdirectories Only (-D) — starting at /docs/ stays inside /docs/*</option>
              <option value="both">Entire Site Tree (-B) — can go both up to / and down into any folder</option>
              <option value="same">Single Directory Only (-S) — do not enter subfolders</option>
            </select>
          </div>
        </div>

        <h3>4. How Files Are Saved &amp; Linked on Disk</h3>
        <div class="grid-2">
          <div class="field">
            <label>Local Folder Layout (`-N`)</label>
            <select id="structure_preset">
              <option value="0">N0: Original Site Tree with Host Folder (`example.com/docs/page.html`)</option>
              <option value="100">N100: Original Site Tree WITHOUT Host Folder (`docs/page.html`)</option>
              <option value="1">N1: Separate Folders (`web/*.html` and `web/images/*`)</option>
              <option value="1003">N1003: Flat Directory (all files placed directly in project folder)</option>
            </select>
          </div>
          <div class="field">
            <label>Link Rewriting Mode (`-K`)</label>
            <select id="link_mode">
              <option value="0">K0: Relative Local Links — works offline from any folder (default)</option>
              <option value="1">K1: Absolute Remote URLs — point unmirrored links to live https:// site</option>
              <option value="3">K3: Root-Relative Paths (`/folder/page.html`) — for local web servers</option>
              <option value="4">K4: Leave All Links Untouched — keep original raw HTML attributes</option>
            </select>
          </div>
        </div>

        <div class="option-card-list">
          <label class="option-item">
            <input type="checkbox" id="near_files" checked />
            <div>
              <div class="title">Fetch External Page Assets (`--near` / `-n`)</div>
              <div class="desc">Downloads images, CSS, and scripts required to render a page even when hosted on an external CDN (like Cloudflare, Google Fonts, or S3), without crawling other pages on that CDN.</div>
            </div>
          </label>
          <label class="option-item">
            <input type="checkbox" id="single_file" />
            <div>
              <div class="title">Self-Contained Single-File HTML (`--single-file` / `-%Z`)</div>
              <div class="desc">Embeds external CSS stylesheets, JS, images, and fonts directly inside each HTML file as base64 `data:` URIs so any saved `.html` file can be moved or opened completely standalone.</div>
            </div>
          </label>
          <label class="option-item">
            <input type="checkbox" id="use_sitemap" />
            <div>
              <div class="title">Discover Hidden Pages via `sitemap.xml` (`--sitemap` / `-%m`)</div>
              <div class="desc">Automatically checks `robots.txt` and `/sitemap.xml` to find pages that aren't linked from the main navigation menu.</div>
            </div>
          </label>
          <label class="option-item">
            <input type="checkbox" id="generate_warc" />
            <div>
              <div class="title">Create ISO-28500 `.warc.gz` Web Archive (`--warc` / `-%r`)</div>
              <div class="desc">Packages every raw HTTP response and SHA-1 payload digest into a standard `.warc.gz` archive alongside the mirrored files (compatible with ReplayWeb.page and ArchiveBox).</div>
            </div>
          </label>
          <label class="option-item">
            <input type="checkbox" id="generate_changes" checked />
            <div>
              <div class="title">Generate Differential Audit Log (`hts-changes.json` / `-%d`)</div>
              <div class="desc">Writes a structured JSON summary of every new, changed, unchanged, and removed file with exact byte sizes.</div>
            </div>
          </label>
        </div>

        <h3>5. Browser Impersonation &amp; Session Auth (Replaces Legacy Proxy Setup)</h3>
        <div class="grid-2">
          <div class="field">
            <label>Browser Fingerprint (`--impersonate`)</label>
            <select id="browser_profile">
              <option value="chrome">Chrome 131 Desktop (Recommended — avoids CDN/Cloudflare bot blocks)</option>
              <option value="firefox">Firefox 133 Desktop</option>
              <option value="safari">Safari 18 macOS</option>
              <option value="classic">Classic `HTTrack/3.x` Bot User-Agent</option>
            </select>
            <div class="help-text">Sends matching `User-Agent`, `Sec-CH-UA`, and `Accept` headers.</div>
          </div>
          <div class="field">
            <label>Authorization Header (`--auth`)</label>
            <input type="text" id="auth_header" placeholder="Bearer eyJhbG... or user:password" />
            <div class="help-text">Mirror private docs or staging sites using a Bearer token or Basic login.</div>
          </div>
        </div>
        <div class="field">
          <label>Session Cookies (`--cookie`)</label>
          <input type="text" id="cookies_raw" placeholder="session_id=abc123xyz; auth_token=..." />
          <div class="help-text">Paste cookies from your browser DevTools (`document.cookie`) to mirror logged-in pages without needing an intercept proxy.</div>
        </div>

        <div class="btns">
          <button id="start_btn" onclick="startMirror()">Start Website Mirror</button>
          <button class="stop" id="stop_btn" onclick="stopMirror()" style="display:none;">Stop Mirror</button>
        </div>
      </div>
    </div>

    <div>
      <div class="card">
        <h2>
          <span>Interactive Filter &amp; MIME Rule Tester (`--why`)</span>
        </h2>
        <div class="help-text" style="margin-bottom:10px;">
          Not sure if a link or media file will be downloaded or skipped? Paste any sample URL below to test it live against your current settings.
        </div>
        <div style="display:flex; gap:8px;">
          <input type="text" id="test_url_input" placeholder="https://example.com/media/demo.mp4" />
          <button class="secondary" onclick="testFilterRule()">Test URL</button>
        </div>
        <div id="test_verdict" class="verdict-box"></div>
      </div>

      <div class="card">
        <h2>
          <span>Live Mirror Progress</span>
          <span id="status_badge" class="status-pill status-idle">IDLE</span>
        </h2>

        <div class="stat-grid">
          <div class="stat-box">
            <div class="num" id="st_scanned">0 / 0</div>
            <div class="lbl">Links Scanned / Queued</div>
          </div>
          <div class="stat-box">
            <div class="num" id="st_written">0</div>
            <div class="lbl">Files Saved to Disk</div>
          </div>
          <div class="stat-box">
            <div class="num" id="st_filtered">0</div>
            <div class="lbl">Filtered / Skipped by Rules</div>
          </div>
          <div class="stat-box">
            <div class="num" id="st_bytes">0 B</div>
            <div class="lbl">Bytes Downloaded</div>
          </div>
        </div>

        <div class="field">
          <label>Current Activity</label>
          <div id="st_current" style="font-family:monospace; font-size:12px; color:var(--accent); word-break:break-all; min-height:18px;">
            Ready.
          </div>
        </div>

        <div class="field">
          <label>Saved Files, Skipped MIME/URLs &amp; Errors</label>
          <div class="log-box" id="st_log">Waiting for job to start...</div>
        </div>
      </div>
    </div>
  </main>

  <script>
    function formatBytes(b) {
      if (b < 1024) return b + ' B';
      if (b < 1048576) return (b / 1024).toFixed(1) + ' KB';
      return (b / 1048576).toFixed(2) + ' MB';
    }

    function collectExcludeMimes() {
      const chips = Array.from(document.querySelectorAll('.mime-chip:checked')).map(cb => cb.value);
      const custom = document.getElementById('exclude_mimes').value
        .split(',')
        .map(s => s.trim())
        .filter(Boolean);
      return [...chips, ...custom];
    }

    function buildCurrentConfig() {
      const urlsRaw = document.getElementById('urls').value.split(/[\r\n\s,]+/).filter(Boolean);
      const filtersRaw = document.getElementById('filters').value.split(/[\r\n\s]+/).filter(Boolean);
      const includeMimes = document.getElementById('include_mimes').value.split(',').map(s => s.trim()).filter(Boolean);
      const excludeExts = document.getElementById('exclude_extensions').value.split(',').map(s => s.trim()).filter(Boolean);

      return {
        project_name: document.getElementById('project_name').value,
        base_path: document.getElementById('base_path').value,
        urls: urlsRaw,
        action: document.getElementById('action').value,
        structure_preset: parseInt(document.getElementById('structure_preset').value, 10),
        max_depth: parseInt(document.getElementById('max_depth').value, 10),
        link_mode: parseInt(document.getElementById('link_mode').value, 10),
        host_scope: document.getElementById('host_scope').value,
        dir_scope: document.getElementById('dir_scope').value,
        near_files: document.getElementById('near_files').checked,
        generate_changes: document.getElementById('generate_changes').checked,
        generate_warc: document.getElementById('generate_warc').checked,
        single_file: document.getElementById('single_file').checked,
        use_sitemap: document.getElementById('use_sitemap').checked,
        browser_profile: document.getElementById('browser_profile').value,
        auth_header: document.getElementById('auth_header').value,
        cookies_raw: document.getElementById('cookies_raw').value,
        exclude_mimes: collectExcludeMimes(),
        include_mimes: includeMimes,
        exclude_extensions: excludeExts,
        filters: filtersRaw
      };
    }

    async function testFilterRule() {
      const testUrl = document.getElementById('test_url_input').value.trim();
      if (!testUrl) return;
      const cfg = buildCurrentConfig();
      const seeds = cfg.urls.length ? cfg.urls : [testUrl];
      const res = await fetch('/api/test-filter', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          test_url: testUrl,
          seed_urls: seeds,
          near_files: cfg.near_files,
          host_scope: cfg.host_scope,
          dir_scope: cfg.dir_scope,
          exclude_mimes: cfg.exclude_mimes,
          include_mimes: cfg.include_mimes,
          exclude_extensions: cfg.exclude_extensions,
          filters: cfg.filters
        })
      });
      const data = await res.json();
      const box = document.getElementById('test_verdict');
      box.style.display = 'block';
      box.className = 'verdict-box ' + (data.allowed ? 'verdict-ok' : 'verdict-no');
      box.textContent = `${data.allowed ? 'ACCEPTED' : 'BLOCKED'} — ${data.reason}\nInferred Pre-Flight MIME: ${data.inferred_mime}`;
    }

    async function startMirror() {
      const payload = buildCurrentConfig();
      const res = await fetch('/api/start', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload)
      });
      const data = await res.json();
      if (!data.ok) {
        alert(data.message);
      }
      pollStatus();
    }

    async function stopMirror() {
      await fetch('/api/stop', { method: 'POST' });
    }

    async function pollStatus() {
      try {
        const res = await fetch('/api/status');
        const data = await res.json();
        const st = data.stats;

        document.getElementById('st_scanned').textContent = `${st.links_scanned} / ${st.links_total_queued}`;
        document.getElementById('st_written').textContent = st.files_written;
        document.getElementById('st_filtered').textContent = st.links_filtered_out || 0;
        document.getElementById('st_bytes').textContent = formatBytes(st.bytes_received);

        const badge = document.getElementById('status_badge');
        const stopBtn = document.getElementById('stop_btn');
        if (st.is_running) {
          badge.className = 'status-pill status-running';
          badge.textContent = `RUNNING (${st.elapsed_secs}s)`;
          stopBtn.style.display = 'inline-block';
          document.getElementById('st_current').textContent = st.current_url || 'Crawling...';
        } else if (st.is_finished) {
          badge.className = 'status-pill status-done';
          badge.textContent = `FINISHED (${st.elapsed_secs}s)`;
          stopBtn.style.display = 'none';
          document.getElementById('st_current').textContent = `Saved to: ${data.output_dir}`;
        } else {
          badge.className = 'status-pill status-idle';
          badge.textContent = 'IDLE';
          stopBtn.style.display = 'none';
        }

        let lines = [];
        if (st.recent_files && st.recent_files.length) {
          lines.push('=== Saved Files ===');
          st.recent_files.forEach(f => lines.push('[SAVED] ' + f));
        }
        if (st.skipped_files && st.skipped_files.length) {
          lines.push('\n=== Filtered / Skipped by MIME or Scope Rules ===');
          st.skipped_files.forEach(s => lines.push('[SKIP]  ' + s));
        }
        if (st.errors && st.errors.length) {
          lines.push('\n=== Errors ===');
          st.errors.forEach(e => lines.push('[ERR]   ' + e));
        }
        if (lines.length) {
          document.getElementById('st_log').textContent = lines.join('\n');
        }
      } catch (e) {}
    }

    setInterval(pollStatus, 1000);
    pollStatus();
  </script>
</body>
</html>
"##;
