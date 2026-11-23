use std::fs;
use std::path::Path;

const BACKBLUE_GIF: &[u8] = include_bytes!("../../../assets/backblue.gif");
const FADE_GIF: &[u8] = include_bytes!("../../../assets/fade.gif");
const INDEX_HEADER: &str = include_str!("../../../assets/templates/index-header.html");
const INDEX_BODY: &str = include_str!("../../../assets/templates/index-body.html");
const INDEX_FOOTER: &str = include_str!("../../../assets/templates/index-footer.html");

/// Generate the root `index.html`, `backblue.gif`, and `fade.gif` in `output_dir`
/// using HTTrack 3.51-0's authentic templates extracted during REA.
pub fn write_mirror_index(
    output_dir: &Path,
    seed_entries: &[(String, String)], // (local_rel_path, display_title)
) -> anyhow::Result<Vec<String>> {
    let mut written = Vec::new();

    // Write backblue.gif and fade.gif
    fs::write(output_dir.join("backblue.gif"), BACKBLUE_GIF)?;
    written.push("backblue.gif".to_string());
    fs::write(output_dir.join("fade.gif"), FADE_GIF)?;
    written.push("fade.gif".to_string());

    let comment = "<!-- Mirror and index made by HTTrack Website Copier/3.51-0 [XR&CO] -->";
    let header = INDEX_HEADER.replacen("%s", comment, 1).replace("%%", "%");

    let mut body_rows = String::new();
    for (rel_path, title) in seed_entries {
        let row = INDEX_BODY
            .replacen("%s", rel_path, 1)
            .replacen("%s", title, 1);
        body_rows.push_str(&row);
    }

    let first_link = seed_entries
        .first()
        .map(|(r, _)| r.as_str())
        .unwrap_or("index.html");
    let meta_refresh = format!(
        "<meta HTTP-EQUIV=\"Refresh\" CONTENT=\"0; URL={}\">",
        first_link
    );

    let footer = INDEX_FOOTER
        .replacen("%s", comment, 1)
        .replacen("%s", &meta_refresh, 1)
        .replace("%%", "%");

    let full_html = format!("{}{}{}", header, body_rows, footer);
    fs::write(output_dir.join("index.html"), full_html)?;
    written.push("index.html".to_string());

    Ok(written)
}
