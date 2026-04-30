use std::sync::Arc;

use gpui::{
    Animation, AnimationExt, App, Bounds, Context, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div,
    point, prelude::FluentBuilder as _, px, size,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, Size as UiSize,
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    progress::Progress,
    v_flex,
};
use gpui_transfer::{
    TransferCenterState, TransferKind, TransferProgress, TransferStatus, TransferTask,
};
use rust_i18n::t;

use super::FootbarView;

impl FootbarView {
    pub(super) fn sync_transfers_popup_state(&mut self, transfers: &[TransferTask]) {
        if transfers.is_empty() {
            self.transfers_open = false;
        }
    }

    pub(super) fn render_transfers_summary(
        &mut self,
        transfers: &[TransferTask],
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(task) = transfers.first() else {
            return div().into_any_element();
        };

        let transfers_done = transfers
            .iter()
            .filter(|t| t.status == TransferStatus::Finished)
            .count();

        let stripe_a = cx.theme().progress_bar.alpha(0.22);
        let stripe_b = cx.theme().progress_bar.alpha(0.10);
        let progress_el = render_transfers_summary_progress(task, stripe_a, stripe_b, cx.theme());

        let (done, total) = task
            .group_id
            .as_deref()
            .and_then(|gid| cx.global::<TransferCenterState>().group_counts(gid))
            .unwrap_or((transfers_done, transfers.len()));
        let transfers_done_label = format!("{done}/{total}");
        let transfers_for_panel = Arc::new(transfers.to_vec());
        let view = cx.entity();

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .justify_end()
            .pr(px(10.0))
            .child(
                Popover::new("termua-footbar-transfers-popover")
                    .anchor(gpui::Anchor::BottomRight)
                    // Our popover content already renders its own panel (bg/border/shadow/padding).
                    // Disable the default Popover "panel" styling to avoid a double-layer frame.
                    .appearance(false)
                    .open(self.transfers_open)
                    .on_open_change(move |open, window, app| {
                        view.update(app, |this, cx| {
                            this.transfers_open = *open;
                            cx.notify();
                        });
                        window.refresh();
                    })
                    .trigger(
                        Button::new("termua-footbar-transfers-trigger")
                            .xsmall()
                            .compact()
                            .ghost()
                            .debug_selector(|| "termua-footbar-transfers-trigger".to_string())
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(transfers_done_label),
                                    )
                                    .child(progress_el),
                            ),
                    )
                    .content(move |_state, window, cx| {
                        let viewport = window.viewport_size();
                        let w = px(420.0).min(viewport.width - px(24.0));
                        render_transfers_panel(&transfers_for_panel, w, cx.theme())
                    }),
            )
            .into_any_element()
    }
}

fn render_transfers_summary_progress(
    task: &TransferTask,
    stripe_a: Hsla,
    stripe_b: Hsla,
    theme: &gpui_component::Theme,
) -> gpui::AnyElement {
    const SUMMARY_PROGRESS_W: gpui::Pixels = px(220.0);
    const SUMMARY_PROGRESS_H: gpui::Pixels = px(6.0);

    match task.progress {
        TransferProgress::Determinate(pct) => {
            let value = (pct.clamp(0.0, 1.0) * 100.0).clamp(0.0, 100.0);
            let progress = Progress::new(format!(
                "termua-footbar-transfer-progress-summary-{}",
                task.id
            ))
            .with_size(UiSize::Small)
            .value(value);

            div()
                .w(SUMMARY_PROGRESS_W)
                .min_w(px(140.0))
                .max_w(px(320.0))
                .relative()
                .overflow_hidden()
                .rounded(px(3.0))
                .child(progress)
                .when(task.status == TransferStatus::InProgress, |this| {
                    this.child(render_footbar_activity_overlay(
                        format!("termua-footbar-transfer-wave-{}", task.id),
                        stripe_a,
                        stripe_b,
                        SUMMARY_PROGRESS_W,
                        SUMMARY_PROGRESS_H,
                    ))
                })
                .into_any_element()
        }
        TransferProgress::Indeterminate => div()
            .w(SUMMARY_PROGRESS_W)
            .min_w(px(140.0))
            .max_w(px(320.0))
            .h(px(6.0))
            .relative()
            .overflow_hidden()
            .rounded(px(3.0))
            .bg(theme.progress_bar.opacity(0.2))
            .child(render_footbar_activity_overlay(
                format!("termua-footbar-transfer-wave-{}", task.id),
                stripe_a,
                stripe_b,
                SUMMARY_PROGRESS_W,
                SUMMARY_PROGRESS_H,
            ))
            .into_any_element(),
    }
}

fn render_transfers_panel(
    transfers: &[TransferTask],
    w: gpui::Pixels,
    theme: &gpui_component::Theme,
) -> gpui::AnyElement {
    let panel_bg = theme.popover.opacity(0.96);
    let panel_fg = theme.popover_foreground;
    let border = theme.border.opacity(0.9);
    let muted_fg = theme.muted_foreground;
    let success_fg = theme.success;
    let stripe_a = theme.progress_bar.alpha(0.22);
    let stripe_b = theme.progress_bar.alpha(0.10);
    let progress_lane_bg = theme.progress_bar.opacity(0.2);

    let list = v_flex()
        .gap(px(10.0))
        .children(transfers.iter().map(|task| {
            render_transfer_popup_row(
                task,
                muted_fg,
                stripe_a,
                stripe_b,
                progress_lane_bg,
                success_fg,
            )
        }))
        .into_any_element();

    div()
        .id("termua-footbar-transfers-panel")
        .debug_selector(|| "termua-footbar-transfers-panel".to_string())
        .w(w)
        .max_h(px(240.0))
        .overflow_y_scroll()
        .overflow_x_hidden()
        .bg(panel_bg)
        .text_color(panel_fg)
        .border_1()
        .border_color(border)
        .rounded_md()
        .shadow_lg()
        .p(px(10.0))
        .child(list)
        .on_any_mouse_down(
            |_ev: &gpui::MouseDownEvent, _window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
            },
        )
        .into_any_element()
}

fn transfer_icon_path(kind: TransferKind) -> TermuaIcon {
    match kind {
        TransferKind::Upload => TermuaIcon::Upload,
        TransferKind::Download => TermuaIcon::Download,
        TransferKind::Other => TermuaIcon::PlugZap,
    }
}

fn transfer_popup_icon_spec(task: &TransferTask) -> (TermuaIcon, bool) {
    if task.status == TransferStatus::Finished {
        (TermuaIcon::Check, true)
    } else {
        (transfer_icon_path(task.kind), false)
    }
}

fn transfer_tooltip(task: &TransferTask) -> SharedString {
    match task.detail.as_ref() {
        Some(detail) if !detail.as_ref().trim().is_empty() => {
            format!("{}\nTo:{}", task.title.as_ref(), detail.as_ref()).into()
        }
        _ => task.title.clone(),
    }
}

fn transfer_meta(task: &TransferTask) -> Option<String> {
    let pct = match task.progress {
        TransferProgress::Determinate(pct) => Some(pct.clamp(0.0, 1.0)),
        TransferProgress::Indeterminate => None,
    };

    let bytes = match (task.bytes_done, task.bytes_total) {
        (Some(done), Some(total)) if total > 0 => Some(format!(
            "{}/{}",
            super::format_bytes(done),
            super::format_bytes(total)
        )),
        (Some(done), _) => Some(super::format_bytes(done)),
        _ => None,
    };

    match (pct, bytes) {
        (Some(pct), Some(bytes)) => {
            let pct = (pct * 100.0).round().clamp(0.0, 100.0) as u32;
            Some(format!("{pct}%  {bytes}"))
        }
        (Some(pct), None) => {
            let pct = (pct * 100.0).round().clamp(0.0, 100.0) as u32;
            Some(format!("{pct}%"))
        }
        (None, Some(bytes)) => Some(bytes),
        (None, None) => None,
    }
}

fn render_transfer_cancel_button(task: &TransferTask) -> Option<gpui::AnyElement> {
    if task.status != TransferStatus::InProgress {
        return None;
    }

    let cancel_requested = task
        .cancel
        .as_ref()
        .is_some_and(|t| t.load(std::sync::atomic::Ordering::Relaxed));

    if let Some(token) = task.cancel.as_ref() {
        let token = Arc::clone(token);
        return Some(
            Button::new(format!("termua-footbar-transfer-cancel-{}", task.id))
                .xsmall()
                .compact()
                .ghost()
                .icon(Icon::new(IconName::Close).xsmall())
                .tooltip(if cancel_requested {
                    t!("Transfers.Canceling").to_string()
                } else {
                    t!("Transfers.Cancel").to_string()
                })
                .disabled(cancel_requested)
                .on_click(move |_e, _window, cx| {
                    token.store(true, std::sync::atomic::Ordering::Relaxed);
                    cx.refresh_windows();
                })
                .into_any_element(),
        );
    }

    None
}

fn render_transfer_progress_lane(
    task: &TransferTask,
    stripe_a: gpui::Hsla,
    stripe_b: gpui::Hsla,
    progress_lane_bg: gpui::Hsla,
) -> gpui::AnyElement {
    match task.progress {
        TransferProgress::Determinate(pct) => {
            let value = (pct.clamp(0.0, 1.0) * 100.0).clamp(0.0, 100.0);
            let progress =
                Progress::new(format!("termua-footbar-transfer-progress-list-{}", task.id))
                    .with_size(UiSize::Small)
                    .value(value);

            div()
                .w_full()
                .relative()
                .overflow_hidden()
                .rounded(px(3.0))
                .child(progress)
                .when(task.status == TransferStatus::InProgress, |this| {
                    this.child(render_transfer_activity_overlay_full(
                        format!("termua-footbar-transfer-wave-list-{}", task.id),
                        stripe_a,
                        stripe_b,
                    ))
                })
                .into_any_element()
        }
        TransferProgress::Indeterminate => div()
            .w_full()
            .h(px(6.0))
            .relative()
            .overflow_hidden()
            .rounded(px(3.0))
            .bg(progress_lane_bg)
            .when(task.status == TransferStatus::InProgress, |this| {
                this.child(render_transfer_activity_overlay_full(
                    format!("termua-footbar-transfer-wave-list-{}", task.id),
                    stripe_a,
                    stripe_b,
                ))
            })
            .into_any_element(),
    }
}

fn render_transfer_progress_row(
    progress: gpui::AnyElement,
    cancel_btn: Option<gpui::AnyElement>,
) -> gpui::AnyElement {
    h_flex()
        .items_center()
        .gap(px(8.0))
        .child(div().flex_1().min_w_0().child(progress))
        .when_some(cancel_btn, |this, btn| this.child(btn))
        .into_any_element()
}

fn render_transfer_header_row(
    task: &TransferTask,
    icon_path: TermuaIcon,
    icon_color: Option<gpui::Hsla>,
    title: SharedString,
    meta: Option<String>,
    muted_fg: gpui::Hsla,
) -> gpui::AnyElement {
    let mut icon = Icon::default().path(icon_path).xsmall();
    if let Some(icon_color) = icon_color {
        icon = icon.text_color(icon_color);
    }

    h_flex()
        .items_center()
        .gap(px(12.0))
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(6.0))
                .child(icon)
                .child({
                    let mut title_btn =
                        Button::new(format!("termua-footbar-transfer-title-{}", task.id))
                            .xsmall()
                            .compact()
                            .text()
                            .tab_stop(false)
                            .flex_1()
                            .flex_shrink()
                            .min_w_0()
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .text_xs()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(title),
                            );

                    title_btn = title_btn.tooltip(transfer_tooltip(task));
                    title_btn
                }),
        )
        .when_some(meta, |this, meta| {
            this.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(muted_fg)
                    .whitespace_nowrap()
                    .child(meta),
            )
        })
        .into_any_element()
}

fn render_transfer_popup_row(
    task: &TransferTask,
    muted_fg: gpui::Hsla,
    stripe_a: gpui::Hsla,
    stripe_b: gpui::Hsla,
    progress_lane_bg: gpui::Hsla,
    success_fg: gpui::Hsla,
) -> gpui::AnyElement {
    let title = super::truncate_shared(&task.title, 56);
    let (icon_path, is_success) = transfer_popup_icon_spec(task);
    let icon_color = is_success.then_some(success_fg);
    let meta = transfer_meta(task);

    let cancel_btn = render_transfer_cancel_button(task);
    let progress = render_transfer_progress_lane(task, stripe_a, stripe_b, progress_lane_bg);
    let header_row = render_transfer_header_row(task, icon_path, icon_color, title, meta, muted_fg);
    let progress_row = render_transfer_progress_row(progress, cancel_btn);

    div()
        .id(format!("termua-footbar-transfer-row-{}", task.id))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(header_row)
        .child(progress_row)
        .into_any_element()
}

fn render_transfer_activity_canvas(id: String, stripe_a: Hsla, stripe_b: Hsla) -> gpui::AnyElement {
    // A subtle moving "wave" made of bars that slides to the right.
    // This is intentionally low-contrast so it reads as activity rather than noise.
    let anim = Animation::new(std::time::Duration::from_millis(900)).repeat();

    // The animation wrapper reconstructs the canvas each frame, capturing `delta`.
    canvas(|_, _, _| (), |_, _, _, _| {})
        .size_full()
        .with_animation(id, anim, move |_old, delta| {
            let stripe_a = stripe_a;
            let stripe_b = stripe_b;
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, (), window: &mut Window, _cx| {
                    let stripe_w = px(18.0);
                    let gap = px(12.0);
                    let period = stripe_w + gap;

                    let period_f = period / px(1.0);
                    let w_f = bounds.size.width / px(1.0);
                    if period_f <= 0.0 || w_f <= 0.0 {
                        return;
                    }

                    // Move one full period per animation cycle.
                    let start_x = -2.0 * period_f + delta * period_f;
                    let n = (w_f / period_f).ceil() as i32 + 6;

                    for i in 0..n {
                        let x = start_x + i as f32 * period_f;
                        let c = if i % 2 == 0 { stripe_a } else { stripe_b };
                        let b = Bounds {
                            origin: point(bounds.origin.x + px(x), bounds.origin.y),
                            size: size(stripe_w, bounds.size.height),
                        };
                        window.paint_quad(gpui::fill(b, c));
                    }
                },
            )
            .size_full()
        })
        .into_any_element()
}

fn render_footbar_activity_overlay(
    id: String,
    stripe_a: Hsla,
    stripe_b: Hsla,
    w: Pixels,
    h: Pixels,
) -> gpui::AnyElement {
    let animated = render_transfer_activity_canvas(id, stripe_a, stripe_b);

    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .w(w)
        .h(h)
        .child(animated)
        .into_any_element()
}

fn render_transfer_activity_overlay_full(
    id: String,
    stripe_a: Hsla,
    stripe_b: Hsla,
) -> gpui::AnyElement {
    let animated = render_transfer_activity_canvas(id, stripe_a, stripe_b);

    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .child(animated)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_transfer_uses_green_check_icon_in_popup() {
        let task = TransferTask::new("t1", "test")
            .with_kind(TransferKind::Upload)
            .with_status(TransferStatus::Finished);

        let (path, is_success) = transfer_popup_icon_spec(&task);
        assert_eq!(path, TermuaIcon::Check);
        assert!(is_success);
    }

    #[test]
    fn tooltip_includes_destination_path_on_new_line() {
        let task = TransferTask::new("t1", "file.txt")
            .with_kind(TransferKind::Download)
            .with_detail("/tmp/file.txt")
            .with_status(TransferStatus::InProgress);

        assert_eq!(
            transfer_tooltip(&task).as_ref(),
            "file.txt\nTo:/tmp/file.txt"
        );
    }

    #[test]
    fn in_progress_transfer_without_cancel_token_has_no_cancel_button() {
        let task = TransferTask::new("legacy-upload-1", "legacy")
            .with_kind(TransferKind::Upload)
            .with_status(TransferStatus::InProgress);

        assert!(render_transfer_cancel_button(&task).is_none());
    }
}
