use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use gpui::App;
use gpui_component::{Theme, ThemeConfig, ThemeRegistry};

/// Apply selected theme configs (by theme name) onto gpui-component's global Theme.
///
/// This does not change theme *mode* (System/Light/Dark); it only changes which concrete light/dark
/// theme configs are used when the app is in light or dark mode.
pub fn apply_selected_themes(
    light_theme_name: Option<&str>,
    dark_theme_name: Option<&str>,
    cx: &mut App,
) {
    // Look up configs first (immutable access), then mutate Theme once.
    //
    // If a saved theme name no longer exists (e.g. a built-in theme was removed), fall back to
    // the registry defaults rather than leaving the Theme in an arbitrary previous state.
    let default_light = ThemeRegistry::global(cx).default_light_theme().clone();
    let default_dark = ThemeRegistry::global(cx).default_dark_theme().clone();

    let light = light_theme_name
        .and_then(|name| find_theme_by_name(name, cx))
        .unwrap_or(default_light);
    let dark = dark_theme_name
        .and_then(|name| find_theme_by_name(name, cx))
        .unwrap_or(default_dark);

    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
}

pub fn themes_dir_path() -> PathBuf {
    crate::settings::settings_dir_path().join("themes")
}

pub fn ensure_builtin_themes_installed(themes_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(themes_dir)?;

    // We only create built-in theme files if missing, so user edits are preserved.
    let builtins = [(
        "termua-comfort.json",
        include_str!("../themes/termua-comfort.json"),
    )];
    for (file_name, contents) in builtins {
        let path = themes_dir.join(file_name);
        if path.exists() {
            continue;
        }
        std::fs::write(path, contents)?;
    }

    Ok(())
}

pub fn watch_user_themes_dir(themes_dir: PathBuf, cx: &mut App) -> anyhow::Result<()> {
    ThemeRegistry::watch_dir(themes_dir, cx, move |cx| {
        let appearance = crate::settings::load_settings_from_disk()
            .unwrap_or_default()
            .appearance;

        // If no explicit selection is set, use the registry defaults (which may come from custom
        // themes, including built-ins installed into `themes/`).
        let default_light = ThemeRegistry::global(cx).default_light_theme().clone();
        let default_dark = ThemeRegistry::global(cx).default_dark_theme().clone();

        if appearance.light_theme.is_none() {
            Theme::global_mut(cx).light_theme = default_light;
        }
        if appearance.dark_theme.is_none() {
            Theme::global_mut(cx).dark_theme = default_dark;
        }

        apply_selected_themes(
            appearance.light_theme.as_deref(),
            appearance.dark_theme.as_deref(),
            cx,
        );

        // Re-apply the active mode so colors update immediately.
        let mode = Theme::global(cx).mode;
        Theme::change(mode, None, cx);
        cx.refresh_windows();
    })
}

fn find_theme_by_name(name: &str, cx: &App) -> Option<Rc<ThemeConfig>> {
    ThemeRegistry::global(cx)
        .themes()
        .iter()
        .find_map(|(k, v)| (k.as_ref() == name).then(|| v.clone()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui_component::{ActiveTheme, Colorize, ThemeMode};

    use super::*;

    fn write_theme_json(dir: &PathBuf) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("test-theme.json"),
            r##"
{
  "name": "Test Themes",
  "themes": [
    {
      "name": "My Dark",
      "mode": "dark",
      "colors": {
        "background": "#000000ff",
        "foreground": "#dce0e5ff"
      }
    }
  ]
}
"##,
        )
        .unwrap();
    }

    #[gpui::test]
    fn selecting_dark_theme_by_name_updates_background_is_applied(cx: &mut gpui::TestAppContext) {
        let themes_dir =
            std::env::temp_dir().join(format!("termua-theme-manager-test-{}", std::process::id()));
        write_theme_json(&themes_dir);

        {
            let mut app = cx.app.borrow_mut();
            menubar::init(&mut app);
            gpui_term::init(&mut app);

            gpui_component::ThemeRegistry::watch_dir(themes_dir, &mut app, |_cx| {}).unwrap();
        }

        // Let ThemeRegistry's async reload run.
        cx.run_until_parked();

        {
            let mut app = cx.app.borrow_mut();
            apply_selected_themes(None, Some("My Dark"), &mut app);
            gpui_component::Theme::change(ThemeMode::Dark, None, &mut app);
            assert_eq!(app.theme().background.to_hex(), "#000000");
        }
    }

    #[test]
    fn ensure_builtin_themes_writes_default_theme_files() {
        let tmp_dir =
            std::env::temp_dir().join(format!("termua-themes-dir-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let settings_path = tmp_dir.join("termua").join("settings.json");
        let _guard = crate::settings::override_settings_json_path(settings_path);

        let themes_dir = themes_dir_path();
        ensure_builtin_themes_installed(&themes_dir).unwrap();

        assert!(themes_dir.join("termua-comfort.json").exists());
    }
}
