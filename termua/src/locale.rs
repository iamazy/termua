#[cfg(test)]
use std::{cell::Cell, sync::Mutex};

#[cfg(test)]
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    static LOCALE_LOCK_DEPTH: Cell<usize> = const { Cell::new(0) };
}

pub(crate) struct LocaleLockGuard {
    #[cfg(test)]
    _guard: Option<std::sync::MutexGuard<'static, ()>>,
    #[cfg(test)]
    previous_locale: Option<String>,
}

#[cfg(test)]
pub(crate) fn lock() -> LocaleLockGuard {
    lock_impl(true)
}

fn lock_impl(restore_locale: bool) -> LocaleLockGuard {
    #[cfg(test)]
    {
        let should_lock = LOCALE_LOCK_DEPTH.with(|depth| {
            let current = depth.get();
            depth.set(current + 1);
            current == 0
        });

        if should_lock {
            // Recover from a poisoned mutex instead of panicking: a panicking test
            // must not turn every later locale-aware test into a PoisonError failure.
            let guard = LOCALE_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous_locale = restore_locale.then(|| rust_i18n::locale().to_string());
            LocaleLockGuard {
                _guard: Some(guard),
                previous_locale,
            }
        } else {
            LocaleLockGuard {
                _guard: None,
                previous_locale: None,
            }
        }
    }

    #[cfg(not(test))]
    {
        let _ = restore_locale;
        LocaleLockGuard {}
    }
}

pub(crate) fn set_locale(locale: &str) {
    // `set_locale` must persist when called outside an explicit test `lock()`
    // (e.g. `SettingsFile::apply_to_app` in tests), so it uses the non-restoring
    // internal lock.
    let _guard = lock_impl(false);

    rust_i18n::set_locale(locale);
}

impl Drop for LocaleLockGuard {
    fn drop(&mut self) {
        #[cfg(test)]
        {
            if let Some(previous_locale) = self.previous_locale.take() {
                rust_i18n::set_locale(&previous_locale);
            }

            LOCALE_LOCK_DEPTH.with(|depth| {
                let current = depth.get();
                depth.set(current.saturating_sub(1));
            });
        }
    }
}
