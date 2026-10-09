//! Event sink abstraction.
//!
//! The runtime supplies an implementation that persists events and broadcasts
//! them to connected clients. Agents and verification code depend only on this
//! trait, keeping the execution core independent of storage and transport.

use crate::event::EventKind;

/// Receives task events as they are produced.
pub trait EventSink: Send + Sync {
    /// Record a single event. Implementations must not block for long periods
    /// on network I/O.
    fn emit(&self, kind: EventKind);
}

/// A sink that discards events. Useful in tests and dry runs.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _kind: EventKind) {}
}

impl EventSink for Box<dyn EventSink> {
    fn emit(&self, kind: EventKind) {
        (**self).emit(kind)
    }
}

impl<T: EventSink + ?Sized> EventSink for std::sync::Arc<T> {
    fn emit(&self, kind: EventKind) {
        (**self).emit(kind)
    }
}
