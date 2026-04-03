use crossterm::event::KeyEvent;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Render flag
// ---------------------------------------------------------------------------

/// 0 = no render needed, 1 = full render, 2 = partial render
pub static NEED_RENDER: AtomicU8 = AtomicU8::new(0);

/// Request a render. If `partial` is true and no full render is already
/// pending, sets the flag to 2 (partial). Otherwise sets it to 1 (full).
pub fn request_render(partial: bool) {
    if partial {
        // Only upgrade to partial if nothing is pending; don't downgrade a
        // pending full render.
        let _ = NEED_RENDER.compare_exchange(0, 2, Ordering::Relaxed, Ordering::Relaxed);
    } else {
        NEED_RENDER.store(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Event enum
// ---------------------------------------------------------------------------

/// Events produced by the terminal, background tasks, or the tick timer.
#[derive(Debug)]
pub enum Event {
    Key(KeyEvent),
    Resize(u16, u16),
    ScanProgress {
        files: u64,
        dirs: u64,
        total_size: u64,
    },
    ScanComplete,
    Tick,
}

// ---------------------------------------------------------------------------
// Channel type aliases
// ---------------------------------------------------------------------------

pub type EventSender = mpsc::UnboundedSender<Event>;
pub type EventReceiver = mpsc::UnboundedReceiver<Event>;

/// Create a new unbounded event channel.
pub fn channel() -> (EventSender, EventReceiver) {
    mpsc::unbounded_channel()
}
