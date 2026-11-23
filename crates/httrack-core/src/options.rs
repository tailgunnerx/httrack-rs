use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrawlAction {
    /// w / --mirror (default)
    Mirror,
    /// W / --mirror-wizard
    MirrorWizard,
    /// g / --get-files
    GetFiles,
    /// i / --continue
    Continue,
    /// Y / --mirrorlinks
    MirrorLinks,
    /// --spider / --testlinks
    Spider,
    /// --skeleton (-p1)
    Skeleton,
    /// --update (-iC2)
    Update,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectionScope {
    /// D: can only go down into subdirs (default)
    DownOnly,
    /// U: can only go to upper directories
    UpOnly,
    /// B: can both go up & down
    Both,
    /// S: stay on the same directory
    SameDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostScope {
    /// a: stay on the same address (default)
    SameAddress,
    /// d: stay on the same principal/registrable domain
    SameDomain,
    /// l: stay on the same TLD
    SameTld,
    /// e: go everywhere on the web
    Everywhere,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkRewriteMode {
    /// K0: relative URI (default)
    Relative = 0,
    /// K / K1: absolute URL
    AbsoluteUrl = 1,
    /// K3: absolute URI (/folder/foo.html)
    AbsoluteUri = 3,
    /// K4: original URL
    Original = 4,
    /// K5: transparent proxy URL
    TransparentProxy = 5,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StructureType {
    /// N0..N1099 preset numbers
    Preset(u32),
    /// User-defined structure string e.g. "%h%p/%n%q.%t"
    UserDefined(String),
}

/// Modern replacement for legacy proxy configs: realistic browser header & User-Agent profiles
/// so sites behind Cloudflare / CDNs don't block the mirror, plus direct Auth/Cookie injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserProfile {
    /// Classic HTTrack identifier
    ClassicHttrack,
    /// Modern Desktop Chrome (sends realistic User-Agent, Sec-CH-UA, Sec-Fetch-* headers)
    Chrome,
    /// Modern Desktop Firefox
    Firefox,
    /// Modern macOS Safari
    Safari,
}

impl Default for BrowserProfile {
    fn default() -> Self {
        Self::Chrome
    }
}

impl BrowserProfile {
    pub fn from_name(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "httrack" | "classic" => Self::ClassicHttrack,
            "firefox" | "ff" => Self::Firefox,
            "safari" | "mac" => Self::Safari,
            _ => Self::Chrome,
        }
    }

    pub fn user_agent(&self) -> &'static str {
        match self {
            Self::ClassicHttrack => "Mozilla/5.0 (compatible; HTTrack/3.x; +https://www.httrack.com/)",
            Self::Chrome => "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
            Self::Firefox => "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:133.0) Gecko/20100101 Firefox/133.0",
            Self::Safari => "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_7_1) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HtsOptions {
    pub urls: Vec<String>,
    pub output_dir: PathBuf,
    pub cache_dir: Option<PathBuf>,
    pub action: CrawlAction,
    pub max_depth: usize,
    pub ext_depth: usize,
    pub max_file_size_non_html: Option<u64>,
    pub max_file_size_html: Option<u64>,
    pub max_overall_size: Option<u64>,
    pub max_time_secs: Option<u64>,
    pub max_rate_bytes_per_sec: Option<u64>,
    pub connections_per_sec: Option<u32>,
    pub sockets: usize,
    pub timeout_secs: u64,
    pub retries: u32,
    pub extended_parsing: bool,
    pub near_files: bool,
    pub test_all_urls: bool,
    pub url_list_file: Option<PathBuf>,
    pub use_sitemap: bool,
    pub sitemap_url: Option<String>,
    pub structure: StructureType,
    pub link_mode: LinkRewriteMode,
    pub generate_mht: bool,
    pub single_file: bool,
    pub single_file_max_size: u64,
    pub replace_external: bool,
    pub include_query_string: bool,
    pub purge_old: bool,
    pub robots_mode: u8,
    pub url_hack: bool,
    pub keep_www_prefix: bool,
    pub keep_double_slashes: bool,
    pub keep_query_order: bool,
    /// Modern Browser Profile & Auth (replaces legacy proxy setup)
    pub browser_profile: BrowserProfile,
    pub user_agent: String,
    pub auth_header: Option<String>,
    pub cookies_raw: Option<String>,
    pub cookies_file: Option<PathBuf>,
    pub referer: Option<String>,
    pub from_email: Option<String>,
    pub footer: String,
    pub language: String,
    pub custom_headers: Vec<String>,
    /// Intuitive MIME & Extension filters that check BOTH before and during download
    pub exclude_mimes: Vec<String>,
    pub include_mimes: Vec<String>,
    pub exclude_extensions: Vec<String>,
    pub include_extensions: Vec<String>,
    pub cache_mode: u8,
    pub generate_warc: bool,
    pub warc_file: Option<String>,
    pub generate_changes: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub make_index: bool,
    pub build_top_index: bool,
    pub search_index: bool,
    pub priority_mode: u8,
    pub dir_scope: DirectionScope,
    pub host_scope: HostScope,
    pub filters: Vec<String>,
    pub raw_args: Vec<String>,
}

impl Default for HtsOptions {
    fn default() -> Self {
        Self {
            urls: Vec::new(),
            output_dir: PathBuf::from("."),
            cache_dir: None,
            action: CrawlAction::Mirror,
            max_depth: 9999,
            ext_depth: 0,
            max_file_size_non_html: None,
            max_file_size_html: None,
            max_overall_size: None,
            max_time_secs: None,
            max_rate_bytes_per_sec: None,
            connections_per_sec: Some(5),
            sockets: 4,
            timeout_secs: 30,
            retries: 2,
            extended_parsing: true,
            near_files: false,
            test_all_urls: false,
            url_list_file: None,
            use_sitemap: false,
            sitemap_url: None,
            structure: StructureType::Preset(0),
            link_mode: LinkRewriteMode::Relative,
            generate_mht: false,
            single_file: false,
            single_file_max_size: 10_485_760,
            replace_external: false,
            include_query_string: false,
            purge_old: false,
            robots_mode: 2,
            url_hack: true,
            keep_www_prefix: false,
            keep_double_slashes: false,
            keep_query_order: false,
            browser_profile: BrowserProfile::ClassicHttrack,
            user_agent: "Mozilla/5.0 (compatible; HTTrack/3.x; +https://www.httrack.com/)".to_string(),
            auth_header: None,
            cookies_raw: None,
            cookies_file: None,
            referer: None,
            from_email: None,
            footer: "<!-- Mirrored from {url} by HTTrack Website Copier/3.x [XR&CO], {date} -->".to_string(),
            language: "en-US,en;q=0.9,*;q=0.5".to_string(),
            custom_headers: Vec::new(),
            exclude_mimes: Vec::new(),
            include_mimes: Vec::new(),
            exclude_extensions: Vec::new(),
            include_extensions: Vec::new(),
            cache_mode: 1,
            generate_warc: false,
            warc_file: None,
            generate_changes: false,
            quiet: false,
            verbose: false,
            make_index: true,
            build_top_index: false,
            search_index: false,
            priority_mode: 3,
            dir_scope: DirectionScope::DownOnly,
            host_scope: HostScope::SameAddress,
            filters: Vec::new(),
            raw_args: Vec::new(),
        }
    }
}

impl HtsOptions {
    /// Format footer string replacing HTTrack placeholders `{url}`, `{date}`, `{addr}`, `{path}`, `{mime}`, `%s`
    pub fn format_footer(&self, url: &str, addr: &str, path: &str, date_str: &str, mime: &str, status: u16, size: usize) -> String {
        if self.footer.is_empty() || self.footer == "none" {
            return String::new();
        }
        let mut out = self.footer.clone();
        out = out.replace("{url}", url);
        out = out.replace("{addr}", addr);
        out = out.replace("{path}", path);
        out = out.replace("{date}", date_str);
        out = out.replace("{lastmodified}", date_str);
        out = out.replace("{version}", "3.51-0");
        out = out.replace("{mime}", mime);
        out = out.replace("{charset}", "utf-8");
        out = out.replace("{status}", &status.to_string());
        out = out.replace("{size}", &size.to_string());
        if out.contains("%s") {
            out = out.replacen("%s", addr, 1);
            out = out.replacen("%s", path, 1);
            out = out.replacen("%s", date_str, 1);
        }
        out
    }
}
