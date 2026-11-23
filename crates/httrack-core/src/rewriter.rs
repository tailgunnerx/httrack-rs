use std::collections::HashMap;
use regex::Regex;
use scraper::{Html, Selector};
use url::Url;
use crate::options::LinkRewriteMode;
use crate::savename::compute_relative_path;

#[derive(Debug, Clone)]
pub struct DiscoveredLink {
    pub raw_value: String,
    pub resolved_url: Url,
    pub is_asset: bool,
}

/// Normalize a URL according to HTTrack's `%u` url-hack rules
pub fn normalize_url(mut url: Url, strip_fragment: bool) -> Url {
    if strip_fragment {
        url.set_fragment(None);
    }
    // Strip default ports (:80 for http, :443 for https)
    if (url.scheme() == "http" && url.port() == Some(80))
        || (url.scheme() == "https" && url.port() == Some(443))
    {
        let _ = url.set_port(None);
    }
    if url.path().is_empty() {
        url.set_path("/");
    }
    url
}

/// Resolve a raw link found in HTML/CSS against the page's base URL (respecting `<base href="...">` if present)
pub fn resolve_link(base_url: &Url, raw: &str) -> Option<Url> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.to_ascii_lowercase().starts_with("javascript:")
        || trimmed.to_ascii_lowercase().starts_with("mailto:")
        || trimmed.to_ascii_lowercase().starts_with("tel:")
        || trimmed.to_ascii_lowercase().starts_with("data:")
        || trimmed.to_ascii_lowercase().starts_with("about:")
    {
        return None;
    }
    let joined = base_url.join(trimmed).ok()?;
    if joined.scheme() != "http" && joined.scheme() != "https" {
        return None;
    }
    Some(normalize_url(joined, true))
}

/// Extract all links from an HTML document (DOM attributes + inline `<style>` + `style="..."` + extended JS/dirty parsing)
pub fn extract_html_links(html_text: &str, page_url: &Url, extended_parsing: bool) -> Vec<DiscoveredLink> {
    let doc = Html::parse_document(html_text);

    // Check for <base href="...">
    let effective_base = if let Ok(base_sel) = Selector::parse("base[href]") {
        doc.select(&base_sel)
            .next()
            .and_then(|el| el.value().attr("href"))
            .and_then(|h| page_url.join(h).ok())
            .unwrap_or_else(|| page_url.clone())
    } else {
        page_url.clone()
    };

    let mut out = Vec::new();

    // Standard navigational & asset selectors
    let attr_rules: &[(&str, &str, bool)] = &[
        ("a[href]", "href", false),
        ("area[href]", "href", false),
        ("frame[src]", "src", false),
        ("iframe[src]", "src", false),
        ("link[href]", "href", true),
        ("img[src]", "src", true),
        ("img[data-src]", "data-src", true),
        ("script[src]", "src", true),
        ("source[src]", "src", true),
        ("audio[src]", "src", true),
        ("video[src]", "src", true),
        ("video[poster]", "poster", true),
        ("embed[src]", "src", true),
        ("object[data]", "data", true),
        ("form[action]", "action", false),
    ];

    for &(sel_str, attr_name, is_asset) in attr_rules {
        if let Ok(sel) = Selector::parse(sel_str) {
            for el in doc.select(&sel) {
                if let Some(raw_val) = el.value().attr(attr_name) {
                    // Check if <link rel="canonical" or similar navigational vs stylesheet>
                    let mut asset_flag = is_asset;
                    if sel_str == "link[href]" {
                        if let Some(rel) = el.value().attr("rel") {
                            let r = rel.to_ascii_lowercase();
                            if !r.contains("stylesheet") && !r.contains("icon") && !r.contains("preload") {
                                asset_flag = false;
                            }
                        }
                    }
                    if let Some(resolved) = resolve_link(&effective_base, raw_val) {
                        out.push(DiscoveredLink {
                            raw_value: raw_val.to_string(),
                            resolved_url: resolved,
                            is_asset: asset_flag,
                        });
                    }
                }
            }
        }
    }

    // Extract srcset attributes (e.g. `<img srcset="a.png 1x, b.png 2x">` or `<source srcset="...">`)
    if let Ok(srcset_sel) = Selector::parse("[srcset]") {
        for el in doc.select(&srcset_sel) {
            if let Some(srcset_val) = el.value().attr("srcset") {
                for item in srcset_val.split(',') {
                    if let Some(first_tok) = item.split_whitespace().next() {
                        if let Some(resolved) = resolve_link(&effective_base, first_tok) {
                            out.push(DiscoveredLink {
                                raw_value: first_tok.to_string(),
                                resolved_url: resolved,
                                is_asset: true,
                            });
                        }
                    }
                }
            }
        }
    }

    // Extract meta http-equiv="refresh" content="0; url=..."
    if let Ok(meta_sel) = Selector::parse("meta[http-equiv][content]") {
        for el in doc.select(&meta_sel) {
            let eq = el.value().attr("http-equiv").unwrap_or("");
            if eq.eq_ignore_ascii_case("refresh") {
                if let Some(content) = el.value().attr("content") {
                    if let Some(idx) = content.to_ascii_lowercase().find("url=") {
                        let raw_u = content[idx + 4..].trim().trim_matches('\'').trim_matches('"');
                        if let Some(resolved) = resolve_link(&effective_base, raw_u) {
                            out.push(DiscoveredLink {
                                raw_value: raw_u.to_string(),
                                resolved_url: resolved,
                                is_asset: false,
                            });
                        }
                    }
                }
            }
        }
    }

    // Extract CSS url(...) from <style> tags and style="..." attributes
    if let Ok(style_sel) = Selector::parse("style") {
        for el in doc.select(&style_sel) {
            let css_text: String = el.text().collect();
            for link in extract_css_links(&css_text, &effective_base) {
                out.push(link);
            }
        }
    }
    if let Ok(inline_sel) = Selector::parse("[style]") {
        for el in doc.select(&inline_sel) {
            if let Some(st) = el.value().attr("style") {
                for link in extract_css_links(st, &effective_base) {
                    out.push(link);
                }
            }
        }
    }

    // Extended parsing (%P): heuristic scan inside <script> blocks for quoted relative/absolute asset paths
    if extended_parsing {
        if let Ok(script_sel) = Selector::parse("script") {
            let js_re = Regex::new(r#"(?i)(?:src|href|location|url|fetch|import)\s*(?:=|\()\s*['"]([^'"\s<>()]+)['"]"#).unwrap();
            for el in doc.select(&script_sel) {
                let script_text: String = el.text().collect();
                for cap in js_re.captures_iter(&script_text) {
                    let raw_u = &cap[1];
                    if let Some(resolved) = resolve_link(&effective_base, raw_u) {
                        out.push(DiscoveredLink {
                            raw_value: raw_u.to_string(),
                            resolved_url: resolved,
                            is_asset: true,
                        });
                    }
                }
            }
        }
    }

    out
}

/// Extract `url(...)` and `@import "..."` links from CSS text
pub fn extract_css_links(css_text: &str, base_url: &Url) -> Vec<DiscoveredLink> {
    let mut out = Vec::new();
    let url_re = Regex::new(r#"(?i)url\(\s*['"]?([^'"\)\s]+)['"]?\s*\)"#).unwrap();
    for cap in url_re.captures_iter(css_text) {
        let raw_u = &cap[1];
        if let Some(resolved) = resolve_link(base_url, raw_u) {
            out.push(DiscoveredLink {
                raw_value: raw_u.to_string(),
                resolved_url: resolved,
                is_asset: true,
            });
        }
    }
    let import_re = Regex::new(r#"(?i)@import\s+['"]([^'"]+)['"]"#).unwrap();
    for cap in import_re.captures_iter(css_text) {
        let raw_u = &cap[1];
        if let Some(resolved) = resolve_link(base_url, raw_u) {
            out.push(DiscoveredLink {
                raw_value: raw_u.to_string(),
                resolved_url: resolved,
                is_asset: true,
            });
        }
    }
    out
}

/// Rewrite an HTML page's links to point to local mirrored files, and inject the HTTrack Mirror banner comments
/// immediately after `<html...>` and immediately before `</html>` (verified against HTTrack 3.51-0 oracle output!).
pub fn rewrite_html(
    html_text: &str,
    page_url: &Url,
    curr_local_rel_path: &str,
    url_to_local_map: &HashMap<String, String>,
    link_mode: LinkRewriteMode,
    footer_comment: &str,
    inline_data_uris: Option<&HashMap<String, String>>,
) -> String {
    // Determine effective base URL
    let effective_base = {
        let doc = Html::parse_document(html_text);
        if let Ok(base_sel) = Selector::parse("base[href]") {
            doc.select(&base_sel)
                .next()
                .and_then(|el| el.value().attr("href"))
                .and_then(|h| page_url.join(h).ok())
                .unwrap_or_else(|| page_url.clone())
        } else {
            page_url.clone()
        }
    };

    // Rewrite attribute values in-place preserving original HTML formatting
    let attr_re = Regex::new(r#"(?i)\b(href|src|data-src|poster|action|data)\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap();
    let mut rewritten = attr_re
        .replace_all(html_text, |caps: &regex::Captures| {
            let attr_name = &caps[1];
            let (quote, raw_val) = if let Some(m) = caps.get(2) {
                ("\"", m.as_str())
            } else if let Some(m) = caps.get(3) {
                ("'", m.as_str())
            } else {
                ("\"", "")
            };

            // Preserve fragment if any
            let fragment = raw_val.find('#').map(|idx| &raw_val[idx..]).unwrap_or("");
            if let Some(resolved) = resolve_link(&effective_base, raw_val) {
                let key = resolved.as_str();
                // Check --single-file (%Z) data URI inlining for assets
                if let Some(data_map) = inline_data_uris {
                    if let Some(data_uri) = data_map.get(key) {
                        return format!("{}={}{}{}", attr_name, quote, data_uri, quote);
                    }
                }
                if let Some(target_local) = url_to_local_map.get(key) {
                    let replacement = format_rewritten_link(&resolved, target_local, curr_local_rel_path, link_mode, fragment);
                    return format!("{}={}{}{}", attr_name, quote, replacement, quote);
                } else if matches!(link_mode, LinkRewriteMode::AbsoluteUrl) {
                    return format!("{}={}{}{}{}", attr_name, quote, resolved, fragment, quote);
                }
            }
            caps[0].to_string()
        })
        .into_owned();

    // Also rewrite CSS url(...) inside HTML
    rewritten = rewrite_css(&rewritten, &effective_base, curr_local_rel_path, url_to_local_map, link_mode, inline_data_uris);

    // Inject HTTrack mirror footer comment after <html...> and before </html>
    if !footer_comment.is_empty() {
        let html_open_re = Regex::new(r"(?i)(<html[^>]*>)").unwrap();
        if html_open_re.is_match(&rewritten) {
            rewritten = html_open_re
                .replacen(&rewritten, 1, |caps: &regex::Captures| {
                    format!("{}\n{}", &caps[1], footer_comment)
                })
                .into_owned();
        }
        let html_close_re = Regex::new(r"(?i)(</html>)").unwrap();
        if html_close_re.is_match(&rewritten) {
            rewritten = html_close_re
                .replacen(&rewritten, 1, |caps: &regex::Captures| {
                    format!("\n{}\n{}", footer_comment, &caps[1])
                })
                .into_owned();
        } else {
            rewritten.push('\n');
            rewritten.push_str(footer_comment);
            rewritten.push('\n');
        }
    }

    rewritten
}

/// Rewrite `url(...)` and `@import` in a CSS stylesheet
pub fn rewrite_css(
    css_text: &str,
    css_url: &Url,
    curr_local_rel_path: &str,
    url_to_local_map: &HashMap<String, String>,
    link_mode: LinkRewriteMode,
    inline_data_uris: Option<&HashMap<String, String>>,
) -> String {
    let url_re = Regex::new(r#"(?i)url\(\s*(['"]?)([^'"\)\s]+)(['"]?)\s*\)"#).unwrap();
    url_re
        .replace_all(css_text, |caps: &regex::Captures| {
            let q1 = &caps[1];
            let raw_u = &caps[2];
            let q2 = &caps[3];
            if let Some(resolved) = resolve_link(css_url, raw_u) {
                let key = resolved.as_str();
                if let Some(data_map) = inline_data_uris {
                    if let Some(data_uri) = data_map.get(key) {
                        return format!("url({}{}{})", q1, data_uri, q2);
                    }
                }
                if let Some(target_local) = url_to_local_map.get(key) {
                    let rel = format_rewritten_link(&resolved, target_local, curr_local_rel_path, link_mode, "");
                    return format!("url({}{}{})", q1, rel, q2);
                }
            }
            caps[0].to_string()
        })
        .into_owned()
}

fn format_rewritten_link(
    resolved_url: &Url,
    target_local: &str,
    curr_local: &str,
    link_mode: LinkRewriteMode,
    fragment: &str,
) -> String {
    match link_mode {
        LinkRewriteMode::Relative => {
            let mut rel = compute_relative_path(target_local, curr_local);
            // Note: HTTrack K0 preserves `?query` after the rewritten `.html` filename!
            // Example from oracle: `href="sub/page4ef9.html?id=12"`
            if let Some(q) = resolved_url.query() {
                if !q.is_empty() && target_local.ends_with(".html") {
                    rel.push('?');
                    rel.push_str(q);
                }
            }
            rel.push_str(fragment);
            rel
        }
        LinkRewriteMode::AbsoluteUrl => format!("{}{}", resolved_url.as_str(), fragment),
        LinkRewriteMode::AbsoluteUri => {
            let mut s = format!("/{}", target_local);
            s.push_str(fragment);
            s
        }
        LinkRewriteMode::Original => format!("{}{}", resolved_url.as_str(), fragment),
        LinkRewriteMode::TransparentProxy => {
            format!("{}://{}/{}{}", resolved_url.scheme(), resolved_url.host_str().unwrap_or(""), target_local, fragment)
        }
    }
}
