use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use chrono::{DateTime, Utc};
use flate2::write::GzEncoder;
use flate2::Compression;
use sha1::{Digest, Sha1};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use uuid::Uuid;
use crate::cache::CacheRecord;

/// Write an ISO-28500 WARC/1.1 `.warc.gz` archive (matching HTTrack 3.51-0 `--warc` / `-%r`)
pub fn write_warc_archive(
    output_dir: &Path,
    custom_name: Option<&str>,
    records: &[CacheRecord],
    launch_time: DateTime<Utc>,
) -> anyhow::Result<PathBuf> {
    let filename = match custom_name {
        Some(name) => {
            if name.ends_with(".warc.gz") || name.ends_with(".warc") {
                name.to_string()
            } else {
                format!("{}.warc.gz", name)
            }
        }
        None => format!("httrack-{}.warc.gz", launch_time.format("%Y%m%d%H%M%S")),
    };

    let warc_path = output_dir.join(&filename);
    let file = File::create(&warc_path)?;
    let mut gz = GzEncoder::new(file, Compression::default());

    let date_iso = launch_time.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // 1. warcinfo record
    let info_body = format!(
        "software: HTTrack Website Copier/3.51-0 (Rust)\r\nformat: WARC File Format 1.1\r\nconformsTo: http://bibnum.bnf.fr/WARC/WARC_ISO_28500_version1-1_latestdraft.pdf\r\n"
    );
    let info_header = format!(
        "WARC/1.1\r\nWARC-Type: warcinfo\r\nWARC-Date: {}\r\nWARC-Filename: {}\r\nWARC-Record-ID: <urn:uuid:{}>\r\nContent-Type: application/warc-fields\r\nContent-Length: {}\r\n\r\n",
        date_iso,
        filename,
        Uuid::new_v4(),
        info_body.len()
    );
    gz.write_all(info_header.as_bytes())?;
    gz.write_all(info_body.as_bytes())?;
    gz.write_all(b"\r\n\r\n")?;

    // 2. response records for each crawled resource
    for rec in records {
        let mut http_resp = Vec::new();
        let status_header = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
            rec.status_code,
            rec.status_msg,
            rec.mime,
            rec.body_bytes.len()
        );
        http_resp.extend_from_slice(status_header.as_bytes());
        http_resp.extend_from_slice(&rec.body_bytes);

        let mut sha1_hasher = Sha1::new();
        sha1_hasher.update(&rec.body_bytes);
        let payload_digest = format!("sha1:{}", BASE64.encode(sha1_hasher.finalize()));

        let mut block_hasher = Sha1::new();
        block_hasher.update(&http_resp);
        let block_digest = format!("sha1:{}", BASE64.encode(block_hasher.finalize()));

        let warc_header = format!(
            "WARC/1.1\r\nWARC-Type: response\r\nWARC-Target-URI: {}\r\nWARC-Date: {}\r\nWARC-Payload-Digest: {}\r\nWARC-Block-Digest: {}\r\nWARC-Record-ID: <urn:uuid:{}>\r\nContent-Type: application/http; msgtype=response\r\nContent-Length: {}\r\n\r\n",
            rec.url,
            date_iso,
            payload_digest,
            block_digest,
            Uuid::new_v4(),
            http_resp.len()
        );

        gz.write_all(warc_header.as_bytes())?;
        gz.write_all(&http_resp)?;
        gz.write_all(b"\r\n\r\n")?;
    }

    gz.finish()?;
    Ok(warc_path)
}
