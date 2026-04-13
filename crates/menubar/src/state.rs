#[derive(Default, Debug, Clone)]
pub(crate) struct MenuBarState {
    pub expanded: bool,
    pub selected_ix: Option<usize>,
}

impl MenuBarState {
    pub fn on_fold_click(&mut self) {
        if self.expanded {
            self.expanded = false;
            self.selected_ix = None;
        } else {
            self.expanded = true;
            self.selected_ix = Some(0);
        }
    }

    pub fn on_menu_click(&mut self, ix: usize) {
        if !self.expanded {
            // Only fold menu should be visible while collapsed.
            if ix == 0 {
                self.expanded = true;
                self.selected_ix = Some(0);
            }
            return;
        }

        if self.selected_ix == Some(ix) {
            // Clicking current menu closes and collapses (Zed-like).
            self.expanded = false;
            self.selected_ix = None;
        } else {
            self.selected_ix = Some(ix);
        }
    }

    pub fn on_menu_hover(&mut self, ix: usize) {
        if !self.expanded || ix == 0 {
            return;
        }
        self.selected_ix = Some(ix);
    }

    pub fn on_cancel(&mut self) {
        self.expanded = false;
        self.selected_ix = None;
    }

    pub fn on_dismiss(&mut self) {
        self.on_cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::MenuBarState;

    #[test]
    fn fold_click_expands_and_selects_fold_menu() {
        let mut s = MenuBarState::default();
        s.on_fold_click();
        assert!(s.expanded);
        assert_eq!(s.selected_ix, Some(0));
    }

    #[test]
    fn fold_click_when_expanded_collapses_and_clears_selection() {
        let mut s = MenuBarState::default();
        s.on_fold_click();
        s.on_fold_click();
        assert!(!s.expanded);
        assert_eq!(s.selected_ix, None);
    }

    #[test]
    fn hover_opens_non_fold_menu_when_expanded() {
        let mut s = MenuBarState::default();
        s.on_fold_click(); // expand
        s.on_menu_hover(2);
        assert_eq!(s.selected_ix, Some(2));
    }

    #[test]
    fn hover_does_not_open_fold_menu() {
        let mut s = MenuBarState::default();
        s.on_fold_click(); // selected=0
        s.on_menu_hover(0);
        assert_eq!(s.selected_ix, Some(0));
    }

    #[test]
    fn dismiss_collapses() {
        let mut s = MenuBarState::default();
        s.on_fold_click();
        s.on_dismiss();
        assert!(!s.expanded);
        assert_eq!(s.selected_ix, None);
    }
}
