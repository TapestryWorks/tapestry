//! The only write path of a thread: append an event, then project it.

use std::sync::Arc;

use tapestry_protocol::{ConversationItem, Event, EventMsg, SubmissionId, ThreadId, TurnId};

use crate::clock::Clock;
use crate::error::CoreError;
use crate::formatter::formatter_trait::ConversationProjection;
use crate::store::{EventStore, NewEvent};

/// Records a thread's events and keeps its history projection current.
///
/// Events are stamped with the current submission and turn, which the owner
/// sets with [`Recorder::begin_submission`] and [`Recorder::set_turn`].
pub struct Recorder {
    store: Arc<dyn EventStore>,
    clock: Arc<dyn Clock>,
    thread_id: ThreadId,
    projection: ConversationProjection,
    submission_id: SubmissionId,
    turn_id: Option<TurnId>,
}

impl Recorder {
    pub fn new(store: Arc<dyn EventStore>, clock: Arc<dyn Clock>, thread_id: ThreadId) -> Self {
        Self::with_projection(store, clock, thread_id, ConversationProjection::new())
    }

    /// Continue from an existing projection, e.g. one folded from the log.
    pub fn with_projection(
        store: Arc<dyn EventStore>,
        clock: Arc<dyn Clock>,
        thread_id: ThreadId,
        projection: ConversationProjection,
    ) -> Self {
        Self {
            store,
            clock,
            thread_id,
            projection,
            submission_id: SubmissionId::new(),
            turn_id: None,
        }
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    /// Stamp later events with `id` and no turn.
    pub fn begin_submission(&mut self, id: SubmissionId) {
        self.submission_id = id;
        self.turn_id = None;
    }

    pub fn set_turn(&mut self, turn_id: Option<TurnId>) {
        self.turn_id = turn_id;
    }

    pub fn turn_id(&self) -> Option<TurnId> {
        self.turn_id
    }

    pub fn messages(&self) -> Vec<ConversationItem> {
        self.projection.messages()
    }

    /// Read an immutable snapshot of the thread's complete event log.
    pub async fn events(&self) -> Result<Vec<Event>, CoreError> {
        self.store.read_after(self.thread_id, 0).await
    }

    pub fn projection(&self) -> &ConversationProjection {
        &self.projection
    }

    /// Append `msg` to the store and apply it to the history projection. On
    /// failure nothing is projected.
    pub async fn record(&mut self, msg: EventMsg) -> Result<Event, CoreError> {
        let event = self
            .store
            .append(
                self.thread_id,
                NewEvent {
                    id: self.submission_id,
                    turn_id: self.turn_id,
                    timestamp_ms: self.clock.unix_millis(),
                    msg,
                },
            )
            .await?;
        self.projection.apply(&event);
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use tapestry_protocol::{UserMessageEvent, WarningEvent};

    use super::*;
    use crate::clock::FixedClock;
    use crate::store::InMemoryEventStore;

    #[tokio::test]
    async fn record_stamps_envelope_and_projects() {
        let store = Arc::new(InMemoryEventStore::new());
        let clock = Arc::new(FixedClock::new("2026-11-05", "UTC"));
        clock.set_unix_millis(42);
        let thread = ThreadId::new();
        let mut recorder = Recorder::new(store.clone(), clock, thread);

        let sid = SubmissionId::new();
        let turn = TurnId::new();
        recorder.begin_submission(sid);
        recorder.set_turn(Some(turn));
        let event = recorder
            .record(EventMsg::UserMessage(UserMessageEvent {
                name: None,
                content: "hi".into(),
            }))
            .await
            .unwrap();
        assert_eq!(
            (event.id, event.log_position, event.thread_id, event.turn_id),
            (sid, 1, thread, Some(turn))
        );
        assert_eq!(event.timestamp_ms, 42);
        assert_eq!(recorder.messages(), [ConversationItem::user(None, "hi")]);

        recorder.begin_submission(SubmissionId::new());
        let event = recorder
            .record(EventMsg::Warning(WarningEvent {
                message: "x".into(),
            }))
            .await
            .unwrap();
        assert_eq!(event.turn_id, None);
        assert_eq!(event.log_position, 2);
        assert_eq!(store.read_after(thread, 0).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn events_returns_an_immutable_complete_snapshot() {
        let store = Arc::new(InMemoryEventStore::new());
        let mut recorder = Recorder::new(
            store.clone(),
            Arc::new(FixedClock::new("2026-11-05", "UTC")),
            ThreadId::new(),
        );
        recorder
            .record(EventMsg::Warning(WarningEvent {
                message: "first".into(),
            }))
            .await
            .unwrap();
        let snapshot = recorder.events().await.unwrap();
        recorder
            .record(EventMsg::Warning(WarningEvent {
                message: "second".into(),
            }))
            .await
            .unwrap();

        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].log_position, 1);
        assert_eq!(recorder.events().await.unwrap().len(), 2);
    }
}
