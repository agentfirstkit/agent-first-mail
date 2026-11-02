//! CLI-owned cancellation. Library dispatch does not install process handlers.
use crate::error::{AppError, Result};
use std::time::{Duration, Instant};

#[cfg(all(unix, feature = "cloudflare"))]
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

#[cfg(all(unix, feature = "cloudflare"))]
static SHUTDOWN: OnceLock<std::result::Result<Arc<AtomicBool>, String>> = OnceLock::new();

/// Installed once for the lifetime of a consumer CLI invocation. Signal-hook
/// intentionally does not restore default handlers on unregister, so do not
/// scope these registrations to a tick or to one child process.
pub(crate) fn install() -> Result<()> {
    #[cfg(all(unix, feature = "cloudflare"))]
    {
        let state = SHUTDOWN.get_or_init(|| {
            let flag = Arc::new(AtomicBool::new(false));
            for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
                signal_hook::flag::register(signal, Arc::clone(&flag))
                    .map_err(|error| error.to_string())?;
            }
            Ok(flag)
        });
        if let Err(error) = state {
            return Err(AppError::new("consumer_signal_setup_failed", error.clone()));
        }
    }
    Ok(())
}

pub(crate) fn requested() -> bool {
    #[cfg(all(unix, feature = "cloudflare"))]
    {
        SHUTDOWN
            .get()
            .and_then(|state| state.as_ref().ok())
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }
    #[cfg(not(all(unix, feature = "cloudflare")))]
    false
}

pub(crate) fn interrupted() -> AppError {
    AppError::new(
        "consumer_interrupted",
        "Mail consumer stopped; unfinished durable tasks remain pending",
    )
}

pub(crate) fn wait(duration: Duration) {
    let start = Instant::now();
    while !requested() && start.elapsed() < duration {
        std::thread::sleep(
            duration
                .saturating_sub(start.elapsed())
                .min(Duration::from_millis(50)),
        );
    }
}
