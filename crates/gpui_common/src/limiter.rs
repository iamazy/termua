use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use gpui::{App, Context, Global};
use smol::channel::{Receiver, Sender};

#[derive(Clone)]
pub struct PermitPool {
    inner: Arc<PermitPoolInner>,
}

struct PermitPoolInner {
    state: Mutex<PermitPoolState>,
}

struct PermitPoolState {
    max: usize,
    inflight: usize,
    waiters: VecDeque<Sender<()>>,
}

pub struct PermitPoolPermit {
    inner: Arc<PermitPoolInner>,
}

impl Drop for PermitPoolPermit {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().expect("permit pool lock poisoned");
        state.inflight = state.inflight.saturating_sub(1);
        grant_waiters(&mut state);
    }
}

impl PermitPool {
    pub fn new(max: usize) -> Self {
        let max = max.clamp(1, 15);
        Self {
            inner: Arc::new(PermitPoolInner {
                state: Mutex::new(PermitPoolState {
                    max,
                    inflight: 0,
                    waiters: VecDeque::new(),
                }),
            }),
        }
    }

    pub fn max(&self) -> usize {
        self.inner
            .state
            .lock()
            .expect("permit pool lock poisoned")
            .max
    }

    pub fn set_max(&self, max: usize) {
        let max = max.clamp(1, 15);
        let mut state = self.inner.state.lock().expect("permit pool lock poisoned");
        if state.max == max {
            return;
        }
        state.max = max;
        grant_waiters(&mut state);
    }

    pub async fn acquire(&self) -> PermitPoolPermit {
        loop {
            let waiter: Option<Receiver<()>> = {
                let mut state = self.inner.state.lock().expect("permit pool lock poisoned");
                if state.inflight < state.max {
                    state.inflight += 1;
                    None
                } else {
                    let (tx, rx) = smol::channel::bounded(1);
                    state.waiters.push_back(tx);
                    Some(rx)
                }
            };

            if let Some(rx) = waiter {
                // Wait until a permit is granted (or retry if the pool is dropped).
                if rx.recv().await.is_err() {
                    continue;
                }
            }

            return PermitPoolPermit {
                inner: Arc::clone(&self.inner),
            };
        }
    }
}

fn grant_waiters(state: &mut PermitPoolState) {
    while state.inflight < state.max {
        let Some(tx) = state.waiters.pop_front() else {
            break;
        };
        if tx.try_send(()).is_ok() {
            state.inflight += 1;
        }
    }
}

#[derive(Clone)]
struct SftpUploadPermitPool {
    pool: PermitPool,
}

impl Global for SftpUploadPermitPool {}

pub fn sftp_upload_permit_pool<T: 'static>(cx: &mut Context<T>) -> PermitPool {
    if let Some(global) = cx.try_global::<SftpUploadPermitPool>() {
        return global.pool.clone();
    }
    let pool = PermitPool::new(5);
    cx.set_global(SftpUploadPermitPool { pool: pool.clone() });
    pool
}

pub fn sftp_upload_permit_pool_in_app(app: &mut App) -> PermitPool {
    if let Some(global) = app.try_global::<SftpUploadPermitPool>() {
        return global.pool.clone();
    }
    let pool = PermitPool::new(5);
    app.set_global(SftpUploadPermitPool { pool: pool.clone() });
    pool
}

pub fn set_sftp_upload_permit_pool_max_in_app(app: &mut App, max: usize) -> PermitPool {
    let max = max.clamp(1, 15);
    if app.try_global::<SftpUploadPermitPool>().is_none() {
        let pool = PermitPool::new(max);
        app.set_global(SftpUploadPermitPool { pool: pool.clone() });
        return pool;
    }
    let global = app.global_mut::<SftpUploadPermitPool>();
    global.pool.set_max(max);
    global.pool.clone()
}

pub fn set_sftp_upload_permit_pool_max<T: 'static>(cx: &mut Context<T>, max: usize) -> PermitPool {
    let max = max.clamp(1, 15);
    if cx.try_global::<SftpUploadPermitPool>().is_none() {
        let pool = PermitPool::new(max);
        cx.set_global(SftpUploadPermitPool { pool: pool.clone() });
        return pool;
    }
    let global = cx.global_mut::<SftpUploadPermitPool>();
    global.pool.set_max(max);
    global.pool.clone()
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    #[test]
    fn permit_pool_caps_concurrency_across_groups() {
        smol::block_on(async move {
            let pool = Arc::new(PermitPool::new(2));

            let active = Arc::new(AtomicUsize::new(0));
            let max_active = Arc::new(AtomicUsize::new(0));

            let mut tasks = Vec::new();
            for _group in 0..2 {
                for _ in 0..3 {
                    let pool = Arc::clone(&pool);
                    let active = Arc::clone(&active);
                    let max_active = Arc::clone(&max_active);
                    tasks.push(smol::spawn(async move {
                        let _permit = pool.acquire().await;
                        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                        max_active.fetch_max(now, Ordering::SeqCst);
                        smol::Timer::after(std::time::Duration::from_millis(25)).await;
                        active.fetch_sub(1, Ordering::SeqCst);
                    }));
                }
            }

            for t in tasks {
                t.await;
            }

            assert!(max_active.load(Ordering::SeqCst) <= 2);
            assert_eq!(pool.max(), 2);
        });
    }

    #[test]
    fn permit_pool_resize_allows_more_inflight() {
        smol::block_on(async move {
            let pool = Arc::new(PermitPool::new(1));

            let active = Arc::new(AtomicUsize::new(0));
            let max_active = Arc::new(AtomicUsize::new(0));

            // Task 1 grabs the only permit and holds it.
            let t1 = {
                let pool = Arc::clone(&pool);
                let active = Arc::clone(&active);
                let max_active = Arc::clone(&max_active);
                smol::spawn(async move {
                    let _permit = pool.acquire().await;
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(now, Ordering::SeqCst);
                    smol::Timer::after(std::time::Duration::from_millis(80)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                })
            };

            // Give t1 time to acquire.
            smol::Timer::after(std::time::Duration::from_millis(20)).await;
            assert_eq!(max_active.load(Ordering::SeqCst), 1);

            // Task 2 should initially be blocked, but should run after resizing to 2.
            let t2 = {
                let pool = Arc::clone(&pool);
                let active = Arc::clone(&active);
                let max_active = Arc::clone(&max_active);
                smol::spawn(async move {
                    let _permit = pool.acquire().await;
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(now, Ordering::SeqCst);
                    smol::Timer::after(std::time::Duration::from_millis(40)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                })
            };

            // Resize while t1 is still holding the permit.
            pool.set_max(2);

            t1.await;
            t2.await;

            assert_eq!(pool.max(), 2);
            assert!(max_active.load(Ordering::SeqCst) >= 2);
        });
    }
}
