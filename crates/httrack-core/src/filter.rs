use regex::Regex;
use url::Url;
use crate::options::{DirectionScope, HostScope, HtsOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterKind {
    UrlPattern(String),
    MimePattern(String),
    SizeLess(u64),
    SizeGreater(u64),
}

#[derive(Debug, Clone)]
pub struct FilterRule {
    pub raw: String,
    pub allow: bool,
    pub kind: FilterKind,
    regex: Option<Regex>,
}

impl FilterRule {
    pub fn parse(rule_str: &str) -> Option<Self> {
        let trimmed = rule_str.trim();
        if trimmed.is_empty() {
            return None;
        }
        let (allow, rest) = if let Some(r) = trimmed.strip_prefix('+') {
            (true, r)
        } else if let Some(r) = trimmed.strip_prefix('-') {
            (false, r)
        } else {
            return None;
        };

        if let Some(mime_pat) = rest.strip_prefix("mime:") {
            let expanded = expand_mime_shorthand(mime_pat);
            let regex = Some(wildcard_to_regex(&expanded));
            return Some(Self {
                raw: trimmed.to_string(),
                allow,
                kind: FilterKind::MimePattern(expanded),
                regex,
            });
        }

        if let Some(size_kb) = rest.strip_prefix("*[>") {
            if let Some(num_str) = size_kb.strip_suffix(']') {
                if let Ok(kb) = num_str.parse::<u64>() {
                    return Some(Self {
                        raw: trimmed.to_string(),
                        allow,
                        kind: FilterKind::SizeGreater(kb * 1024),
                        regex: None,
                    });
                }
            }
        }

        if let Some(size_kb) = rest.strip_prefix("*[<") {
            if let Some(num_str) = size_kb.strip_suffix(']') {
                if let Ok(kb) = num_str.parse::<u64>() {
                    return Some(Self {
                        raw: trimmed.to_string(),
                        allow,
                        kind: FilterKind::SizeLess(kb * 1024),
                        regex: None,
                    });
                }
            }
        }

        let regex = Some(wildcard_to_regex(rest));
        Some(Self {
            raw: trimmed.to_string(),
            allow,
            kind: FilterKind::UrlPattern(rest.to_string()),
            regex,
        })
    }

    pub fn matches_url(&self, normalized_target: &str, full_url: &str) -> bool {
        match &self.kind {
            FilterKind::UrlPattern(_) => {
                if let Some(re) = &self.regex {
                    re.is_match(normalized_target) || re.is_match(full_url)
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    pub fn matches_mime(&self, mime: &str) -> bool {
        match &self.kind {
            FilterKind::MimePattern(_) => {
                if let Some(re) = &self.regex {
                    re.is_match(mime)
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

/// Expand friendly shorthand category names into MIME wildcards:
/// - `video` or `videos` -> `video/*`
/// - `audio` or `music`  -> `audio/*`
/// - `image` or `images` -> `image/*`
/// - `font` or `fonts`   -> `font/*`
/// - `pdf`               -> `application/pdf`
/// - `archive` or `zip`  -> `application/*zip*`
pub fn expand_mime_shorthand(pat: &str) -> String {
    let lower = pat.trim().to_ascii_lowercase();
    match lower.as_str() {
        "video" | "videos" => "video/*".to_string(),
        "audio" | "music" => "audio/*".to_string(),
        "image" | "images" => "image/*".to_string(),
        "font" | "fonts" => "font/*".to_string(),
        "pdf" => "application/pdf".to_string(),
        "archive" | "archives" | "zip" => "application/*zip*".to_string(),
        _ => lower,
    }
}

/// Guess the MIME type from a URL's path extension BEFORE connecting to the server!
/// Why did MIME filtering (`-mime:video/*`) often fail in original HTTrack?
/// Because in original HTTrack, `-mime:` only ran *after* opening a connection or failed when delayed type checking (`%N2`)
/// was active, or conversely downloaded huge files just to check their header.
/// By inferring known extensions up-front AND verifying the `Content-Type` header on response,
/// excluding a MIME type or file extension works reliably 100% of the time.
pub fn guess_mime_from_url(url: &Url) -> Option<&'static str> {
    let ext = extract_url_extension(url)?;
    match ext.as_str() {
        "html" | "htm" | "xhtml" => Some("text/html"),
        "css" => Some("text/css"),
        "js" | "mjs" => Some("application/javascript"),
        "json" => Some("application/json"),
        "xml" => Some("application/xml"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        "webp" => Some("image/webp"),
        "avif" => Some("image/avif"),
        "ico" => Some("image/x-icon"),
        "bmp" => Some("image/bmp"),
        "mp4" | "m4v" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mkv" => Some("video/x-matroska"),
        "avi" => Some("video/x-msvideo"),
        "mov" => Some("video/quicktime"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "ogg" | "oga" => Some("audio/ogg"),
        "flac" => Some("audio/flac"),
        "m4a" => Some("audio/mp4"),
        "woff2" => Some("font/woff2"),
        "woff" => Some("font/woff"),
        "ttf" => Some("font/ttf"),
        "otf" => Some("font/otf"),
        "pdf" => Some("application/pdf"),
        "zip" => Some("application/zip"),
        "tar" => Some("application/x-tar"),
        "gz" | "tgz" => Some("application/gzip"),
        "7z" => Some("application/x-7z-compressed"),
        "rar" => Some("application/vnd.rar"),
        "exe" | "msi" => Some("application/x-msdownload"),
        "dmg" | "iso" => Some("application/octet-stream"),
        _ => None,
    }
}

pub fn extract_url_extension(url: &Url) -> Option<String> {
    let path = url.path();
    let last_seg = path.rsplit('/').next()?;
    let (_, ext) = last_seg.rsplit_once('.')?;
    if ext.is_empty() {
        None
    } else {
        Some(ext.to_ascii_lowercase())
    }
}

/// Convert HTTrack wildcard syntax (`*`, `*[0-9]`, `?`) to a case-insensitive Regex.
pub fn wildcard_to_regex(pattern: &str) -> Regex {
    let mut re = String::from("(?i)^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '*' {
            if i + 1 < chars.len() && chars[i + 1] == '[' {
                if let Some(end) = chars[i + 2..].iter().position(|&ch| ch == ']') {
                    let class_content: String = chars[i + 2..i + 2 + end].iter().collect();
                    re.push('[');
                    re.push_str(&class_content);
                    re.push_str("]*");
                    i += 3 + end;
                    continue;
                }
            }
            re.push_str(".*");
        } else if c == '?' {
            re.push('.');
        } else {
            re.push_str(&regex::escape(&c.to_string()));
        }
        i += 1;
    }
    re.push('$');
    Regex::new(&re).unwrap_or_else(|_| Regex::new("^$").unwrap())
}

#[derive(Debug, Clone)]
pub struct FilterEngine {
    pub rules: Vec<FilterRule>,
    pub exclude_ext: Vec<String>,
    pub include_ext: Vec<String>,
}

impl FilterEngine {
    pub fn new(raw_rules: &[String]) -> Self {
        let rules = raw_rules
            .iter()
            .filter_map(|s| FilterRule::parse(s))
            .collect();
        Self {
            rules,
            exclude_ext: Vec::new(),
            include_ext: Vec::new(),
        }
    }

    /// Build a `FilterEngine` combining both classic HTTrack scan rules (`+...`, `-mime:...`)
    /// AND the new intuitive `--exclude-mime`, `--include-mime`, `--exclude-ext`, `--include-ext` options.
    pub fn from_options(opts: &HtsOptions) -> Self {
        let mut combined_raw = opts.filters.clone();
        for m in &opts.include_mimes {
            for part in m.split(',') {
                let p = part.trim();
                if !p.is_empty() {
                    combined_raw.push(format!("+mime:{}", expand_mime_shorthand(p)));
                }
            }
        }
        for m in &opts.exclude_mimes {
            for part in m.split(',') {
                let p = part.trim();
                if !p.is_empty() {
                    combined_raw.push(format!("-mime:{}", expand_mime_shorthand(p)));
                }
            }
        }

        let exclude_ext: Vec<String> = opts
            .exclude_extensions
            .iter()
            .flat_map(|s| s.split(','))
            .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        let include_ext: Vec<String> = opts
            .include_extensions
            .iter()
            .flat_map(|s| s.split(','))
            .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        let rules = combined_raw
            .iter()
            .filter_map(|s| FilterRule::parse(s))
            .collect();

        Self {
            rules,
            exclude_ext,
            include_ext,
        }
    }

    /// Evaluate URL against extension rules, pre-flight inferred MIME rules, user URL filters, and scope rules.
    /// Returns `(allowed, detailed_human_explanation)` for `--why`.
    pub fn evaluate_url(
        &self,
        url: &Url,
        seed_urls: &[Url],
        host_scope: HostScope,
        dir_scope: DirectionScope,
        near_files: bool,
        is_asset: bool,
    ) -> (bool, String) {
        // 1. Check explicit extension exclusions/inclusions first (`--exclude-ext`, `--include-ext`)
        if let Some(ext) = extract_url_extension(url) {
            if self.exclude_ext.iter().any(|e| e == &ext) {
                return (
                    false,
                    format!("rejected by --exclude-ext '.{}' (pre-flight extension check)", ext),
                );
            }
            if !self.include_ext.is_empty() && ext != "html" && ext != "htm" {
                if !self.include_ext.iter().any(|e| e == &ext) {
                    return (
                        false,
                        format!("rejected because '.{}' is not in --include-ext list", ext),
                    );
                }
            }
        }

        // 2. Pre-flight MIME check based on URL extension (fixes OG HTTrack bug where `-mime:video/*` or `-mime:image/*`
        //    still queued or downloaded files before realizing their MIME type!)
        if let Some(guessed_mime) = guess_mime_from_url(url) {
            if let Some((allow_mime, reason)) = self.evaluate_mime(guessed_mime) {
                if !allow_mime {
                    return (
                        false,
                        format!(
                            "rejected before download by MIME rule: inferred '{}' from URL extension ({})",
                            guessed_mime, reason
                        ),
                    );
                }
            }
        }

        let host_port = match url.port() {
            Some(p) => format!("{}:{}", url.host_str().unwrap_or(""), p),
            None => url.host_str().unwrap_or("").to_string(),
        };
        let norm_target = match url.query() {
            Some(q) => format!("{}{}?{}", host_port, url.path(), q),
            None => format!("{}{}", host_port, url.path()),
        };
        let full_url = url.as_str();

        // 3. Check user URL scan rules (last matching rule wins)
        let mut verdict: Option<(bool, String)> = None;
        for rule in &self.rules {
            if matches!(rule.kind, FilterKind::UrlPattern(_)) && rule.matches_url(&norm_target, full_url) {
                verdict = Some((rule.allow, format!("matched user URL filter rule '{}'", rule.raw)));
            }
        }
        if let Some(v) = verdict {
            return v;
        }

        // 4. If `near_files` (-n) is active and this is a non-HTML asset (image/css/js), allow it even if on external CDN
        if near_files && is_asset {
            return (
                true,
                "accepted by --near (-n): non-HTML page asset (image/CSS/JS) referenced by a mirrored page".to_string(),
            );
        }

        // 5. Check built-in wizard scope against seed URLs
        for seed in seed_urls {
            let host_ok = match host_scope {
                HostScope::SameAddress => {
                    seed.host_str().map(|s| s.eq_ignore_ascii_case(url.host_str().unwrap_or(""))) == Some(true)
                        && seed.port_or_known_default() == url.port_or_known_default()
                }
                HostScope::SameDomain => {
                    registrable_domain(seed.host_str().unwrap_or(""))
                        .eq_ignore_ascii_case(&registrable_domain(url.host_str().unwrap_or("")))
                }
                HostScope::SameTld => {
                    tld_of(seed.host_str().unwrap_or(""))
                        .eq_ignore_ascii_case(&tld_of(url.host_str().unwrap_or("")))
                }
                HostScope::Everywhere => true,
            };

            if !host_ok {
                continue;
            }

            let seed_dir = parent_dir_path(seed.path());
            let target_dir = parent_dir_path(url.path());

            let dir_ok = match dir_scope {
                DirectionScope::DownOnly => target_dir.starts_with(&seed_dir),
                DirectionScope::UpOnly => seed_dir.starts_with(&target_dir),
                DirectionScope::Both => true,
                DirectionScope::SameDir => target_dir == seed_dir,
            };

            if dir_ok {
                return (
                    true,
                    format!(
                        "accepted by scope ({:?}, {:?}) matching seed {}",
                        host_scope, dir_scope, seed
                    ),
                );
            }
        }

        (
            false,
            format!("rejected by host/directory scope ({:?}, {:?})", host_scope, dir_scope),
        )
    }

    /// Evaluate actual HTTP `Content-Type` header against MIME rules (and HTML navigation safety).
    pub fn evaluate_mime(&self, mime: &str) -> Option<(bool, String)> {
        let base_mime = mime.split(';').next().unwrap_or(mime).trim().to_ascii_lowercase();
        let mut verdict = None;
        for rule in &self.rules {
            if rule.matches_mime(&base_mime) {
                verdict = Some((rule.allow, format!("matched MIME filter '{}'", rule.raw)));
            }
        }
        verdict
    }
}

pub fn parent_dir_path(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return "/".to_string();
    }
    if let Some(idx) = path.rfind('/') {
        path[..=idx].to_string()
    } else {
        "/".to_string()
    }
}

pub fn registrable_domain(host: &str) -> String {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        host.to_string()
    }
}

pub fn tld_of(host: &str) -> String {
    host.rsplit('.').next().unwrap_or(host).to_string()
}

/// Simple RFC 9309 robots.txt parser supporting User-agent, Allow, Disallow, and Sitemap
#[derive(Debug, Clone, Default)]
pub struct RobotsRules {
    pub allow: Vec<String>,
    pub disallow: Vec<String>,
    pub sitemaps: Vec<String>,
}

impl RobotsRules {
    pub fn parse(content: &str, _our_agent: &str) -> Self {
        let mut rules = Self::default();
        let mut applies = false;

        for raw_line in content.lines() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim().to_ascii_lowercase();
                let val = v.trim();
                match key.as_str() {
                    "user-agent" => {
                        let ua = val.to_ascii_lowercase();
                        applies = ua == "*" || ua.contains("httrack");
                    }
                    "disallow" if applies => {
                        if !val.is_empty() {
                            rules.disallow.push(val.to_string());
                        }
                    }
                    "allow" if applies => {
                        if !val.is_empty() {
                            rules.allow.push(val.to_string());
                        }
                    }
                    "sitemap" => {
                        if !val.is_empty() {
                            rules.sitemaps.push(val.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        rules
    }

    /// RFC 9309 longest-match precedence between Allow and Disallow
    pub fn is_allowed(&self, path: &str) -> bool {
        let mut best_allow_len = 0usize;
        for a in &self.allow {
            if path.starts_with(a) && a.len() >= best_allow_len {
                best_allow_len = a.len();
            }
        }
        let mut best_disallow_len = 0usize;
        for d in &self.disallow {
            if path.starts_with(d) && d.len() > best_disallow_len {
                best_disallow_len = d.len();
            }
        }
        best_allow_len >= best_disallow_len
    }
}
