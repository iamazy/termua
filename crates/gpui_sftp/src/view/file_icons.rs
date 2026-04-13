use gpui_common::TermuaIcon;

pub(super) fn icon_path_for_file_name(name: &str) -> Option<TermuaIcon> {
    if is_image_file_name(name) {
        return Some(TermuaIcon::Image);
    }
    if is_database_file_name(name) {
        return Some(TermuaIcon::Database);
    }
    if is_json_file_name(name) {
        return Some(TermuaIcon::Braces);
    }
    if is_pdf_file_name(name) {
        return Some(TermuaIcon::Pdf);
    }
    if is_video_file_name(name) {
        return Some(TermuaIcon::Play);
    }
    if is_archive_file_name(name) {
        return Some(TermuaIcon::FileArchive);
    }
    if is_text_file_name(name) {
        return Some(TermuaIcon::FileText);
    }
    None
}

fn is_image_file_name(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if ext.is_empty() {
        return false;
    }
    match ext.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff" | "svg"
        | "avif" | "heic" | "heif" => true,
        _ => false,
    }
}

fn is_text_file_name(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if ext.is_empty() {
        return false;
    }
    match ext.to_ascii_lowercase().as_str() {
        // Plain text / markup / config
        "txt" | "md" | "markdown" | "rst" | "log" | "csv" | "tsv" | "yaml" | "yml" | "toml"
        | "ini" | "cfg" | "conf" | "properties" | "env" | "xml" | "html" | "htm" | "css"
        | "scss" | "sass" | "less" | "graphql" | "gql" => true,

        // Source code / scripts (still "text" for our purposes)
        "rs" | "go" | "py" | "rb" | "php" | "java" | "kt" | "kts" | "swift" | "c" | "h" | "cc"
        | "hh" | "cpp" | "hpp" | "cs" | "fs" | "dart" | "lua" | "sh" | "bash" | "zsh" | "fish"
        | "ps1" | "psm1" | "bat" | "cmd" | "sql" | "proto" => true,

        // Common "dotfile" extensions
        "gitignore" | "gitattributes" | "editorconfig" | "dockerignore" => true,

        _ => false,
    }
}

fn is_database_file_name(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if ext.is_empty() {
        return false;
    }
    match ext.to_ascii_lowercase().as_str() {
        "db" | "sqlite" | "sqlite3" | "db3" | "s3db" | "sl3" | "sqlitedb" => true,
        _ => false,
    }
}

fn is_json_file_name(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if ext.is_empty() {
        return false;
    }
    matches!(ext.to_ascii_lowercase().as_str(), "json" | "jsonc")
}

fn is_pdf_file_name(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if ext.is_empty() {
        return false;
    }
    ext.eq_ignore_ascii_case("pdf")
}

fn is_video_file_name(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if ext.is_empty() {
        return false;
    }
    match ext.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "wmv" | "flv" | "mpeg" | "mpg" | "mpe"
        | "3gp" | "3g2" | "ts" | "mts" | "m2ts" => true,
        _ => false,
    }
}

fn is_archive_file_name(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if ext.is_empty() {
        return false;
    }
    match ext.to_ascii_lowercase().as_str() {
        "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "tbz" | "tbz2" | "xz" | "txz"
        | "zst" | "lz4" => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_file_name_detection_uses_extension() {
        assert_eq!(icon_path_for_file_name("a.png"), Some(TermuaIcon::Image));
        assert_eq!(icon_path_for_file_name("a.JPG"), Some(TermuaIcon::Image));
        assert_eq!(
            icon_path_for_file_name("a.photo.jpeg"),
            Some(TermuaIcon::Image)
        );
        assert_eq!(icon_path_for_file_name("a.webp"), Some(TermuaIcon::Image));
        assert_eq!(icon_path_for_file_name("a.svg"), Some(TermuaIcon::Image));
        assert_ne!(icon_path_for_file_name("a.txt"), Some(TermuaIcon::Image));
        assert_eq!(icon_path_for_file_name("a"), None);
        assert_eq!(icon_path_for_file_name("a."), None);
        assert_eq!(icon_path_for_file_name(".bashrc"), None);
    }

    #[test]
    fn text_file_name_detection_uses_extension() {
        assert_eq!(icon_path_for_file_name("a.txt"), Some(TermuaIcon::FileText));
        assert_eq!(icon_path_for_file_name("a.md"), Some(TermuaIcon::FileText));
        assert_eq!(
            icon_path_for_file_name("a.yaml"),
            Some(TermuaIcon::FileText)
        );
        assert_eq!(icon_path_for_file_name("a.rs"), Some(TermuaIcon::FileText));
        assert_eq!(icon_path_for_file_name("a.py"), Some(TermuaIcon::FileText));
        assert_eq!(
            icon_path_for_file_name(".gitignore"),
            Some(TermuaIcon::FileText)
        );
        assert_ne!(icon_path_for_file_name("a.png"), Some(TermuaIcon::FileText));
        assert_eq!(icon_path_for_file_name("a"), None);
        assert_eq!(icon_path_for_file_name("a."), None);
    }

    #[test]
    fn database_file_name_detection_uses_extension() {
        assert_eq!(icon_path_for_file_name("a.db"), Some(TermuaIcon::Database));
        assert_eq!(
            icon_path_for_file_name("a.sqlite"),
            Some(TermuaIcon::Database)
        );
        assert_eq!(
            icon_path_for_file_name("a.sqlite3"),
            Some(TermuaIcon::Database)
        );
        assert_eq!(icon_path_for_file_name("a.DB3"), Some(TermuaIcon::Database));
        assert_eq!(
            icon_path_for_file_name("a.s3db"),
            Some(TermuaIcon::Database)
        );
        assert_eq!(icon_path_for_file_name("a.sql"), Some(TermuaIcon::FileText));
        assert_eq!(icon_path_for_file_name("a.bin"), None);
    }

    #[test]
    fn json_file_name_detection_uses_extension() {
        assert_eq!(icon_path_for_file_name("a.json"), Some(TermuaIcon::Braces));
        assert_eq!(icon_path_for_file_name("a.JSON"), Some(TermuaIcon::Braces));
        assert_eq!(icon_path_for_file_name("a.jsonc"), Some(TermuaIcon::Braces));
        // JSON should prefer braces over file-text.
        assert_ne!(
            icon_path_for_file_name("a.json"),
            Some(TermuaIcon::FileText)
        );
    }

    #[test]
    fn pdf_file_name_detection_uses_extension() {
        assert_eq!(icon_path_for_file_name("a.pdf"), Some(TermuaIcon::Pdf));
        assert_eq!(icon_path_for_file_name("a.PDF"), Some(TermuaIcon::Pdf));
        assert_eq!(
            icon_path_for_file_name("a.pdf.txt"),
            Some(TermuaIcon::FileText)
        );
        assert_eq!(icon_path_for_file_name("a"), None);
    }

    #[test]
    fn video_file_name_detection_uses_extension() {
        assert_eq!(icon_path_for_file_name("a.mp4"), Some(TermuaIcon::Play));
        assert_eq!(icon_path_for_file_name("a.MKV"), Some(TermuaIcon::Play));
        assert_eq!(icon_path_for_file_name("a.webm"), Some(TermuaIcon::Play));
        assert_ne!(icon_path_for_file_name("a.mp4.txt"), Some(TermuaIcon::Play));
        assert_eq!(icon_path_for_file_name("a"), None);
    }

    #[test]
    fn archive_file_name_detection_uses_extension() {
        assert_eq!(
            icon_path_for_file_name("a.zip"),
            Some(TermuaIcon::FileArchive)
        );
        assert_eq!(
            icon_path_for_file_name("a.7z"),
            Some(TermuaIcon::FileArchive)
        );
        assert_eq!(
            icon_path_for_file_name("a.tar"),
            Some(TermuaIcon::FileArchive)
        );
        assert_eq!(
            icon_path_for_file_name("a.tar.gz"),
            Some(TermuaIcon::FileArchive)
        );
        assert_eq!(
            icon_path_for_file_name("a.TGZ"),
            Some(TermuaIcon::FileArchive)
        );
        assert_ne!(
            icon_path_for_file_name("a.zip.txt"),
            Some(TermuaIcon::FileArchive)
        );
        assert_eq!(icon_path_for_file_name("a"), None);
    }
}
