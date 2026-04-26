use gpui::{
    Context, Entity, Focusable, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Render, Styled, Window, div,
};
use gpui_component::menu::ContextMenu;

use super::TerminalView;
use crate::{element::TerminalElement, terminal::Terminal};

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal_handle = self.terminal.clone();
        let terminal_view_handle = cx.entity();

        self.sync_scroll_for_render(cx);
        let focused = self.focus_handle.is_focused(window);

        let mut root = self.terminal_view_root_base(cx);
        root = self.terminal_view_root_mouse_handlers(root, cx);
        root = root.child(self.terminal_view_inner_wrapper(
            terminal_handle,
            terminal_view_handle.clone(),
            focused,
            cx,
        ));
        root = root.children(self.collect_overlay_elements(window, cx));

        if !self.context_menu_enabled {
            return root.into_any_element();
        }

        let context_menu_enabled = self.context_menu_enabled;
        let action_context = self.focus_handle.clone();
        let menu_terminal_handle = self.terminal.clone();
        let terminal_view = terminal_view_handle.clone();
        let context_menu_provider = self.context_menu_provider.clone();

        ContextMenu::new("terminal-view-context-menu", root)
            .menu(move |menu, window, cx| {
                Self::build_terminal_context_menu(
                    context_menu_enabled,
                    action_context.clone(),
                    menu_terminal_handle.clone(),
                    terminal_view.clone(),
                    context_menu_provider.clone(),
                    menu,
                    window,
                    cx,
                )
            })
            .into_any_element()
    }
}

impl TerminalView {
    fn terminal_view_root_base(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id("terminal-view")
            .size_full()
            .relative()
            .track_focus(&self.focus_handle(cx))
            .key_context(self.dispatch_context(cx))
            .on_action(cx.listener(TerminalView::send_text))
            .on_action(cx.listener(TerminalView::send_keystroke))
            .on_action(cx.listener(TerminalView::open_search))
            .on_action(cx.listener(TerminalView::search_next))
            .on_action(cx.listener(TerminalView::search_previous))
            .on_action(cx.listener(TerminalView::close_search))
            .on_action(cx.listener(TerminalView::search_paste))
            .on_action(cx.listener(TerminalView::copy))
            .on_action(cx.listener(TerminalView::paste))
            .on_action(cx.listener(TerminalView::clear))
            .on_action(cx.listener(TerminalView::reset_font_size))
            .on_action(cx.listener(TerminalView::increase_font_size))
            .on_action(cx.listener(TerminalView::decrease_font_size))
            .on_action(cx.listener(TerminalView::scroll_line_up))
            .on_action(cx.listener(TerminalView::scroll_line_down))
            .on_action(cx.listener(TerminalView::scroll_page_up))
            .on_action(cx.listener(TerminalView::scroll_page_down))
            .on_action(cx.listener(TerminalView::scroll_to_top))
            .on_action(cx.listener(TerminalView::scroll_to_bottom))
            .on_action(cx.listener(TerminalView::toggle_vi_mode))
            .on_action(cx.listener(TerminalView::show_character_palette))
            .on_action(cx.listener(TerminalView::select_all))
            .on_action(cx.listener(TerminalView::start_cast_recording))
            .on_action(cx.listener(TerminalView::stop_cast_recording))
            .on_action(cx.listener(TerminalView::toggle_cast_recording))
            .on_key_down(cx.listener(Self::key_down))
    }

    fn terminal_view_root_mouse_handlers(
        &mut self,
        root: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let root = root.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                // Treat the left gutter (line numbers/padding) as UI chrome: allow selecting
                // command blocks there rather than starting a terminal selection.
                let content_bounds = {
                    let terminal = this.terminal.read(cx);
                    terminal.last_content().terminal_bounds.bounds
                };
                if event.position.x < content_bounds.origin.x {
                    this.close_suggestions(cx);
                    this.select_command_block_at_y(
                        event.position.y,
                        event.modifiers.shift,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }
            }),
        );

        if !self.context_menu_enabled {
            return root;
        }

        root.on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.close_suggestions(cx);

                // We treat the left gutter (outside the terminal content bounds) as "UI
                // chrome", not part of the terminal application. Allow the
                // context menu there even when the terminal is in mouse
                // mode (e.g. vim/tmux).
                let (content_bounds, mouse_mode_enabled, has_selection) = {
                    let terminal = this.terminal.read(cx);
                    (
                        terminal.last_content().terminal_bounds.bounds,
                        terminal.mouse_mode(event.modifiers.shift),
                        terminal.last_content().selection.is_some(),
                    )
                };
                let clicked_in_gutter = event.position.x < content_bounds.origin.x;
                if clicked_in_gutter {
                    // Pre-select the block (if any) so context-menu actions apply.
                    this.select_command_block_at_y(
                        event.position.y,
                        event.modifiers.shift,
                        window,
                        cx,
                    );
                    return;
                }

                // When the terminal is in mouse mode (e.g. vim/tmux), don't open the context
                // menu; let the application handle right clicks.
                if mouse_mode_enabled {
                    cx.stop_propagation();
                    return;
                }

                if !has_selection {
                    this.terminal.update(cx, |terminal, _| {
                        terminal.select_word_at_event_position(event);
                    });
                    window.refresh();
                }
            }),
        )
    }

    fn terminal_view_inner_wrapper(
        &mut self,
        terminal_handle: Entity<Terminal>,
        terminal_view_handle: Entity<TerminalView>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        // NOTE: Keep a wrapper div around `TerminalElement`; without it the terminal
        // element can interfere with overlay UI (context menu, etc).
        div()
            .id("terminal-view-inner")
            .size_full()
            .relative()
            .child(TerminalElement::new(
                terminal_handle,
                terminal_view_handle,
                self.focus_handle.clone(),
                focused,
                self.should_show_cursor(focused, cx),
                self.scroll.block_below_cursor.clone(),
            ))
    }
}
