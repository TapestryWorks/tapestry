//! Event log storage: the single source of truth for a thread.
//!
//! [`EventStore`] is the backend contract; [`InMemoryEventStore`] is the
//! default backend. Each subscriber keeps its own cursor, so subscribers never
//! steal events from each other and a slow subscriber never loses events.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock, Weak};

use async_trait::async_trait;
use tapestry_protocol::{Event, EventMsg, SubmissionId, ThreadId, TurnId};
use tokio::sync::watch;

use crate::error::CoreError;

/// An event before the store assigns its position in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    pub id: SubmissionId,
    pub turn_id: Option<TurnId>,
    pub timestamp_ms: u64,
    pub msg: EventMsg,
}

/// Append-only, per-thread event log.
#[async_trait]
pub trait EventStore: Send + Sync {
    /// Append `event`, assigning the next log position (starting at 1) of `thread`.
    async fn append(&self, thread: ThreadId, event: NewEvent) -> Result<Event, CoreError>;

    /// Stored events of `thread` with `log_position > after_log_position`, in order.
    async fn read_after(
        &self,
        thread: ThreadId,
        after_log_position: u64,
    ) -> Result<Vec<Event>, CoreError>;

    /// Replay events with `log_position > after_log_position`, then follow newly appended ones.
    async fn subscribe(
        &self,
        thread: ThreadId,
        after_log_position: u64,
    ) -> Result<EventSubscription, CoreError>;
}

/// Backend-specific source behind an [`EventSubscription`].
#[async_trait]
pub trait SubscriptionSource: Send {
    /// Next event in log-position order; `None` once the log can no longer grow.
    async fn next(&mut self) -> Option<Result<Event, CoreError>>;
}

/// One subscriber's ordered view of a thread's event log.
pub struct EventSubscription {
    source: Box<dyn SubscriptionSource>,
}

impl EventSubscription {
    pub fn new(source: impl SubscriptionSource + 'static) -> Self {
        Self {
            source: Box::new(source),
        }
    }

    pub async fn next(&mut self) -> Option<Result<Event, CoreError>> {
        self.source.next().await
    }

    /// Read until the terminal event (`TurnComplete`, `TurnAborted`, `Error`
    /// or `SessionConfigured`) of `submission_id`. The result may include
    /// events of other submissions; correlate them with `Event.id`.
    pub async fn collect_events_for(
        &mut self,
        submission_id: SubmissionId,
    ) -> Result<Vec<Event>, CoreError> {
        let mut events = Vec::new();
        while let Some(event) = self.next().await {
            let event = event?;
            let done = event.id == submission_id
                && matches!(
                    event.msg,
                    EventMsg::TurnComplete(_)
                        | EventMsg::TurnAborted(_)
                        | EventMsg::Error(_)
                        | EventMsg::SessionConfigured(_)
                );
            events.push(event);
            if done {
                break;
            }
        }
        Ok(events)
    }
}

struct ThreadLog {
    events: Vec<Event>,
    /// Latest log position; wakes subscribers on append.
    notify: watch::Sender<u64>,
}

impl ThreadLog {
    fn new() -> Self {
        Self {
            events: Vec::new(),
            notify: watch::channel(0).0,
        }
    }

    fn after(&self, after_log_position: u64) -> &[Event] {
        // Log positions start at 1 and have no gaps, so they map directly to indices.
        let start = usize::try_from(after_log_position)
            .unwrap_or(usize::MAX)
            .min(self.events.len());
        &self.events[start..]
    }
}

#[derive(Default)]
struct Inner {
    threads: RwLock<HashMap<ThreadId, ThreadLog>>,
}

/// Default backend: keeps every thread's log in memory, unbounded.
///
/// Subscriptions end (yield `None`) once every clone of the store is dropped.
#[derive(Clone, Default)]
pub struct InMemoryEventStore {
    inner: Arc<Inner>,
}

impl InMemoryEventStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EventStore for InMemoryEventStore {
    async fn append(&self, thread: ThreadId, event: NewEvent) -> Result<Event, CoreError> {
        let mut threads = self
            .inner
            .threads
            .write()
            .map_err(|_| CoreError::EventStore("lock poisoned".into()))?;
        let log = threads.entry(thread).or_insert_with(ThreadLog::new);
        let log_position = log.events.len() as u64 + 1;
        let event = Event {
            id: event.id,
            log_position,
            thread_id: thread,
            turn_id: event.turn_id,
            timestamp_ms: event.timestamp_ms,
            msg: event.msg,
        };
        log.events.push(event.clone());
        log.notify.send_replace(event.log_position);
        Ok(event)
    }

    async fn read_after(
        &self,
        thread: ThreadId,
        after_log_position: u64,
    ) -> Result<Vec<Event>, CoreError> {
        let threads = self
            .inner
            .threads
            .read()
            .map_err(|_| CoreError::EventStore("lock poisoned".into()))?;
        Ok(threads
            .get(&thread)
            .map(|log| log.after(after_log_position).to_vec())
            .unwrap_or_default())
    }

    async fn subscribe(
        &self,
        thread: ThreadId,
        after_log_position: u64,
    ) -> Result<EventSubscription, CoreError> {
        let mut threads = self
            .inner
            .threads
            .write()
            .map_err(|_| CoreError::EventStore("lock poisoned".into()))?;
        let changed = threads
            .entry(thread)
            .or_insert_with(ThreadLog::new)
            .notify
            .subscribe();
        Ok(EventSubscription::new(InMemorySubscription {
            inner: Arc::downgrade(&self.inner),
            thread,
            cursor: after_log_position,
            buffer: VecDeque::new(),
            changed,
        }))
    }
}

struct InMemorySubscription {
    inner: Weak<Inner>,
    thread: ThreadId,
    cursor: u64,
    buffer: VecDeque<Event>,
    changed: watch::Receiver<u64>,
}

#[async_trait]
impl SubscriptionSource for InMemorySubscription {
    async fn next(&mut self) -> Option<Result<Event, CoreError>> {
        loop {
            if let Some(event) = self.buffer.pop_front() {
                self.cursor = event.log_position;
                return Some(Ok(event));
            }
            {
                let inner = self.inner.upgrade()?;
                // Mark the current version seen before reading, so an append
                // racing with this read still wakes `changed()` below.
                self.changed.borrow_and_update();
                let threads = match inner.threads.read() {
                    Ok(threads) => threads,
                    Err(_) => return Some(Err(CoreError::EventStore("lock poisoned".into()))),
                };
                if let Some(log) = threads.get(&self.thread) {
                    self.buffer.extend(log.after(self.cursor).iter().cloned());
                }
            }
            if self.buffer.is_empty() && self.changed.changed().await.is_err() {
                return None;
            }
        }
    }
}

/// Test backend that can be told to start rejecting appends.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub(crate) struct FlakyStore {
        inner: InMemoryEventStore,
        /// Appends still allowed before failing; `None` never fails.
        remaining: Mutex<Option<usize>>,
        fail_once_after: Mutex<Option<usize>>,
        read_failures: Mutex<usize>,
    }

    impl FlakyStore {
        /// Allow `n` more appends, then reject every later one.
        pub(crate) fn fail_after(&self, n: usize) {
            *self.remaining.lock().unwrap() = Some(n);
        }

        pub(crate) fn heal(&self) {
            *self.remaining.lock().unwrap() = None;
        }

        pub(crate) fn fail_once_after(&self, n: usize) {
            *self.fail_once_after.lock().unwrap() = Some(n);
        }

        pub(crate) fn fail_next_read(&self) {
            *self.read_failures.lock().unwrap() = 1;
        }
    }

    #[async_trait]
    impl EventStore for FlakyStore {
        async fn append(&self, thread: ThreadId, event: NewEvent) -> Result<Event, CoreError> {
            let fail_once = {
                let mut remaining = self.fail_once_after.lock().unwrap();
                match *remaining {
                    Some(0) => {
                        *remaining = None;
                        true
                    }
                    Some(count) => {
                        *remaining = Some(count - 1);
                        false
                    }
                    None => false,
                }
            };
            if fail_once {
                return Err(CoreError::EventStore("disk full".into()));
            }
            if let Some(remaining) = self.remaining.lock().unwrap().as_mut() {
                if *remaining == 0 {
                    return Err(CoreError::EventStore("disk full".into()));
                }
                *remaining -= 1;
            }
            self.inner.append(thread, event).await
        }

        async fn read_after(
            &self,
            thread: ThreadId,
            after_log_position: u64,
        ) -> Result<Vec<Event>, CoreError> {
            let should_fail = {
                let mut read_failures = self.read_failures.lock().unwrap();
                if *read_failures > 0 {
                    *read_failures -= 1;
                    true
                } else {
                    false
                }
            };
            if should_fail {
                return Err(CoreError::EventStore("read unavailable".into()));
            }
            self.inner.read_after(thread, after_log_position).await
        }

        async fn subscribe(
            &self,
            thread: ThreadId,
            after_log_position: u64,
        ) -> Result<EventSubscription, CoreError> {
            self.inner.subscribe(thread, after_log_position).await
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tapestry_protocol::WarningEvent;

    use super::*;

    fn new_event(n: usize) -> NewEvent {
        NewEvent {
            id: SubmissionId::new(),
            turn_id: None,
            timestamp_ms: n as u64,
            msg: EventMsg::Warning(WarningEvent {
                message: n.to_string(),
            }),
        }
    }

    async fn fill(store: &InMemoryEventStore, thread: ThreadId, n: usize) {
        for i in 0..n {
            store.append(thread, new_event(i)).await.unwrap();
        }
    }

    async fn take(sub: &mut EventSubscription, n: usize) -> Vec<u64> {
        let mut positions = Vec::new();
        for _ in 0..n {
            let event = tokio::time::timeout(Duration::from_secs(1), sub.next())
                .await
                .expect("subscription stalled")
                .unwrap()
                .unwrap();
            positions.push(event.log_position);
        }
        positions
    }

    #[tokio::test]
    async fn log_positions_are_contiguous_from_one() {
        let store = InMemoryEventStore::new();
        let thread = ThreadId::new();
        fill(&store, thread, 3).await;
        let events = store.read_after(thread, 0).await.unwrap();
        let positions: Vec<_> = events.iter().map(|e| e.log_position).collect();
        assert_eq!(positions, [1, 2, 3]);
        assert!(events.iter().all(|e| e.thread_id == thread));
        assert_eq!(events[2].timestamp_ms, 2);
    }

    #[tokio::test]
    async fn threads_are_independent() {
        let store = InMemoryEventStore::new();
        let (a, b) = (ThreadId::new(), ThreadId::new());
        for i in 0..3 {
            store.append(a, new_event(i)).await.unwrap();
            store.append(b, new_event(i)).await.unwrap();
        }
        for thread in [a, b] {
            let positions: Vec<_> = store
                .read_after(thread, 0)
                .await
                .unwrap()
                .iter()
                .map(|e| e.log_position)
                .collect();
            assert_eq!(positions, [1, 2, 3]);
        }
    }

    #[tokio::test]
    async fn read_after_cursor() {
        let store = InMemoryEventStore::new();
        let thread = ThreadId::new();
        fill(&store, thread, 10).await;
        let positions: Vec<_> = store
            .read_after(thread, 4)
            .await
            .unwrap()
            .iter()
            .map(|e| e.log_position)
            .collect();
        assert_eq!(positions, (5..=10).collect::<Vec<_>>());
        assert!(store.read_after(thread, 10).await.unwrap().is_empty());
        assert!(store.read_after(thread, 99).await.unwrap().is_empty());
        assert!(store
            .read_after(ThreadId::new(), 0)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn every_subscriber_receives_every_event() {
        let store = InMemoryEventStore::new();
        let thread = ThreadId::new();
        let mut first = store.subscribe(thread, 0).await.unwrap();
        let mut second = store.subscribe(thread, 0).await.unwrap();
        let writer = {
            let store = store.clone();
            tokio::spawn(async move { fill(&store, thread, 5).await })
        };
        assert_eq!(take(&mut first, 5).await, [1, 2, 3, 4, 5]);
        assert_eq!(take(&mut second, 5).await, [1, 2, 3, 4, 5]);
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn late_subscriber_replays_then_follows() {
        let store = InMemoryEventStore::new();
        let thread = ThreadId::new();
        fill(&store, thread, 3).await;
        let mut sub = store.subscribe(thread, 0).await.unwrap();
        assert_eq!(take(&mut sub, 3).await, [1, 2, 3]);
        fill(&store, thread, 2).await;
        assert_eq!(take(&mut sub, 2).await, [4, 5]);

        let mut from_middle = store.subscribe(thread, 3).await.unwrap();
        assert_eq!(take(&mut from_middle, 2).await, [4, 5]);
    }

    #[tokio::test]
    async fn slow_subscriber_loses_nothing() {
        let store = InMemoryEventStore::new();
        let thread = ThreadId::new();
        let mut sub = store.subscribe(thread, 0).await.unwrap();
        fill(&store, thread, 200).await;
        assert_eq!(take(&mut sub, 200).await, (1..=200).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn subscription_ends_when_store_is_dropped() {
        let store = InMemoryEventStore::new();
        let thread = ThreadId::new();
        fill(&store, thread, 1).await;
        let mut sub = store.subscribe(thread, 0).await.unwrap();
        drop(store);
        assert!(sub.next().await.is_none());
    }

    #[tokio::test]
    async fn collect_events_for_stops_at_matching_terminal_event() {
        let store = InMemoryEventStore::new();
        let thread = ThreadId::new();
        let (a, b) = (SubmissionId::new(), SubmissionId::new());
        let error = |id| NewEvent {
            id,
            turn_id: None,
            timestamp_ms: 0,
            msg: EventMsg::Error(tapestry_protocol::ErrorEvent {
                message: "x".into(),
            }),
        };
        store.append(thread, error(a)).await.unwrap();
        store.append(thread, error(b)).await.unwrap();
        store.append(thread, new_event(0)).await.unwrap();
        let mut sub = store.subscribe(thread, 0).await.unwrap();
        let events = sub.collect_events_for(b).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].id, b);
        assert_eq!(take(&mut sub, 1).await, [3]);
    }
}
