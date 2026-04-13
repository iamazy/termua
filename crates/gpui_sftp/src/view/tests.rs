use std::path::PathBuf;

use gpui_common::TermuaIcon;

use super::*;

fn delegate_with_tree(tree: TreeState) -> SftpTable {
    let sort = SortSpec::default();
    SftpTable {
        sftp: None,
        tree: Some(tree),
        loading: std::collections::HashSet::new(),
        show_hidden: false,
        selected_ids: std::collections::HashSet::new(),
        selection_anchor_id: None,
        columns: sftp_table_columns(),
        sort,
        visible: Vec::new(),
        context_row: None,
        pending_toast: None,
        pending_toast_epoch: 0,
        transfers: std::collections::HashMap::new(),
        op: None,
    }
}

fn unique_tmp_path(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "termua-gpui-sftp-{label}-{}-{n}",
        std::process::id()
    ))
}

#[gpui::test]
fn sftp_table_rows_count_matches_visible_len(cx: &mut gpui::TestAppContext) {
    let app = cx.app.borrow_mut();

    let mut tree = TreeState::new(Entry::new("/", "/", EntryKind::Dir));
    tree.upsert_children("/", vec![Entry::new("/a", "a", EntryKind::File)]);

    let mut d = delegate_with_tree(tree);
    d.rebuild_visible();

    assert_eq!(d.visible.len(), 1);
    assert_eq!(d.rows_count(&app), 1);
}

#[test]
fn sftp_table_does_not_include_root_row() {
    let mut tree = TreeState::new(Entry::new("/", "/", EntryKind::Dir));
    tree.upsert_children(
        "/",
        vec![
            Entry::new("/a", "a", EntryKind::File),
            Entry::new("/b", "b", EntryKind::File),
        ],
    );

    let mut d = delegate_with_tree(tree);
    d.rebuild_visible();

    assert_eq!(
        d.visible.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["/a", "/b"]
    );
}

#[test]
fn sftp_table_has_expected_columns() {
    let cols = sftp_table_columns();
    let keys = cols.iter().map(|c| c.key.as_ref()).collect::<Vec<_>>();
    assert_eq!(keys, ["name", "size", "modified", "perms"]);
}

#[test]
fn sftp_dir_icon_path_is_folder_closed_blue() {
    assert_eq!(sftp_dir_icon_path(false), TermuaIcon::FolderClosedBlue);
}

#[test]
fn sftp_dir_icon_path_is_folder_open_blue_when_expanded() {
    assert_eq!(sftp_dir_icon_path(true), TermuaIcon::FolderOpenBlue);
}

#[test]
fn context_menu_for_blank_does_not_include_rename_or_delete_or_download() {
    use ContextMenu as S;
    use ContextMenuAction as A;

    let spec = sftp_context_menu(ContextMenuTarget::Background);
    assert!(spec.contains(&S::Action(A::Upload)));
    assert!(spec.contains(&S::Action(A::NewFolder)));
    assert!(!spec.contains(&S::Action(A::Download)));
    assert!(!spec.contains(&S::Action(A::Rename)));
    assert!(!spec.contains(&S::Action(A::Delete)));
}

#[test]
fn context_menu_for_dir_does_not_include_download() {
    use ContextMenu as S;
    use ContextMenuAction as A;

    let spec = sftp_context_menu(ContextMenuTarget::Single(EntryKind::Dir));
    assert!(spec.contains(&S::Action(A::Upload)));
    assert!(spec.contains(&S::Action(A::NewFolder)));
    assert!(spec.contains(&S::Action(A::Rename)));
    assert!(spec.contains(&S::Action(A::Delete)));
    assert!(!spec.contains(&S::Action(A::Download)));
}

#[test]
fn context_menu_for_file_does_not_include_upload_or_new_folder() {
    use ContextMenu as S;
    use ContextMenuAction as A;

    let spec = sftp_context_menu(ContextMenuTarget::Single(EntryKind::File));
    assert!(spec.contains(&S::Action(A::Download)));
    assert!(spec.contains(&S::Action(A::Rename)));
    assert!(spec.contains(&S::Action(A::Delete)));
    assert!(!spec.contains(&S::Action(A::Upload)));
    assert!(!spec.contains(&S::Action(A::NewFolder)));
}

#[test]
fn context_menu_for_multi_selection_only_includes_delete() {
    use ContextMenu as S;
    use ContextMenuAction as A;

    let spec = sftp_context_menu(ContextMenuTarget::Multi {
        has_file: true,
        has_dir: true,
    });
    assert!(spec.contains(&S::Action(A::Delete)));
    assert!(!spec.contains(&S::Action(A::Download)));
    assert!(!spec.contains(&S::Action(A::Rename)));
    assert!(!spec.contains(&S::Action(A::Upload)));
    assert!(!spec.contains(&S::Action(A::NewFolder)));
}

#[test]
fn delete_selected_item_title_includes_name_when_present() {
    assert_eq!(
        delete_selected_item_title(Some("notes.txt")),
        "Delete \"notes.txt\"?"
    );
}

#[test]
fn delete_selected_item_title_falls_back_when_missing() {
    assert_eq!(delete_selected_item_title(None), "Delete selected item?");
    assert_eq!(
        delete_selected_item_title(Some("")),
        "Delete selected item?"
    );
    assert_eq!(
        delete_selected_item_title(Some("   ")),
        "Delete selected item?"
    );
}

#[test]
fn sftp_table_can_expand_dir_rows_in_place() {
    let mut tree = TreeState::new(Entry::new("/", "/", EntryKind::Dir));
    tree.upsert_children("/", vec![Entry::new("/dir", "dir", EntryKind::Dir)]);
    tree.upsert_children(
        "/dir",
        vec![Entry::new("/dir/file.txt", "file.txt", EntryKind::File)],
    );

    let mut d = delegate_with_tree(tree);
    d.rebuild_visible();
    assert_eq!(
        d.visible
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        vec!["dir"]
    );

    // Expand "dir" - it already has loaded children, so no refresh is needed.
    assert_eq!(d.toggle_dir_local(0), None);
    assert_eq!(
        d.visible
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        vec!["dir", "file.txt"]
    );

    // Collapse back.
    assert_eq!(d.toggle_dir_local(0), None);
    assert_eq!(
        d.visible
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        vec!["dir"]
    );
}

#[test]
fn external_file_drop_rejects_directories() {
    let base = unique_tmp_path("drop");
    std::fs::create_dir_all(&base).unwrap();
    let file_path = base.join("a.txt");
    std::fs::write(&file_path, b"hello").unwrap();
    let dir_path = base.join("folder");
    std::fs::create_dir_all(&dir_path).unwrap();

    assert!(accept_external_file_drop_paths(&[file_path]));
    assert!(!accept_external_file_drop_paths(std::slice::from_ref(
        &dir_path
    )));
    assert!(!accept_external_file_drop_paths(&[dir_path]));

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn drop_upload_target_dir_uses_root_unless_hovering_a_directory_row() {
    let mut tree = TreeState::new(Entry::new("/", "/", EntryKind::Dir));
    tree.upsert_children(
        "/",
        vec![
            Entry::new("/dir", "dir", EntryKind::Dir),
            Entry::new("/file.txt", "file.txt", EntryKind::File),
        ],
    );

    let mut d = delegate_with_tree(tree);
    d.rebuild_visible();

    assert_eq!(d.drop_upload_target_dir(None).as_deref(), Some("/"));

    let dir_ix = d.visible.iter().position(|r| r.id == "/dir").unwrap();
    let file_ix = d.visible.iter().position(|r| r.id == "/file.txt").unwrap();

    assert_eq!(
        d.drop_upload_target_dir(Some(dir_ix)).as_deref(),
        Some("/dir")
    );
    assert_eq!(
        d.drop_upload_target_dir(Some(file_ix)).as_deref(),
        Some("/")
    );
}

#[test]
fn selection_plain_click_selects_single_row() {
    let mut tree = TreeState::new(Entry::new("/", "/", EntryKind::Dir));
    tree.upsert_children(
        "/",
        vec![
            Entry::new("/a", "a", EntryKind::File),
            Entry::new("/b", "b", EntryKind::File),
            Entry::new("/c", "c", EntryKind::File),
        ],
    );
    let mut d = delegate_with_tree(tree);
    d.rebuild_visible();

    d.click_row_local(0, gpui::Modifiers::none());
    assert_eq!(d.selected_ids_sorted(), vec!["/a".to_string()]);

    d.click_row_local(2, gpui::Modifiers::none());
    assert_eq!(d.selected_ids_sorted(), vec!["/c".to_string()]);
}

#[test]
fn selection_secondary_click_toggles_rows() {
    let mut tree = TreeState::new(Entry::new("/", "/", EntryKind::Dir));
    tree.upsert_children(
        "/",
        vec![
            Entry::new("/a", "a", EntryKind::File),
            Entry::new("/b", "b", EntryKind::File),
            Entry::new("/c", "c", EntryKind::File),
        ],
    );
    let mut d = delegate_with_tree(tree);
    d.rebuild_visible();

    d.click_row_local(0, gpui::Modifiers::none());
    d.click_row_local(2, gpui::Modifiers::secondary_key());
    assert_eq!(
        d.selected_ids_sorted(),
        vec!["/a".to_string(), "/c".to_string()]
    );

    d.click_row_local(0, gpui::Modifiers::secondary_key());
    assert_eq!(d.selected_ids_sorted(), vec!["/c".to_string()]);
}

#[test]
fn selection_shift_click_selects_range() {
    let mut tree = TreeState::new(Entry::new("/", "/", EntryKind::Dir));
    tree.upsert_children(
        "/",
        vec![
            Entry::new("/a", "a", EntryKind::File),
            Entry::new("/b", "b", EntryKind::File),
            Entry::new("/c", "c", EntryKind::File),
            Entry::new("/d", "d", EntryKind::File),
        ],
    );
    let mut d = delegate_with_tree(tree);
    d.rebuild_visible();

    d.click_row_local(1, gpui::Modifiers::none());
    d.click_row_local(
        3,
        gpui::Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    assert_eq!(
        d.selected_ids_sorted(),
        vec!["/b".to_string(), "/c".to_string(), "/d".to_string()]
    );
}

#[test]
fn context_menu_target_right_click_selects_row_when_not_selected() {
    let mut tree = TreeState::new(Entry::new("/", "/", EntryKind::Dir));
    tree.upsert_children(
        "/",
        vec![
            Entry::new("/a", "a", EntryKind::File),
            Entry::new("/b", "b", EntryKind::File),
            Entry::new("/c", "c", EntryKind::File),
        ],
    );
    let mut d = delegate_with_tree(tree);
    d.rebuild_visible();

    d.click_row_local(0, gpui::Modifiers::none());
    d.click_row_local(1, gpui::Modifiers::secondary_key());
    assert_eq!(
        d.selected_ids_sorted(),
        vec!["/a".to_string(), "/b".to_string()]
    );

    // Right-clicking a non-selected row should select only that row.
    d.set_context_menu_target(Some(2));
    assert_eq!(d.selected_ids_sorted(), vec!["/c".to_string()]);
}

#[test]
fn context_menu_target_right_click_preserves_multi_selection_when_inside_selection() {
    let mut tree = TreeState::new(Entry::new("/", "/", EntryKind::Dir));
    tree.upsert_children(
        "/",
        vec![
            Entry::new("/a", "a", EntryKind::File),
            Entry::new("/b", "b", EntryKind::File),
            Entry::new("/c", "c", EntryKind::File),
        ],
    );
    let mut d = delegate_with_tree(tree);
    d.rebuild_visible();

    d.click_row_local(0, gpui::Modifiers::none());
    d.click_row_local(2, gpui::Modifiers::secondary_key());
    assert_eq!(
        d.selected_ids_sorted(),
        vec!["/a".to_string(), "/c".to_string()]
    );

    // Right-clicking an already-selected row should keep the selection.
    d.set_context_menu_target(Some(2));
    assert_eq!(
        d.selected_ids_sorted(),
        vec!["/a".to_string(), "/c".to_string()]
    );
}

#[test]
fn delete_target_ids_prefers_multi_selection_over_context_row() {
    let mut tree = TreeState::new(Entry::new("/", "/", EntryKind::Dir));
    tree.upsert_children(
        "/",
        vec![
            Entry::new("/a", "a", EntryKind::File),
            Entry::new("/b", "b", EntryKind::File),
            Entry::new("/c", "c", EntryKind::File),
        ],
    );
    let mut d = delegate_with_tree(tree);
    d.rebuild_visible();

    d.click_row_local(0, gpui::Modifiers::none());
    d.click_row_local(2, gpui::Modifiers::secondary_key());

    // Even if context_row points elsewhere, delete should target the whole selection.
    let targets = d.delete_target_ids(Some(1));
    assert_eq!(
        targets,
        vec!["/a".to_string(), "/c".to_string()],
        "{targets:?}"
    );
}

#[test]
fn delete_target_ids_falls_back_to_context_row_when_no_selection() {
    let mut tree = TreeState::new(Entry::new("/", "/", EntryKind::Dir));
    tree.upsert_children(
        "/",
        vec![
            Entry::new("/a", "a", EntryKind::File),
            Entry::new("/b", "b", EntryKind::File),
        ],
    );
    let mut d = delegate_with_tree(tree);
    d.rebuild_visible();

    assert_eq!(d.delete_target_ids(Some(1)), vec!["/b".to_string()]);
}

#[test]
fn table_row_ix_from_mouse_y_accounts_for_negative_scroll_offset() {
    // When the content is scrolled down, the scroll handle's offset is typically negative
    // (content translated upward). The row mapping should still work.
    let row_h = px(20.0);
    let scroll_offset_y = px(-40.0); // scrolled down by 40px => two rows

    // Mouse is at the top of the body area (just below the header).
    let body_y = px(0.0);
    assert_eq!(
        table_row_ix_from_mouse_y(body_y, scroll_offset_y, row_h),
        Some(2)
    );
}

#[test]
fn folder_drop_ring_shadow_is_spread_only() {
    let border = gpui::hsla(210.0 / 360.0, 0.6, 0.5, 1.0);
    assert_eq!(
        folder_drop_ring_shadow(border),
        vec![gpui::BoxShadow {
            color: border.opacity(0.25),
            offset: gpui::point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(2.0),
        }]
    );
}
