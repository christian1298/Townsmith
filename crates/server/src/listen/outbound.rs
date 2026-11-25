//! Bounded per-connection outbound frame queue.
//!
//! The sequential listen loop writes each response frame directly to the
//! socket. A concurrent listener needs the opposite shape: one thread owns the
//! reader and producer side, while a dedicated writer thread drains encoded
//! frames and performs the blocking socket write. This module provides the
//! hand-off between them without an async runtime and without a new
//! dependency.
//!
//! # Contract
//!
//! - The queue is a fixed-capacity FIFO. [`OutboundQueue::push`] rejects a
//!   frame with [`OutboundError::Full`] (code `outbound.backpressure`) once
//!   `capacity` frames are queued; the frame is dropped, so a caller that must
//!   not lose it retries after the consumer makes progress.
//! - After [`OutboundQueue::close`], pushes fail with
//!   [`OutboundError::Closed`] (code `outbound.closed`) but already queued
//!   frames stay readable: a consumer drains them in order and only then sees
//!   `None`. This keeps a final `Goodbye` or a last chat line from being lost
//!   when the session ends.
//! - One producer and one consumer are assumed. A mutex and condition variable
//!   make the queue safe for that pair; multiple producers would still be
//!   memory-safe but would not give each producer a meaningful ordering.
//! - [`OutboundQueue::pop`] is non-blocking, [`OutboundQueue::pop_blocking`]
//!   waits indefinitely, and [`OutboundQueue::pop_timeout`] bounds the wait so
//!   a writer thread can also check shutdown or flush timers.
//!
//! [`serve_session`](super::serve_session) installs one queue per accepted
//! connection. The reader thread encodes responses and calls
//! [`OutboundQueue::push`]; a writer thread owns a cloned `TcpStream` and
//! drains it with [`drain`]. When either side ends, the queue is closed: the
//! writer flushes everything already accepted and then exits. A full or closed
//! queue is never silently ignored by the listener — the session stops reading
//! commands, the connection is shut down, and the failure is logged with
//! [`OutboundError::code`].

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Write};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
#[cfg(test)]
use std::time::{Duration, Instant};

/// Default bound for one connection's outbound frame backlog.
pub(crate) const DEFAULT_CAPACITY: usize = 256;

/// A frame was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutboundError {
    /// The queue is at capacity. The frame was dropped; retry after the
    /// consumer drains one.
    Full,
    /// The queue was closed. The frame was dropped.
    Closed,
    /// The requested capacity was zero.
    ZeroCapacity,
}

impl OutboundError {
    /// Stable code for logs and diagnostics, matching the server's
    /// `SERVER_*` key/value output style.
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Full => "outbound.backpressure",
            Self::Closed => "outbound.closed",
            Self::ZeroCapacity => "outbound.zero_capacity",
        }
    }
}

impl fmt::Display for OutboundError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Full => "outbound queue is full",
            Self::Closed => "outbound queue is closed",
            Self::ZeroCapacity => "outbound queue capacity must be greater than zero",
        })
    }
}

impl std::error::Error for OutboundError {}

/// Bounded FIFO of encoded response frames for one connection.
///
/// The queue owns `Vec<u8>` frames exactly as they are handed to the socket, so
/// the writer thread does no protocol work and cannot reorder responses.
#[derive(Debug)]
pub(crate) struct OutboundQueue {
    capacity: usize,
    state: Mutex<State>,
    ready: Condvar,
}

#[derive(Debug, Default)]
struct State {
    frames: VecDeque<Vec<u8>>,
    closed: bool,
}

impl OutboundQueue {
    /// Create a queue that holds at most `capacity` frames.
    ///
    /// A zero capacity is rejected rather than treated as unbounded: every
    /// connection must have a real backpressure limit.
    pub(crate) fn new(capacity: usize) -> Result<Self, OutboundError> {
        if capacity == 0 {
            return Err(OutboundError::ZeroCapacity);
        }
        Ok(Self::build(capacity))
    }

    /// Queue one encoded frame.
    ///
    /// Fails without queueing when the queue is closed or already holds
    /// `capacity` frames.
    pub(crate) fn push(&self, frame: Vec<u8>) -> Result<(), OutboundError> {
        let mut state = self.lock();
        if state.closed {
            return Err(OutboundError::Closed);
        }
        if state.frames.len() >= self.capacity {
            return Err(OutboundError::Full);
        }
        state.frames.push_back(frame);
        self.ready.notify_one();
        Ok(())
    }

    /// Remove the oldest frame, if any.
    ///
    /// Never blocks. A closed queue still yields its remaining frames; `None`
    /// means the queue is empty, whether or not it has been closed.
    #[cfg(test)]
    pub(crate) fn pop(&self) -> Option<Vec<u8>> {
        self.lock().frames.pop_front()
    }

    /// Wait for a frame, returning `None` once the queue is closed and drained.
    ///
    /// This is the writer-thread primitive. It blocks on the condition variable
    /// and cannot miss a frame produced between the empty check and the wait.
    pub(crate) fn pop_blocking(&self) -> Option<Vec<u8>> {
        let mut state = self.lock();
        loop {
            if let Some(frame) = state.frames.pop_front() {
                return Some(frame);
            }
            if state.closed {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Wait for a frame for at most `timeout`.
    ///
    /// Returns `None` when the queue is closed and drained or when the timeout
    /// expires. Queued frames are always returned before the timeout is
    /// considered, so a zero timeout still drains one frame.
    #[cfg(test)]
    pub(crate) fn pop_timeout(&self, timeout: Duration) -> Option<Vec<u8>> {
        let mut state = self.lock();
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(frame) = state.frames.pop_front() {
                return Some(frame);
            }
            if state.closed {
                return None;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let (guard, _) = self
                .ready
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            state = guard;
        }
    }

    /// Stop accepting frames and wake every waiter.
    ///
    /// Idempotent. Frames queued before the close remain available to the
    /// consumer.
    pub(crate) fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        self.ready.notify_all();
    }

    /// Whether [`Self::close`] has been called.
    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// Number of frames currently queued.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().frames.len()
    }

    /// Whether no frames are currently queued.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.lock().frames.is_empty()
    }

    /// Fixed maximum number of queued frames.
    #[cfg(test)]
    pub(crate) const fn capacity(&self) -> usize {
        self.capacity
    }

    fn build(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for OutboundQueue {
    fn default() -> Self {
        Self::build(DEFAULT_CAPACITY)
    }
}

/// Write every queued frame to `writer`, blocking until the queue is closed and
/// drained.
///
/// Frames are written verbatim, one at a time, and flushed before the next one
/// is popped. That matches the inline path's per-frame flush and keeps the byte
/// stream identical. A write error returns immediately; the caller closes the
/// queue so the reader stops producing.
pub(crate) fn drain<W: Write>(writer: &mut W, queue: &OutboundQueue) -> io::Result<()> {
    while let Some(frame) = queue.pop_blocking() {
        writer.write_all(&frame)?;
        writer.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn frame(value: u32) -> Vec<u8> {
        value.to_be_bytes().to_vec()
    }

    fn value(bytes: Vec<u8>) -> u32 {
        u32::from_be_bytes(bytes.try_into().expect("four-byte frame"))
    }

    #[test]
    fn default_capacity_is_the_bounded_default() {
        let queue = OutboundQueue::default();
        assert_eq!(DEFAULT_CAPACITY, 256);
        assert_eq!(queue.capacity(), 256);
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
        assert!(!queue.is_closed());
    }

    #[test]
    fn zero_capacity_is_rejected() {
        let error = OutboundQueue::new(0).expect_err("zero capacity must fail");
        assert_eq!(error, OutboundError::ZeroCapacity);
        assert_eq!(error.code(), "outbound.zero_capacity");
    }

    #[test]
    fn fifo_order_is_strict() {
        let queue = OutboundQueue::new(4).expect("queue");
        for value in [10, 11, 12] {
            queue.push(frame(value)).expect("room for three frames");
        }

        assert_eq!(queue.len(), 3);
        assert_eq!(queue.pop().map(value), Some(10));
        assert_eq!(queue.pop().map(value), Some(11));
        assert_eq!(queue.pop().map(value), Some(12));
        assert_eq!(queue.pop(), None);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn full_reports_backpressure_and_keeps_queued_frames() {
        let queue = OutboundQueue::new(2).expect("queue");
        queue.push(frame(1)).expect("first frame");
        queue.push(frame(2)).expect("second frame");

        let error = queue.push(frame(3)).expect_err("third frame is full");
        assert_eq!(error, OutboundError::Full);
        assert_eq!(error.code(), "outbound.backpressure");
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop().map(value), Some(1));
        assert_eq!(queue.pop().map(value), Some(2));

        // The rejected frame was not silently queued behind the others.
        assert_eq!(queue.pop(), None);
        queue.push(frame(4)).expect("space after pop");
        assert_eq!(queue.pop().map(value), Some(4));
    }

    #[test]
    fn close_drains_queued_frames_before_returning_none() {
        let queue = OutboundQueue::new(4).expect("queue");
        queue.push(frame(1)).expect("first frame");
        queue.push(frame(2)).expect("second frame");

        queue.close();
        queue.close();
        assert!(queue.is_closed());
        assert_eq!(queue.len(), 2);

        assert_eq!(queue.pop().map(value), Some(1));
        assert_eq!(queue.pop().map(value), Some(2));
        assert_eq!(queue.pop(), None);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn push_after_close_is_rejected() {
        let queue = OutboundQueue::new(2).expect("queue");
        queue.close();

        let error = queue
            .push(frame(1))
            .expect_err("closed queue rejects pushes");
        assert_eq!(error, OutboundError::Closed);
        assert_eq!(error.code(), "outbound.closed");
        assert!(queue.is_empty());
    }

    #[test]
    fn blocking_pop_drains_after_close() {
        let queue = OutboundQueue::new(2).expect("queue");
        queue.push(frame(7)).expect("frame");
        queue.close();

        assert_eq!(queue.pop_blocking().map(value), Some(7));
        assert_eq!(queue.pop_blocking(), None);
    }

    #[test]
    fn timeout_wakes_on_close_without_waiting_for_the_deadline() {
        let queue = Arc::new(OutboundQueue::new(2).expect("queue"));
        let waiter = {
            let queue = Arc::clone(&queue);
            thread::spawn(move || queue.pop_timeout(Duration::from_secs(30)))
        };

        queue.close();
        assert_eq!(waiter.join().expect("waiter thread"), None);
    }

    #[test]
    fn concurrent_producer_and_consumer_preserve_order_and_counts() {
        const FRAMES: u32 = 512;
        // Smaller than the frame count, so the producer exercises the
        // backpressure path instead of filling the queue in one pass.
        let queue = Arc::new(OutboundQueue::new(8).expect("queue"));

        let producer = {
            let queue = Arc::clone(&queue);
            thread::spawn(move || {
                for index in 0..FRAMES {
                    loop {
                        match queue.push(frame(index)) {
                            Ok(()) => break,
                            Err(OutboundError::Full) => thread::yield_now(),
                            Err(error) => panic!("unexpected push error: {error}"),
                        }
                    }
                }
                queue.close();
            })
        };

        let mut expected = 0;
        while let Some(bytes) = queue.pop_timeout(Duration::from_secs(10)) {
            assert_eq!(value(bytes), expected);
            expected += 1;
        }

        producer.join().expect("producer thread");
        assert!(queue.is_closed());
        assert!(queue.is_empty());
        assert_eq!(expected, FRAMES);
    }

    #[test]
    fn queue_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OutboundQueue>();
    }

    #[test]
    fn drain_writes_raw_frames_in_fifo_order() {
        let queue = Arc::new(OutboundQueue::new(8).expect("queue"));
        for value in [10, 11, 12, 13] {
            queue.push(frame(value)).expect("room");
        }
        queue.close();

        let mut writer = Vec::new();
        drain(&mut writer, &queue).expect("drain after close");

        let expected: Vec<u8> = [10u32, 11, 12, 13]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect();
        assert_eq!(writer, expected);
        assert!(queue.is_empty());
    }

    /// A writer that accepts `remaining` bytes and then fails.
    struct FailAfter {
        remaining: usize,
        kind: io::ErrorKind,
    }

    impl Write for FailAfter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::new(self.kind, "injected write fault"));
            }
            let count = buffer.len().min(self.remaining);
            self.remaining -= count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn drain_stops_on_the_first_write_error_and_leaves_later_frames() {
        let queue = Arc::new(OutboundQueue::new(4).expect("queue"));
        for value in [1, 2, 3] {
            queue.push(frame(value)).expect("room");
        }
        queue.close();

        let mut writer = FailAfter {
            remaining: 3,
            kind: io::ErrorKind::BrokenPipe,
        };
        let error = drain(&mut writer, &queue).expect_err("the injected fault must surface");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

        // The partially written frame is gone; frames behind it were never
        // popped, so closing the queue did not discard them.
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop().map(value), Some(2));
        assert_eq!(queue.pop().map(value), Some(3));
        assert_eq!(queue.pop(), None);
    }
}
