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
}

pub(crate) fn lock() -> LocaleLockGuard {
    #[cfg(test)]
    {
        let should_lock = LOCALE_LOCK_DEPTH.with(|depth| {
            let current = depth.get();
            depth.set(current + 1);
            current == 0
        });

        if should_lock {
            LocaleLockGuard {
                _guard: Some(LOCALE_LOCK.lock().unwrap()),
            }
        } else {
            LocaleLockGuard { _guard: None }
        }
    }

    #[cfg(not(test))]
    {
        LocaleLockGuard {}
    }
}

pub(crate) fn set_locale(locale: &str) {
    let _guard = lock();

    rust_i18n::set_locale(locale);
}

impl Drop for LocaleLockGuard {
    fn drop(&mut self) {
        #[cfg(test)]
        {
            LOCALE_LOCK_DEPTH.with(|depth| {
                let current = depth.get();
                depth.set(current.saturating_sub(1));
            });
        }
    }
}
