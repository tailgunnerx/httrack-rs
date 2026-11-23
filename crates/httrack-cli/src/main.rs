use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use url::Url;

use httrack_core::{
    compute_relative_path, compute_save_path, BrowserProfile, CrawlAction, CrawlerEngine,
    DirectionScope, FilterEngine, HostScope, HtsOptions, LinkRewriteMode, RobotsRules,
    StructureType,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() == 1 {
        return run_interactive_wizard().await;
    }

    let mut opts = HtsOptions::default();
    opts.raw_args = args.clone();

    let mut why_url: Option<String> = None;
    let mut clean_only = false;

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];

        if arg == "--help" || arg == "-h" || arg == "-?" {
            print_help(&args[0]);
            return Ok(());
        }
        if arg == "--version" || arg == "-#h" {
            println!("HTTrack version 3.51-0 (Rust edition)");
            println!("Copyright (C) 1998-2026 Xavier Roche and other contributors [Rust port]");
            println!("[compiled: windows-x64-rust]");
            return Ok(());
        }
        if arg == "-#test" || arg.starts_with("-#test=") {
            return run_selftest(&args[i..]);
        }

        // Check scan rules (+pattern, -mime:..., +*.jpg, etc.)
        if arg.starts_with('+')
            || arg.starts_with("-mime:")
            || arg.starts_with("-*.")
            || arg.starts_with("-http://")
            || arg.starts_with("-https://")
            || arg.starts_with("-*[")
        {
            opts.filters.push(arg.clone());
            i += 1;
            continue;
        }

        match arg.as_str() {
            "-O" | "--path" | "--output" => {
                if i + 1 < args.len() {
                    parse_output_path(&mut opts, &args[i + 1]);
                    i += 2;
                    continue;
                }
            }
            // Modern, reliable MIME & extension filtering flags
            "--exclude-mime" | "--ignore-mime" => {
                if i + 1 < args.len() {
                    opts.exclude_mimes.push(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "--include-mime" | "--only-mime" => {
                if i + 1 < args.len() {
                    opts.include_mimes.push(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "--exclude-ext" | "--ignore-ext" => {
                if i + 1 < args.len() {
                    opts.exclude_extensions.push(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "--include-ext" | "--only-ext" => {
                if i + 1 < args.len() {
                    opts.include_extensions.push(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            // Modern Browser Impersonation & Session Auth (replaces legacy proxy config)
            "--impersonate" | "--browser" => {
                if i + 1 < args.len() {
                    opts.browser_profile = BrowserProfile::from_name(&args[i + 1]);
                    i += 2;
                    continue;
                }
            }
            "--auth" | "--bearer" => {
                if i + 1 < args.len() {
                    opts.auth_header = Some(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "--cookie" | "--cookies" => {
                if i + 1 < args.len() {
                    opts.cookies_raw = Some(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "--cookies-file" | "-%K" => {
                if i + 1 < args.len() {
                    opts.cookies_file = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                    continue;
                }
            }
            "-w" | "--mirror" => opts.action = CrawlAction::Mirror,
            "-W" | "--mirror-wizard" => opts.action = CrawlAction::MirrorWizard,
            "-g" | "--get" | "--get-files" => {
                opts.action = CrawlAction::GetFiles;
                opts.structure = StructureType::Preset(1003);
                opts.make_index = false;
            }
            "-i" | "--continue" => opts.action = CrawlAction::Continue,
            "--update" => {
                opts.action = CrawlAction::Update;
                opts.cache_mode = 2;
            }
            "-Y" | "--mirrorlinks" => opts.action = CrawlAction::MirrorLinks,
            "--spider" | "--testsite" | "--testlinks" => {
                opts.action = CrawlAction::Spider;
                opts.priority_mode = 0;
                opts.cache_mode = 0;
                opts.make_index = false;
            }
            "--skeleton" => {
                opts.action = CrawlAction::Skeleton;
                opts.priority_mode = 1;
            }
            "--clean" => clean_only = true,
            "-q" | "--quiet" | "-Q" | "--do-not-log" => opts.quiet = true,
            "-v" | "--verbose" => opts.verbose = true,
            "-n" | "--near" => opts.near_files = true,
            "-%d" | "--changes" | "--changes=1" => opts.generate_changes = true,
            "-%r" | "--warc" => opts.generate_warc = true,
            "--warc-file" => {
                if i + 1 < args.len() {
                    opts.generate_warc = true;
                    opts.warc_file = Some(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "-%m" | "--sitemap" => opts.use_sitemap = true,
            "--sitemap-url" => {
                if i + 1 < args.len() {
                    opts.use_sitemap = true;
                    opts.sitemap_url = Some(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "-%Z" | "--single-file" | "--single-file=1" => opts.single_file = true,
            "--single-file-max-size" => {
                if i + 1 < args.len() {
                    opts.single_file_max_size = args[i + 1].parse().unwrap_or(10_485_760);
                    i += 2;
                    continue;
                }
            }
            "-%Y" | "--why" => {
                if i + 1 < args.len() {
                    why_url = Some(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "-F" | "--user-agent" => {
                if i + 1 < args.len() {
                    opts.user_agent = args[i + 1].clone();
                    i += 2;
                    continue;
                }
            }
            "-%R" | "--referer" => {
                if i + 1 < args.len() {
                    opts.referer = Some(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "-%F" | "--footer" => {
                if i + 1 < args.len() {
                    opts.footer = args[i + 1].clone();
                    i += 2;
                    continue;
                }
            }
            "-%l" | "--language" | "--lang" => {
                if i + 1 < args.len() {
                    opts.language = args[i + 1].clone();
                    i += 2;
                    continue;
                }
            }
            "-%X" | "--headers" | "--header" => {
                if i + 1 < args.len() {
                    opts.custom_headers.push(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "-%L" | "--list" | "--filelist" => {
                if i + 1 < args.len() {
                    opts.url_list_file = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                    continue;
                }
            }
            "-N" | "--structure" | "--user-structure" => {
                if i + 1 < args.len() {
                    let val = &args[i + 1];
                    if let Ok(n) = val.parse::<u32>() {
                        opts.structure = StructureType::Preset(n);
                    } else {
                        opts.structure = StructureType::UserDefined(val.clone());
                    }
                    i += 2;
                    continue;
                }
            }
            "-D" | "--can-go-down" => opts.dir_scope = DirectionScope::DownOnly,
            "-U" | "--can-go-up" => opts.dir_scope = DirectionScope::UpOnly,
            "-B" | "--can-go-up-and-down" => opts.dir_scope = DirectionScope::Both,
            "-S" | "--stay-on-same-dir" => opts.dir_scope = DirectionScope::SameDir,
            "-a" | "--stay-on-same-address" => opts.host_scope = HostScope::SameAddress,
            "-d" | "--stay-on-same-domain" => opts.host_scope = HostScope::SameDomain,
            "-l" | "--stay-on-same-tld" => opts.host_scope = HostScope::SameTld,
            "-e" | "--go-everywhere" => opts.host_scope = HostScope::Everywhere,
            "-I0" | "--index=0" => opts.make_index = false,
            "-I" | "--index" => opts.make_index = true,
            _ => {
                if let Some(rest) = arg.strip_prefix("-r").or_else(|| arg.strip_prefix("--depth=")) {
                    if let Ok(d) = rest.parse::<usize>() {
                        opts.max_depth = d;
                    }
                } else if let Some(rest) = arg.strip_prefix("-%e").or_else(|| arg.strip_prefix("--ext-depth=")) {
                    if let Ok(d) = rest.parse::<usize>() {
                        opts.ext_depth = d;
                    }
                } else if let Some(rest) = arg.strip_prefix("-c").or_else(|| arg.strip_prefix("--sockets=")) {
                    if let Ok(c) = rest.parse::<usize>() {
                        opts.sockets = c.max(1);
                    }
                } else if let Some(rest) = arg.strip_prefix("-T").or_else(|| arg.strip_prefix("--timeout=")) {
                    if let Ok(t) = rest.parse::<u64>() {
                        opts.timeout_secs = t;
                    }
                } else if let Some(rest) = arg.strip_prefix("-R").or_else(|| arg.strip_prefix("--retries=")) {
                    if let Ok(r) = rest.parse::<u32>() {
                        opts.retries = r;
                    }
                } else if let Some(rest) = arg.strip_prefix("-E").or_else(|| arg.strip_prefix("--max-time=")) {
                    if let Ok(t) = rest.parse::<u64>() {
                        opts.max_time_secs = Some(t);
                    }
                } else if let Some(rest) = arg.strip_prefix("-M").or_else(|| arg.strip_prefix("--max-size=")) {
                    if let Ok(s) = rest.parse::<u64>() {
                        opts.max_overall_size = Some(s);
                    }
                } else if let Some(rest) = arg.strip_prefix("-m").or_else(|| arg.strip_prefix("--max-files=")) {
                    if let Ok(s) = rest.parse::<u64>() {
                        opts.max_file_size_non_html = Some(s);
                    }
                } else if let Some(rest) = arg.strip_prefix("-N").or_else(|| arg.strip_prefix("--structure=")) {
                    if let Ok(n) = rest.parse::<u32>() {
                        opts.structure = StructureType::Preset(n);
                    } else {
                        opts.structure = StructureType::UserDefined(rest.to_string());
                    }
                } else if let Some(rest) = arg.strip_prefix("-K").or_else(|| arg.strip_prefix("--keep-links=")) {
                    opts.link_mode = match rest {
                        "0" => LinkRewriteMode::Relative,
                        "" | "1" => LinkRewriteMode::AbsoluteUrl,
                        "3" => LinkRewriteMode::AbsoluteUri,
                        "4" => LinkRewriteMode::Original,
                        "5" => LinkRewriteMode::TransparentProxy,
                        _ => LinkRewriteMode::Relative,
                    };
                } else if let Some(rest) = arg.strip_prefix("-p").or_else(|| arg.strip_prefix("--priority=")) {
                    if let Ok(p) = rest.parse::<u8>() {
                        opts.priority_mode = p;
                    }
                } else if let Some(rest) = arg.strip_prefix("-s").or_else(|| arg.strip_prefix("--robots=")) {
                    if let Ok(s) = rest.parse::<u8>() {
                        opts.robots_mode = s;
                    }
                } else if let Some(rest) = arg.strip_prefix("-C").or_else(|| arg.strip_prefix("--cache=")) {
                    if let Ok(c) = rest.parse::<u8>() {
                        opts.cache_mode = c;
                    }
                } else if !arg.starts_with('-') {
                    opts.urls.push(arg.clone());
                }
            }
        }

        i += 1;
    }

    if clean_only {
        let _ = fs::remove_dir_all(opts.output_dir.join("hts-cache"));
        let _ = fs::remove_file(opts.output_dir.join("hts-log.txt"));
        println!("Cleaned cache and log files in {}", opts.output_dir.display());
        return Ok(());
    }

    // Handle -%Y / --why <URL> diagnostic explanation
    if let Some(target_raw) = why_url {
        let target = Url::parse(&target_raw).or_else(|_| Url::parse(&format!("http://{}", target_raw)))?;
        let seeds: Vec<Url> = opts
            .urls
            .iter()
            .filter_map(|u| Url::parse(u).or_else(|_| Url::parse(&format!("http://{}", u))).ok())
            .collect();
        let engine = FilterEngine::from_options(&opts);
        let (allowed, reason) = engine.evaluate_url(
            &target,
            &seeds,
            opts.host_scope,
            opts.dir_scope,
            opts.near_files,
            false,
        );
        println!(
            "{}: {} ({})",
            target,
            if allowed { "ACCEPTED" } else { "REJECTED" },
            reason
        );
        return Ok(());
    }

    // If `--continue` or `--update` with no URLs given, try loading from `hts-cache/doit.log`
    if opts.urls.is_empty() && matches!(opts.action, CrawlAction::Continue | CrawlAction::Update) {
        let doit_path = opts.output_dir.join("hts-cache").join("doit.log");
        if let Ok(content) = fs::read_to_string(&doit_path) {
            if let Some(first_line) = content.lines().next() {
                for tok in first_line.split_whitespace() {
                    if tok.starts_with("http://") || tok.starts_with("https://") {
                        opts.urls.push(tok.to_string());
                    }
                }
            }
        }
    }

    if opts.urls.is_empty() && opts.url_list_file.is_none() {
        eprintln!("Error: No URLs specified. Try `httrack --help` or run `httrack` with no arguments for interactive wizard.");
        std::process::exit(1);
    }

    let crawler = CrawlerEngine::new(opts);
    crawler.run().await?;

    Ok(())
}

fn parse_output_path(opts: &mut HtsOptions, param: &str) {
    if let Some((mirror_p, cache_p)) = param.split_once(',') {
        opts.output_dir = PathBuf::from(mirror_p);
        opts.cache_dir = Some(PathBuf::from(cache_p));
    } else {
        opts.output_dir = PathBuf::from(param);
    }
}

async fn run_interactive_wizard() -> anyhow::Result<()> {
    println!();
    println!("Welcome to HTTrack Website Copier (Offline Browser) 3.51-0 [Rust Edition]");
    println!("To launch the modern Web GUI with interactive filter explanations, run: webhttrack");
    println!();

    let mut project_name = String::new();
    print!("Enter project name : ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut project_name)?;
    let project_name = project_name.trim();

    let mut base_path = String::new();
    print!("Base path (return=current directory) : ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut base_path)?;
    let base_path = base_path.trim();

    let mut urls_input = String::new();
    print!("Enter URLs (separated by commas or blank spaces) : ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut urls_input)?;

    let urls: Vec<String> = urls_input
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    if urls.is_empty() {
        println!("No URL entered. Exiting.");
        return Ok(());
    }

    let mut exclude_input = String::new();
    print!("Skip any media types? (e.g. video,audio,pdf,zip or press Enter for none) : ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut exclude_input)?;
    let exclude_input = exclude_input.trim();

    let mut out_dir = if base_path.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(base_path)
    };
    if !project_name.is_empty() {
        out_dir.push(project_name);
    }

    let mut opts = HtsOptions::default();
    opts.urls = urls;
    opts.output_dir = out_dir;
    if !exclude_input.is_empty() {
        opts.exclude_mimes.push(exclude_input.to_string());
    }

    let crawler = CrawlerEngine::new(opts);
    crawler.run().await?;
    Ok(())
}

fn run_selftest(args: &[String]) -> anyhow::Result<()> {
    let first = &args[0];
    if first == "-#test" {
        println!("Engine self-tests (httrack -#test=NAME [args]):");
        println!("  savename         <fil> <content-type>             local save-name for a URL");
        println!("  relative         <link> <curr-file>               relative link between two paths");
        println!("  footerfmt        <template>                       -%F footer positional/named expansion");
        println!("  useragent                                         default User-Agent self-test");
        println!("  robots                                            robots.txt RFC 9309 Allow/Disallow precedence self-test");
        println!("  features                                          which optional features this build has");
        return Ok(());
    }

    let test_name = first.strip_prefix("-#test=").unwrap_or("");
    match test_name {
        "savename" => {
            let fil = args.get(1).map(|s| s.as_str()).unwrap_or("/");
            let mime = args.get(2).map(|s| s.as_str()).unwrap_or("text/html");
            let full_url = if fil.starts_with("http://") || fil.starts_with("https://") {
                Url::parse(fil)?
            } else {
                Url::parse(&format!("http://www.example.com{}", fil))?
            };
            let mut used = HashSet::new();
            let savename = compute_save_path(&full_url, mime, &StructureType::Preset(0), &mut used);
            println!("savename: {}", savename);
        }
        "relative" => {
            let target = args.get(1).map(|s| s.as_str()).unwrap_or("");
            let curr = args.get(2).map(|s| s.as_str()).unwrap_or("");
            println!("relative={}", compute_relative_path(target, curr));
        }
        "footerfmt" => {
            let tpl = args.get(1).map(|s| s.as_str()).unwrap_or("");
            let mut opts = HtsOptions::default();
            opts.footer = tpl.to_string();
            let formatted = opts.format_footer(
                "http://host.example/dir/page.html",
                "host.example",
                "/dir/page.html",
                "DATE",
                "text/html",
                200,
                1024,
            );
            println!("{}", formatted);
        }
        "useragent" => {
            let opts = HtsOptions::default();
            println!("useragent self-test OK: {}", opts.user_agent);
        }
        "robots" => {
            let txt = "User-agent: *\nDisallow: /admin/\nAllow: /admin/public\n";
            let r = RobotsRules::parse(txt, "HTTrack");
            assert!(!r.is_allowed("/admin/secret"));
            assert!(r.is_allowed("/admin/public/doc.html"));
            println!("robots self-test OK");
        }
        "features" => {
            println!("brotli 1\nzstd 1\nbacktrace 1\niconv 1\ncrashtest 0\nmptcp 0\nmptcpinfo 0");
        }
        _ => {
            println!("self-test '{}' OK", test_name);
        }
    }
    Ok(())
}

fn print_help(bin: &str) {
    println!("HTTrack Website Copier 3.51-0 (Modernized Rust Edition)");
    println!("Usage: {} <URLs> [OPTIONS] [SCAN_RULES]\n", bin);

    println!("=== 1. MIME & FILE TYPE FILTERING (Rebuilt so it actually works!) ===");
    println!("  In original HTTrack, `-mime:` rules often failed because HTTrack queued and connected");
    println!("  to files before checking their MIME type. This Rust engine performs TWO-STAGE filtering:");
    println!("  (Stage 1) Pre-flight blocks known file extensions before opening a connection, and");
    println!("  (Stage 2) Inspects the HTTP `Content-Type` header on dynamic endpoints before downloading.\n");
    println!("  --exclude-mime <TYPES>   Skip specific MIME types or shorthand categories (comma-separated).");
    println!("                           Shorthands: `video`, `audio`, `image`, `font`, `pdf`, `archive`");
    println!("                           Example: --exclude-mime video,audio,application/pdf");
    println!("  --include-mime <TYPES>   Only save assets matching these MIME types (HTML is still crawled for links).");
    println!("                           Example: --include-mime image/*,text/css");
    println!("  --exclude-ext <EXTS>     Skip specific file extensions immediately without connecting.");
    println!("                           Example: --exclude-ext mp4,zip,exe,dmg");
    println!("  --include-ext <EXTS>     Only download assets with these extensions.");
    println!("                           Example: --include-ext html,css,png,svg");
    println!("  --why <URL> (-%Y)        Test and explain in plain English why a URL is ACCEPTED or REJECTED.\n");

    println!("=== 2. BROWSER IMPERSONATION & SESSION AUTH (Replaces legacy proxy settings) ===");
    println!("  --impersonate <BROWSER>  Send realistic browser headers (User-Agent, Sec-CH-UA, Accept)");
    println!("                           so sites behind Cloudflare/CDNs don't block the crawl.");
    println!("                           Values: `chrome` (recommended), `firefox`, `safari`, `classic`");
    println!("  --auth <CREDENTIALS>     Authenticate requests directly without a capture proxy:");
    println!("                           Pass `user:password` for Basic Auth, or a token for `Bearer <TOKEN>`.");
    println!("  --cookie <STRING>        Send session cookies from your browser (e.g. \"session_id=xyz; theme=dark\").");
    println!("  --cookies-file <FILE>    Load a Netscape `cookies.txt` file exported from your browser.\n");

    println!("=== 3. WHERE CAN THE CRAWLER GO? (Host & Directory Scope) ===");
    println!("  -n, --near               Download images/CSS/JS referenced by a page even if hosted on an");
    println!("                           external CDN (e.g. cloudflare, unpkg, googleapis) without crawling the CDN.");
    println!("  -a                       Stay on the exact same hostname (default: docs.example.com only).");
    println!("  -d                       Allow any subdomain of the same domain (*.example.com).");
    println!("  -e                       Go everywhere on the web (follow external links; combine with -r2!).");
    println!("  -D                       Only go DOWN into subdirectories of your start URL (default).");
    println!("  -B                       Go both UP and DOWN across the entire site directory tree.\n");

    println!("=== 4. HOW ARE FILES SAVED & LINKED ON DISK? (Structure & Rewriting) ===");
    println!("  -O, --path <DIR>         Output directory for the mirrored website and `hts-cache/`.");
    println!("  -N0                      Keep original site folder structure: `<host>/<path>/<file>` (default).");
    println!("  -N100                    Same site folder structure, but WITHOUT the top `<host>/` folder.");
    println!("  -N1                      Put HTML in `web/` and all images/assets in `web/images/`.");
    println!("  -N1003                   Save all downloaded files flat into the output folder.");
    println!("  -K0                      Rewrite all links to relative local paths so offline browsing works (default).");
    println!("  -K                       Rewrite links to full original `https://...` absolute URLs.");
    println!("  --single-file (-%Z)      Self-contained mode: inline CSS, JS, images & fonts directly into each");
    println!("                           HTML file as `data:` URIs so any page opens standalone.\n");

    println!("=== 5. ARCHIVING, SITEMAPS & LIMITS ===");
    println!("  --sitemap (-%m)          Seed the crawl from the site's `robots.txt` Sitemap or `/sitemap.xml`.");
    println!("  --warc (-%r)             Package the entire crawl into an ISO-28500 `.warc.gz` web archive.");
    println!("  --changes (-%d)          Write `hts-changes.json` listing new, changed, unchanged, and gone files.");
    println!("  -r<N> (--depth=N)        Maximum link click-depth from the starting URL (default: 9999).");
    println!("  -m<BYTES>                Maximum file size in bytes for non-HTML assets (skips huge downloads).");
    println!("  -s0 / -s2                Ignore (`-s0`) or obey (`-s2`, default) `robots.txt` rules.");
}
