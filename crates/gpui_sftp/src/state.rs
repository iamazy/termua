use std::collections::{HashMap, HashSet};

/// Minimal, UI-agnostic representation of a remote filesystem entry.
///
/// We keep this free of any SFTP/GPUI types so it is easy to unit test.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// Full, canonical remote path (used as a stable id).
    pub path: String,
    /// Basename for display/sorting within a directory.
    pub name: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    /// Unix timestamp (seconds since epoch).
    pub modified: Option<u64>,
    /// Unix permission bits formatted as `rwxr-xr-x` (no leading file type).
    pub perms: Option<String>,
}

impl Entry {
    pub fn new(path: impl Into<String>, name: impl Into<String>, kind: EntryKind) -> Self {
        Self::new_with_meta(path, name, kind, None, None, None)
    }

    pub fn new_with_meta(
        path: impl Into<String>,
        name: impl Into<String>,
        kind: EntryKind,
        size: Option<u64>,
        modified: Option<u64>,
        perms: Option<String>,
    ) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            kind,
            size,
            modified,
            perms,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SortColumn {
    Name,
    Type,
    Size,
    Modified,
    Perms,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SortSpec {
    pub column: SortColumn,
    pub direction: SortDirection,
}

impl Default for SortSpec {
    fn default() -> Self {
        Self {
            column: SortColumn::Name,
            direction: SortDirection::Asc,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TreeState {
    /// The root node id (remote canonical path).
    pub root: String,
    nodes: HashMap<String, Node>,
    expanded: HashSet<String>,
}

#[derive(Clone, Debug)]
struct Node {
    entry: Entry,
    children: Vec<String>,
    loaded_children: bool,
}

impl TreeState {
    pub fn new(root: Entry) -> Self {
        let root_id = root.path.clone();
        let mut nodes = HashMap::new();
        nodes.insert(
            root_id.clone(),
            Node {
                entry: root,
                children: Vec::new(),
                loaded_children: false,
            },
        );

        // Default: show root expanded (so users see something immediately).
        let mut expanded = HashSet::new();
        expanded.insert(root_id.clone());

        Self {
            root: root_id,
            nodes,
            expanded,
        }
    }

    pub fn is_expanded(&self, id: &str) -> bool {
        self.expanded.contains(id)
    }

    pub fn set_expanded(&mut self, id: &str, expanded: bool) {
        if expanded {
            self.expanded.insert(id.to_string());
        } else {
            self.expanded.remove(id);
        }
    }

    pub fn upsert_children(&mut self, parent_id: &str, children: Vec<Entry>) {
        let mut child_ids = Vec::with_capacity(children.len());
        for child in children {
            let id = child.path.clone();
            child_ids.push(id.clone());
            self.nodes
                .entry(id.clone())
                .and_modify(|n| n.entry = child.clone())
                .or_insert_with(|| Node {
                    entry: child,
                    children: Vec::new(),
                    loaded_children: false,
                });
        }

        if let Some(parent) = self.nodes.get_mut(parent_id) {
            parent.children = child_ids;
            parent.loaded_children = true;
        }
    }

    pub fn visible_rows_sorted(&self, sort: SortSpec) -> Vec<VisibleRow> {
        let mut out = Vec::new();
        self.flatten_into_sorted(&self.root, 0, sort, &mut out);
        out
    }

    fn flatten_into_sorted(
        &self,
        id: &str,
        depth: usize,
        sort: SortSpec,
        out: &mut Vec<VisibleRow>,
    ) {
        let Some(node) = self.nodes.get(id) else {
            return;
        };

        out.push(VisibleRow {
            id: node.entry.path.clone(),
            name: node.entry.name.clone(),
            kind: node.entry.kind,
            size: node.entry.size,
            modified: node.entry.modified,
            perms: node.entry.perms.clone(),
            depth,
            is_dir: node.entry.kind == EntryKind::Dir,
            is_expanded: self.is_expanded(id),
            loaded_children: node.loaded_children,
        });

        if node.entry.kind != EntryKind::Dir || !self.is_expanded(id) {
            return;
        }

        let mut children: Vec<&str> = node.children.iter().map(|s| s.as_str()).collect();
        children.sort_by(|a, b| self.compare_ids(a, b, sort));
        for child in children {
            self.flatten_into_sorted(child, depth + 1, sort, out);
        }
    }

    fn compare_ids(&self, a: &str, b: &str, sort: SortSpec) -> std::cmp::Ordering {
        let a = self.nodes.get(a).map(|n| &n.entry);
        let b = self.nodes.get(b).map(|n| &n.entry);
        let (Some(a), Some(b)) = (a, b) else {
            return std::cmp::Ordering::Equal;
        };
        compare_entries(a, b, sort)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VisibleRow {
    pub id: String,
    pub name: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub modified: Option<u64>,
    pub perms: Option<String>,
    pub depth: usize,
    pub is_dir: bool,
    pub is_expanded: bool,
    pub loaded_children: bool,
}

/// Convert a remote path into breadcrumb segments.
///
/// Each entry is `(label, full_path)`, where `full_path` can be used for navigation.
pub(crate) fn breadcrumbs_for_path(path: &str) -> Vec<(String, String)> {
    let path = path.trim();
    if path.is_empty() || path == "." {
        return vec![(".".to_string(), ".".to_string())];
    }
    if path == "/" {
        return vec![("/".to_string(), "/".to_string())];
    }

    let is_abs = path.starts_with('/');
    let normalized = path.trim_end_matches('/');
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();

    if parts.is_empty() {
        return if is_abs {
            vec![("/".to_string(), "/".to_string())]
        } else {
            vec![(".".to_string(), ".".to_string())]
        };
    }

    let mut out = Vec::new();
    if is_abs {
        out.push(("/".to_string(), "/".to_string()));
        let mut acc = String::new();
        for part in parts {
            if acc.is_empty() {
                acc = format!("/{part}");
            } else {
                acc = format!("{acc}/{part}");
            }
            out.push((part.to_string(), acc.clone()));
        }
    } else {
        let mut acc = String::new();
        for part in parts {
            if acc.is_empty() {
                acc = part.to_string();
            } else {
                acc = format!("{acc}/{part}");
            }
            out.push((part.to_string(), acc.clone()));
        }
    }

    out
}

/// Best-effort display label for a directory path.
pub(crate) fn display_name_for_dir(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "." {
        return ".".to_string();
    }
    if path == "/" {
        return "/".to_string();
    }

    let normalized = path.trim_end_matches('/');
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    parts.last().map(|s| s.to_string()).unwrap_or_else(|| {
        if path.starts_with('/') {
            "/".to_string()
        } else {
            ".".to_string()
        }
    })
}

fn compare_entries(a: &Entry, b: &Entry, sort: SortSpec) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    // Strong rule from user: directories always first.
    let kind_rank = |k: EntryKind| match k {
        EntryKind::Dir => 0u8,
        _ => 1u8,
    };
    let dir_rank = kind_rank(a.kind).cmp(&kind_rank(b.kind));
    if dir_rank != Ordering::Equal {
        return dir_rank;
    }

    let ord = match sort.column {
        SortColumn::Name => a.name.cmp(&b.name),
        SortColumn::Type => a
            .kind_rank()
            .cmp(&b.kind_rank())
            .then_with(|| a.name.cmp(&b.name)),
        SortColumn::Size => opt_u64_cmp(a.size, b.size).then_with(|| a.name.cmp(&b.name)),
        SortColumn::Modified => {
            opt_u64_cmp(a.modified, b.modified).then_with(|| a.name.cmp(&b.name))
        }
        SortColumn::Perms => a
            .perms
            .as_deref()
            .unwrap_or("")
            .cmp(b.perms.as_deref().unwrap_or(""))
            .then_with(|| a.name.cmp(&b.name)),
    };

    match sort.direction {
        SortDirection::Asc => ord,
        SortDirection::Desc => ord.reverse(),
    }
}

impl Entry {
    fn kind_rank(&self) -> u8 {
        match self.kind {
            EntryKind::Dir => 0,
            EntryKind::File => 1,
            EntryKind::Symlink => 2,
            EntryKind::Other => 3,
        }
    }
}

fn opt_u64_cmp(a: Option<u64>, b: Option<u64>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_dirs_before_files_and_by_name() {
        let root = Entry::new("/home/u", "~", EntryKind::Dir);
        let mut tree = TreeState::new(root);

        tree.upsert_children(
            "/home/u",
            vec![
                Entry::new("/home/u/b", "b", EntryKind::Dir),
                Entry::new("/home/u/a.txt", "a.txt", EntryKind::File),
                Entry::new("/home/u/a", "a", EntryKind::Dir),
            ],
        );

        let rows = tree.visible_rows_sorted(SortSpec::default());
        // root row + 3 children (root is expanded by default)
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].name, "a");
        assert_eq!(rows[1].kind, EntryKind::Dir);
        assert_eq!(rows[2].name, "b");
        assert_eq!(rows[2].kind, EntryKind::Dir);
        assert_eq!(rows[3].name, "a.txt");
        assert_eq!(rows[3].kind, EntryKind::File);
    }

    #[test]
    fn flatten_respects_expansion_and_depth() {
        let root = Entry::new("/home/u", "~", EntryKind::Dir);
        let mut tree = TreeState::new(root);

        tree.upsert_children(
            "/home/u",
            vec![
                Entry::new("/home/u/dir", "dir", EntryKind::Dir),
                Entry::new("/home/u/file", "file", EntryKind::File),
            ],
        );
        tree.upsert_children(
            "/home/u/dir",
            vec![
                Entry::new("/home/u/dir/n1", "n1", EntryKind::File),
                Entry::new("/home/u/dir/n2", "n2", EntryKind::File),
            ],
        );

        // Collapsed dir: only root + children.
        tree.set_expanded("/home/u/dir", false);
        let rows = tree.visible_rows_sorted(SortSpec::default());
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["~", "dir", "file"]
        );
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].depth, 1);

        // Expanded dir: nested children appear with deeper indent.
        tree.set_expanded("/home/u/dir", true);
        let rows = tree.visible_rows_sorted(SortSpec::default());
        assert_eq!(
            rows.iter()
                .map(|r| (r.name.as_str(), r.depth))
                .collect::<Vec<_>>(),
            vec![("~", 0), ("dir", 1), ("n1", 2), ("n2", 2), ("file", 1)]
        );
    }

    #[test]
    fn sorting_can_be_changed_but_dirs_stay_first() {
        let root = Entry::new("/home/u", "~", EntryKind::Dir);
        let mut tree = TreeState::new(root);

        tree.upsert_children(
            "/home/u",
            vec![
                Entry::new_with_meta("/home/u/dir", "dir", EntryKind::Dir, None, None, None),
                Entry::new_with_meta(
                    "/home/u/small",
                    "small",
                    EntryKind::File,
                    Some(1),
                    None,
                    None,
                ),
                Entry::new_with_meta("/home/u/big", "big", EntryKind::File, Some(10), None, None),
            ],
        );

        let rows = tree.visible_rows_sorted(SortSpec {
            column: SortColumn::Size,
            direction: SortDirection::Desc,
        });

        // root + dir + files. Even when sorting by size desc, dir remains first.
        assert_eq!(rows[1].name, "dir");
        assert_eq!(rows[2].name, "big");
        assert_eq!(rows[3].name, "small");
    }

    #[test]
    fn breadcrumbs_for_absolute_paths() {
        assert_eq!(
            breadcrumbs_for_path("/home/u"),
            vec![
                ("/".to_string(), "/".to_string()),
                ("home".to_string(), "/home".to_string()),
                ("u".to_string(), "/home/u".to_string())
            ]
        );
        assert_eq!(
            breadcrumbs_for_path("/"),
            vec![("/".to_string(), "/".to_string())]
        );
    }

    #[test]
    fn breadcrumbs_for_relative_paths() {
        assert_eq!(
            breadcrumbs_for_path("a/b"),
            vec![
                ("a".to_string(), "a".to_string()),
                ("b".to_string(), "a/b".to_string())
            ]
        );
        assert_eq!(
            breadcrumbs_for_path("."),
            vec![(".".to_string(), ".".to_string())]
        );
    }

    #[test]
    fn display_name_for_dir_uses_basename() {
        assert_eq!(display_name_for_dir("/home/u"), "u");
        assert_eq!(display_name_for_dir("/"), "/");
        assert_eq!(display_name_for_dir("a/b"), "b");
        assert_eq!(display_name_for_dir("."), ".");
    }
}
