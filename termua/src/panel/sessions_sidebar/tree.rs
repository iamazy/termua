use std::collections::BTreeMap;

use gpui::SharedString;
use gpui_common::TermuaIcon;
use gpui_component::tree::TreeItem;

use crate::store::{Session, SessionType};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionTreeSummary {
    pub id: i64,
    pub protocol: SessionType,
    pub group_path: String,
    pub label: String,
    pub ssh_host: Option<String>,
    pub search_text: String,
}

impl SessionTreeSummary {
    pub(super) fn from_session(session: &Session) -> Self {
        let mut search_text = String::new();
        search_text.push_str(&session.label.to_lowercase());
        search_text.push(' ');
        search_text.push_str(&session.group_path.to_lowercase());
        if let Some(host) = session.ssh_host.as_deref() {
            search_text.push(' ');
            search_text.push_str(&host.to_lowercase());
        }

        Self {
            id: session.id,
            protocol: session.protocol.clone(),
            group_path: session.group_path.clone(),
            label: session.label.clone(),
            ssh_host: session.ssh_host.clone(),
            search_text,
        }
    }
}

pub(super) fn parse_session_id(item_id: &str) -> Option<i64> {
    let id = item_id.strip_prefix("session:")?;
    let (_, id) = id.split_once(':')?;
    id.parse::<i64>().ok()
}

pub(super) fn folder_debug_name(folder_id: &str) -> String {
    folder_id
        .strip_prefix("folder:")
        .unwrap_or(folder_id)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn folder_icon_asset_path(expanded: bool) -> TermuaIcon {
    if expanded {
        TermuaIcon::FolderOpenBlue
    } else {
        TermuaIcon::FolderClosedBlue
    }
}

pub(super) fn find_tree_item_by_id<'a>(items: &'a [TreeItem], id: &str) -> Option<&'a TreeItem> {
    for item in items {
        if item.id.as_ref() == id {
            return Some(item);
        }
        if let Some(found) = find_tree_item_by_id(&item.children, id) {
            return Some(found);
        }
    }
    None
}

#[derive(Default)]
struct FolderNode {
    children: BTreeMap<String, FolderNode>,
    sessions: Vec<SessionTreeSummary>,
}

fn split_group_path(group_path: &str) -> Vec<String> {
    group_path
        .split('>')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn matches_query(session: &SessionTreeSummary, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    session.search_text.contains(&query.to_lowercase())
}

#[cfg(test)]
pub(super) fn build_tree_items(all_sessions: &[Session], query: &str) -> Vec<TreeItem> {
    let sessions = all_sessions
        .iter()
        .map(SessionTreeSummary::from_session)
        .collect::<Vec<_>>();
    build_tree_items_from_summaries(&sessions, query)
}

pub(super) fn build_tree_items_from_summaries(
    all_sessions: &[SessionTreeSummary],
    query: &str,
) -> Vec<TreeItem> {
    let sessions: Vec<SessionTreeSummary> = all_sessions
        .iter()
        .filter(|s| matches_query(s, query))
        .cloned()
        .collect();

    let mut root = FolderNode::default();
    for session in sessions {
        let parts = split_group_path(&session.group_path);
        let mut node = &mut root;
        for part in parts {
            node = node.children.entry(part).or_default();
        }
        node.sessions.push(session);
    }

    let mut out = Vec::new();
    for (name, child) in root.children {
        out.push(folder_node_to_tree_item(name.clone(), child, name));
    }
    // Sessions with empty group go at root.
    for session in root.sessions {
        out.push(session_summary_to_tree_item(&session));
    }
    out
}

fn folder_node_to_tree_item(name: String, node: FolderNode, full_path: String) -> TreeItem {
    let id = format!("folder:{full_path}");
    let item = TreeItem::new(id, name).expanded(true);

    let mut children = Vec::new();
    for (child_name, child_node) in node.children {
        let child_path = format!("{full_path}>{child_name}");
        children.push(folder_node_to_tree_item(child_name, child_node, child_path));
    }
    for session in node.sessions {
        children.push(session_summary_to_tree_item(&session));
    }

    item.children(children)
}

fn session_summary_to_tree_item(session: &SessionTreeSummary) -> TreeItem {
    let proto = match session.protocol {
        SessionType::Local => "local",
        SessionType::Ssh => "ssh",
        SessionType::Serial => "serial",
    };
    TreeItem::new(
        format!("session:{proto}:{}", session.id),
        SharedString::from(session.label.clone()),
    )
}
