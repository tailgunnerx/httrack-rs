use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use md5::{Digest, Md5};
use url::Url;
use crate::options::StructureType;

/// Compute the 4-hex-char query hash (`%q`) used by HTTrack (`md5(query)[0..4]`)
/// Verified via `httrack.exe -#test=savename "/folder/foo.cgi?q=45" "text/html"` -> `fooa789.html`
pub fn query_md5_short(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }
    let mut hasher = Md5::new();
    hasher.update(query.as_bytes());
    let result = hasher.finalize();
    format!("{:02x}{:02x}", result[0], result[1])
}

/// Compute full 32-hex-char MD5 (`%M` or `%Q`)
pub fn md5_full(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Map MIME type to preferred file extension when extension is missing or dynamic (`cgi`, `php`, `asp`, etc.)
pub fn mime_to_ext(mime: &str) -> Option<&'static str> {
    let base_mime = mime.split(';').next().unwrap_or(mime).trim().to_ascii_lowercase();
    match base_mime.as_str() {
        "text/html" | "application/xhtml+xml" => Some("html"),
        "text/css" => Some("css"),
        "application/javascript" | "text/javascript" | "application/x-javascript" => Some("js"),
        "application/json" => Some("json"),
        "application/xml" | "text/xml" => Some("xml"),
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/svg+xml" => Some("svg"),
        "image/webp" => Some("webp"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
        "font/woff2" => Some("woff2"),
        "font/woff" => Some("woff"),
        "application/pdf" => Some("pdf"),
        "text/plain" => Some("txt"),
        _ => None,
    }
}

pub fn is_dynamic_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "cgi" | "php" | "php3" | "php4" | "php5" | "asp" | "aspx" | "jsp" | "pl" | "cfm" | "dll" | "do" | "action"
    )
}

/// Sanitize a single path segment for Windows/POSIX filesystems
pub fn sanitize_segment(seg: &str) -> String {
    let mut s: String = seg
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    if s.is_empty() {
        "_".to_string()
    } else {
        s
    }
}

/// Format host folder name matching HTTrack (e.g. `127.0.0.1_18923` or `www.example.com`)
pub fn host_folder_name(url: &Url) -> String {
    let host = url.host_str().unwrap_or("unknown");
    match url.port() {
        Some(port) => format!("{}_{}", sanitize_segment(host), port),
        None => sanitize_segment(host),
    }
}

/// Build the relative local save path (using forward slashes, relative to `output_dir`)
/// matching HTTrack's `savename` algorithm and `-N` structure options, plus collision resolution (`index-2.html`).
pub fn compute_save_path(
    url: &Url,
    content_type: &str,
    structure: &StructureType,
    used_paths: &mut HashSet<String>,
) -> String {
    let host_dir = host_folder_name(url);
    let raw_path = url.path();

    // Split path into directory segments and filename
    let (dir_segments, raw_filename) = if raw_path.is_empty() || raw_path.ends_with('/') {
        let segs: Vec<String> = raw_path
            .split('/')
            .filter(|s| !s.is_empty() && *s != "." && *s != "..")
            .map(sanitize_segment)
            .collect();
        (segs, "index.html".to_string())
    } else {
        let parts: Vec<&str> = raw_path
            .split('/')
            .filter(|s| !s.is_empty() && *s != "." && *s != "..")
            .collect();
        if parts.is_empty() {
            (Vec::new(), "index.html".to_string())
        } else {
            let file = sanitize_segment(parts[parts.len() - 1]);
            let dirs = parts[..parts.len() - 1]
                .iter()
                .map(|s| sanitize_segment(s))
                .collect();
            (dirs, file)
        }
    };

    // Split filename into stem and extension
    let (mut stem, mut ext) = match raw_filename.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() => (s.to_string(), e.to_string()),
        _ => (raw_filename.clone(), String::new()),
    };

    let is_html_mime = content_type.to_ascii_lowercase().contains("text/html")
        || content_type.to_ascii_lowercase().contains("application/xhtml+xml");

    if ext.is_empty() || is_dynamic_ext(&ext) || (is_html_mime && ext.to_ascii_lowercase() != "html" && ext.to_ascii_lowercase() != "htm") {
        if let Some(mapped) = mime_to_ext(content_type) {
            ext = mapped.to_string();
        } else if ext.is_empty() {
            ext = "html".to_string();
        }
    }

    // Append 4-char MD5 of query string if present (HTTrack's `fooa789.html`)
    if let Some(q) = url.query() {
        if !q.is_empty() {
            let qhash = query_md5_short(q);
            stem.push_str(&qhash);
        }
    }

    let dir_joined = if dir_segments.is_empty() {
        String::new()
    } else {
        dir_segments.join("/")
    };

    let base_rel = match structure {
        StructureType::UserDefined(template) => {
            expand_user_structure(template, url, &host_dir, &dir_joined, &stem, &ext)
        }
        StructureType::Preset(preset) => {
            let file_with_ext = if ext.is_empty() {
                stem.clone()
            } else {
                format!("{}.{}", stem, ext)
            };
            match preset {
                // N100: Site-structure, without www.domain.xxx/
                100 => {
                    if dir_joined.is_empty() {
                        file_with_ext
                    } else {
                        format!("{}/{}", dir_joined, file_with_ext)
                    }
                }
                // N1: HTML in web/, images/other in web/images/
                1 => {
                    if is_html_mime {
                        format!("web/{}", file_with_ext)
                    } else {
                        format!("web/images/{}", file_with_ext)
                    }
                }
                // N2: HTML in web/HTML, images/other in web/images
                2 => {
                    if is_html_mime {
                        format!("web/HTML/{}", file_with_ext)
                    } else {
                        format!("web/images/{}", file_with_ext)
                    }
                }
                // N3: All in web/
                3 => format!("web/{}", file_with_ext),
                // N4: HTML in web/, other in web/<ext>/
                4 => {
                    if is_html_mime {
                        format!("web/{}", file_with_ext)
                    } else {
                        format!("web/{}/{}", if ext.is_empty() { "other" } else { &ext }, file_with_ext)
                    }
                }
                // N1003: flat in output directory (used by -g / --get-files)
                1003 => file_with_ext,
                // N0 (default): host/path/file.ext
                _ => {
                    if dir_joined.is_empty() {
                        format!("{}/{}", host_dir, file_with_ext)
                    } else {
                        format!("{}/{}/{}", host_dir, dir_joined, file_with_ext)
                    }
                }
            }
        }
    };

    // Deduplicate colliding save paths (`index.html` -> `index-2.html`, `index-3.html` just like HTTrack!)
    let mut candidate = base_rel.clone();
    let mut counter = 2usize;
    while used_paths.contains(&candidate.to_ascii_lowercase()) {
        candidate = match base_rel.rsplit_once('.') {
            Some((prefix, extension)) => format!("{}-{}.{}", prefix, counter, extension),
            None => format!("{}-{}", base_rel, counter),
        };
        counter += 1;
    }
    used_paths.insert(candidate.to_ascii_lowercase());
    candidate
}

fn expand_user_structure(
    template: &str,
    url: &Url,
    host_dir: &str,
    dir_joined: &str,
    stem: &str,
    ext: &str,
) -> String {
    let q = url.query().unwrap_or("");
    let q_short = query_md5_short(q);
    let q_full = if q.is_empty() { String::new() } else { md5_full(q) };
    let url_md5 = md5_full(url.as_str());
    let file_full = if ext.is_empty() { stem.to_string() } else { format!("{}.{}", stem, ext) };
    let path_str = if dir_joined.is_empty() { String::new() } else { format!("/{}", dir_joined) };

    let mut out = template.to_string();
    out = out.replace("%h", host_dir);
    out = out.replace("%p", &path_str);
    out = out.replace("%n", stem);
    out = out.replace("%N", &file_full);
    out = out.replace("%t", ext);
    out = out.replace("%q", &q_short);
    out = out.replace("%Q", &q_full);
    out = out.replace("%M", &url_md5);
    out = out.replace("%k", q);
    out = out.replace("%r", url.scheme());
    out.trim_start_matches('/').replace("//", "/")
}

/// Compute relative URI from `curr_file` to `target_file` (both relative to mirror root, using `/`).
/// Matches `httrack.exe -#test=relative "www.example.com/a/b/c.html" "www.example.com/a/index.html"` -> `b/c.html`
pub fn compute_relative_path(target_file: &str, curr_file: &str) -> String {
    let target_parts: Vec<&str> = target_file.split('/').filter(|s| !s.is_empty()).collect();
    let curr_parts: Vec<&str> = curr_file.split('/').filter(|s| !s.is_empty()).collect();

    let curr_dir = if curr_parts.len() > 1 {
        &curr_parts[..curr_parts.len() - 1]
    } else {
        &[][..]
    };

    let mut common = 0usize;
    while common < curr_dir.len()
        && common + 1 < target_parts.len()
        && curr_dir[common].eq_ignore_ascii_case(target_parts[common])
    {
        common += 1;
    }

    let mut rel_parts: Vec<&str> = Vec::new();
    for _ in common..curr_dir.len() {
        rel_parts.push("..");
    }
    for part in &target_parts[common..] {
        rel_parts.push(part);
    }

    if rel_parts.is_empty() {
        target_parts.last().copied().unwrap_or("index.html").to_string()
    } else {
        rel_parts.join("/")
    }
}

/// Ensure a relative path is safely contained within `root` (matching `-#test=pathcontained`)
pub fn safe_join_contained(root: &Path, rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    let mut out = root.to_path_buf();
    for comp in p.components() {
        match comp {
            Component::Normal(seg) => out.push(seg),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}
