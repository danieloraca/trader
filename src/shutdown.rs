use crate::error::{BotError, Result};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::flag;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

#[derive(Clone)]
pub struct Shutdown {
    requested: Arc<AtomicBool>,
}

impl Shutdown {
    pub fn install_signal_handlers() -> Result<Self> {
        let shutdown = Self {
            requested: Arc::new(AtomicBool::new(false)),
        };

        flag::register(SIGINT, Arc::clone(&shutdown.requested)).map_err(|error| {
            BotError::Config(format!("failed to register SIGINT handler: {error}"))
        })?;
        flag::register(SIGTERM, Arc::clone(&shutdown.requested)).map_err(|error| {
            BotError::Config(format!("failed to register SIGTERM handler: {error}"))
        })?;

        Ok(shutdown)
    }

    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn request(&self) {
        self.requested.store(true, Ordering::Relaxed);
    }
}

pub fn sleep_or_shutdown(duration: Duration, shutdown: &Shutdown) -> bool {
    const CHECK_INTERVAL: Duration = Duration::from_millis(250);
    let mut remaining = duration;

    while !remaining.is_zero() {
        if shutdown.is_requested() {
            return true;
        }

        let sleep_for = remaining.min(CHECK_INTERVAL);
        thread::sleep(sleep_for);
        remaining = remaining.saturating_sub(sleep_for);
    }

    shutdown.is_requested()
}

#[cfg(test)]
mod tests {
    use super::{Shutdown, sleep_or_shutdown};
    use std::time::Duration;

    #[test]
    fn sleep_returns_immediately_when_shutdown_is_already_requested() {
        let shutdown = Shutdown::new_for_test();
        shutdown.request();

        assert!(sleep_or_shutdown(Duration::from_secs(30), &shutdown));
    }
}
