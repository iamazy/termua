use std::borrow::Cow;

use anyhow::anyhow;
use gpui::{AssetSource, Result, SharedString};
use gpui_component_assets::Assets as ComponentAssets;
use rust_embed::RustEmbed;

/// termua's application AssetSource.
///
/// We compose `gpui-component-assets` (for `icons/*.svg`) with app-specific assets (for
/// `icons/*.{svg,png}`).
pub struct TermuaAssets;

#[derive(RustEmbed)]
#[folder = "../../assets"]
#[include = "icons/**/*.svg"]
#[include = "icons/**/*.png"]
struct EmbeddedAssets;

impl AssetSource for TermuaAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        // Prefer app-local assets, so we can safely override `gpui-component-assets` if needed.
        if let Some(f) = EmbeddedAssets::get(path) {
            return Ok(Some(f.data));
        }

        if let Some(f) = ComponentAssets::get(path) {
            return Ok(Some(f.data));
        }

        Err(anyhow!("could not find asset at path \"{path}\""))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = EmbeddedAssets::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect();

        out.extend(ComponentAssets::iter().filter_map(|p| p.starts_with(path).then(|| p.into())));

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TermuaIcon;

    #[test]
    fn termua_assets_exposes_git_bash_svg() {
        let assets = TermuaAssets;
        let bytes = assets.load("icons/git-bash.svg").unwrap();
        assert!(bytes.is_some());
    }

    #[test]
    fn termua_assets_exposes_record_svg() {
        let assets = TermuaAssets;
        let bytes = assets.load("icons/record.svg").unwrap();
        assert!(bytes.is_some());
    }

    #[test]
    fn termua_assets_exposes_dice_icons() {
        let assets = TermuaAssets;
        assert!(assets.load("icons/dice-1.svg").unwrap().is_some());
        assert!(assets.load("icons/dice-4.svg").unwrap().is_some());
    }

    #[test]
    fn termua_assets_exposes_alert_circle_svg() {
        let assets = TermuaAssets;
        let bytes = assets.load("icons/alert-circle.svg").unwrap();
        assert!(bytes.is_some());
    }

    #[test]
    fn termua_assets_still_exposes_component_icons() {
        let assets = TermuaAssets;
        let bytes = assets.load("icons/close.svg").unwrap();
        assert!(bytes.is_some());
    }

    #[test]
    fn termua_assets_exposes_folder_icons() {
        let assets = TermuaAssets;
        let closed = assets
            .load("icons/folder-closed-blue.svg")
            .unwrap()
            .unwrap();
        let open = assets.load("icons/folder-open-blue.svg").unwrap().unwrap();

        let closed_svg = std::str::from_utf8(&closed).expect("folder svg should be valid utf-8");
        let open_svg = std::str::from_utf8(&open).expect("folder svg should be valid utf-8");

        assert!(
            closed_svg.contains("#60a5fa"),
            "closed folder icon should use the lighter blue palette"
        );
        assert!(
            open_svg.contains("#60a5fa"),
            "open folder icon should use the lighter blue palette"
        );
        assert!(
            !closed_svg.contains("#3b82f6"),
            "closed folder icon should not use the darker blue palette"
        );
        assert!(
            !open_svg.contains("#3b82f6"),
            "open folder icon should not use the darker blue palette"
        );
    }

    #[test]
    fn termua_assets_loads_termua_icon_enum_paths() {
        let assets = TermuaAssets;
        for icon in [
            TermuaIcon::Alacritty,
            TermuaIcon::Wezterm,
            TermuaIcon::FolderOpenBlue,
            TermuaIcon::FolderClosedBlue,
            TermuaIcon::GitBash,
            TermuaIcon::Pwsh,
        ] {
            assert!(
                assets.load(icon.path()).unwrap().is_some(),
                "expected {} to be loadable",
                icon.path()
            );
        }
    }
}
