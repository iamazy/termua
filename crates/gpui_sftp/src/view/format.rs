use super::*;

pub(super) fn default_download_dir() -> PathBuf {
    // Prefer ~/Downloads when it exists; fall back to home dir; otherwise /tmp.
    if let Some(home) = home::home_dir() {
        let dl = home.join("Downloads");
        return if dl.exists() { dl } else { home };
    }
    PathBuf::from("/tmp")
}

pub(super) fn entry_from_meta(path: Utf8PathBuf, meta: Metadata) -> Entry {
    let name = path
        .file_name()
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.to_string());
    let kind = match meta.ty {
        FileType::Dir => EntryKind::Dir,
        FileType::File => EntryKind::File,
        FileType::Symlink => EntryKind::Symlink,
        FileType::Other => EntryKind::Other,
    };
    let perms = meta.permissions.map(format_perms);
    Entry::new_with_meta(
        path.to_string(),
        name,
        kind,
        meta.size,
        meta.modified,
        perms,
    )
}

pub(super) fn format_perms(p: FilePermissions) -> String {
    fn tri(r: bool, w: bool, x: bool) -> [char; 3] {
        [
            if r { 'r' } else { '-' },
            if w { 'w' } else { '-' },
            if x { 'x' } else { '-' },
        ]
    }
    let o = tri(p.owner_read, p.owner_write, p.owner_exec);
    let g = tri(p.group_read, p.group_write, p.group_exec);
    let other = tri(p.other_read, p.other_write, p.other_exec);
    let mut out = String::with_capacity(9);
    out.extend(o);
    out.extend(g);
    out.extend(other);
    out
}

pub(super) fn format_size(kind: EntryKind, size: Option<u64>) -> String {
    if kind == EntryKind::Dir {
        return "-".to_string();
    }
    let Some(size) = size else {
        return "-".to_string();
    };
    human_bytes(size)
}

pub(super) fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut f = n as f64;
    let mut unit = 0usize;
    while f >= 1024.0 && unit + 1 < UNITS.len() {
        f /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{:.1} {}", f, UNITS[unit])
    }
}

pub(super) fn format_modified(ts: Option<u64>) -> String {
    let Some(ts) = ts else {
        return "-".to_string();
    };
    let Ok(dt) = OffsetDateTime::from_unix_timestamp(ts as i64) else {
        return ts.to_string();
    };
    // Keep it short and Finder-ish.
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute()
    )
}

pub(super) fn fenced_text_as_markdown(name: &str, text: &str) -> String {
    // Syntax highlighting for large code blocks can be expensive. Keep it for small previews only.
    const MAX_HIGHLIGHT_BYTES: usize = 2 * 1024;

    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    let lang: Option<&'static str> = if text.len() <= MAX_HIGHLIGHT_BYTES {
        match ext.as_str() {
            "rs" => Some("rust"),
            "py" => Some("python"),
            "js" => Some("javascript"),
            "ts" => Some("typescript"),
            "tsx" => Some("tsx"),
            "jsx" => Some("jsx"),
            "json" => Some("json"),
            "yaml" | "yml" => Some("yaml"),
            "toml" => Some("toml"),
            "html" | "htm" => Some("html"),
            "css" => Some("css"),
            "sh" | "bash" | "zsh" => Some("bash"),
            "xml" => Some("xml"),
            _ => None,
        }
    } else {
        None
    };

    // Use a longer fence if the text itself contains triple backticks.
    let fence = if text.contains("```") { "````" } else { "```" };
    match lang {
        Some(lang) => format!("{fence}{lang}\n{text}\n{fence}\n"),
        None => format!("{fence}\n{text}\n{fence}\n"),
    }
}

#[cfg(test)]
mod preview_format_tests {
    use super::*;

    #[test]
    fn fenced_text_omits_language_for_large_previews() {
        let text = "a".repeat(100 * 1024);
        let md = fenced_text_as_markdown("main.rs", &text);
        assert!(md.starts_with("```\n"));
    }

    #[test]
    fn fenced_text_includes_language_for_small_previews() {
        let md = fenced_text_as_markdown("main.rs", "fn main() {}\n");
        assert!(md.starts_with("```rust\n"));
    }
}
