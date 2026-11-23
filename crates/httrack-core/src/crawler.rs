use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use regex::Regex;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, AUTHORIZATION, COOKIE, REFERER,
    USER_AGENT,
};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::cache::{write_changes_json, write_hts_cache, write_hts_log, CacheRecord};
use crate::filter::{extract_url_extension, FilterEngine, RobotsRules};
use crate::index_builder::write_mirror_index;
use crate::options::{BrowserProfile, CrawlAction, HtsOptions};
use crate::rewriter::{extract_css_links, extract_html_links, normalize_url, rewrite_css, rewrite_html};
use crate::savename::{compute_save_path, safe_join_contained};
use crate::warc::write_warc_archive;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrawlStats {
    pub is_running: bool,
    pub is_finished: bool,
    pub links_scanned: usize,
    pub links_total_queued: usize,
    pub links_filtered_out: usize,
    pub files_written: usize,
    pub bytes_received: usize,
    pub bytes_written: usize,
    pub errors_count: usize,
    pub warnings_count: usize,
    pub current_url: String,
    pub elapsed_secs: u64,
    pub recent_files: Vec<String>,
    pub skipped_files: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
struct QueueItem {
    url: Url,
    depth: usize,
    from_url: String,
    #[allow(dead_code)]
    is_asset: bool,
}

#[derive(Debug, Clone)]
struct FetchedDocument {
    url: Url,
    local_rel_path: String,
    mime: String,
    status_code: u16,
    date_gmt: String,
    text: String,
    is_html: bool,
    is_css: bool,
}

pub struct CrawlerEngine {
    pub options: HtsOptions,
    pub stats: Arc<Mutex<CrawlStats>>,
    pub stop_flag: Arc<AtomicBool>,
}

impl CrawlerEngine {
    pub fn new(options: HtsOptions) -> Self {
        Self {
            options,
            stats: Arc::new(Mutex::new(CrawlStats::default())),
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_shared_state(
        options: HtsOptions,
        stats: Arc<Mutex<CrawlStats>>,
        stop_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            options,
            stats,
            stop_flag,
        }
    }

    pub async fn run(&self) -> anyhow::Result<CrawlStats> {
        let launch_time = Utc::now();
        let start_instant = Instant::now();

        {
            let mut st = self.stats.lock().unwrap();
            *st = CrawlStats {
                is_running: true,
                is_finished: false,
                ..Default::default()
            };
        }

        fs::create_dir_all(&self.options.output_dir)?;

        // Write hts-in_progress.lock while crawling
        let lock_path = self.options.output_dir.join("hts-in_progress.lock");
        let _ = fs::write(&lock_path, format!("pid={}\n", std::process::id()));

        // Build HTTP client with modern BrowserProfile impersonation & Auth/Cookie headers
        let mut default_headers = HeaderMap::new();
        let effective_ua = if self.options.user_agent.contains("HTTrack")
            && self.options.browser_profile != BrowserProfile::ClassicHttrack
        {
            self.options.browser_profile.user_agent()
        } else {
            &self.options.user_agent
        };

        if let Ok(v) = HeaderValue::from_str(effective_ua) {
            default_headers.insert(USER_AGENT, v);
        }
        if let Ok(v) = HeaderValue::from_str(&self.options.language) {
            default_headers.insert(ACCEPT_LANGUAGE, v);
        }

        // Inject realistic BrowserProfile headers (Sec-CH-UA, Sec-Fetch-*, Accept) so modern CDNs don't block the crawl
        match self.options.browser_profile {
            BrowserProfile::Chrome => {
                default_headers.insert(
                    ACCEPT,
                    HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8"),
                );
                default_headers.insert(
                    HeaderName::from_static("sec-ch-ua"),
                    HeaderValue::from_static("\"Google Chrome\";v=\"131\", \"Chromium\";v=\"131\", \"Not_A Brand\";v=\"24\""),
                );
                default_headers.insert(
                    HeaderName::from_static("sec-ch-ua-mobile"),
                    HeaderValue::from_static("?0"),
                );
                default_headers.insert(
                    HeaderName::from_static("sec-ch-ua-platform"),
                    HeaderValue::from_static("\"Windows\""),
                );
                default_headers.insert(
                    HeaderName::from_static("upgrade-insecure-requests"),
                    HeaderValue::from_static("1"),
                );
            }
            BrowserProfile::Firefox => {
                default_headers.insert(
                    ACCEPT,
                    HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"),
                );
                default_headers.insert(
                    HeaderName::from_static("upgrade-insecure-requests"),
                    HeaderValue::from_static("1"),
                );
            }
            BrowserProfile::Safari => {
                default_headers.insert(
                    ACCEPT,
                    HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
                );
            }
            BrowserProfile::ClassicHttrack => {}
        }

        if let Some(ref referer) = self.options.referer {
            if let Ok(v) = HeaderValue::from_str(referer) {
                default_headers.insert(REFERER, v);
            }
        }

        // Inject Authorization header (--auth "Bearer ..." or "Basic user:pass")
        if let Some(ref auth) = self.options.auth_header {
            let formatted_auth = if auth.contains(' ') {
                auth.clone()
            } else if auth.contains(':') {
                format!("Basic {}", BASE64.encode(auth.as_bytes()))
            } else {
                format!("Bearer {}", auth)
            };
            if let Ok(v) = HeaderValue::from_str(&formatted_auth) {
                default_headers.insert(AUTHORIZATION, v);
            }
        }

        // Inject Cookie string (--cookie "session=abc; theme=dark" or --cookies-file cookies.txt)
        let mut combined_cookies = Vec::new();
        if let Some(ref raw_c) = self.options.cookies_raw {
            if !raw_c.trim().is_empty() {
                combined_cookies.push(raw_c.trim().to_string());
            }
        }
        if let Some(ref cfile) = self.options.cookies_file {
            if let Ok(parsed_c) = parse_netscape_cookies_file(cfile) {
                if !parsed_c.is_empty() {
                    combined_cookies.push(parsed_c);
                }
            }
        }
        if !combined_cookies.is_empty() {
            let cookie_line = combined_cookies.join("; ");
            if let Ok(v) = HeaderValue::from_str(&cookie_line) {
                default_headers.insert(COOKIE, v);
            }
        }

        for line in &self.options.custom_headers {
            if let Some((k, v)) = line.split_once(':') {
                if let (Ok(hn), Ok(hv)) = (
                    HeaderName::from_bytes(k.trim().as_bytes()),
                    HeaderValue::from_str(v.trim()),
                ) {
                    default_headers.insert(hn, hv);
                }
            }
        }

        let client = reqwest::Client::builder()
            .default_headers(default_headers)
            .cookie_store(true)
            .timeout(Duration::from_secs(self.options.timeout_secs))
            .build()?;

        // Parse seed URLs (including --list file if provided)
        let mut raw_seeds = self.options.urls.clone();
        if let Some(ref list_file) = self.options.url_list_file {
            if let Ok(content) = fs::read_to_string(list_file) {
                for line in content.lines() {
                    let t = line.trim();
                    if !t.is_empty() && !t.starts_with('#') {
                        raw_seeds.push(t.to_string());
                    }
                }
            }
        }

        let mut seed_urls = Vec::new();
        for raw in &raw_seeds {
            let with_scheme = if raw.starts_with("http://") || raw.starts_with("https://") {
                raw.clone()
            } else {
                format!("http://{}", raw)
            };
            if let Ok(u) = Url::parse(&with_scheme) {
                seed_urls.push(normalize_url(u, true));
            }
        }

        let filter_engine = FilterEngine::from_options(&self.options);
        let mut robots_map: HashMap<String, RobotsRules> = HashMap::new();
        let mut cache_records: Vec<CacheRecord> = Vec::new();
        let mut used_local_paths: HashSet<String> = HashSet::new();
        let mut url_to_local_map: HashMap<String, String> = HashMap::new();
        let mut visited_urls: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<QueueItem> = VecDeque::new();
        let mut deferred_docs: Vec<FetchedDocument> = Vec::new();
        let mut inline_assets_data_uri: HashMap<String, String> = HashMap::new();
        let mut seed_index_entries: Vec<(String, String)> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        // 1. Fetch robots.txt & sitemaps for seed hosts
        let mut seen_hosts = HashSet::new();
        for seed in &seed_urls {
            let origin = format!(
                "{}://{}{}",
                seed.scheme(),
                seed.host_str().unwrap_or(""),
                seed.port().map(|p| format!(":{}", p)).unwrap_or_default()
            );
            if seen_hosts.insert(origin.clone()) && self.options.robots_mode > 0 {
                let robots_url = format!("{}/robots.txt", origin);
                if let Ok(resp) = client.get(&robots_url).send().await {
                    let status = resp.status().as_u16();
                    let status_msg = resp.status().canonical_reason().unwrap_or("OK").to_string();
                    let date_hdr = resp
                        .headers()
                        .get("date")
                        .and_then(|v| v.to_str().ok())
                        .map(|d| format!("date:{}", d))
                        .unwrap_or_else(|| format!("date:{}", Utc::now().format("%a, %d %b %Y %H:%M:%S GMT")));
                    let mime = resp
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("text/plain")
                        .split(';')
                        .next()
                        .unwrap_or("text/plain")
                        .trim()
                        .to_string();
                    let body = resp.bytes().await.unwrap_or_default().to_vec();

                    cache_records.push(CacheRecord {
                        time_str: Utc::now().format("%H:%M:%S").to_string(),
                        local_size: body.len(),
                        remote_size: body.len(),
                        status_code: status,
                        status_msg,
                        mime,
                        etag_or_date: date_hdr,
                        url: robots_url.clone(),
                        local_rel_path: None,
                        from_url: String::new(),
                        body_bytes: body.clone(),
                        is_html_or_css: false,
                    });

                    if status >= 200 && status < 300 {
                        let text = String::from_utf8_lossy(&body);
                        let rules = RobotsRules::parse(&text, effective_ua);
                        if self.options.use_sitemap {
                            for sm_url in &rules.sitemaps {
                                self.seed_from_sitemap(&client, sm_url, &mut queue, &mut visited_urls).await;
                            }
                        }
                        robots_map.insert(origin.clone(), rules);
                    }
                }
            }

            if self.options.use_sitemap {
                let sm = self
                    .options
                    .sitemap_url
                    .clone()
                    .unwrap_or_else(|| format!("{}/sitemap.xml", origin));
                self.seed_from_sitemap(&client, &sm, &mut queue, &mut visited_urls).await;
            }
        }

        // Enqueue seed URLs
        for seed in &seed_urls {
            let key = seed.as_str().to_string();
            if visited_urls.insert(key) {
                queue.push_back(QueueItem {
                    url: seed.clone(),
                    depth: 0,
                    from_url: String::new(),
                    is_asset: false,
                });
            }
        }

        {
            let mut st = self.stats.lock().unwrap();
            st.links_total_queued = queue.len();
        }

        if !self.options.quiet {
            println!(
                "Mirror launched on {} by HTTrack Website Copier/3.51-0 (Rust) [XR&CO]",
                launch_time.format("%a, %d %b %Y %H:%M:%S")
            );
            println!("mirroring {} with the wizard help..", self.options.urls.join(" "));
        }

        // 2. Crawl loop
        while let Some(item) = queue.pop_front() {
            if self.stop_flag.load(Ordering::Relaxed) {
                warnings.push("Crawl stopped by user request".to_string());
                break;
            }

            // Check max time limit (-E)
            if let Some(max_secs) = self.options.max_time_secs {
                if start_instant.elapsed().as_secs() >= max_secs {
                    warnings.push(format!("Reached max mirror time limit ({}s)", max_secs));
                    break;
                }
            }

            // Check max overall size (-M)
            if let Some(max_bytes) = self.options.max_overall_size {
                let cur_bytes = self.stats.lock().unwrap().bytes_received as u64;
                if cur_bytes >= max_bytes {
                    warnings.push(format!("Reached max overall size ({} bytes)", max_bytes));
                    break;
                }
            }

            // Check robots.txt rule
            let origin = format!(
                "{}://{}{}",
                item.url.scheme(),
                item.url.host_str().unwrap_or(""),
                item.url.port().map(|p| format!(":{}", p)).unwrap_or_default()
            );
            if self.options.robots_mode > 0 {
                if let Some(rules) = robots_map.get(&origin) {
                    if !rules.is_allowed(item.url.path()) {
                        continue;
                    }
                }
            }

            {
                let mut st = self.stats.lock().unwrap();
                st.current_url = item.url.as_str().to_string();
                st.links_scanned += 1;
                st.elapsed_secs = start_instant.elapsed().as_secs();
            }

            // Fetch URL with retries
            let mut last_err = None;
            let mut response_opt = None;
            for _attempt in 0..=self.options.retries {
                let mut req = client.get(item.url.as_str());
                if !item.from_url.is_empty() {
                    req = req.header(REFERER, &item.from_url);
                }
                match req.send().await {
                    Ok(resp) => {
                        response_opt = Some(resp);
                        break;
                    }
                    Err(e) => {
                        last_err = Some(e.to_string());
                    }
                }
            }

            let resp = match response_opt {
                Some(r) => r,
                None => {
                    let err_msg = format!(
                        "Failed to fetch {} (from {}): {}",
                        item.url,
                        item.from_url,
                        last_err.unwrap_or_default()
                    );
                    let mut st = self.stats.lock().unwrap();
                    st.errors_count += 1;
                    st.errors.push(err_msg);
                    continue;
                }
            };

            let status_code = resp.status().as_u16();
            let status_msg = resp.status().canonical_reason().unwrap_or("OK").to_string();
            let raw_content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            let mime = raw_content_type
                .split(';')
                .next()
                .unwrap_or("application/octet-stream")
                .trim()
                .to_ascii_lowercase();

            let date_gmt = resp
                .headers()
                .get("date")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let date_for_footer = if date_gmt.is_empty() {
                Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
            } else {
                date_gmt.clone()
            };
            let etag_or_date = if let Some(etag) = resp.headers().get("etag").and_then(|v| v.to_str().ok()) {
                format!("etag:{}", etag)
            } else {
                format!("date:{}", date_for_footer)
            };

            let is_html = mime == "text/html" || mime == "application/xhtml+xml";
            let is_css = mime == "text/css";

            // Stage-2 MIME check on the actual HTTP Content-Type header BEFORE reading body!
            // Note: Unless the user explicitly blocked `text/html`, we always let HTML pages be parsed
            // for links even when `--include-mime image/*` is used, so the crawler can actually find images on pages!
            if let Some((allow_mime, reason)) = filter_engine.evaluate_mime(&mime) {
                if !allow_mime {
                    let mut st = self.stats.lock().unwrap();
                    st.links_filtered_out += 1;
                    if st.skipped_files.len() < 25 {
                        st.skipped_files.push(format!("{} [{} -> {}]", item.url, mime, reason));
                    }
                    continue;
                }
            } else if !self.options.include_mimes.is_empty() && !is_html && !is_css {
                // If user specified `--include-mime` whitelist (e.g. `--include-mime image/*`),
                // reject non-HTML assets that did not match the whitelist.
                let mut st = self.stats.lock().unwrap();
                st.links_filtered_out += 1;
                if st.skipped_files.len() < 25 {
                    st.skipped_files.push(format!(
                        "{} [{} not in --include-mime whitelist]",
                        item.url, mime
                    ));
                }
                continue;
            }

            // Check Content-Length against max_file_size_non_html / max_file_size_html before downloading body
            if let Some(cl) = resp.content_length() {
                if !is_html {
                    if let Some(max_non_html) = self.options.max_file_size_non_html {
                        if cl > max_non_html {
                            let mut st = self.stats.lock().unwrap();
                            st.links_filtered_out += 1;
                            st.skipped_files.push(format!("{} [{} bytes > max file size]", item.url, cl));
                            continue;
                        }
                    }
                } else if let Some(max_html) = self.options.max_file_size_html {
                    if cl > max_html {
                        continue;
                    }
                }
            }

            let body_bytes = match resp.bytes().await {
                Ok(b) => b.to_vec(),
                Err(e) => {
                    let mut st = self.stats.lock().unwrap();
                    st.errors_count += 1;
                    st.errors.push(format!("Read error on {}: {}", item.url, e));
                    continue;
                }
            };

            {
                let mut st = self.stats.lock().unwrap();
                st.bytes_received += body_bytes.len();
            }

            if status_code < 200 || status_code >= 300 {
                cache_records.push(CacheRecord {
                    time_str: Utc::now().format("%H:%M:%S").to_string(),
                    local_size: body_bytes.len(),
                    remote_size: body_bytes.len(),
                    status_code,
                    status_msg: status_msg.clone(),
                    mime,
                    etag_or_date,
                    url: item.url.as_str().to_string(),
                    local_rel_path: None,
                    from_url: item.from_url.clone(),
                    body_bytes,
                    is_html_or_css: false,
                });
                let mut st = self.stats.lock().unwrap();
                st.errors_count += 1;
                st.errors.push(format!("HTTP {} ({}) at {} (from {})", status_code, status_msg, item.url, item.from_url));
                continue;
            }

            // Check priority_mode (-p0 spider only, -p1 html only, -p2 non-html only)
            // Also check if user specified `--include-ext` or `--include-mime` and whether HTML itself should be saved
            let mut should_save = match (self.options.action, self.options.priority_mode) {
                (CrawlAction::Spider, _) | (_, 0) => false,
                (CrawlAction::Skeleton, _) | (_, 1) => is_html,
                (_, 2) => !is_html,
                _ => true,
            };

            if is_html && !self.options.include_extensions.is_empty() {
                let ext = extract_url_extension(&item.url).unwrap_or_else(|| "html".to_string());
                if !filter_engine.include_ext.iter().any(|e| e == &ext) {
                    // Still crawl links on the HTML page, if user only asked to save e.g. `--include-ext pdf,png`
                    should_save = true;
                }
            }

            let local_rel_path = if should_save {
                let rel = compute_save_path(
                    &item.url,
                    &mime,
                    &self.options.structure,
                    &mut used_local_paths,
                );
                url_to_local_map.insert(item.url.as_str().to_string(), rel.clone());
                Some(rel)
            } else {
                None
            };

            // If --single-file (%Z) is active and this is a non-HTML asset within single_file_max_size, store data: URI
            if self.options.single_file && !is_html && (body_bytes.len() as u64) <= self.options.single_file_max_size {
                let b64 = BASE64.encode(&body_bytes);
                inline_assets_data_uri.insert(
                    item.url.as_str().to_string(),
                    format!("data:{};base64,{}", mime, b64),
                );
            }

            // Parse links if HTML or CSS
            if is_html || is_css {
                let text = String::from_utf8_lossy(&body_bytes).into_owned();

                // Record seed page title for root index.html
                if item.depth == 0 && is_html {
                    if let Some(ref rel) = local_rel_path {
                        let title = extract_page_title(&text).unwrap_or_else(|| item.url.as_str().to_string());
                        seed_index_entries.push((rel.clone(), title));
                    }
                }

                // Discover links if within depth limit
                let can_follow = !matches!(self.options.action, CrawlAction::GetFiles)
                    && item.depth < self.options.max_depth;

                if can_follow {
                    let discovered = if is_html {
                        extract_html_links(&text, &item.url, self.options.extended_parsing)
                    } else {
                        extract_css_links(&text, &item.url)
                    };

                    for dlink in discovered {
                        let target_key = dlink.resolved_url.as_str().to_string();
                        if visited_urls.contains(&target_key) {
                            continue;
                        }
                        let (allowed, why) = filter_engine.evaluate_url(
                            &dlink.resolved_url,
                            &seed_urls,
                            self.options.host_scope,
                            self.options.dir_scope,
                            self.options.near_files,
                            dlink.is_asset,
                        );
                        if allowed {
                            visited_urls.insert(target_key);
                            queue.push_back(QueueItem {
                                url: dlink.resolved_url,
                                depth: item.depth + 1,
                                from_url: item.url.as_str().to_string(),
                                is_asset: dlink.is_asset,
                            });
                            let mut st = self.stats.lock().unwrap();
                            st.links_total_queued += 1;
                        } else {
                            visited_urls.insert(target_key);
                            let mut st = self.stats.lock().unwrap();
                            st.links_filtered_out += 1;
                            if st.skipped_files.len() < 25 {
                                st.skipped_files.push(format!("{} [{}]", dlink.resolved_url, why));
                            }
                        }
                    }
                }

                // Defer writing HTML/CSS until all URLs have been mapped to local paths so cross-links resolve cleanly!
                if let Some(ref rel) = local_rel_path {
                    deferred_docs.push(FetchedDocument {
                        url: item.url.clone(),
                        local_rel_path: rel.clone(),
                        mime: mime.clone(),
                        status_code,
                        date_gmt: date_for_footer,
                        text,
                        is_html,
                        is_css,
                    });
                }
            } else if let Some(ref rel) = local_rel_path {
                // Write binary asset immediately to disk
                if let Some(dest_path) = safe_join_contained(&self.options.output_dir, rel) {
                    if let Some(parent) = dest_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&dest_path, &body_bytes)?;
                    let mut st = self.stats.lock().unwrap();
                    st.files_written += 1;
                    st.bytes_written += body_bytes.len();
                    if st.recent_files.len() >= 25 {
                        st.recent_files.remove(0);
                    }
                    st.recent_files.push(rel.clone());
                }
            }

            cache_records.push(CacheRecord {
                time_str: Utc::now().format("%H:%M:%S").to_string(),
                local_size: body_bytes.len(),
                remote_size: body_bytes.len(),
                status_code,
                status_msg,
                mime,
                etag_or_date,
                url: item.url.as_str().to_string(),
                local_rel_path,
                from_url: item.from_url,
                body_bytes,
                is_html_or_css: is_html || is_css,
            });
        }

        // 3. Rewrite and write all deferred HTML and CSS documents now that `url_to_local_map` is complete
        let inline_map_opt = if self.options.single_file {
            Some(&inline_assets_data_uri)
        } else {
            None
        };

        for doc in &deferred_docs {
            let final_content = if doc.is_html {
                let host_port = match doc.url.port() {
                    Some(p) => format!("{}:{}", doc.url.host_str().unwrap_or(""), p),
                    None => doc.url.host_str().unwrap_or("").to_string(),
                };
                let footer = self.options.format_footer(
                    doc.url.as_str(),
                    &host_port,
                    doc.url.path(),
                    &doc.date_gmt,
                    &doc.mime,
                    doc.status_code,
                    doc.text.len(),
                );
                rewrite_html(
                    &doc.text,
                    &doc.url,
                    &doc.local_rel_path,
                    &url_to_local_map,
                    self.options.link_mode,
                    &footer,
                    inline_map_opt,
                )
            } else if doc.is_css {
                rewrite_css(
                    &doc.text,
                    &doc.url,
                    &doc.local_rel_path,
                    &url_to_local_map,
                    self.options.link_mode,
                    inline_map_opt,
                )
            } else {
                doc.text.clone()
            };

            if let Some(dest_path) = safe_join_contained(&self.options.output_dir, &doc.local_rel_path) {
                if let Some(parent) = dest_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let bytes = final_content.as_bytes();
                fs::write(&dest_path, bytes)?;
                let mut st = self.stats.lock().unwrap();
                st.files_written += 1;
                st.bytes_written += bytes.len();
                if st.recent_files.len() >= 25 {
                    st.recent_files.remove(0);
                }
                st.recent_files.push(doc.local_rel_path.clone());
            }
        }

        // 4. Write root index.html, backblue.gif, fade.gif if make_index (-I) is enabled
        let mut static_files_written = Vec::new();
        if self.options.make_index && !seed_index_entries.is_empty() {
            if let Ok(written) = write_mirror_index(&self.options.output_dir, &seed_index_entries) {
                static_files_written = written;
            }
        }

        let finish_time = Utc::now();
        let elapsed_secs = start_instant.elapsed().as_secs();

        // 5. Write hts-cache/ (doit.log, new.lst, new.txt, new.zip)
        if self.options.cache_mode > 0 {
            write_hts_cache(
                &self.options.output_dir,
                &self.options,
                &cache_records,
                &static_files_written,
                launch_time,
            )?;
        }

        // 6. Write hts-changes.json if --changes (-%d) is enabled
        if self.options.generate_changes {
            write_changes_json(&self.options.output_dir, &cache_records, finish_time)?;
        }

        // 7. Write WARC/1.1 archive if --warc (-%r) is enabled
        if self.options.generate_warc {
            write_warc_archive(
                &self.options.output_dir,
                self.options.warc_file.as_deref(),
                &cache_records,
                launch_time,
            )?;
        }

        // 8. Write hts-log.txt
        let final_stats = {
            let mut st = self.stats.lock().unwrap();
            st.is_running = false;
            st.is_finished = true;
            st.elapsed_secs = elapsed_secs;
            st.warnings_count = warnings.len();
            st.clone()
        };

        write_hts_log(
            &self.options.output_dir,
            &self.options,
            launch_time,
            elapsed_secs,
            final_stats.links_scanned,
            final_stats.files_written,
            final_stats.bytes_written,
            final_stats.bytes_received,
            &final_stats.errors,
            &warnings,
        )?;

        // Remove hts-in_progress.lock
        let _ = fs::remove_file(&lock_path);

        if !self.options.quiet {
            println!("Done.");
            println!("Thanks for using HTTrack!");
        }

        Ok(final_stats)
    }

    async fn seed_from_sitemap(
        &self,
        client: &reqwest::Client,
        sitemap_url: &str,
        queue: &mut VecDeque<QueueItem>,
        visited: &mut HashSet<String>,
    ) {
        if let Ok(resp) = client.get(sitemap_url).send().await {
            if resp.status().is_success() {
                if let Ok(xml) = resp.text().await {
                    let loc_re = Regex::new(r"(?i)<loc>\s*([^<\s]+)\s*</loc>").unwrap();
                    for cap in loc_re.captures_iter(&xml) {
                        if let Ok(u) = Url::parse(&cap[1]) {
                            let norm = normalize_url(u, true);
                            if visited.insert(norm.as_str().to_string()) {
                                queue.push_back(QueueItem {
                                    url: norm,
                                    depth: 0,
                                    from_url: sitemap_url.to_string(),
                                    is_asset: false,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}

fn extract_page_title(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("title").ok()?;
    let el = doc.select(&sel).next()?;
    let title: String = el.text().collect();
    let trimmed = title.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Parse a Netscape / browser-exported `cookies.txt` file or simple `key=value` cookie file
fn parse_netscape_cookies_file(path: &Path) -> anyhow::Result<String> {
    let text = fs::read_to_string(path)?;
    let mut pairs = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() >= 7 {
            pairs.push(format!("{}={}", cols[5].trim(), cols[6].trim()));
        } else if line.contains('=') && !line.contains('\t') {
            pairs.push(line.to_string());
        }
    }
    Ok(pairs.join("; "))
}
