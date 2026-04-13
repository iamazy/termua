use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
mod linux_pam_auth;

#[cfg(target_os = "macos")]
mod macos_pam_auth;

#[cfg(windows)]
mod win_auth;

pub mod overlay;
pub mod view;

pub trait Authenticator: Send + Sync {
    fn verify_password(&self, password: &str) -> anyhow::Result<bool>;
}

fn current_username() -> Option<String> {
    #[cfg(windows)]
    {
        std::env::var("USERNAME")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    #[cfg(not(windows))]
    {
        std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

#[cfg(target_os = "macos")]
fn platform_authenticator_for_username(username: &str) -> Option<Arc<dyn Authenticator>> {
    macos_pam_auth::PamAuthenticator::new(username)
        .ok()
        .map(|a| Arc::new(a) as Arc<dyn Authenticator>)
}

#[cfg(target_os = "linux")]
fn platform_authenticator_for_username(username: &str) -> Option<Arc<dyn Authenticator>> {
    linux_pam_auth::PamAuthenticator::new(username)
        .ok()
        .map(|a| Arc::new(a) as Arc<dyn Authenticator>)
}

#[cfg(windows)]
fn platform_authenticator_for_username(username: &str) -> Option<Arc<dyn Authenticator>> {
    Some(Arc::new(win_auth::WindowsAuthenticator::new(username)) as Arc<dyn Authenticator>)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn platform_authenticator_for_username(_username: &str) -> Option<Arc<dyn Authenticator>> {
    None
}

#[derive(Debug)]
struct DenyAllAuthenticator;

impl Authenticator for DenyAllAuthenticator {
    fn verify_password(&self, _password: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

pub struct LockState {
    supported: bool,
    user_enabled: bool,
    idle_timeout: Duration,
    last_activity: Mutex<Instant>,
    locked: bool,
    monitor_started: bool,
    authenticator: Arc<dyn Authenticator>,
}

impl LockState {
    fn set_last_activity_now(&self) {
        let mut last_activity = self
            .last_activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *last_activity = Instant::now();
    }

    pub fn new_default() -> Self {
        let authenticator =
            current_username().and_then(|u| platform_authenticator_for_username(&u));
        let (supported, authenticator) = match authenticator {
            Some(authenticator) => (true, authenticator),
            None => (
                false,
                Arc::new(DenyAllAuthenticator) as Arc<dyn Authenticator>,
            ),
        };

        Self {
            supported,
            user_enabled: true,
            idle_timeout: Duration::from_secs(5 * 60),
            last_activity: Mutex::new(Instant::now()),
            locked: false,
            monitor_started: false,
            authenticator,
        }
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    pub fn locking_supported(&self) -> bool {
        self.supported
    }

    pub fn locking_enabled(&self) -> bool {
        self.supported && self.user_enabled
    }

    pub fn set_user_enabled(&mut self, enabled: bool) {
        self.user_enabled = enabled;

        if !self.locking_enabled() {
            self.locked = false;
        }

        self.set_last_activity_now();
    }

    pub fn set_idle_timeout(&mut self, idle_timeout: Duration) {
        self.idle_timeout = idle_timeout;
        self.set_last_activity_now();
    }

    pub fn lock_now(&mut self) -> bool {
        if !self.locking_enabled() {
            return false;
        }
        self.locked = true;
        true
    }

    pub fn start_monitor_once(&mut self) -> bool {
        if self.monitor_started {
            return false;
        }
        self.monitor_started = true;
        true
    }

    pub fn report_activity(&self) {
        if self.locked {
            return;
        }
        self.set_last_activity_now();
    }

    pub fn should_lock(&self) -> bool {
        if !self.locking_enabled() || self.locked {
            return false;
        }
        if self.idle_timeout == Duration::from_secs(0) {
            return false;
        }
        let last_activity = *self
            .last_activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        last_activity.elapsed() >= self.idle_timeout
    }

    #[cfg(test)]
    pub fn tick(&mut self) {
        if self.should_lock() {
            self.locked = true;
        }
    }

    fn unlock_success(&mut self) {
        self.locked = false;
        self.set_last_activity_now();
    }

    pub fn try_unlock(&mut self, password: &str) -> anyhow::Result<bool> {
        if !self.locked {
            return Ok(true);
        }
        let ok = self.authenticator.verify_password(password)?;
        if ok {
            self.unlock_success();
        }
        Ok(ok)
    }

    #[cfg(test)]
    pub fn new_for_test(idle_timeout: Duration) -> Self {
        Self {
            supported: true,
            user_enabled: true,
            idle_timeout,
            last_activity: Mutex::new(Instant::now()),
            locked: false,
            monitor_started: true,
            authenticator: Arc::new(DenyAllAuthenticator),
        }
    }

    #[cfg(test)]
    pub fn new_for_test_with_auth(
        idle_timeout: Duration,
        authenticator: Arc<dyn Authenticator>,
    ) -> Self {
        Self {
            supported: true,
            user_enabled: true,
            idle_timeout,
            last_activity: Mutex::new(Instant::now()),
            locked: false,
            monitor_started: true,
            authenticator,
        }
    }

    #[cfg(test)]
    pub fn force_lock_for_test(&mut self) {
        self.locked = true;
    }

    #[cfg(test)]
    pub fn try_unlock_sync_for_test(&mut self, password: &str) -> anyhow::Result<bool> {
        self.try_unlock(password)
    }
}

impl gpui::Global for LockState {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{
        AppContext, IntoElement, ParentElement, Render, Styled, Window, div,
        prelude::FluentBuilder as _,
    };

    use super::*;

    #[test]
    fn lock_state_enters_locked_after_idle_timeout() {
        // Use generous timings to avoid flaky failures on slower/loaded CI runners.
        let mut state = LockState::new_for_test(Duration::from_millis(50));
        assert!(!state.locked());

        std::thread::sleep(Duration::from_millis(100));
        state.tick();

        assert!(state.locked());
    }

    #[test]
    fn lock_state_activity_resets_idle_deadline() {
        // Use generous timings to avoid flaky failures on slower/loaded CI runners.
        let mut state = LockState::new_for_test(Duration::from_millis(200));

        std::thread::sleep(Duration::from_millis(80));
        state.report_activity();

        std::thread::sleep(Duration::from_millis(80));
        state.tick();

        assert!(!state.locked());
    }

    struct FakeAuthenticator {
        ok: bool,
    }

    impl Authenticator for FakeAuthenticator {
        fn verify_password(&self, _password: &str) -> anyhow::Result<bool> {
            Ok(self.ok)
        }
    }

    #[test]
    fn lock_state_unlocks_on_successful_auth() {
        let auth = Arc::new(FakeAuthenticator { ok: true });
        let mut state = LockState::new_for_test_with_auth(Duration::from_secs(60), auth);
        state.force_lock_for_test();
        assert!(state.locked());

        assert!(state.try_unlock_sync_for_test("pw").unwrap());
        assert!(!state.locked());
    }

    #[test]
    fn lock_state_default_disables_lock_when_username_unavailable() {
        struct VarGuard {
            key: &'static str,
            prev: Option<String>,
        }

        impl Drop for VarGuard {
            fn drop(&mut self) {
                match &self.prev {
                    Some(v) => unsafe { std::env::set_var(self.key, v) },
                    None => unsafe { std::env::remove_var(self.key) },
                }
            }
        }

        #[cfg(windows)]
        let keys: [&'static str; 1] = ["USERNAME"];
        #[cfg(not(windows))]
        let keys: [&'static str; 2] = ["USER", "LOGNAME"];

        let _guards: Vec<VarGuard> = keys
            .into_iter()
            .map(|key| {
                let prev = std::env::var(key).ok();
                unsafe { std::env::remove_var(key) };
                VarGuard { key, prev }
            })
            .collect();

        let state = LockState::new_default();
        assert!(
            !state.locking_supported(),
            "lock should be disabled if we can't determine the current username"
        );
    }

    struct LockOverlayTestView {
        lock_overlay: super::overlay::LockOverlayState,
    }

    impl LockOverlayTestView {
        fn new(window: &mut Window, cx: &mut gpui::Context<Self>) -> Self {
            Self {
                lock_overlay: super::overlay::LockOverlayState::new(window, cx),
            }
        }

        fn unlock_from_overlay(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
            self.lock_overlay.unlock_with_password(window, cx);
        }
    }

    impl Render for LockOverlayTestView {
        fn render(
            &mut self,
            _window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let overlay = self
                .lock_overlay
                .render_overlay_if_locked(Self::unlock_from_overlay, cx);

            div()
                .size_full()
                .when_some(overlay, |this, overlay| this.child(overlay))
        }
    }

    #[gpui::test]
    fn lock_overlay_state_renders_overlay_when_locked(cx: &mut gpui::TestAppContext) {
        use std::time::Duration;

        cx.update(|app| {
            gpui_component::init(app);
            menubar::init(app);
            gpui_term::init(app);
            app.set_global(LockState::new_for_test(Duration::from_secs(60)));
            app.set_global(crate::notification::NotifyState::default());
        });

        let window = cx.add_empty_window();
        window.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(900.)),
                gpui::AvailableSpace::Definite(gpui::px(600.)),
            ),
            |window, app| {
                let view = app.new(|cx| LockOverlayTestView::new(window, cx));
                app.global_mut::<LockState>().force_lock_for_test();
                div().size_full().child(view)
            },
        );
        window.run_until_parked();

        assert!(
            window.debug_bounds("termua-lock-overlay").is_some(),
            "expected LockOverlayUiState to render the overlay while locked"
        );
    }

    #[gpui::test]
    fn lock_overlay_does_not_render_touch_id_button(cx: &mut gpui::TestAppContext) {
        use std::time::Duration;

        cx.update(|app| {
            gpui_component::init(app);
            menubar::init(app);
            gpui_term::init(app);
            app.set_global(LockState::new_for_test_with_auth(
                Duration::from_secs(60),
                Arc::new(FakeAuthenticator { ok: false }),
            ));
            app.set_global(crate::notification::NotifyState::default());
        });

        let window = cx.add_empty_window();
        window.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(900.)),
                gpui::AvailableSpace::Definite(gpui::px(600.)),
            ),
            |window, app| {
                let view = app.new(|cx| LockOverlayTestView::new(window, cx));
                app.global_mut::<LockState>().force_lock_for_test();
                div().size_full().child(view)
            },
        );
        window.run_until_parked();

        assert!(
            window.debug_bounds("termua-lock-touch-id").is_none(),
            "expected lock overlay to omit the Touch ID button"
        );
    }
}
