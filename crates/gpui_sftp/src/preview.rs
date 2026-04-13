use std::io;

use gpui::ImageFormat;
use smol::io::{AsyncRead, AsyncReadExt};

use crate::state::EntryKind;

pub const MAX_IMAGE_PREVIEW_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_TEXT_PREVIEW_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewKind {
    Image(ImageFormat),
    Text,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewGate {
    Hidden,
    TooLarge {
        limit_bytes: usize,
    },
    Allowed {
        kind: PreviewKind,
        limit_bytes: usize,
    },
}

pub fn preview_limit_bytes(kind: PreviewKind) -> usize {
    match kind {
        PreviewKind::Image(_) => MAX_IMAGE_PREVIEW_BYTES,
        PreviewKind::Text => MAX_TEXT_PREVIEW_BYTES,
    }
}

pub fn preview_kind_for_name(name: &str) -> Option<PreviewKind> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "dockerfile" | "makefile" | "cmakelists.txt" => return Some(PreviewKind::Text),
        _ => {}
    }

    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    match ext.as_str() {
        // Images
        "png" => Some(PreviewKind::Image(ImageFormat::Png)),
        "jpg" | "jpeg" => Some(PreviewKind::Image(ImageFormat::Jpeg)),
        "webp" => Some(PreviewKind::Image(ImageFormat::Webp)),
        "gif" => Some(PreviewKind::Image(ImageFormat::Gif)),
        "svg" => Some(PreviewKind::Image(ImageFormat::Svg)),
        "bmp" => Some(PreviewKind::Image(ImageFormat::Bmp)),
        "tif" | "tiff" => Some(PreviewKind::Image(ImageFormat::Tiff)),
        "ico" => Some(PreviewKind::Image(ImageFormat::Ico)),

        // Text
        "txt" | "text" | "log" => Some(PreviewKind::Text),
        "md" | "markdown" => Some(PreviewKind::Text),
        "json" | "jsonl" => Some(PreviewKind::Text),
        "toml" | "yaml" | "yml" => Some(PreviewKind::Text),
        "ini" | "conf" | "cfg" | "properties" | "env" => Some(PreviewKind::Text),
        "csv" | "tsv" => Some(PreviewKind::Text),
        "xml" | "html" | "htm" => Some(PreviewKind::Text),
        "css" | "scss" | "less" => Some(PreviewKind::Text),
        "js" | "jsx" | "ts" | "tsx" => Some(PreviewKind::Text),
        "py" | "rb" | "php" => Some(PreviewKind::Text),
        "sh" | "bash" | "zsh" | "fish" | "ps1" => Some(PreviewKind::Text),
        "rs" | "go" | "java" | "kt" | "swift" | "cs" => Some(PreviewKind::Text),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hxx" => Some(PreviewKind::Text),
        "sql" => Some(PreviewKind::Text),
        "gitignore" | "gitattributes" => Some(PreviewKind::Text),

        _ => None,
    }
}

pub fn gate_preview(
    show_preview: bool,
    name: &str,
    kind: EntryKind,
    size: Option<u64>,
) -> PreviewGate {
    if !show_preview {
        return PreviewGate::Hidden;
    }
    if kind != EntryKind::File {
        return PreviewGate::Hidden;
    }
    if is_pdf_name(name) {
        return PreviewGate::Hidden;
    }

    let Some(kind) = preview_kind_for_name(name) else {
        return PreviewGate::Hidden;
    };
    let limit_bytes = preview_limit_bytes(kind);
    if size.is_some_and(|s| s as usize > limit_bytes) {
        return PreviewGate::TooLarge { limit_bytes };
    }

    PreviewGate::Allowed { kind, limit_bytes }
}

fn is_pdf_name(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.trim())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

pub async fn read_bytes_with_limit<R: AsyncRead + Unpin>(
    r: &mut R,
    max_bytes: usize,
) -> io::Result<(Vec<u8>, bool)> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            return Ok((out, false));
        }

        if out.len().saturating_add(n) > max_bytes {
            let remaining = max_bytes.saturating_sub(out.len());
            if remaining > 0 {
                out.extend_from_slice(&buf[..remaining]);
            }
            return Ok((out, true));
        }

        out.extend_from_slice(&buf[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::EntryKind;

    #[test]
    fn kind_detection_is_case_insensitive() {
        assert_eq!(
            preview_kind_for_name("photo.JPG"),
            Some(PreviewKind::Image(ImageFormat::Jpeg))
        );
        assert_eq!(
            preview_kind_for_name("diagram.PnG"),
            Some(PreviewKind::Image(ImageFormat::Png))
        );
    }

    #[test]
    fn kind_detection_treats_md_as_text() {
        assert_eq!(preview_kind_for_name("README.md"), Some(PreviewKind::Text));
        assert_eq!(
            preview_kind_for_name("README.markdown"),
            Some(PreviewKind::Text)
        );
    }

    #[test]
    fn kind_detection_treats_html_as_text() {
        assert_eq!(preview_kind_for_name("index.html"), Some(PreviewKind::Text));
        assert_eq!(preview_kind_for_name("index.htm"), Some(PreviewKind::Text));
    }

    #[test]
    fn kind_detection_rejects_pdf() {
        assert_eq!(preview_kind_for_name("file.pdf"), None);
        assert_eq!(preview_kind_for_name("file.PDF"), None);
    }

    #[test]
    fn preview_limits_are_kind_specific() {
        assert_eq!(
            preview_limit_bytes(PreviewKind::Image(ImageFormat::Png)),
            MAX_IMAGE_PREVIEW_BYTES
        );
        assert_eq!(
            preview_limit_bytes(PreviewKind::Text),
            MAX_TEXT_PREVIEW_BYTES
        );
    }

    #[test]
    fn read_bytes_with_limit_truncates() {
        smol::block_on(async {
            let data = vec![42u8; 128];
            let mut cursor = smol::io::Cursor::new(data);
            let (out, truncated) = read_bytes_with_limit(&mut cursor, 64).await.unwrap();
            assert_eq!(out.len(), 64);
            assert!(truncated);
        });
    }

    #[test]
    fn read_bytes_with_limit_reads_all_when_under_limit() {
        smol::block_on(async {
            let data = vec![7u8; 10];
            let mut cursor = smol::io::Cursor::new(data.clone());
            let (out, truncated) = read_bytes_with_limit(&mut cursor, 64).await.unwrap();
            assert_eq!(out, data);
            assert!(!truncated);
        });
    }

    #[test]
    fn gate_preview_hides_when_disabled() {
        assert_eq!(
            gate_preview(false, "a.png", EntryKind::File, Some(1)),
            PreviewGate::Hidden
        );
    }

    #[test]
    fn gate_preview_hides_non_files() {
        assert_eq!(
            gate_preview(true, "a.png", EntryKind::Dir, Some(1)),
            PreviewGate::Hidden
        );
    }

    #[test]
    fn gate_preview_hides_pdf() {
        assert_eq!(
            gate_preview(true, "a.pdf", EntryKind::File, Some(1)),
            PreviewGate::Hidden
        );
    }

    #[test]
    fn gate_preview_enforces_kind_specific_limits() {
        assert_eq!(
            gate_preview(
                true,
                "a.png",
                EntryKind::File,
                Some((MAX_IMAGE_PREVIEW_BYTES + 1) as u64)
            ),
            PreviewGate::TooLarge {
                limit_bytes: MAX_IMAGE_PREVIEW_BYTES
            }
        );

        assert_eq!(
            gate_preview(
                true,
                "a.md",
                EntryKind::File,
                Some((MAX_TEXT_PREVIEW_BYTES + 1) as u64)
            ),
            PreviewGate::TooLarge {
                limit_bytes: MAX_TEXT_PREVIEW_BYTES
            }
        );
    }
}
