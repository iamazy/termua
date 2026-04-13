# gpui_menubar

Cross-platform menubar helpers for GPUI apps.

## Behavior

- **macOS**
  - Use the native OS menubar (screen-top) via `cx.set_menus(...)`.
  - `FoldableAppMenuBar` / `MenubarTitleBar` render no in-window menu triggers.
- **Linux / Windows**
  - Render an in-window menubar intended to be embedded into a custom titlebar.
  - Default state is collapsed to a single icon-only fold button.
  - Clicking the fold button toggles expand/collapse.
  - Hovering a top-level menu opens it automatically (fold button excluded).
  - Dismissing any open popup collapses the menubar.

### Menu Convention

This crate uses GPUI app menus (`cx.set_menus`, `cx.get_menus`).

- `menus[0]` is treated as the **fold/app menu** (hamburger button on Linux/Windows, first top-level menu on macOS).
- `menus[1..]` are normal top-level menus.

## Usage

```rust
use gpui::{App, Application, Menu, MenuItem, WindowDecorations, WindowOptions};
use gpui_component::Root;
use gpui_menubar::{MenubarTitleBar, title_bar_options};

Application::new().run(|cx: &mut App| {
    gpui_component::init(cx);
    gpui_menubar::init(cx);

    cx.set_menus(vec![
        Menu { name: "Menu".into(), items: vec![MenuItem::action("Quit", Quit)] },
        Menu { name: "File".into(), items: vec![MenuItem::action("New", NewFile)] },
    ]);

    cx.open_window(
        WindowOptions {
            titlebar: Some(title_bar_options()),
            window_decorations: cfg!(target_os = "linux").then_some(WindowDecorations::Client),
            ..Default::default()
        },
        |window, cx| {
            let view = cx.new(|_| MyView);
            cx.new(|cx| Root::new(view, window, cx))
        },
    ).unwrap();
});
```

Note: this crate does **not** call `gpui_component::init(cx)` for you; initialize `gpui-component`
once at application startup.

