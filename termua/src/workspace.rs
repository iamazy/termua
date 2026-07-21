use std::path::{Path, PathBuf};

use anyhow::Context as _;
use gpui_dock::DockAreaState;

pub(crate) const STATE_VERSION: usize = 2;

pub(crate) fn state_path() -> PathBuf {
    #[cfg(test)]
    if !crate::settings::settings_json_path_is_overridden() {
        let thread = std::thread::current();
        let test_id = thread
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{:?}", thread.id()))
            .replace(|ch: char| !ch.is_ascii_alphanumeric(), "_");
        return std::env::temp_dir()
            .join(format!("termua-workspace-tests-{}", std::process::id()))
            .join(test_id)
            .join("workspace.json");
    }

    crate::settings::settings_dir_path().join("workspace.json")
}

pub(crate) fn save_to_path(path: &Path, state: &DockAreaState) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create workspace state directory {parent:?}"))?;
    }
    let json = serde_json::to_string_pretty(state).context("serialize dock workspace state")?;
    crate::atomic_write::write_string(path, &json)
}

pub(crate) fn load_from_path(path: &Path) -> anyhow::Result<DockAreaState> {
    let json = std::fs::read_to_string(path)
        .with_context(|| format!("read dock workspace state {path:?}"))?;
    serde_json::from_str(&json).context("deserialize dock workspace state")
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use gpui_dock::{DockAreaState, PanelState};

    fn unique_state_path(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("termua-workspace-{label}-{nanos}.json"))
    }

    #[test]
    fn workspace_state_path_is_next_to_settings() {
        let settings_path = unique_state_path("settings");
        let _guard = crate::settings::override_settings_json_path(settings_path.clone());

        assert_eq!(
            super::state_path(),
            settings_path.parent().unwrap().join("workspace.json")
        );
    }

    #[test]
    fn default_test_workspace_state_path_is_isolated_from_user_config() {
        let path = super::state_path();
        assert!(path.starts_with(std::env::temp_dir()));
        assert_ne!(
            path,
            crate::settings::settings_dir_path().join("workspace.json")
        );
    }

    #[test]
    fn dock_state_round_trips_through_disk() {
        let path = unique_state_path("round-trip");
        let state = DockAreaState {
            version: Some(1),
            center: PanelState::default(),
            left_dock: None,
            right_dock: None,
            bottom_dock: None,
        };

        super::save_to_path(&path, &state).expect("save dock state");
        let loaded = super::load_from_path(&path).expect("load dock state");

        assert_eq!(loaded, state);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn corrupt_dock_state_is_reported() {
        let path = unique_state_path("corrupt");
        std::fs::write(&path, "not json").expect("write corrupt state");

        assert!(super::load_from_path(&path).is_err());
        std::fs::remove_file(path).ok();
    }
}
