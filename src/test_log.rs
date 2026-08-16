//! Capturing `tracing` output in tests.
//!
//! Warn-level logging is behaviour the specs name explicitly
//! (`DbCallSlowWarning`, `TrajectoryWriteFailureIsSilent`,
//! `UsageWriteFailureIsSilent` in `docs/specs/observability.allium`), so tests
//! need to assert on it. This is the one harness for doing that — don't grow a
//! second copy alongside the test that needs it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

/// An in-memory sink for a `tracing_subscriber::fmt` writer, so tests can
/// assert on the rendered log text.
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run `body` with a `fmt` subscriber installed as the default for the
/// current task, and return everything it logged as text. Relies on
// allow-phantom-symbol: current_thread names tokio's runtime flavour, not our code
/// `#[tokio::test]`'s default `current_thread` runtime so the thread-local
/// subscriber guard survives every `.await` in `body` (the task never
/// migrates to another OS thread).
pub(crate) async fn logged_during<F, Fut>(body: F) -> String
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let buffer = LogBuffer::default();
    let make_writer = {
        let buffer = buffer.clone();
        move || buffer.clone()
    };
    let subscriber = tracing_subscriber::fmt()
        .with_writer(make_writer)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    body().await;
    let bytes = buffer.0.lock().unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}
