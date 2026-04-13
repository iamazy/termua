use gpui::{Pixels, px};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RightSidebarTab {
    Notifications,
    Assistant,
}

pub struct RightSidebarState {
    pub visible: bool,
    pub width: Pixels,
    pub active_tab: RightSidebarTab,
}

impl Default for RightSidebarState {
    fn default() -> Self {
        Self {
            visible: false,
            width: px(360.0),
            active_tab: RightSidebarTab::Notifications,
        }
    }
}

impl gpui::Global for RightSidebarState {}

impl RightSidebarState {
    pub fn toggle_tab(&mut self, tab: RightSidebarTab) {
        if self.visible && self.active_tab == tab {
            self.visible = false;
        } else {
            self.visible = true;
            self.active_tab = tab;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_same_tab_twice_hides_sidebar() {
        let mut s = RightSidebarState::default();
        s.toggle_tab(RightSidebarTab::Notifications);
        assert!(s.visible);
        assert_eq!(s.active_tab, RightSidebarTab::Notifications);

        s.toggle_tab(RightSidebarTab::Notifications);
        assert!(!s.visible);
        assert_eq!(s.active_tab, RightSidebarTab::Notifications);
    }

    #[test]
    fn toggling_other_tab_shows_and_switches() {
        let mut s = RightSidebarState::default();
        s.toggle_tab(RightSidebarTab::Notifications);
        assert!(s.visible);
        assert_eq!(s.active_tab, RightSidebarTab::Notifications);

        s.toggle_tab(RightSidebarTab::Assistant);
        assert!(s.visible);
        assert_eq!(s.active_tab, RightSidebarTab::Assistant);
    }
}
