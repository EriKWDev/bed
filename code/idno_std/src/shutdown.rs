//! Process shutdown flag set by SIGINT/SIGTERM (unix) so Ctrl+C runs the
//! same graceful disconnect path as a normal quit.

use std::sync::atomic::Ordering;

pub fn requested() -> bool {
    crate::process_runtime()
        .shutdown_requested
        .load(Ordering::Relaxed)
}

pub fn request() {
    crate::process_runtime()
        .shutdown_requested
        .store(true, Ordering::Relaxed);
}

#[cfg(unix)]
pub fn install_handler() {
    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    extern "C" fn on_signal(_sig: i32) {
        // Second signal while shutting down: exit hard.
        if crate::process_runtime()
            .shutdown_requested
            .swap(true, Ordering::Relaxed)
        {
            std::process::exit(130);
        }
    }
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    let handler = on_signal as extern "C" fn(i32) as usize;
    unsafe {
        signal(SIGINT, handler);
        signal(SIGTERM, handler);
    }
}

#[cfg(not(unix))]
pub fn install_handler() {}
