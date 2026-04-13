use super::*;

pub(super) fn parent_dir(path: &str) -> Option<String> {
    let p = Utf8PathBuf::from(path);
    p.parent().map(|p| p.to_string())
}

pub(super) fn join_remote(parent: &str, name: &str) -> String {
    if parent == "/" {
        return format!("/{name}");
    }
    let parent = parent.trim_end_matches('/');
    if parent.is_empty() || parent == "." {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

pub(super) fn is_hidden_name(name: &str) -> bool {
    // Typical Unix convention: dotfiles are hidden.
    // Note: "." and ".." are special, but most SFTP servers won't return them from readdir anyway.
    name.starts_with('.') && name != "." && name != ".."
}

pub(super) fn apply_hidden_filter(rows: Vec<VisibleRow>, show_hidden: bool) -> Vec<VisibleRow> {
    if show_hidden {
        return rows;
    }

    // If a hidden directory is omitted, omit its descendants too (otherwise they'd show up
    // without context in the flattened list).
    let mut out = Vec::with_capacity(rows.len());
    let mut hidden_dir_depth: Option<usize> = None;
    for row in rows {
        if let Some(depth) = hidden_dir_depth {
            if row.depth > depth {
                continue;
            }
            hidden_dir_depth = None;
        }

        if is_hidden_name(&row.name) {
            if row.is_dir {
                hidden_dir_depth = Some(row.depth);
            }
            continue;
        }

        out.push(row);
    }

    out
}
