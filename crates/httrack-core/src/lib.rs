pub mod cache;
pub mod crawler;
pub mod filter;
pub mod index_builder;
pub mod options;
pub mod rewriter;
pub mod savename;
pub mod warc;

pub use crawler::{CrawlStats, CrawlerEngine};
pub use filter::{
    expand_mime_shorthand, extract_url_extension, guess_mime_from_url, FilterEngine, FilterRule,
    RobotsRules,
};
pub use options::{
    BrowserProfile, CrawlAction, DirectionScope, HostScope, HtsOptions, LinkRewriteMode,
    StructureType,
};
pub use rewriter::{extract_css_links, extract_html_links, normalize_url, rewrite_css, rewrite_html};
pub use savename::{compute_relative_path, compute_save_path, query_md5_short};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use url::Url;

    #[test]
    fn test_savename_oracle_match() {
        let mut used = HashSet::new();
        let u1 = Url::parse("http://www.example.com/folder/page.html").unwrap();
        assert_eq!(
            compute_save_path(&u1, "text/html", &StructureType::Preset(0), &mut used),
            "www.example.com/folder/page.html"
        );

        let u2 = Url::parse("http://www.example.com/folder/foo.cgi?q=45").unwrap();
        assert_eq!(
            compute_save_path(&u2, "text/html", &StructureType::Preset(0), &mut used),
            "www.example.com/folder/fooa789.html"
        );

        let u3 = Url::parse("http://www.example.com/").unwrap();
        assert_eq!(
            compute_save_path(&u3, "text/html", &StructureType::Preset(0), &mut used),
            "www.example.com/index.html"
        );

        let u4 = Url::parse("http://www.example.com/index.html").unwrap();
        assert_eq!(
            compute_save_path(&u4, "text/html", &StructureType::Preset(0), &mut used),
            "www.example.com/index-2.html"
        );
    }

    #[test]
    fn test_relative_path_oracle_match() {
        assert_eq!(
            compute_relative_path("www.example.com/a/b/c.html", "www.example.com/a/index.html"),
            "b/c.html"
        );
        assert_eq!(
            compute_relative_path("127.0.0.1_18923/index-2.html", "127.0.0.1_18923/sub/page4ef9.html"),
            "../index-2.html"
        );
    }

    #[test]
    fn test_html_rewriter_oracle_match() {
        let html = r#"<html><head><title>Test Site</title><link rel="stylesheet" href="/style.css"></head><body><a href="sub/page.html?id=12">Page</a><img src="logo.png"></body></html>"#;
        let page_url = Url::parse("http://127.0.0.1:18923/").unwrap();
        let mut map = HashMap::new();
        map.insert("http://127.0.0.1:18923/style.css".to_string(), "127.0.0.1_18923/style.css".to_string());
        map.insert("http://127.0.0.1:18923/sub/page.html?id=12".to_string(), "127.0.0.1_18923/sub/page4ef9.html".to_string());
        map.insert("http://127.0.0.1:18923/logo.png".to_string(), "127.0.0.1_18923/logo.png".to_string());

        let footer = "<!-- Mirrored from http://127.0.0.1:18923/ by HTTrack Website Copier/3.x [XR&CO], Mon, 23 Nov 2026 20:10:56 GMT -->";
        let out = rewrite_html(
            html,
            &page_url,
            "127.0.0.1_18923/index.html",
            &map,
            LinkRewriteMode::Relative,
            footer,
            None,
        );
        assert!(out.contains(r#"href="style.css""#));
        assert!(out.contains(r#"href="sub/page4ef9.html?id=12""#));
        assert!(out.contains(r#"src="logo.png""#));
        assert!(out.contains(footer));
    }

    #[test]
    fn test_two_stage_mime_and_ext_filtering() {
        let mut opts = HtsOptions::default();
        opts.exclude_mimes = vec!["video".to_string(), "application/pdf".to_string()];
        opts.exclude_extensions = vec!["zip,exe".to_string()];

        let engine = FilterEngine::from_options(&opts);
        let seeds = vec![Url::parse("https://example.com/").unwrap()];

        // 1. Pre-flight blocks .mp4 via `--exclude-mime video` before even connecting!
        let u_mp4 = Url::parse("https://example.com/media/intro.mp4").unwrap();
        let (ok_mp4, why_mp4) = engine.evaluate_url(
            &u_mp4,
            &seeds,
            HostScope::SameAddress,
            DirectionScope::DownOnly,
            false,
            true,
        );
        assert!(!ok_mp4);
        assert!(why_mp4.contains("video/mp4"));

        // 2. Pre-flight blocks .zip via `--exclude-ext zip,exe`
        let u_zip = Url::parse("https://example.com/downloads/release.zip").unwrap();
        let (ok_zip, why_zip) = engine.evaluate_url(
            &u_zip,
            &seeds,
            HostScope::SameAddress,
            DirectionScope::DownOnly,
            false,
            true,
        );
        assert!(!ok_zip);
        assert!(why_zip.contains("--exclude-ext"));

        // 3. Stage-2 blocks dynamic endpoint returning `Content-Type: video/webm`
        let (ok_hdr, _) = engine.evaluate_mime("video/webm; codecs=vp9").unwrap();
        assert!(!ok_hdr);
    }

    #[test]
    fn test_robots_rfc9309_precedence() {
        let txt = "User-agent: *\nDisallow: /private/\nAllow: /private/public/\nSitemap: http://example.com/sitemap.xml\n";
        let rules = RobotsRules::parse(txt, "HTTrack");
        assert!(!rules.is_allowed("/private/secret.html"));
        assert!(rules.is_allowed("/private/public/index.html"));
        assert!(rules.is_allowed("/about.html"));
        assert_eq!(rules.sitemaps, vec!["http://example.com/sitemap.xml"]);
    }
}
