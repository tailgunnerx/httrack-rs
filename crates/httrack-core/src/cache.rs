use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;
use crate::options::HtsOptions;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheRecord {
    pub time_str: String,
    pub local_size: usize,
    pub remote_size: usize,
    pub status_code: u16,
    pub status_msg: String,
    pub mime: String,
    pub etag_or_date: String,
    pub url: String,
    pub local_rel_path: Option<String>,
    pub from_url: String,
    pub body_bytes: Vec<u8>,
    pub is_html_or_css: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeFileEntry {
    pub url: String,
    pub file: String,
    pub size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeCounts {
    pub new: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub gone: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HtsChangesReport {
    pub schema: u32,
    pub generator: String,
    pub date: String,
    pub first_crawl: bool,
    pub partial: bool,
    pub purged: bool,
    pub counts: ChangeCounts,
    pub new: Vec<ChangeFileEntry>,
    pub changed: Vec<ChangeFileEntry>,
    pub unchanged: Vec<ChangeFileEntry>,
    pub gone: Vec<ChangeFileEntry>,
}

/// Write `hts-cache/doit.log`, `hts-cache/new.lst`, `hts-cache/new.txt`, and `hts-cache/new.zip`
/// matching HTTrack 3.51-0's exact cache structure verified in our REA oracle run.
pub fn write_hts_cache(
    output_dir: &Path,
    opts: &HtsOptions,
    records: &[CacheRecord],
    static_files_written: &[String],
    launch_time: DateTime<Utc>,
) -> anyhow::Result<()> {
    let cache_dir = opts
        .cache_dir
        .clone()
        .unwrap_or_else(|| output_dir.join("hts-cache"));
    fs::create_dir_all(&cache_dir)?;

    // Rotate new.zip / new.txt / new.lst to old.* if they exist
    for ext in &["zip", "txt", "lst"] {
        let new_p = cache_dir.join(format!("new.{}", ext));
        let old_p = cache_dir.join(format!("old.{}", ext));
        if new_p.exists() {
            let _ = fs::rename(&new_p, &old_p);
        }
    }

    // 1. Write hts-cache/doit.log
    let doit_path = cache_dir.join("doit.log");
    let args_line = if opts.raw_args.is_empty() {
        format!("{} -O \"{}\"", opts.urls.join(" "), output_dir.display())
    } else {
        opts.raw_args.join(" ")
    };
    let date_human = launch_time.format("%a, %d %b %Y %H:%M:%S").to_string();
    let doit_content = format!(
        "{}\nFile generated automatically on {}, do NOT edit\n\nTo update a mirror, just launch httrack without any parameters\nThe existing cache will be used (and modified)\nTo have other options, retype all parameters and launch HTTrack\nTo continue an interrupted mirror, just launch httrack without any parameters\n",
        args_line, date_human
    );
    fs::write(doit_path, doit_content)?;

    // 2. Write hts-cache/new.lst
    let mut lst_lines = String::new();
    for f in static_files_written {
        lst_lines.push_str(&format!("[{}]\n", f));
    }
    for rec in records {
        if let Some(rel) = &rec.local_rel_path {
            lst_lines.push_str(&format!("[{}]\n", rel));
        }
    }
    fs::write(cache_dir.join("new.lst"), lst_lines)?;

    // 3. Write hts-cache/new.txt
    let mut txt = String::from(
        "date\tsize'/'remotesize\tflags(request:Update,Range state:File response:Modified,Chunked,gZipped)\tstatuscode\tstatus ('servermsg')\tMIME\tEtag|Date\tURL\tlocalfile\t(from URL)\n",
    );
    for rec in records {
        let status_label = if rec.status_code >= 200 && rec.status_code < 300 {
            format!("added ('{}')", rec.status_msg.replace(' ', "%20"))
        } else {
            format!("error ('{}')", rec.status_msg.replace(' ', "%20"))
        };
        let abs_local = rec
            .local_rel_path
            .as_ref()
            .map(|r| {
                output_dir
                    .join(r)
                    .to_string_lossy()
                    .replace('\\', "/")
                    .replace(' ', "%20")
            })
            .unwrap_or_default();
        txt.push_str(&format!(
            "{}\t{}/{}\t---M--\t{}\t{}\t{}\t{}\t{}\t{}\t(from {})\n",
            rec.time_str,
            rec.local_size,
            rec.remote_size,
            rec.status_code,
            status_label,
            rec.mime,
            rec.etag_or_date.replace(' ', "%20"),
            rec.url,
            abs_local,
            rec.from_url
        ));
    }
    fs::write(cache_dir.join("new.txt"), txt)?;

    // 4. Write hts-cache/new.zip
    let zip_file = File::create(cache_dir.join("new.zip"))?;
    let mut zip = ZipWriter::new(zip_file);
    let zip_opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for rec in records {
        zip.start_file(&rec.url, zip_opts)?;
        if rec.is_html_or_css || rec.local_rel_path.is_none() {
            zip.write_all(&rec.body_bytes)?;
        }
    }
    zip.finish()?;

    Ok(())
}

/// Write `hts-changes.json` (HTTrack 3.51-0 `--changes` / `-%d` format)
pub fn write_changes_json(
    output_dir: &Path,
    records: &[CacheRecord],
    finish_time: DateTime<Utc>,
) -> anyhow::Result<()> {
    let mut new_entries = Vec::new();
    for rec in records {
        if let Some(rel) = &rec.local_rel_path {
            let disk_size = output_dir
                .join(rel)
                .metadata()
                .map(|m| m.len() as usize)
                .unwrap_or(rec.local_size);
            new_entries.push(ChangeFileEntry {
                url: rec.url.clone(),
                file: rel.clone(),
                size: disk_size,
            });
        }
    }

    let report = HtsChangesReport {
        schema: 1,
        generator: "HTTrack Website Copier/3.51-0 (Rust)".to_string(),
        date: finish_time.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        first_crawl: true,
        partial: false,
        purged: true,
        counts: ChangeCounts {
            new: new_entries.len(),
            changed: 0,
            unchanged: 0,
            gone: 0,
        },
        new: new_entries,
        changed: Vec::new(),
        unchanged: Vec::new(),
        gone: Vec::new(),
    };

    let json_str = serde_json::to_string_pretty(&report)?;
    fs::write(output_dir.join("hts-changes.json"), json_str)?;
    Ok(())
}

/// Write `hts-log.txt` matching HTTrack 3.51-0 format
pub fn write_hts_log(
    output_dir: &Path,
    opts: &HtsOptions,
    launch_time: DateTime<Utc>,
    elapsed_secs: u64,
    links_scanned: usize,
    files_written: usize,
    bytes_written: usize,
    bytes_received: usize,
    errors: &[String],
    warnings: &[String],
) -> anyhow::Result<()> {
    let date_str = launch_time.format("%a, %d %b %Y %H:%M:%S").to_string();
    let cmd_str = if opts.raw_args.is_empty() {
        format!("httrack {}", opts.urls.join(" "))
    } else {
        opts.raw_args.join(" ")
    };

    let mut log = format!(
        "HTTrack3.51-0 (Rust) launched on {} at {}\n({})\n\nInformation, Warnings and Errors reported for this mirror:\nnote:\tthe hts-log.txt file, and hts-cache folder, may contain sensitive information,\n\tsuch as username/password authentication for websites mirrored in this project\n\tdo not share these files/folders if you want these information to remain private\n\n",
        date_str,
        opts.urls.join(" "),
        cmd_str
    );

    for w in warnings {
        log.push_str(&format!("Warning: \t{}\n", w));
    }
    for e in errors {
        log.push_str(&format!("Error: \t{}\n", e));
    }

    let rate = if elapsed_secs > 0 {
        bytes_received / (elapsed_secs as usize)
    } else {
        bytes_received
    };

    log.push_str(&format!(
        "\nHTTrack Website Copier/3.51-0 mirror complete in {} seconds : {} links scanned, {} files written ({} bytes overall) [{} bytes received at {} bytes/sec]\n({} errors, {} warnings, 0 messages)\n",
        elapsed_secs.max(1),
        links_scanned,
        files_written,
        bytes_written,
        bytes_received,
        rate,
        errors.len(),
        warnings.len()
    ));

    if opts.generate_changes {
        let now_t = Utc::now().format("%H:%M:%S").to_string();
        log.push_str(&format!(
            "{}\tInfo: \tChange report: first crawl, {} files mirrored, nothing to compare against (hts-changes.json)\n",
            now_t, files_written
        ));
    }

    fs::write(output_dir.join("hts-log.txt"), log)?;
    Ok(())
}
