#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Frame, Margin, RichText, ScrollArea, Stroke, Vec2,
};
use url::Url;

use httrack_core::{
    guess_mime_from_url, BrowserProfile, CrawlAction, CrawlStats, CrawlerEngine, DirectionScope,
    FilterEngine, FilterRule, HostScope, HtsOptions, LinkRewriteMode, StructureType,
};

const ICON_PNG: &[u8] = include_bytes!("../../../assets/html/server/div/48x48/httrack.png");

/// The 6 classic steps of the original WinHTTrack MFC Wizard
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WizardStep {
    Welcome,
    ProjectName,
    WebAddresses,
    ReadyToLaunch,
    MirroringInProgress,
    EndOfMirror,
}

/// The classic tabs inside WinHTTrack's `Set options...` (`Preferences and mirror options`) modal dialog
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionsTab {
    ScanRules,
    MimeTypes,
    Links,
    Build,
    Limits,
    FlowControl,
    BrowserId,
    LogIndexCache,
    Spider,
    ExportsAndAuth,
}

#[derive(Debug, Clone)]
struct RuleCheckFeedback {
    pub is_valid: bool,
    pub summary: String,
    pub details: Vec<(bool, String)>,
}

struct WinHttrackApp {
    rt: tokio::runtime::Runtime,
    wizard_step: WizardStep,

    // Classic Modal Dialogs
    show_options_modal: bool,
    options_tab: OptionsTab,
    show_add_url_modal: bool,
    show_about_modal: bool,
    show_log_modal: bool,

    // Active Info Bubble Modal (title, body) when a user clicks any `(?)` info bubble
    active_info_popup: Option<(String, String)>,

    // Add URL dialog fields
    add_url_input: String,
    add_url_login: String,
    add_url_password: String,

    // Step 2: Project fields
    project_name: String,
    project_category: String,
    base_path: String,

    // Step 3: Action & URLs
    action_idx: usize,
    urls_text: String,

    // Step 4: Ready screen checkboxes
    save_settings_only: bool,
    on_hold_schedule: bool,

    // "Set options..." -> Scan Rules & MIME Types
    skip_video: bool,
    skip_audio: bool,
    skip_images: bool,
    skip_fonts: bool,
    skip_pdfs: bool,
    skip_archives: bool,
    custom_exclude_mimes: String,
    custom_include_mimes: String,
    exclude_extensions: String,
    include_extensions: String,
    wildcard_filters: String,

    // Scan Rules Syntax & Config Validator result
    scan_rules_check: Option<RuleCheckFeedback>,

    // Interactive Rule Tester inside Scan Rules / MIME Types
    test_url_input: String,
    test_result: Option<(bool, String, String)>,

    // "Set options..." -> Limits & Flow Control
    max_depth: usize,
    ext_depth: usize,
    max_non_html_mb: String,
    max_overall_mb: String,
    max_time_secs: String,
    sockets: usize,
    timeout_secs: u64,
    retries: u32,

    // "Set options..." -> Links & Build
    host_scope_idx: usize,
    dir_scope_idx: usize,
    structure_idx: usize,
    user_structure_str: String,
    link_mode_idx: usize,
    near_files: bool,
    test_all_links: bool,
    extended_parsing: bool,
    single_file: bool,

    // "Set options..." -> Browser ID, Spider, Log/Index/Cache, Auth
    browser_profile_idx: usize,
    custom_user_agent: String,
    html_footer: String,
    accept_language: String,
    obey_robots: bool,
    use_sitemap: bool,
    make_index: bool,
    generate_warc: bool,
    generate_changes: bool,
    use_cache: bool,
    auth_header: String,
    cookies_raw: String,
    cookies_file: String,

    // Live Crawl State
    stats: Arc<Mutex<CrawlStats>>,
    stop_flag: Arc<AtomicBool>,
    last_output_dir: Option<PathBuf>,
    status_message: String,
}

/// Clear, self-explanatory Action names + detailed breakdown for the (?) info bubble
const ACTION_CHOICES: &[(&str, &str, &str)] = &[
    (
        "1. Full Website Mirror — Download pages, images, CSS & JS for offline browsing (Default)",
        "Downloads the starting webpage(s), follows internal links across the site, downloads required images/stylesheets/scripts, and rewrites links so you can browse the entire site offline.",
        "Use this when you want a complete local copy of a website or documentation site on your hard drive.",
    ),
    (
        "2. HTML Pages Only (Skeleton Mode) — Download webpage text only, skip all images & media",
        "Crawls the website and saves only the HTML documents (.html), skipping all images, videos, stylesheets, scripts, and binary downloads.",
        "Use this when you only care about reading or archiving the text/articles of a site and want a very fast, tiny download.",
    ),
    (
        "3. Single URL / Direct File Download — Only save the exact URLs listed (do NOT follow links)",
        "Downloads ONLY the exact URLs you typed into the Web Addresses box and stops immediately without following any links inside those pages (originally called 'Get individual files').",
        "Use this when you just want to grab one specific webpage or a list of specific files without crawling the rest of the website.",
    ),
    (
        "4. Mirror Starting Page + First-Level External Sites — Follow outbound links 1 level deep",
        "Downloads the starting page AND mirrors the external pages/sites directly linked from that first page (originally called 'Download all sites in pages / multiple mirror').",
        "Use this when your starting URL is a directory, bookmark page, or link-list and you want to archive every page it links to.",
    ),
    (
        "5. Broken Link Checker (Spider Only) — Test all links for 404/500 errors without saving files",
        "Crawls the website to verify that every link and image loads properly, reporting broken links in the log without writing mirrored files to disk (originally called 'Test links in pages').",
        "Use this to audit a website for dead links or missing images.",
    ),
    (
        "6. Resume Interrupted Download — Continue an incomplete mirror from hts-cache",
        "Reads the project folder's existing hts-cache and resumes an interrupted or stopped crawl where it left off without re-downloading completed files.",
        "Use this if a previous mirror was cancelled, lost internet connection, or hit a time limit.",
    ),
    (
        "7. Update Existing Mirror — Sync local copy with new or changed files on the live website",
        "Checks your existing local mirror against the live server and only downloads new or modified pages.",
        "Use this to refresh a website you already mirrored in the past.",
    ),
];

impl WinHttrackApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_classic_win32_theme(&cc.egui_ctx);

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime");

        let default_base = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("C:\\My Web Sites"))
            .join("My Web Sites")
            .to_string_lossy()
            .to_string();

        Self {
            rt,
            wizard_step: WizardStep::Welcome,

            show_options_modal: false,
            options_tab: OptionsTab::MimeTypes,
            show_add_url_modal: false,
            show_about_modal: false,
            show_log_modal: false,
            active_info_popup: None,

            add_url_input: "https://".to_string(),
            add_url_login: String::new(),
            add_url_password: String::new(),

            project_name: "my_mirror".to_string(),
            project_category: "General".to_string(),
            base_path: default_base,

            action_idx: 0,
            urls_text: "https://example.com/".to_string(),

            save_settings_only: false,
            on_hold_schedule: false,

            skip_video: false,
            skip_audio: false,
            skip_images: false,
            skip_fonts: false,
            skip_pdfs: false,
            skip_archives: false,
            custom_exclude_mimes: String::new(),
            custom_include_mimes: String::new(),
            exclude_extensions: String::new(),
            include_extensions: String::new(),
            wildcard_filters: "+*.png +*.gif +*.jpg +*.jpeg +*.css +*.js -ad.doubleclick.net/*".to_string(),

            scan_rules_check: None,

            test_url_input: "https://example.com/media/clip.mp4".to_string(),
            test_result: None,

            max_depth: 9999,
            ext_depth: 0,
            max_non_html_mb: String::new(),
            max_overall_mb: String::new(),
            max_time_secs: String::new(),
            sockets: 4,
            timeout_secs: 30,
            retries: 2,

            host_scope_idx: 0,
            dir_scope_idx: 1,
            structure_idx: 0,
            user_structure_str: "%h%p/%n%q.%t".to_string(),
            link_mode_idx: 0,
            near_files: true,
            test_all_links: false,
            extended_parsing: true,
            single_file: false,

            browser_profile_idx: 0,
            custom_user_agent: String::new(),
            html_footer: "<!-- Mirrored from {url} by HTTrack Website Copier/3.x [XR&CO], {date} -->".to_string(),
            accept_language: "en, *".to_string(),
            obey_robots: true,
            use_sitemap: false,
            make_index: true,
            generate_warc: false,
            generate_changes: true,
            use_cache: true,
            auth_header: String::new(),
            cookies_raw: String::new(),
            cookies_file: String::new(),

            stats: Arc::new(Mutex::new(CrawlStats::default())),
            stop_flag: Arc::new(AtomicBool::new(false)),
            last_output_dir: None,
            status_message: "Welcome to WinHTTrack!".to_string(),
        }
    }

    fn build_hts_options(&self) -> HtsOptions {
        let mut opts = HtsOptions::default();

        opts.urls = self
            .urls_text
            .split(|c: char| c == '\n' || c == '\r' || c == ',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut out_dir = PathBuf::from(self.base_path.trim());
        if !self.project_name.trim().is_empty() {
            out_dir.push(self.project_name.trim());
        }
        opts.output_dir = out_dir;

        // Updated Action index mapping matching ACTION_CHOICES:
        // 0: Full Website Mirror
        // 1: HTML Pages Only (Skeleton)
        // 2: Single URL / Direct File Download (GetFiles)
        // 3: Mirror Starting Page + First-Level External Sites (MirrorLinks)
        // 4: Broken Link Checker (Spider)
        // 5: Resume Interrupted Download (Continue)
        // 6: Update Existing Mirror (Update)
        opts.action = match self.action_idx {
            1 => CrawlAction::Skeleton,
            2 => CrawlAction::GetFiles,
            3 => CrawlAction::MirrorLinks,
            4 => CrawlAction::Spider,
            5 => CrawlAction::Continue,
            6 => CrawlAction::Update,
            _ => CrawlAction::Mirror,
        };
        if matches!(opts.action, CrawlAction::Skeleton) {
            opts.priority_mode = 1;
        } else if matches!(opts.action, CrawlAction::Spider) {
            opts.priority_mode = 0;
            opts.make_index = false;
        }

        opts.max_depth = self.max_depth;
        opts.ext_depth = self.ext_depth;
        opts.sockets = self.sockets.max(1);
        opts.timeout_secs = self.timeout_secs;
        opts.retries = self.retries;

        let mut ex_mimes = Vec::new();
        if self.skip_video {
            ex_mimes.push("video/*".to_string());
        }
        if self.skip_audio {
            ex_mimes.push("audio/*".to_string());
        }
        if self.skip_images {
            ex_mimes.push("image/*".to_string());
        }
        if self.skip_fonts {
            ex_mimes.push("font/*".to_string());
        }
        if self.skip_pdfs {
            ex_mimes.push("application/pdf".to_string());
        }
        if self.skip_archives {
            ex_mimes.push("application/*zip*".to_string());
        }
        for part in self.custom_exclude_mimes.split(',') {
            let p = part.trim();
            if !p.is_empty() {
                ex_mimes.push(p.to_string());
            }
        }
        opts.exclude_mimes = ex_mimes;

        opts.include_mimes = self
            .custom_include_mimes
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        opts.exclude_extensions = self
            .exclude_extensions
            .split(',')
            .map(|s| s.trim().trim_start_matches('.').to_string())
            .filter(|s| !s.is_empty())
            .collect();

        opts.include_extensions = self
            .include_extensions
            .split(',')
            .map(|s| s.trim().trim_start_matches('.').to_string())
            .filter(|s| !s.is_empty())
            .collect();

        opts.filters = self
            .wildcard_filters
            .split_whitespace()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if let Ok(mb) = self.max_non_html_mb.trim().parse::<u64>() {
            if mb > 0 {
                opts.max_file_size_non_html = Some(mb * 1024 * 1024);
            }
        }
        if let Ok(mb) = self.max_overall_mb.trim().parse::<u64>() {
            if mb > 0 {
                opts.max_overall_size = Some(mb * 1024 * 1024);
            }
        }
        if let Ok(secs) = self.max_time_secs.trim().parse::<u64>() {
            if secs > 0 {
                opts.max_time_secs = Some(secs);
            }
        }

        opts.dir_scope = match self.dir_scope_idx {
            0 => DirectionScope::SameDir,
            2 => DirectionScope::UpOnly,
            3 => DirectionScope::Both,
            _ => DirectionScope::DownOnly,
        };

        opts.host_scope = match self.host_scope_idx {
            1 => HostScope::SameDomain,
            2 => HostScope::SameTld,
            3 => HostScope::Everywhere,
            _ => HostScope::SameAddress,
        };

        opts.structure = match self.structure_idx {
            1 => StructureType::Preset(1),
            2 => StructureType::Preset(2),
            3 => StructureType::Preset(3),
            4 => StructureType::Preset(4),
            5 => StructureType::Preset(100),
            6 => StructureType::Preset(1003),
            7 => StructureType::UserDefined(self.user_structure_str.clone()),
            _ => StructureType::Preset(0),
        };

        opts.link_mode = match self.link_mode_idx {
            1 => LinkRewriteMode::AbsoluteUrl,
            2 => LinkRewriteMode::AbsoluteUri,
            3 => LinkRewriteMode::Original,
            _ => LinkRewriteMode::Relative,
        };

        opts.near_files = self.near_files;
        opts.test_all_urls = self.test_all_links;
        opts.extended_parsing = self.extended_parsing;
        opts.single_file = self.single_file;
        opts.use_sitemap = self.use_sitemap;
        opts.make_index = self.make_index;
        opts.generate_warc = self.generate_warc;
        opts.generate_changes = self.generate_changes;
        opts.cache_mode = if self.use_cache { 1 } else { 0 };
        opts.robots_mode = if self.obey_robots { 2 } else { 0 };
        opts.footer = self.html_footer.clone();
        opts.language = self.accept_language.clone();

        opts.browser_profile = match self.browser_profile_idx {
            1 => BrowserProfile::Firefox,
            2 => BrowserProfile::Safari,
            3 => BrowserProfile::ClassicHttrack,
            _ => BrowserProfile::Chrome,
        };
        if !self.custom_user_agent.trim().is_empty() {
            opts.user_agent = self.custom_user_agent.trim().to_string();
        } else {
            opts.user_agent = opts.browser_profile.user_agent().to_string();
        }

        if !self.auth_header.trim().is_empty() {
            opts.auth_header = Some(self.auth_header.trim().to_string());
        }
        if !self.cookies_raw.trim().is_empty() {
            opts.cookies_raw = Some(self.cookies_raw.trim().to_string());
        }
        if !self.cookies_file.trim().is_empty() {
            opts.cookies_file = Some(PathBuf::from(self.cookies_file.trim()));
        }

        opts.quiet = true;
        opts
    }

    /// Validate all scan rules & filter patterns for syntax errors, missing `+`/`-` prefixes,
    /// unbalanced brackets `*[...]`, or accidental `-*` block-everything rules.
    fn validate_scan_rules_config(&mut self) {
        let tokens: Vec<&str> = self
            .wildcard_filters
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .collect();

        let mut details = Vec::new();
        let mut errors_found = 0usize;

        if tokens.is_empty() {
            self.scan_rules_check = Some(RuleCheckFeedback {
                is_valid: true,
                summary: "No custom wildcard scan rules entered (default scope rules will be used).".to_string(),
                details,
            });
            return;
        }

        for (idx, tok) in tokens.iter().enumerate() {
            let num = idx + 1;
            if !tok.starts_with('+') && !tok.starts_with('-') {
                errors_found += 1;
                details.push((
                    false,
                    format!(
                        "Rule #{} ('{}'): Missing '+' or '-' prefix! Every rule must start with '+' (to allow) or '-' (to block), e.g. '+{}' or '-{}'.",
                        num, tok, tok, tok
                    ),
                ));
                continue;
            }

            let body = &tok[1..];
            if body.is_empty() {
                errors_found += 1;
                details.push((
                    false,
                    format!(
                        "Rule #{} ('{}'): Empty pattern after '{}' sign.",
                        num, tok, &tok[..1]
                    ),
                ));
                continue;
            }

            // Check unbalanced square brackets in *[a-z] or *[>100]
            let open_brackets = body.matches('[').count();
            let close_brackets = body.matches(']').count();
            if open_brackets != close_brackets {
                errors_found += 1;
                details.push((
                    false,
                    format!(
                        "Rule #{} ('{}'): Unbalanced square brackets '[' and ']'.",
                        num, tok
                    ),
                ));
                continue;
            }

            // Check if size filter `*[>N]` or `*[<N]` has a valid integer
            if body.starts_with("*[>") || body.starts_with("*[<") {
                if let Some(inner) = body.strip_prefix("*[>").or_else(|| body.strip_prefix("*[<")) {
                    if let Some(num_str) = inner.strip_suffix(']') {
                        if num_str.parse::<u64>().is_err() {
                            errors_found += 1;
                            details.push((
                                false,
                                format!(
                                    "Rule #{} ('{}'): Size filter must contain a valid number in KB, e.g. '-*[>5000]'.",
                                    num, tok
                                ),
                            ));
                            continue;
                        }
                    }
                }
            }

            // Check if `mime:` rule is missing subtype
            if let Some(m) = body.strip_prefix("mime:") {
                if m.is_empty() {
                    errors_found += 1;
                    details.push((
                        false,
                        format!(
                            "Rule #{} ('{}'): Empty MIME pattern after 'mime:'. Try '-mime:video/*' or '+mime:image/*'.",
                            num, tok
                        ),
                    ));
                    continue;
                }
            }

            // Warn if `-*` appears last and blocks everything
            if *tok == "-*" && idx == tokens.len() - 1 {
                errors_found += 1;
                details.push((
                    false,
                    format!(
                        "Rule #{} ('-*'): Placed at the very end of the list! Since the LAST matching rule wins, '-*' at the end will block ALL links. Put '-*' first before '+...' allow rules.",
                        num
                    ),
                ));
                continue;
            }

            // Parse using FilterRule::parse
            match FilterRule::parse(tok) {
                Some(parsed) => {
                    let action_word = if parsed.allow { "ALLOWS" } else { "BLOCKS" };
                    let desc = match parsed.kind {
                        httrack_core::filter::FilterKind::UrlPattern(p) => {
                            format!("Rule #{} ('{}'): OK — {} URLs matching '{}'", num, tok, action_word, p)
                        }
                        httrack_core::filter::FilterKind::MimePattern(m) => {
                            format!("Rule #{} ('{}'): OK — {} MIME type '{}'", num, tok, action_word, m)
                        }
                        httrack_core::filter::FilterKind::SizeLess(b) => {
                            format!("Rule #{} ('{}'): OK — {} files smaller than {} KB", num, tok, action_word, b / 1024)
                        }
                        httrack_core::filter::FilterKind::SizeGreater(b) => {
                            format!("Rule #{} ('{}'): OK — {} files larger than {} KB", num, tok, action_word, b / 1024)
                        }
                    };
                    details.push((true, desc));
                }
                None => {
                    errors_found += 1;
                    details.push((
                        false,
                        format!("Rule #{} ('{}'): Could not be parsed as a valid filter rule.", num, tok),
                    ));
                }
            }
        }

        let summary = if errors_found == 0 {
            format!("✅ All {} scan rule(s) are valid and ready!", tokens.len())
        } else {
            format!(
                "⚠️ Found {} configuration issue(s) in your scan rules. Please review below:",
                errors_found
            )
        };

        self.scan_rules_check = Some(RuleCheckFeedback {
            is_valid: errors_found == 0,
            summary,
            details,
        });
    }

    fn start_mirror(&mut self) {
        let opts = self.build_hts_options();
        if opts.urls.is_empty() {
            self.status_message = "Error: Please enter at least one Web Address (URL)!".to_string();
            return;
        }

        self.last_output_dir = Some(opts.output_dir.clone());
        self.stop_flag.store(false, Ordering::Relaxed);
        self.wizard_step = WizardStep::MirroringInProgress;
        self.status_message = "Receiving files..".to_string();

        let stats_clone = self.stats.clone();
        let stop_clone = self.stop_flag.clone();

        self.rt.spawn(async move {
            let engine = CrawlerEngine::with_shared_state(opts, stats_clone, stop_clone);
            let _ = engine.run().await;
        });
    }

    fn run_rule_tester(&mut self) {
        let raw = self.test_url_input.trim();
        if raw.is_empty() {
            return;
        }
        let target = match Url::parse(raw).or_else(|_| Url::parse(&format!("https://{}", raw))) {
            Ok(u) => u,
            Err(e) => {
                self.test_result = Some((false, format!("Invalid URL syntax: {}", e), "none".to_string()));
                return;
            }
        };

        let opts = self.build_hts_options();
        let mut seeds: Vec<Url> = opts
            .urls
            .iter()
            .filter_map(|u| Url::parse(u).or_else(|_| Url::parse(&format!("https://{}", u))).ok())
            .collect();
        if seeds.is_empty() {
            seeds.push(target.clone());
        }

        let engine = FilterEngine::from_options(&opts);
        let inferred = guess_mime_from_url(&target).unwrap_or("unknown (checked on HTTP Content-Type header)");
        let is_asset = guess_mime_from_url(&target)
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

        self.test_result = Some((allowed, reason, inferred.to_string()));
    }
}

/// Helper widget: draws a clickable blue `(?)` info bubble that shows a tooltip on hover
/// AND opens a clean popup explanation window when clicked!
fn info_bubble(
    ui: &mut egui::Ui,
    active_popup: &mut Option<(String, String)>,
    title: &str,
    explanation: &str,
) {
    let btn = egui::Button::new(
        RichText::new(" ? ")
            .size(11.5)
            .strong()
            .color(Color32::from_rgb(0, 75, 165)),
    )
    .fill(Color32::from_rgb(220, 234, 252))
    .stroke(Stroke::new(1.0, Color32::from_rgb(120, 165, 225)))
    .corner_radius(CornerRadius::same(10));

    let resp = ui
        .add(btn)
        .on_hover_ui(|ui| {
            ui.set_max_width(340.0);
            ui.label(RichText::new(title).strong().color(Color32::from_rgb(15, 40, 95)));
            ui.add_space(3.0);
            ui.label(RichText::new(explanation).size(12.0));
            ui.add_space(2.0);
            ui.label(
                RichText::new("(Click '?' to pin this explanation in a popup window)")
                    .size(10.5)
                    .italics()
                    .color(Color32::from_rgb(100, 105, 115)),
            );
        });

    if resp.clicked() {
        *active_popup = Some((title.to_string(), explanation.to_string()));
    }
}

impl eframe::App for WinHttrackApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let (is_running, is_finished) = {
            let st = self.stats.lock().unwrap();
            (st.is_running, st.is_finished)
        };
        if is_running {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        } else if is_finished && self.wizard_step == WizardStep::MirroringInProgress {
            self.wizard_step = WizardStep::EndOfMirror;
            self.status_message = "Site mirroring finished!".to_string();
        }

        // 1. Classic WinHTTrack Menu Bar (File | Preferences | Mirror | Log | Window | Help)
        egui::TopBottomPanel::top("classic_menubar")
            .frame(
                Frame::NONE
                    .fill(Color32::from_rgb(240, 240, 240))
                    .inner_margin(Margin::symmetric(6, 2))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(200, 200, 200))),
            )
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("New project\tCtrl+N").clicked() {
                            self.wizard_step = WizardStep::Welcome;
                            ui.close_menu();
                        }
                        if ui.button("Open...\tCtrl+O").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                self.base_path = folder.to_string_lossy().to_string();
                                self.wizard_step = WizardStep::ProjectName;
                            }
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Browse sites...").clicked() {
                            if let Some(ref out_dir) = self.last_output_dir {
                                let _ = std::process::Command::new("explorer").arg(out_dir).spawn();
                            }
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Exit").clicked() {
                            std::process::exit(0);
                        }
                    });

                    ui.menu_button("Preferences", |ui| {
                        if ui.button("Modify options...").clicked() {
                            self.show_options_modal = true;
                            ui.close_menu();
                        }
                        if ui.button("MIME Types...").clicked() {
                            self.options_tab = OptionsTab::MimeTypes;
                            self.show_options_modal = true;
                            ui.close_menu();
                        }
                        if ui.button("Scan Rules...").clicked() {
                            self.options_tab = OptionsTab::ScanRules;
                            self.show_options_modal = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Reset to default options").clicked() {
                            self.skip_video = false;
                            self.skip_audio = false;
                            self.skip_images = false;
                            self.custom_exclude_mimes.clear();
                            ui.close_menu();
                        }
                    });

                    ui.menu_button("Mirror", |ui| {
                        if ui.button("Start / Resume Mirror").clicked() {
                            if !is_running {
                                self.start_mirror();
                            }
                            ui.close_menu();
                        }
                        if ui.button("Stop the mirroring operation").clicked() {
                            self.stop_flag.store(true, Ordering::Relaxed);
                            ui.close_menu();
                        }
                    });

                    ui.menu_button("Log", |ui| {
                        if ui.button("View log files").clicked() {
                            self.show_log_modal = true;
                            ui.close_menu();
                        }
                        if ui.button("View error and warning reports").clicked() {
                            self.show_log_modal = true;
                            ui.close_menu();
                        }
                    });

                    ui.menu_button("Window", |ui| {
                        if ui.button("Wizard View").clicked() {
                            self.wizard_step = WizardStep::WebAddresses;
                            ui.close_menu();
                        }
                        if ui.button("View file transfers").clicked() {
                            self.wizard_step = WizardStep::MirroringInProgress;
                            ui.close_menu();
                        }
                    });

                    ui.menu_button("Help", |ui| {
                        if ui.button("About WinHTTrack Website Copier...").clicked() {
                            self.show_about_modal = true;
                            ui.close_menu();
                        }
                    });
                });
            });

        // 2. Classic WinHTTrack Toolbar row
        egui::TopBottomPanel::top("classic_toolbar")
            .frame(
                Frame::NONE
                    .fill(Color32::from_rgb(232, 232, 235))
                    .inner_margin(Margin::symmetric(8, 4))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(185, 185, 190))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("📄 New")
                        .on_hover_text("Start a new project")
                        .clicked()
                    {
                        self.wizard_step = WizardStep::Welcome;
                    }
                    if ui
                        .button("📁 Open")
                        .on_hover_text("Select base path folder")
                        .clicked()
                    {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.base_path = folder.to_string_lossy().to_string();
                        }
                    }
                    ui.separator();
                    if ui
                        .button("⚙ Set options...")
                        .on_hover_text("Preferences and mirror options (Scan Rules, MIME Types, Limits, Links, Build)")
                        .clicked()
                    {
                        self.show_options_modal = true;
                    }
                    if ui
                        .button("📋 View log")
                        .on_hover_text("View error and info log files")
                        .clicked()
                    {
                        self.show_log_modal = true;
                    }
                    ui.separator();
                    if is_running {
                        if ui
                            .button(RichText::new("⏹ Stop").color(Color32::from_rgb(180, 0, 0)).strong())
                            .on_hover_text("Stop the mirroring operation")
                            .clicked()
                        {
                            self.stop_flag.store(true, Ordering::Relaxed);
                        }
                    } else if ui
                        .button(RichText::new("▶ Start Mirror").color(Color32::from_rgb(0, 110, 40)).strong())
                        .on_hover_text("Launch the mirroring operation now")
                        .clicked()
                    {
                        self.start_mirror();
                    }
                    ui.separator();
                    if ui
                        .button("❓ About")
                        .on_hover_text("About WinHTTrack Website Copier")
                        .clicked()
                    {
                        self.show_about_modal = true;
                    }
                });
            });

        // 3. Bottom WinHTTrack Wizard Navigation Bar (< Back | Next > | Finish / Cancel) + Status Bar
        egui::TopBottomPanel::bottom("classic_bottom_bar")
            .frame(
                Frame::NONE
                    .fill(Color32::from_rgb(228, 228, 232))
                    .inner_margin(Margin::symmetric(10, 6))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(175, 175, 180))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&self.status_message)
                            .size(12.0)
                            .color(Color32::from_rgb(40, 40, 45)),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized([82.0, 26.0], egui::Button::new("Cancel / Exit"))
                            .clicked()
                        {
                            if is_running {
                                self.stop_flag.store(true, Ordering::Relaxed);
                            } else {
                                std::process::exit(0);
                            }
                        }

                        ui.add_space(6.0);

                        let can_finish = matches!(
                            self.wizard_step,
                            WizardStep::ReadyToLaunch | WizardStep::EndOfMirror
                        );
                        if ui
                            .add_enabled(
                                can_finish,
                                egui::Button::new(RichText::new("FINISH").strong())
                                    .min_size(Vec2::new(82.0, 26.0)),
                            )
                            .clicked()
                        {
                            if self.wizard_step == WizardStep::ReadyToLaunch {
                                if self.save_settings_only {
                                    self.status_message = "Settings saved.".to_string();
                                } else {
                                    self.start_mirror();
                                }
                            } else {
                                std::process::exit(0);
                            }
                        }

                        let can_next = matches!(
                            self.wizard_step,
                            WizardStep::Welcome | WizardStep::ProjectName | WizardStep::WebAddresses
                        );
                        if ui
                            .add_enabled(
                                can_next,
                                egui::Button::new(RichText::new("Next >").strong())
                                    .min_size(Vec2::new(82.0, 26.0)),
                            )
                            .clicked()
                        {
                            self.wizard_step = match self.wizard_step {
                                WizardStep::Welcome => WizardStep::ProjectName,
                                WizardStep::ProjectName => WizardStep::WebAddresses,
                                WizardStep::WebAddresses => WizardStep::ReadyToLaunch,
                                other => other,
                            };
                        }

                        let can_back = !matches!(
                            self.wizard_step,
                            WizardStep::Welcome | WizardStep::MirroringInProgress
                        );
                        if ui
                            .add_enabled(
                                can_back,
                                egui::Button::new("< Back").min_size(Vec2::new(82.0, 26.0)),
                            )
                            .clicked()
                        {
                            self.wizard_step = match self.wizard_step {
                                WizardStep::ProjectName => WizardStep::Welcome,
                                WizardStep::WebAddresses => WizardStep::ProjectName,
                                WizardStep::ReadyToLaunch => WizardStep::WebAddresses,
                                WizardStep::EndOfMirror => WizardStep::WebAddresses,
                                other => other,
                            };
                        }
                    });
                });
            });

        // 4. Classic Left Pane: WinHTTrack Banner & Project Tree
        egui::SidePanel::left("classic_left_tree")
            .resizable(true)
            .default_width(210.0)
            .frame(
                Frame::NONE
                    .fill(Color32::from_rgb(255, 255, 255))
                    .inner_margin(Margin::symmetric(8, 8))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(180, 180, 185))),
            )
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(Color32::from_rgb(45, 65, 135))
                    .inner_margin(Margin::symmetric(10, 10))
                    .corner_radius(CornerRadius::same(3))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new("WinHTTrack")
                                .size(17.0)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.label(
                            RichText::new("Website Copier 3.51-0")
                                .size(11.5)
                                .color(Color32::from_rgb(210, 225, 255)),
                        );
                        ui.label(
                            RichText::new("[Rust Native Engine]")
                                .size(11.0)
                                .color(Color32::from_rgb(180, 210, 255)),
                        );
                    });

                ui.add_space(10.0);
                ui.label(RichText::new("📁 My Web Sites").strong().color(Color32::from_rgb(20, 20, 30)));
                ui.indent("proj_tree", |ui| {
                    let cat = if self.project_category.trim().is_empty() {
                        "General"
                    } else {
                        self.project_category.trim()
                    };
                    ui.label(RichText::new(format!("📂 {}", cat)).color(Color32::from_rgb(50, 50, 60)));
                    ui.indent("proj_node", |ui| {
                        let pname = if self.project_name.trim().is_empty() {
                            "(unnamed)"
                        } else {
                            self.project_name.trim()
                        };
                        ui.label(
                            RichText::new(format!("🌐 {}", pname))
                                .strong()
                                .color(Color32::from_rgb(0, 70, 160)),
                        );
                    });
                });

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(
                    RichText::new("HTTrack Project Wizard")
                        .strong()
                        .size(12.0)
                        .color(Color32::from_rgb(70, 70, 80)),
                );
                ui.add_space(4.0);

                for (step, label) in [
                    (WizardStep::Welcome, "1. Welcome"),
                    (WizardStep::ProjectName, "2. Project Name & Path"),
                    (WizardStep::WebAddresses, "3. Web Addresses (URL)"),
                    (WizardStep::ReadyToLaunch, "4. Ready to Mirror"),
                    (WizardStep::MirroringInProgress, "5. Mirror in Progress"),
                    (WizardStep::EndOfMirror, "6. End of Mirror"),
                ] {
                    let active = self.wizard_step == step;
                    if ui
                        .selectable_label(
                            active,
                            RichText::new(label).strong().color(if active {
                                Color32::from_rgb(0, 70, 160)
                            } else {
                                Color32::from_rgb(60, 60, 70)
                            }),
                        )
                        .clicked()
                    {
                        self.wizard_step = step;
                    }
                }
            });

        // 5. Right Main Wizard Dialog Panel
        egui::CentralPanel::default()
            .frame(
                Frame::NONE
                    .fill(Color32::from_rgb(240, 240, 243))
                    .inner_margin(Margin::symmetric(18, 14)),
            )
            .show(ctx, |ui| {
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    match self.wizard_step {
                        WizardStep::Welcome => self.render_step1_welcome(ui),
                        WizardStep::ProjectName => self.render_step2_project(ui),
                        WizardStep::WebAddresses => self.render_step3_urls(ui),
                        WizardStep::ReadyToLaunch => self.render_step4_ready(ui),
                        WizardStep::MirroringInProgress => self.render_step5_mirroring(ui),
                        WizardStep::EndOfMirror => self.render_step6_finished(ui),
                    }
                });
            });

        // 6. Modal Dialogs
        if self.show_options_modal {
            self.render_set_options_modal(ctx);
        }
        if self.show_add_url_modal {
            self.render_add_url_modal(ctx);
        }
        if self.show_log_modal {
            self.render_log_modal(ctx);
        }
        if self.show_about_modal {
            self.render_about_modal(ctx);
        }
        if self.active_info_popup.is_some() {
            self.render_info_bubble_popup(ctx);
        }
    }
}

impl WinHttrackApp {
    fn render_step1_welcome(&mut self, ui: &mut egui::Ui) {
        classic_group_box(ui, "Welcome to WinHTTrack!", |ui| {
            ui.add_space(10.0);
            ui.label(
                RichText::new("Welcome to WinHTTrack Website Copier!")
                    .size(18.0)
                    .strong()
                    .color(Color32::from_rgb(25, 45, 110)),
            );
            ui.add_space(14.0);
            ui.label(
                RichText::new(
                    "Please click on the NEXT button to\n\n\
                     - start a new project\n\
                     - or resume a partial download",
                )
                .size(14.0)
                .color(Color32::from_rgb(30, 30, 35)),
            );
            ui.add_space(22.0);
            ui.separator();
            ui.add_space(10.0);
            ui.label(
                RichText::new("Open Source offline browser — Rust Native Edition (v3.51-0)")
                    .size(12.5)
                    .strong()
                    .color(Color32::from_rgb(60, 70, 90)),
            );
            ui.label(
                RichText::new(
                    "• Hover or click any blue [ ? ] info bubble for a plain-English explanation of what an option does\n\
                     • Two-Stage MIME & File Extension filtering (checks both pre-flight before connecting and on HTTP headers)\n\
                     • Built-in 'Check Rules for Errors' validator & Live URL Rule Tester inside 'Set options...'",
                )
                .size(12.0)
                .color(Color32::from_rgb(70, 70, 80)),
            );
            ui.add_space(20.0);
            if ui
                .add_sized(
                    [140.0, 30.0],
                    egui::Button::new(RichText::new("Next >").strong()),
                )
                .clicked()
            {
                self.wizard_step = WizardStep::ProjectName;
            }
        });
    }

    fn render_step2_project(&mut self, ui: &mut egui::Ui) {
        classic_group_box(ui, "HTTrack Project Wizard...", |ui| {
            ui.label(
                RichText::new("Type a new project name, or select existing project to update/resume")
                    .size(13.0)
                    .strong()
                    .color(Color32::from_rgb(25, 45, 110)),
            );
            ui.add_space(14.0);

            egui::Grid::new("step2_grid")
                .num_columns(2)
                .spacing([14.0, 12.0])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("New project name:").strong());
                        info_bubble(
                            ui,
                            &mut self.active_info_popup,
                            "New Project Name",
                            "WinHTTrack creates a dedicated folder with this name inside your Base path. To update or resume a project later, enter the same project name.",
                        );
                    });
                    ui.add(
                        egui::TextEdit::singleline(&mut self.project_name)
                            .desired_width(340.0)
                            .hint_text("Enter project name here"),
                    );
                    ui.end_row();

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Project category:").strong());
                        info_bubble(
                            ui,
                            &mut self.active_info_popup,
                            "Project Category",
                            "An optional label used to group your mirrored websites in the left sidebar tree (for example: 'Docs', 'Research', 'Backups').",
                        );
                    });
                    ui.add(
                        egui::TextEdit::singleline(&mut self.project_category)
                            .desired_width(340.0)
                            .hint_text("Select or create a category name"),
                    );
                    ui.end_row();

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Base path:").strong());
                        info_bubble(
                            ui,
                            &mut self.active_info_popup,
                            "Base Path (-O)",
                            "The parent directory on your computer where all your mirrored website folders will be stored.",
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.base_path)
                                .desired_width(295.0)
                                .hint_text("C:\\My Web Sites"),
                        );
                        if ui
                            .button("...")
                            .on_hover_text("Click here to select path")
                            .clicked()
                        {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                self.base_path = folder.to_string_lossy().to_string();
                            }
                        }
                    });
                    ui.end_row();
                });

            ui.add_space(14.0);
            let full_preview = PathBuf::from(&self.base_path).join(&self.project_name);
            ui.label(
                RichText::new(format!(
                    "Files for this mirror will be saved in:  {}",
                    full_preview.display()
                ))
                .size(12.0)
                .color(Color32::from_rgb(70, 75, 85)),
            );
        });
    }

    fn render_step3_urls(&mut self, ui: &mut egui::Ui) {
        classic_group_box(ui, "Action:", |ui| {
            let all_actions_guide = ACTION_CHOICES
                .iter()
                .map(|(title, what, when)| format!("• {}\n  What it does: {}\n  When to use: {}", title, what, when))
                .collect::<Vec<_>>()
                .join("\n\n");

            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("classic_action_combo")
                    .width(510.0)
                    .selected_text(ACTION_CHOICES[self.action_idx].0)
                    .show_ui(ui, |ui| {
                        for (idx, (lbl, what, _when)) in ACTION_CHOICES.iter().enumerate() {
                            ui.selectable_value(&mut self.action_idx, idx, *lbl)
                                .on_hover_text(*what);
                        }
                    });

                info_bubble(
                    ui,
                    &mut self.active_info_popup,
                    "How the 7 Action Modes Differ",
                    &all_actions_guide,
                );
            });

            ui.add_space(6.0);
            // Highlighted explanation banner showing what the currently selected Action does and when to use it
            Frame::NONE
                .fill(Color32::from_rgb(232, 241, 252))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(10, 8))
                .stroke(Stroke::new(1.0, Color32::from_rgb(165, 195, 235)))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let (_, what, when) = ACTION_CHOICES[self.action_idx];
                    ui.label(
                        RichText::new(format!("What this mode does: {}", what))
                            .size(12.0)
                            .strong()
                            .color(Color32::from_rgb(15, 40, 95)),
                    );
                    ui.label(
                        RichText::new(format!("When to use it: {}", when))
                            .size(11.5)
                            .color(Color32::from_rgb(50, 65, 95)),
                    );
                });
        });

        ui.add_space(10.0);

        classic_group_box(ui, "Web Addresses: (URL)", |ui| {
            ui.horizontal(|ui| {
                ui.label("Enter one or more starting URLs (one per line):");
                info_bubble(
                    ui,
                    &mut self.active_info_popup,
                    "Web Addresses (URL)",
                    "Enter the starting web address(es) you want to copy (e.g. https://example.com/docs/). By default, WinHTTrack will stay on the same hostname and inside the starting folder unless you widen the scope in Set options -> Spider.",
                );
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.urls_text)
                            .desired_rows(5)
                            .desired_width(430.0)
                            .font(FontId::monospace(13.0))
                            .hint_text("Enter address(es) here (one per line)"),
                    );
                });

                ui.add_space(8.0);
                ui.vertical(|ui| {
                    if ui
                        .add_sized([125.0, 28.0], egui::Button::new("Add a URL..."))
                        .on_hover_text("Click to add a URL (with optional HTTP login/password)")
                        .clicked()
                    {
                        self.show_add_url_modal = true;
                    }
                    ui.add_space(6.0);
                    if ui
                        .add_sized([125.0, 28.0], egui::Button::new("Load URL(s)..."))
                        .on_hover_text("Load URL(s) from a text file (*.txt)")
                        .clicked()
                    {
                        if let Some(f) = rfd::FileDialog::new()
                            .add_filter("Address List text file (*.txt)", &["txt"])
                            .pick_file()
                        {
                            if let Ok(txt) = std::fs::read_to_string(f) {
                                if !self.urls_text.ends_with('\n') && !self.urls_text.is_empty() {
                                    self.urls_text.push('\n');
                                }
                                self.urls_text.push_str(&txt);
                            }
                        }
                    }
                });
            });
        });

        ui.add_space(10.0);

        classic_group_box(ui, "Preferences and mirror options:", |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_sized(
                        [145.0, 32.0],
                        egui::Button::new(RichText::new("⚙ Set options...").strong()),
                    )
                    .on_hover_text("Click to open the 10-tabbed Preferences dialog (MIME Types, Scan Rules, Links, Build, Limits, Flow Control, Browser ID)")
                    .clicked()
                {
                    self.show_options_modal = true;
                }

                info_bubble(
                    ui,
                    &mut self.active_info_popup,
                    "Preferences and Mirror Options (Set options...)",
                    "Opens the tabbed configuration window where you can ignore specific MIME types (videos, images, PDFs, archives), add wildcard URL scan rules, validate your filter syntax, set depth/size limits, and choose how files are structured on disk.",
                );

                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Quick MIME Exclusions (also editable under Set options -> MIME Types):")
                            .strong()
                            .size(12.0),
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.skip_video, "Skip Videos");
                        ui.checkbox(&mut self.skip_audio, "Skip Audio");
                        ui.checkbox(&mut self.skip_images, "Skip Images");
                        ui.checkbox(&mut self.skip_pdfs, "Skip PDFs");
                        ui.checkbox(&mut self.skip_archives, "Skip Archives (.zip)");
                    });
                });
            });
        });
    }

    fn render_step4_ready(&mut self, ui: &mut egui::Ui) {
        classic_group_box(ui, "Start", |ui| {
            ui.label(
                RichText::new(
                    "Please adjust connection parameters if necessary,\n\
                     then press FINISH to launch the mirroring operation.",
                )
                .size(14.0)
                .strong()
                .color(Color32::from_rgb(25, 45, 110)),
            );
            ui.add_space(14.0);

            ui.checkbox(
                &mut self.save_settings_only,
                "Save settings only, do not launch download now.",
            );
            ui.checkbox(
                &mut self.on_hold_schedule,
                "On hold (wait before starting transfer)",
            );

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(10.0);

            let opts = self.build_hts_options();
            ui.label(RichText::new("Summary of Mirror Parameters:").strong());
            ui.label(format!("• Action Mode: {}", ACTION_CHOICES[self.action_idx].0));
            ui.label(format!("• Output folder: {}", opts.output_dir.display()));
            ui.label(format!("• Target URLs: {}", opts.urls.join(", ")));
            ui.label(format!("• Browser identity: {:?}", opts.browser_profile));
            if !opts.exclude_mimes.is_empty() {
                ui.label(format!("• Excluded MIME types: {}", opts.exclude_mimes.join(", ")));
            }
            if !opts.exclude_extensions.is_empty() {
                ui.label(format!(
                    "• Excluded file extensions: {}",
                    opts.exclude_extensions.join(", ")
                ));
            }

            ui.add_space(18.0);
            if ui
                .add_sized(
                    [180.0, 34.0],
                    egui::Button::new(
                        RichText::new("▶ FINISH (Start Mirror)")
                            .strong()
                            .color(Color32::from_rgb(0, 100, 30)),
                    ),
                )
                .clicked()
            {
                self.start_mirror();
            }
        });
    }

    fn render_step5_mirroring(&mut self, ui: &mut egui::Ui) {
        let st = self.stats.lock().unwrap().clone();

        classic_group_box(
            ui,
            &format!(
                "Site mirroring in progress [{}, {}]",
                st.current_url,
                format_bytes(st.bytes_received)
            ),
            |ui| {
                egui::Grid::new("classic_stats_grid")
                    .num_columns(4)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Bytes saved:").strong());
                        ui.label(format!(
                            "{} ({} received)",
                            format_bytes(st.bytes_written),
                            format_bytes(st.bytes_received)
                        ));

                        ui.label(RichText::new("Links scanned:").strong());
                        ui.label(format!("{} / {}", st.links_scanned, st.links_total_queued));
                        ui.end_row();

                        ui.label(RichText::new("Time:").strong());
                        ui.label(format!("{} s", st.elapsed_secs));

                        ui.label(RichText::new("Files written:").strong());
                        ui.label(format!("{}", st.files_written));
                        ui.end_row();

                        ui.label(RichText::new("Transfer rate:").strong());
                        let rate = if st.elapsed_secs > 0 {
                            st.bytes_received / (st.elapsed_secs as usize)
                        } else {
                            st.bytes_received
                        };
                        ui.label(format!("{}/s", format_bytes(rate)));

                        ui.label(RichText::new("Filtered / Skipped:").strong());
                        ui.label(format!("{}", st.links_filtered_out));
                        ui.end_row();

                        ui.label(RichText::new("Active connections:").strong());
                        ui.label(if st.is_running { "1 (receiving)" } else { "0 (ready)" });

                        ui.label(RichText::new("Errors:").strong());
                        ui.label(
                            RichText::new(format!("{}", st.errors_count)).color(
                                if st.errors_count > 0 {
                                    Color32::from_rgb(180, 0, 0)
                                } else {
                                    Color32::from_rgb(30, 30, 30)
                                },
                            ),
                        );
                        ui.end_row();
                    });

                ui.add_space(10.0);
                let progress = if st.links_total_queued > 0 {
                    (st.links_scanned as f32 / st.links_total_queued as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                ui.add(
                    egui::ProgressBar::new(progress)
                        .text(format!("In progress: {}", st.current_url))
                        .animate(st.is_running),
                );
            },
        );

        ui.add_space(10.0);

        classic_group_box(ui, "File download information", |ui| {
            egui::Grid::new("transfer_slots_table")
                .striped(true)
                .num_columns(4)
                .min_col_width(110.0)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Status").strong());
                    ui.label(RichText::new("File / URL").strong());
                    ui.label(RichText::new("Size / Progress").strong());
                    ui.label(RichText::new("Action").strong());
                    ui.end_row();

                    if st.is_running {
                        ui.label("receive");
                        ui.label(&st.current_url);
                        ui.label(format_bytes(st.bytes_received));
                        if ui.small_button("SKIP").clicked() {
                            self.stop_flag.store(true, Ordering::Relaxed);
                        }
                        ui.end_row();
                    }

                    for f in st.recent_files.iter().rev().take(8) {
                        ui.label(RichText::new("ready").color(Color32::from_rgb(0, 110, 30)));
                        ui.label(f);
                        ui.label("saved");
                        ui.label("-");
                        ui.end_row();
                    }

                    for s in st.skipped_files.iter().rev().take(5) {
                        ui.label(RichText::new("skipped").color(Color32::from_rgb(160, 90, 0)));
                        ui.label(s);
                        ui.label("filtered");
                        ui.label("-");
                        ui.end_row();
                    }
                });
        });
    }

    fn render_step6_finished(&mut self, ui: &mut egui::Ui) {
        let st = self.stats.lock().unwrap().clone();

        classic_group_box(ui, "End of mirror", |ui| {
            ui.label(
                RichText::new("Mirroring operation complete.")
                    .size(16.0)
                    .strong()
                    .color(Color32::from_rgb(0, 110, 40)),
            );
            ui.add_space(6.0);
            ui.label(
                "Click Exit/FINISH to quit WinHTTrack.\n\
                 See log file(s) if necessary to ensure that everything is OK.\n\n\
                 Thanks for using WinHTTrack!",
            );

            ui.add_space(12.0);
            ui.label(format!(
                "Summary: {} links scanned, {} files written ({}), {} links skipped by MIME/filter rules, {} errors.",
                st.links_scanned,
                st.files_written,
                format_bytes(st.bytes_written),
                st.links_filtered_out,
                st.errors_count
            ));

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui
                    .add_sized(
                        [190.0, 34.0],
                        egui::Button::new(RichText::new("🌐 Browse Mirrored Website").strong()),
                    )
                    .clicked()
                {
                    if let Some(ref out_dir) = self.last_output_dir {
                        let index_html = out_dir.join("index.html");
                        if index_html.exists() {
                            let _ = std::process::Command::new("explorer").arg(index_html).spawn();
                        } else {
                            let _ = std::process::Command::new("explorer").arg(out_dir).spawn();
                        }
                    }
                }

                if ui
                    .add_sized([140.0, 34.0], egui::Button::new("📋 View log files"))
                    .clicked()
                {
                    self.show_log_modal = true;
                }

                if ui
                    .add_sized([140.0, 34.0], egui::Button::new("📄 New project..."))
                    .clicked()
                {
                    self.wizard_step = WizardStep::Welcome;
                }
            });
        });
    }

    /// Classic WinHTTrack `Set options...` (`Preferences and mirror options`) multi-tabbed modal dialog
    /// Styled with real raised 3D Win32 folder tabs, section dividers, info bubbles `(?)` on every setting,
    /// and a "Check Rules for Errors" validator button in Scan Rules!
    fn render_set_options_modal(&mut self, ctx: &egui::Context) {
        let mut open = self.show_options_modal;
        egui::Window::new("Preferences and mirror options (Set options...)")
            .open(&mut open)
            .resizable(true)
            .default_size([790.0, 560.0])
            .show(ctx, |ui| {
                // Distinct 2-row Win32 Folder Tab Strip with visible borders and active blue tab highlight
                Frame::NONE
                    .fill(Color32::from_rgb(220, 223, 230))
                    .inner_margin(Margin::symmetric(8, 6))
                    .corner_radius(CornerRadius::same(4))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(165, 170, 180)))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let tabs_row1 = [
                            (OptionsTab::ScanRules, "Scan Rules"),
                            (OptionsTab::MimeTypes, "MIME Types"),
                            (OptionsTab::Links, "Links"),
                            (OptionsTab::Build, "Build"),
                            (OptionsTab::Limits, "Limits"),
                        ];
                        let tabs_row2 = [
                            (OptionsTab::FlowControl, "Flow Control"),
                            (OptionsTab::BrowserId, "Browser ID"),
                            (OptionsTab::LogIndexCache, "Log, Index, Cache"),
                            (OptionsTab::Spider, "Spider"),
                            (OptionsTab::ExportsAndAuth, "Login & Auth"),
                        ];

                        ui.horizontal(|ui| {
                            for (tab, label) in tabs_row1 {
                                draw_classic_folder_tab(ui, &mut self.options_tab, tab, label);
                            }
                        });
                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            for (tab, label) in tabs_row2 {
                                draw_classic_folder_tab(ui, &mut self.options_tab, tab, label);
                            }
                        });
                    });

                // Framed Tab Page Container
                Frame::NONE
                    .fill(Color32::from_rgb(245, 246, 248))
                    .inner_margin(Margin::symmetric(14, 12))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(160, 165, 175)))
                    .show(ui, |ui| {
                        ScrollArea::vertical()
                            .max_height(395.0)
                            .show(ui, |ui| match self.options_tab {
                                OptionsTab::MimeTypes => {
                                    classic_group_box(
                                        ui,
                                        "MIME Types & File Category Exclusions",
                                        |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new("Check any file categories you want to IGNORE / SKIP:")
                                                        .strong(),
                                                );
                                                info_bubble(
                                                    ui,
                                                    &mut self.active_info_popup,
                                                    "How Two-Stage MIME Filtering Works",
                                                    "Why did MIME filters fail in the original HTTrack? Because original HTTrack often queued URLs and opened connections before checking their MIME type.\n\nOur Two-Stage Filter checks:\n1. Pre-Flight (Stage 1): Infers the MIME type from the URL extension (.mp4 -> video/mp4, .png -> image/png) and blocks it before opening a connection.\n2. HTTP Header Gate (Stage 2): Inspects the server's Content-Type response header on dynamic links and aborts before downloading the body.",
                                                );
                                            });
                                            ui.add_space(6.0);

                                            egui::Grid::new("mime_checkbox_grid")
                                                .num_columns(2)
                                                .spacing([20.0, 8.0])
                                                .show(ui, |ui| {
                                                    ui.horizontal(|ui| {
                                                        ui.checkbox(&mut self.skip_video, "Skip Videos (video/*)");
                                                        info_bubble(
                                                            ui,
                                                            &mut self.active_info_popup,
                                                            "Skip Videos (video/*)",
                                                            "Skips all video files (.mp4, .webm, .mkv, .avi, .mov) and any server response with Content-Type: video/*.",
                                                        );
                                                    });
                                                    ui.horizontal(|ui| {
                                                        ui.checkbox(&mut self.skip_audio, "Skip Audio (audio/*)");
                                                        info_bubble(
                                                            ui,
                                                            &mut self.active_info_popup,
                                                            "Skip Audio (audio/*)",
                                                            "Skips all audio files (.mp3, .wav, .ogg, .flac, .m4a) and any server response with Content-Type: audio/*.",
                                                        );
                                                    });
                                                    ui.end_row();

                                                    ui.horizontal(|ui| {
                                                        ui.checkbox(&mut self.skip_images, "Skip Images (image/*)");
                                                        info_bubble(
                                                            ui,
                                                            &mut self.active_info_popup,
                                                            "Skip Images (image/*)",
                                                            "Skips all images (.png, .jpg, .jpeg, .gif, .webp, .svg, .ico). Great when you only want fast text/HTML documentation.",
                                                        );
                                                    });
                                                    ui.horizontal(|ui| {
                                                        ui.checkbox(&mut self.skip_fonts, "Skip Web Fonts (font/*)");
                                                        info_bubble(
                                                            ui,
                                                            &mut self.active_info_popup,
                                                            "Skip Web Fonts (font/*)",
                                                            "Skips downloadable web font files (.woff2, .woff, .ttf, .otf). Pages will still render using system fonts.",
                                                        );
                                                    });
                                                    ui.end_row();

                                                    ui.horizontal(|ui| {
                                                        ui.checkbox(&mut self.skip_pdfs, "Skip PDF Documents (application/pdf)");
                                                        info_bubble(
                                                            ui,
                                                            &mut self.active_info_popup,
                                                            "Skip PDF Documents",
                                                            "Skips all .pdf files and any download returning Content-Type: application/pdf.",
                                                        );
                                                    });
                                                    ui.horizontal(|ui| {
                                                        ui.checkbox(&mut self.skip_archives, "Skip Archives (.zip, .tar.gz, .7z)");
                                                        info_bubble(
                                                            ui,
                                                            &mut self.active_info_popup,
                                                            "Skip ZIP & Binary Archives",
                                                            "Skips compressed archives (.zip, .tar, .gz, .tgz, .7z, .rar) and Content-Type: application/*zip*.",
                                                        );
                                                    });
                                                    ui.end_row();
                                                });

                                            ui.add_space(10.0);
                                            ui.separator();
                                            ui.add_space(8.0);

                                            ui.horizontal(|ui| {
                                                ui.label(RichText::new("Custom Excluded MIME Types (--exclude-mime):").strong());
                                                info_bubble(
                                                    ui,
                                                    &mut self.active_info_popup,
                                                    "Custom Excluded MIME Types",
                                                    "Enter comma-separated MIME types or wildcards to block (for example: 'video/*, application/octet-stream, text/csv'). You can also use category shorthands like 'video, audio, pdf, archive'.",
                                                );
                                            });
                                            ui.add(
                                                egui::TextEdit::singleline(&mut self.custom_exclude_mimes)
                                                    .desired_width(f32::INFINITY)
                                                    .hint_text("e.g. video/*, application/octet-stream"),
                                            );

                                            ui.add_space(8.0);
                                            ui.horizontal(|ui| {
                                                ui.label(RichText::new("Custom Excluded File Extensions (--exclude-ext):").strong());
                                                info_bubble(
                                                    ui,
                                                    &mut self.active_info_popup,
                                                    "Custom Excluded File Extensions",
                                                    "Enter comma-separated file extensions (with or without a dot) to reject immediately before connecting (for example: 'mp4, zip, exe, msi, dmg, iso').",
                                                );
                                            });
                                            ui.add(
                                                egui::TextEdit::singleline(&mut self.exclude_extensions)
                                                    .desired_width(f32::INFINITY)
                                                    .hint_text("e.g. mp4, zip, exe, msi, dmg, iso"),
                                            );

                                            ui.add_space(8.0);
                                            ui.horizontal(|ui| {
                                                ui.label(RichText::new("Whitelist Only Specific MIME Types (--include-mime):").strong());
                                                info_bubble(
                                                    ui,
                                                    &mut self.active_info_popup,
                                                    "Whitelist Only Specific MIME Types",
                                                    "Leave blank to allow all non-excluded types. If you enter MIME types here (e.g. 'image/*, text/css'), ONLY matching assets will be saved to disk, while HTML pages are still scanned so the crawler can find those assets.",
                                                );
                                            });
                                            ui.add(
                                                egui::TextEdit::singleline(&mut self.custom_include_mimes)
                                                    .desired_width(f32::INFINITY)
                                                    .hint_text("Leave blank for all, or e.g. image/*, text/css"),
                                            );
                                        },
                                    );

                                    ui.add_space(10.0);
                                    classic_group_box(ui, "Test a Sample URL Against Your MIME & Filter Rules (--why)", |ui| {
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::TextEdit::singleline(&mut self.test_url_input)
                                                    .desired_width(470.0),
                                            );
                                            if ui.button("🔍 Test URL").clicked() {
                                                self.run_rule_tester();
                                            }
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Live URL Rule Tester (--why)",
                                                "Type any sample URL (e.g. https://example.com/files/movie.mp4) and click 'Test URL' to see immediately whether your current MIME and Scan Rules will ACCEPT or BLOCK it.",
                                            );
                                        });
                                        if let Some((allowed, ref reason, ref mime)) = self.test_result {
                                            ui.add_space(4.0);
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} — {} (Inferred Pre-Flight MIME: {})",
                                                    if allowed { "✅ ACCEPTED" } else { "🚫 BLOCKED" },
                                                    reason,
                                                    mime
                                                ))
                                                .strong()
                                                .color(if allowed {
                                                    Color32::from_rgb(0, 120, 40)
                                                } else {
                                                    Color32::from_rgb(180, 0, 0)
                                                }),
                                            );
                                        }
                                    });
                                }

                                OptionsTab::ScanRules => {
                                    classic_group_box(ui, "Scan Rules (Exclude links / Include links)", |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new("Use wildcards to include (+pattern) or exclude (-pattern) URLs or links:")
                                                    .strong(),
                                            );
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "How Scan Rules Work",
                                                "• Separate multiple rules with spaces or new lines.\n\
                                                 • Prefix with '+' to FORCE-ALLOW matching URLs (e.g. +*.png +*.css)\n\
                                                 • Prefix with '-' to BLOCK matching URLs (e.g. -*.zip -*/logout* -*/admin/*)\n\
                                                 • Prefix with '-mime:' or '+mime:' for MIME rules (e.g. -mime:video/*)\n\
                                                 • Size rules: '-*[>5000]' blocks files larger than 5,000 KB.\n\
                                                 • IMPORTANT: Rules are evaluated from left to right, and the LAST matching rule wins!",
                                            );
                                        });
                                        ui.add_space(6.0);
                                        ui.add(
                                            egui::TextEdit::multiline(&mut self.wildcard_filters)
                                                .desired_rows(4)
                                                .desired_width(f32::INFINITY)
                                                .font(FontId::monospace(13.0)),
                                        );

                                        ui.add_space(8.0);
                                        ui.horizontal_wrapped(|ui| {
                                            // Check Rules button requested by user!
                                            let check_btn = egui::Button::new(
                                                RichText::new("✔ Check Rules for Errors")
                                                    .strong()
                                                    .color(Color32::from_rgb(0, 80, 30)),
                                            )
                                            .fill(Color32::from_rgb(215, 245, 222))
                                            .stroke(Stroke::new(1.0, Color32::from_rgb(80, 165, 100)));

                                            if ui
                                                .add(check_btn)
                                                .on_hover_text("Validate your scan rules for syntax errors, missing +/- signs, or ordering mistakes")
                                                .clicked()
                                            {
                                                self.validate_scan_rules_config();
                                            }

                                            ui.separator();
                                            ui.label("Quick Add:");
                                            if ui.button("+ Images (+*.png +*.jpg +*.gif)").clicked() {
                                                self.wildcard_filters.push_str(" +*.png +*.jpg +*.jpeg +*.gif +*.svg");
                                                self.validate_scan_rules_config();
                                            }
                                            if ui.button("+ CSS/JS (+*.css +*.js)").clicked() {
                                                self.wildcard_filters.push_str(" +*.css +*.js");
                                                self.validate_scan_rules_config();
                                            }
                                            if ui.button("- Exclude Logouts (-*logout*)").clicked() {
                                                self.wildcard_filters.push_str(" -*logout* -*signout*");
                                                self.validate_scan_rules_config();
                                            }
                                        });

                                        // Display validation report if user clicked "Check Rules for Errors"
                                        if let Some(ref report) = self.scan_rules_check {
                                            ui.add_space(10.0);
                                            Frame::NONE
                                                .fill(if report.is_valid {
                                                    Color32::from_rgb(232, 248, 236)
                                                } else {
                                                    Color32::from_rgb(253, 235, 235)
                                                })
                                                .corner_radius(CornerRadius::same(4))
                                                .inner_margin(Margin::symmetric(10, 8))
                                                .stroke(Stroke::new(
                                                    1.0,
                                                    if report.is_valid {
                                                        Color32::from_rgb(70, 160, 90)
                                                    } else {
                                                        Color32::from_rgb(200, 60, 60)
                                                    },
                                                ))
                                                .show(ui, |ui| {
                                                    ui.set_width(ui.available_width());
                                                    ui.label(
                                                        RichText::new(&report.summary)
                                                            .strong()
                                                            .color(if report.is_valid {
                                                                Color32::from_rgb(15, 95, 35)
                                                            } else {
                                                                Color32::from_rgb(160, 20, 20)
                                                            }),
                                                    );
                                                    ui.add_space(4.0);
                                                    for (ok, line) in &report.details {
                                                        ui.label(
                                                            RichText::new(format!(
                                                                "{} {}",
                                                                if *ok { "✓" } else { "✗" },
                                                                line
                                                            ))
                                                            .font(FontId::monospace(11.5))
                                                            .color(if *ok {
                                                                Color32::from_rgb(30, 80, 40)
                                                            } else {
                                                                Color32::from_rgb(175, 20, 20)
                                                            }),
                                                        );
                                                    }
                                                });
                                        }
                                    });
                                }

                                OptionsTab::Links => {
                                    classic_group_box(ui, "Links Options", |ui| {
                                        ui.horizontal(|ui| {
                                            ui.checkbox(
                                                &mut self.near_files,
                                                "Get non-HTML files related to a link, e.g. external .ZIP or pictures (--near / -n)",
                                            );
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Get non-HTML files 'near' a page (-n / --near)",
                                                "Most modern websites host their images, CSS stylesheets, and JS scripts on an external CDN (like cdn.cloudflare.com or fonts.googleapis.com).\n\nEnabling this option tells WinHTTrack to download non-HTML assets referenced by a page even if they live on an external domain, WITHOUT crawling external webpages on that domain.",
                                            );
                                        });
                                        ui.add_space(10.0);
                                        ui.separator();
                                        ui.add_space(10.0);

                                        ui.horizontal(|ui| {
                                            ui.checkbox(
                                                &mut self.extended_parsing,
                                                "Attempt to detect all links (even in unknown tags or JavaScript code) (-%P)",
                                            );
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Extended Link Parsing (-%P)",
                                                "In addition to standard <a href> and <img src> tags, scans inside <script> blocks, data-src, srcset, and inline CSS styles for quoted asset paths.",
                                            );
                                        });
                                        ui.add_space(10.0);
                                        ui.separator();
                                        ui.add_space(10.0);

                                        ui.horizontal(|ui| {
                                            ui.checkbox(
                                                &mut self.test_all_links,
                                                "Test all links (even forbidden ones) (-t)",
                                            );
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Test All Links (-t)",
                                                "Checks HTTP status codes even for external links that are excluded from being saved to disk.",
                                            );
                                        });
                                    });
                                }

                                OptionsTab::Build => {
                                    classic_group_box(ui, "Local Structure & Link Rewriting", |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("Local Structure Type (-N — how files are organized on disk):").strong());
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Structure Type (-N)",
                                                "Controls the folder hierarchy created inside your project folder:\n\
                                                 • N0 (Default): Creates a folder named after the website host (e.g. example.com/docs/index.html).\n\
                                                 • N100: Same folder tree, but skips the top example.com/ folder.\n\
                                                 • N1: Places all HTML pages in web/ and all images/assets in web/images/.\n\
                                                 • N1003: Saves every downloaded file flat inside the root project folder.",
                                            );
                                        });
                                        let struct_labels = [
                                            "N0: Site-structure (default) — example.com/folder/page.html",
                                            "N1: Html in web/, images/other files in web/images/",
                                            "N2: Html in web/HTML, images/other in web/images",
                                            "N3: Html in web/, images/other in web/",
                                            "N4: Html in web/, images/other in web/xxx (grouped by file extension)",
                                            "N100: Site-structure, WITHOUT www.domain.xxx/ top folder",
                                            "N1003: Flat folder (all files placed directly in project folder)",
                                            "User-defined structure (%h%p/%n%q.%t)...",
                                        ];
                                        egui::ComboBox::from_id_salt("modal_struct_combo")
                                            .width(540.0)
                                            .selected_text(struct_labels[self.structure_idx])
                                            .show_ui(ui, |ui| {
                                                for (i, l) in struct_labels.iter().enumerate() {
                                                    ui.selectable_value(&mut self.structure_idx, i, *l);
                                                }
                                            });

                                        if self.structure_idx == 7 {
                                            ui.add_space(6.0);
                                            ui.label("User-defined structure template (%h=host, %p=path, %n=name, %q=query-md5, %t=ext):");
                                            ui.add(egui::TextEdit::singleline(&mut self.user_structure_str).desired_width(360.0));
                                        }

                                        ui.add_space(12.0);
                                        ui.separator();
                                        ui.add_space(10.0);

                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("Link Rewriting Mode (-K):").strong());
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Link Rewriting Mode (-K)",
                                                "Controls how href=\"...\" and src=\"...\" attributes inside saved HTML files are rewritten:\n\
                                                 • K0 (Default - Relative URI): Rewrites links to relative local paths (e.g. ../images/logo.png) so you can open the HTML files from any folder or USB drive offline.\n\
                                                 • K (Absolute URL): Converts links to full http://... URLs.\n\
                                                 • K3 (Absolute URI): Rewrites links starting with '/' for hosting on a local web server.\n\
                                                 • K4 (Original URL): Leaves all HTML links completely unmodified.",
                                            );
                                        });
                                        let link_labels = [
                                            "K0: Relative URI / Absolute URL (default — works offline from any folder)",
                                            "K: Absolute URL (points links to live http://... address)",
                                            "K3: Absolute URI (/folder/page.html — for hosting on a local web server)",
                                            "K4: Original URL (leaves all href/src links untouched as they were on the server)",
                                        ];
                                        egui::ComboBox::from_id_salt("modal_link_combo")
                                            .width(540.0)
                                            .selected_text(link_labels[self.link_mode_idx])
                                            .show_ui(ui, |ui| {
                                                for (i, l) in link_labels.iter().enumerate() {
                                                    ui.selectable_value(&mut self.link_mode_idx, i, *l);
                                                }
                                            });

                                        ui.add_space(12.0);
                                        ui.separator();
                                        ui.add_space(10.0);

                                        ui.horizontal(|ui| {
                                            ui.checkbox(
                                                &mut self.single_file,
                                                "Self-contained Single-File HTML (-%Z / --single-file)",
                                            );
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Single-File Base64 Inlining (-%Z)",
                                                "Embeds each page's CSS stylesheets, JavaScript, images, and fonts directly inside the .html file as base64 'data:' URIs so every saved HTML file is self-contained.",
                                            );
                                        });
                                    });
                                }

                                OptionsTab::Limits => {
                                    classic_group_box(ui, "Mirroring Depth & Size Limits", |ui| {
                                        egui::Grid::new("limits_grid")
                                            .num_columns(2)
                                            .spacing([16.0, 12.0])
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.label("Maximum mirroring depth from root address (-rN):");
                                                    info_bubble(
                                                        ui,
                                                        &mut self.active_info_popup,
                                                        "Maximum Mirroring Depth (-rN)",
                                                        "How many link 'clicks' deep to follow from your starting URL. 1 = only the start page and links directly on it. 9999 = the entire site.",
                                                    );
                                                });
                                                ui.add(egui::DragValue::new(&mut self.max_depth).range(0..=99999));
                                                ui.end_row();

                                                ui.horizontal(|ui| {
                                                    ui.label("Maximum mirroring depth for external addresses (-%eN):");
                                                    info_bubble(
                                                        ui,
                                                        &mut self.active_info_popup,
                                                        "External Depth (-%eN)",
                                                        "How many clicks deep to follow links that leave your starting domain (0 = do not crawl external domains).",
                                                    );
                                                });
                                                ui.add(egui::DragValue::new(&mut self.ext_depth).range(0..=999));
                                                ui.end_row();

                                                ui.horizontal(|ui| {
                                                    ui.label("Max size of any non-HTML file (MB) (-mN):");
                                                    info_bubble(
                                                        ui,
                                                        &mut self.active_info_popup,
                                                        "Max Non-HTML File Size (-mN)",
                                                        "Skips any single media/binary file larger than this size in megabytes.",
                                                    );
                                                });
                                                ui.add(
                                                    egui::TextEdit::singleline(&mut self.max_non_html_mb)
                                                        .desired_width(120.0)
                                                        .hint_text("No limit"),
                                                );
                                                ui.end_row();

                                                ui.horizontal(|ui| {
                                                    ui.label("Max overall site size to download (MB) (-MN):");
                                                    info_bubble(
                                                        ui,
                                                        &mut self.active_info_popup,
                                                        "Max Overall Site Size (-MN)",
                                                        "Stops the entire mirror automatically once total downloaded data reaches this cap in megabytes.",
                                                    );
                                                });
                                                ui.add(
                                                    egui::TextEdit::singleline(&mut self.max_overall_mb)
                                                        .desired_width(120.0)
                                                        .hint_text("No limit"),
                                                );
                                                ui.end_row();

                                                ui.horizontal(|ui| {
                                                    ui.label("Max time duration for mirror (seconds) (-EN):");
                                                    info_bubble(
                                                        ui,
                                                        &mut self.active_info_popup,
                                                        "Max Mirror Duration (-EN)",
                                                        "Stops the mirror automatically after this many seconds (for example, 3600 = 1 hour).",
                                                    );
                                                });
                                                ui.add(
                                                    egui::TextEdit::singleline(&mut self.max_time_secs)
                                                        .desired_width(120.0)
                                                        .hint_text("No limit"),
                                                );
                                                ui.end_row();
                                            });
                                    });
                                }

                                OptionsTab::FlowControl => {
                                    classic_group_box(ui, "Flow Control (Connections, Timeouts & Retries)", |ui| {
                                        egui::Grid::new("flow_grid")
                                            .num_columns(2)
                                            .spacing([16.0, 12.0])
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.label("Number of simultaneous connections (-cN):");
                                                    info_bubble(
                                                        ui,
                                                        &mut self.active_info_popup,
                                                        "Simultaneous Connections (-cN)",
                                                        "Number of parallel HTTP connections. 4 is recommended; setting this too high may cause servers or firewalls to rate-limit your IP.",
                                                    );
                                                });
                                                ui.add(egui::DragValue::new(&mut self.sockets).range(1..=32));
                                                ui.end_row();

                                                ui.horizontal(|ui| {
                                                    ui.label("File timeout in seconds (-TN):");
                                                    info_bubble(
                                                        ui,
                                                        &mut self.active_info_popup,
                                                        "Request Timeout (-TN)",
                                                        "How many seconds to wait for a non-responding server before aborting or retrying the request.",
                                                    );
                                                });
                                                ui.add(egui::DragValue::new(&mut self.timeout_secs).range(5..=300));
                                                ui.end_row();

                                                ui.horizontal(|ui| {
                                                    ui.label("Number of retries on non-fatal errors (-RN):");
                                                    info_bubble(
                                                        ui,
                                                        &mut self.active_info_popup,
                                                        "Retries (-RN)",
                                                        "How many times to retry a failed download when a temporary network timeout occurs.",
                                                    );
                                                });
                                                ui.add(egui::DragValue::new(&mut self.retries).range(0..=10));
                                                ui.end_row();
                                            });
                                    });
                                }

                                OptionsTab::BrowserId => {
                                    classic_group_box(ui, "Browser Identity & HTML Footer", |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("Browser Impersonation Profile (--impersonate):").strong());
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Browser Impersonation Profile",
                                                "Many modern websites block crawlers that advertise 'HTTrack' in their User-Agent. Selecting Chrome 131 sends realistic desktop browser headers (User-Agent, Sec-CH-UA, Accept) so CDNs serve pages normally.",
                                            );
                                        });
                                        let bp_labels = [
                                            "Chrome 131 Desktop (Recommended — sends modern User-Agent & Sec-CH-UA headers)",
                                            "Firefox 133 Desktop",
                                            "Safari 18 Desktop (macOS)",
                                            "Classic HTTrack/3.x Bot (Mozilla/5.0 (compatible; HTTrack/3.x))",
                                        ];
                                        egui::ComboBox::from_id_salt("modal_bp_combo")
                                            .width(540.0)
                                            .selected_text(bp_labels[self.browser_profile_idx])
                                            .show_ui(ui, |ui| {
                                                for (i, l) in bp_labels.iter().enumerate() {
                                                    ui.selectable_value(&mut self.browser_profile_idx, i, *l);
                                                }
                                            });

                                        ui.add_space(10.0);
                                        ui.separator();
                                        ui.add_space(8.0);

                                        ui.label("Custom User-Agent override (-F, leave blank to use profile above):");
                                        ui.add(egui::TextEdit::singleline(&mut self.custom_user_agent).desired_width(f32::INFINITY));

                                        ui.add_space(8.0);
                                        ui.horizontal(|ui| {
                                            ui.label("HTML Footer comment placed in each mirrored HTML file (-%F):");
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "HTML Footer Comment (-%F)",
                                                "An HTML comment stamp injected into mirrored pages recording the source URL and timestamp ({url}, {date}). Set to 'none' to disable.",
                                            );
                                        });
                                        ui.add(egui::TextEdit::singleline(&mut self.html_footer).desired_width(f32::INFINITY));

                                        ui.add_space(8.0);
                                        ui.label("Accepted Languages (-%l):");
                                        ui.add(egui::TextEdit::singleline(&mut self.accept_language).desired_width(240.0));
                                    });
                                }

                                OptionsTab::LogIndexCache => {
                                    classic_group_box(ui, "Log, Index, Cache & Archives", |ui| {
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut self.use_cache, "Use a cache for updates and retries (hts-cache/new.zip) (-C1)");
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Use a Cache (hts-cache/)",
                                                "Stores HTTP metadata and original HTML in hts-cache/new.zip so you can resume interrupted mirrors or run fast incremental updates later.",
                                            );
                                        });
                                        ui.add_space(6.0);
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut self.make_index, "Create a Start Page (root index.html) (-I)");
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Create a Start Page (-I)",
                                                "Generates an index.html landing page at the root of your project folder that redirects to or lists your mirrored site(s).",
                                            );
                                        });
                                        ui.add_space(6.0);
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut self.generate_changes, "Write hts-changes.json differential change summary (-%d / --changes)");
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Differential Audit Report (hts-changes.json)",
                                                "Writes a machine-readable JSON summary of every file downloaded, its original URL, local relative path, and byte size.",
                                            );
                                        });
                                        ui.add_space(6.0);
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut self.generate_warc, "Write an ISO-28500 WARC/1.1 archive (.warc.gz) (-%r / --warc)");
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "ISO-28500 WARC/1.1 Archive (--warc)",
                                                "Packages every HTTP response and SHA-1 digest into a standard .warc.gz archival file compatible with ReplayWeb.page and Internet Archive tools.",
                                            );
                                        });
                                    });
                                }

                                OptionsTab::Spider => {
                                    classic_group_box(ui, "Spider, Travel Mode & Robots.txt", |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("Travel mode (Directory Navigation Direction):").strong());
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Travel Mode (Folder Boundaries)",
                                                "If your starting URL is https://example.com/docs/guide/index.html:\n\
                                                 • Can go down (Default -D): Only crawls inside /docs/guide/* and its subfolders.\n\
                                                 • Can both go up & down (-B): Allows crawling parent folders like /docs/ or /about/ on the same site.\n\
                                                 • Stay in the same directory (-S): Only crawls files directly in /docs/guide/ without entering subfolders.",
                                            );
                                        });
                                        let dir_labels = [
                                            "Stay in the same directory (-S)",
                                            "Can go down into subdirectories (default -D)",
                                            "Can go up to parent directories (-U)",
                                            "Can both go up & down across the entire site (-B)",
                                        ];
                                        egui::ComboBox::from_id_salt("modal_dir_combo")
                                            .width(480.0)
                                            .selected_text(dir_labels[self.dir_scope_idx])
                                            .show_ui(ui, |ui| {
                                                for (i, l) in dir_labels.iter().enumerate() {
                                                    ui.selectable_value(&mut self.dir_scope_idx, i, *l);
                                                }
                                            });

                                        ui.add_space(10.0);
                                        ui.separator();
                                        ui.add_space(8.0);

                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("Global travel mode (Host / Domain Boundary):").strong());
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Global Travel Mode (Domain Boundaries)",
                                                "Controls whether the spider can follow links to other subdomains or external websites:\n\
                                                 • Stay on the same address (Default -a): Stays strictly on the exact starting hostname (e.g. docs.example.com).\n\
                                                 • Stay on the same principal domain (-d): Also crawls sibling subdomains (*.example.com).\n\
                                                 • Go everywhere on the web (-e): Follows links to external websites (always combine with a small Max Depth like -r2!).",
                                            );
                                        });
                                        let host_labels = [
                                            "Stay on the same address (default -a — e.g. docs.example.com only)",
                                            "Stay on the same principal domain (-d — allow any *.example.com subdomain)",
                                            "Stay on the same top level domain (-l — allow any .com/.org)",
                                            "Go everywhere on the web (-e — follow external sites)",
                                        ];
                                        egui::ComboBox::from_id_salt("modal_host_combo")
                                            .width(480.0)
                                            .selected_text(host_labels[self.host_scope_idx])
                                            .show_ui(ui, |ui| {
                                                for (i, l) in host_labels.iter().enumerate() {
                                                    ui.selectable_value(&mut self.host_scope_idx, i, *l);
                                                }
                                            });

                                        ui.add_space(10.0);
                                        ui.separator();
                                        ui.add_space(8.0);

                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut self.obey_robots, "Follow robots.txt rules on sites (-s2)");
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Follow robots.txt Rules",
                                                "When checked, respects the site's /robots.txt Allow and Disallow directives. Uncheck (-s0) if you are archiving your own site and robots.txt blocks folders you need.",
                                            );
                                        });
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut self.use_sitemap, "Seed crawl from site's sitemap.xml (--sitemap / -%m)");
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Seed from sitemap.xml (-%m)",
                                                "Automatically fetches /sitemap.xml (and any Sitemap: declared in robots.txt) to discover orphan pages that aren't linked from the main menu.",
                                            );
                                        });
                                    });
                                }

                                OptionsTab::ExportsAndAuth => {
                                    classic_group_box(ui, "Session Cookies & Authorization (Modern Replacement for Proxy)", |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("Authorization Header (--auth):").strong());
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Authorization Header (--auth)",
                                                "To mirror a password-protected site or private documentation:\n\
                                                 • Enter 'username:password' for HTTP Basic Auth, OR\n\
                                                 • Enter 'Bearer <token>' for token-authenticated sites.",
                                            );
                                        });
                                        ui.add(
                                            egui::TextEdit::singleline(&mut self.auth_header)
                                                .desired_width(f32::INFINITY)
                                                .hint_text("Bearer <token>   or   username:password"),
                                        );
                                        ui.add_space(8.0);
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("Session Cookie String (--cookie):").strong());
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Session Cookie String (--cookie)",
                                                "If a website uses a modern web login form (which old intercept proxies can't capture over HTTPS), simply log in using Chrome/Firefox, copy your cookie string from DevTools, and paste it here.",
                                            );
                                        });
                                        ui.add(
                                            egui::TextEdit::singleline(&mut self.cookies_raw)
                                                .desired_width(f32::INFINITY)
                                                .hint_text("sessionid=abc123; cf_clearance=..."),
                                        );
                                        ui.add_space(8.0);
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("Netscape cookies.txt File (--cookies-file):").strong());
                                            info_bubble(
                                                ui,
                                                &mut self.active_info_popup,
                                                "Netscape cookies.txt File",
                                                "Load a cookies.txt file exported from your web browser.",
                                            );
                                        });
                                        ui.horizontal(|ui| {
                                            ui.add(egui::TextEdit::singleline(&mut self.cookies_file).desired_width(420.0));
                                            if ui.button("Browse...").clicked() {
                                                if let Some(f) = rfd::FileDialog::new().pick_file() {
                                                    self.cookies_file = f.to_string_lossy().to_string();
                                                }
                                            }
                                        });
                                    });
                                }
                            });
                    });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_sized([90.0, 28.0], egui::Button::new(RichText::new("OK").strong()))
                        .clicked()
                    {
                        self.show_options_modal = false;
                    }
                    if ui
                        .add_sized([90.0, 28.0], egui::Button::new("Cancel"))
                        .clicked()
                    {
                        self.show_options_modal = false;
                    }
                });
            });
        if !open {
            self.show_options_modal = false;
        }
    }

    /// Popup modal when a user clicks any blue `(?)` info bubble
    fn render_info_bubble_popup(&mut self, ctx: &egui::Context) {
        if let Some((title, body)) = self.active_info_popup.clone() {
            let mut open = true;
            egui::Window::new(format!("ℹ️ {}", title))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_width(480.0)
                .show(ctx, |ui| {
                    Frame::NONE
                        .fill(Color32::from_rgb(235, 243, 254))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(12, 10))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(150, 185, 235)))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(&body)
                                    .size(13.0)
                                    .color(Color32::from_rgb(20, 30, 50)),
                            );
                        });
                    ui.add_space(10.0);
                    if ui
                        .add_sized([90.0, 26.0], egui::Button::new(RichText::new("Got it").strong()))
                        .clicked()
                    {
                        self.active_info_popup = None;
                    }
                });
            if !open {
                self.active_info_popup = None;
            }
        }
    }

    fn render_add_url_modal(&mut self, ctx: &egui::Context) {
        let mut open = self.show_add_url_modal;
        egui::Window::new("Insert URL")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("URL:").strong());
                ui.add(egui::TextEdit::singleline(&mut self.add_url_input).desired_width(f32::INFINITY));
                ui.add_space(8.0);
                classic_group_box(ui, "Authentication (only if needed)", |ui| {
                    egui::Grid::new("auth_url_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Login:");
                        ui.add(egui::TextEdit::singleline(&mut self.add_url_login).desired_width(240.0));
                        ui.end_row();
                        ui.label("Password:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.add_url_password)
                                .password(true)
                                .desired_width(240.0),
                        );
                        ui.end_row();
                    });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.add_sized([80.0, 26.0], egui::Button::new("OK")).clicked() {
                        let u = self.add_url_input.trim();
                        if !u.is_empty() && u != "https://" {
                            if !self.urls_text.is_empty() && !self.urls_text.ends_with('\n') {
                                self.urls_text.push('\n');
                            }
                            self.urls_text.push_str(u);
                        }
                        if !self.add_url_login.trim().is_empty() {
                            self.auth_header = format!(
                                "{}:{}",
                                self.add_url_login.trim(),
                                self.add_url_password.trim()
                            );
                        }
                        self.add_url_input = "https://".to_string();
                        self.show_add_url_modal = false;
                    }
                    if ui.add_sized([80.0, 26.0], egui::Button::new("Cancel")).clicked() {
                        self.show_add_url_modal = false;
                    }
                });
            });
        if !open {
            self.show_add_url_modal = false;
        }
    }

    fn render_log_modal(&mut self, ctx: &egui::Context) {
        let mut open = self.show_log_modal;
        let st = self.stats.lock().unwrap().clone();
        egui::Window::new("View log files (hts-log.txt)")
            .open(&mut open)
            .default_size([620.0, 380.0])
            .show(ctx, |ui| {
                ScrollArea::vertical().max_height(310.0).show(ui, |ui| {
                    if let Some(ref out_dir) = self.last_output_dir {
                        let log_file = out_dir.join("hts-log.txt");
                        if let Ok(content) = std::fs::read_to_string(&log_file) {
                            ui.label(RichText::new(content).font(FontId::monospace(12.0)));
                            ui.separator();
                        }
                    }
                    ui.label(RichText::new("--- Saved Files ---").strong());
                    for f in &st.recent_files {
                        ui.label(RichText::new(format!("Saved: {}", f)).font(FontId::monospace(11.5)));
                    }
                    ui.label(RichText::new("--- Skipped by MIME / Filter Rules ---").strong());
                    for s in &st.skipped_files {
                        ui.label(RichText::new(format!("Skipped: {}", s)).font(FontId::monospace(11.5)));
                    }
                    ui.label(RichText::new("--- Errors ---").strong());
                    for e in &st.errors {
                        ui.label(RichText::new(format!("Error: {}", e)).font(FontId::monospace(11.5)));
                    }
                });
                ui.separator();
                if ui.button("Close").clicked() {
                    self.show_log_modal = false;
                }
            });
        if !open {
            self.show_log_modal = false;
        }
    }

    fn render_about_modal(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about_modal;
        egui::Window::new("About WinHTTrack Website Copier")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(450.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("WinHTTrack Website Copier 3.51-0 (Rust Alternative)")
                        .strong()
                        .size(14.5),
                );
                ui.add_space(6.0);
                ui.label(
                    "An unofficial, community memory-safe Rust alternative to HTTrack Website Copier.\n\n\
                     Original HTTrack Concept, Engine & Templates:\n\
                     Copyright (C) 1998-2026 Xavier Roche & contributors\n\
                     Licensed under GPL-3.0-or-later",
                );
                ui.add_space(6.0);
                ui.hyperlink_to("🌐 Official HTTrack Website (https://www.httrack.com/)", "https://www.httrack.com/");
                ui.hyperlink_to("📦 Official Upstream Repository (https://github.com/xroche/httrack)", "https://github.com/xroche/httrack");
                ui.add_space(10.0);
                if ui.button("OK").clicked() {
                    self.show_about_modal = false;
                }
            });
        if !open {
            self.show_about_modal = false;
        }
    }
}

/// Draws a distinct Win32-style raised folder tab button with a clear border, active fill, and divider
fn draw_classic_folder_tab(
    ui: &mut egui::Ui,
    current_tab: &mut OptionsTab,
    tab: OptionsTab,
    label: &str,
) {
    let active = *current_tab == tab;
    let fill = if active {
        Color32::from_rgb(35, 85, 165)
    } else {
        Color32::from_rgb(242, 243, 247)
    };
    let text_color = if active {
        Color32::WHITE
    } else {
        Color32::from_rgb(30, 35, 45)
    };
    let stroke = if active {
        Stroke::new(1.5, Color32::from_rgb(15, 55, 125))
    } else {
        Stroke::new(1.0, Color32::from_rgb(160, 165, 175))
    };

    let btn = egui::Button::new(RichText::new(label).size(12.5).strong().color(text_color))
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(4))
        .min_size(Vec2::new(138.0, 28.0));

    if ui.add(btn).clicked() {
        *current_tab = tab;
    }
}

fn configure_classic_win32_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.window_fill = Color32::from_rgb(240, 240, 243);
    visuals.panel_fill = Color32::from_rgb(240, 240, 243);
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(245, 245, 247);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(255, 255, 255);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(225, 226, 230);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(165, 168, 175));
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(215, 228, 248);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0, 102, 204));
    visuals.widgets.active.bg_fill = Color32::from_rgb(195, 216, 245);
    visuals.selection.bg_fill = Color32::from_rgb(0, 95, 184);
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    ctx.set_visuals(visuals);
}

fn classic_group_box(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    Frame::NONE
        .fill(Color32::from_rgb(250, 250, 252))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(14, 12))
        .stroke(Stroke::new(1.0, Color32::from_rgb(175, 180, 190)))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.label(
                    RichText::new(title)
                        .strong()
                        .size(13.0)
                        .color(Color32::from_rgb(25, 45, 110)),
                );
                ui.separator();
                ui.add_space(4.0);
            }
            add_contents(ui);
        });
}

fn format_bytes(b: usize) -> String {
    if b < 1024 {
        format!("{} B", b)
    } else if b < 1024 * 1024 {
        format!("{:.1} KiB", b as f64 / 1024.0)
    } else {
        format!("{:.2} MiB", b as f64 / (1024.0 * 1024.0))
    }
}

fn load_window_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("WinHTTrack Website Copier 3.51-0")
        .with_inner_size([880.0, 600.0])
        .with_min_inner_size([760.0, 500.0]);

    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "WinHTTrack Website Copier",
        native_options,
        Box::new(|cc| Ok(Box::new(WinHttrackApp::new(cc)))),
    )
}
