pub(crate) fn ensure_app_global_with<T: gpui::Global>(
    app: &mut gpui::App,
    init: impl FnOnce() -> T,
) {
    if app.try_global::<T>().is_none() {
        app.set_global(init());
    }
}

pub(crate) fn ensure_app_global<T: gpui::Global + Default>(app: &mut gpui::App) {
    ensure_app_global_with(app, T::default);
}

pub(crate) fn ensure_ctx_global_with<T: gpui::Global, U>(
    cx: &mut gpui::Context<'_, U>,
    init: impl FnOnce() -> T,
) {
    if cx.try_global::<T>().is_none() {
        cx.set_global(init());
    }
}

pub(crate) fn ensure_ctx_global<T: gpui::Global + Default, U>(cx: &mut gpui::Context<'_, U>) {
    ensure_ctx_global_with(cx, T::default);
}
