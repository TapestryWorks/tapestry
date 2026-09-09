use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use tracing::{debug, warn};

use tapestry_protocol::{
    ErrorEvent, Event, EventMsg, Op, SessionConfiguredEvent, Submission, SubmissionId, ThreadId,
    TurnAbortedEvent, TurnId,
};

use crate::agent::AgentLoop;
use crate::config::ThreadConfig;
use crate::error::CoreError;
use crate::model::ModelClient;
use crate::tool::ToolRegistry;

const DEFAULT_CHANNEL_CAPACITY: usize = 64;

/// Handle for submitting operations and receiving events from a thread.
#[derive(Clone)]
pub struct ThreadHandle {
    submission_tx: mpsc::Sender<Submission>,
    event_rx: Arc<Mutex<mpsc::Receiver<Event>>>,
}

impl ThreadHandle {
    pub async fn submit(&self, op: Op) -> Result<SubmissionId, CoreError> {
        let submission = Submission::new(op);
        let id = submission.id;
        self.submission_tx
            .send(submission)
            .await
            .map_err(|_| CoreError::ChannelClosed)?;
        Ok(id)
    }

    pub async fn next_event(&self) -> Option<Event> {
        self.event_rx.lock().await.recv().await
    }

    pub async fn collect_events_for(&self, submission_id: SubmissionId) -> Vec<Event> {
        let mut events = Vec::new();
        while let Some(event) = self.next_event().await {
            let done = matches!(
                event.msg,
                EventMsg::TurnComplete(_)
                    | EventMsg::TurnAborted(_)
                    | EventMsg::Error(_)
                    | EventMsg::SessionConfigured(_)
            );
            let same = event.id == submission_id;
            events.push(event);
            if same && done {
                break;
            }
        }
        events
    }
}

/// A long-lived agent thread with SQ/EQ channels.
pub struct TapestryThread {
    thread_id: ThreadId,
    handle: ThreadHandle,
}

impl TapestryThread {
    pub fn spawn(model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> (Self, ThreadHandle) {
        let thread_id = ThreadId::new();
        let (submission_tx, submission_rx) = mpsc::channel(DEFAULT_CHANNEL_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(DEFAULT_CHANNEL_CAPACITY);

        let handle = ThreadHandle {
            submission_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
        };

        let worker_handle = handle.clone();
        tokio::spawn(async move {
            if let Err(err) =
                run_thread_loop(thread_id, submission_rx, event_tx, model, tools).await
            {
                warn!(?err, "thread loop exited with error");
            }
        });

        let thread = Self {
            thread_id,
            handle: worker_handle.clone(),
        };

        (thread, handle)
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    pub fn handle(&self) -> ThreadHandle {
        self.handle.clone()
    }
}

async fn run_thread_loop(
    thread_id: ThreadId,
    mut submission_rx: mpsc::Receiver<Submission>,
    event_tx: mpsc::Sender<Event>,
    model: Arc<dyn ModelClient>,
    tools: Arc<ToolRegistry>,
) -> Result<(), CoreError> {
    let mut config = ThreadConfig::default();
    let agent = AgentLoop::new(model, tools);
    let mut active_turn: Option<TurnId> = None;
    let mut configured = false;

    while let Some(submission) = submission_rx.recv().await {
        debug!(submission_id = %submission.id, "processing submission");

        let submission_id = submission.id;
        let result = process_submission(
            &agent,
            thread_id,
            &mut config,
            &mut configured,
            &mut active_turn,
            submission,
            &event_tx,
        )
        .await;

        if let Err(err) = result {
            let _ = event_tx
                .send(Event::new(
                    submission_id,
                    EventMsg::Error(ErrorEvent {
                        message: err.to_string(),
                    }),
                ))
                .await;
        }
    }

    Ok(())
}

async fn process_submission(
    agent: &AgentLoop,
    thread_id: ThreadId,
    config: &mut ThreadConfig,
    configured: &mut bool,
    active_turn: &mut Option<TurnId>,
    submission: Submission,
    event_tx: &mpsc::Sender<Event>,
) -> Result<(), CoreError> {
    match submission.op {
        Op::Configure {
            model,
            system_prompt,
        } => {
            config.model = model.clone();
            config.system_prompt = system_prompt;
            *configured = true;
            event_tx
                .send(Event::new(
                    submission.id,
                    EventMsg::SessionConfigured(SessionConfiguredEvent { thread_id, model }),
                ))
                .await
                .map_err(|_| CoreError::ChannelClosed)?;
        }
        Op::ThreadSettings { settings } => {
            config.apply_settings(&settings);
            if !*configured {
                *configured = true;
                event_tx
                    .send(Event::new(
                        submission.id,
                        EventMsg::SessionConfigured(SessionConfiguredEvent {
                            thread_id,
                            model: config.model.clone(),
                        }),
                    ))
                    .await
                    .map_err(|_| CoreError::ChannelClosed)?;
            }
        }
        Op::Interrupt => {
            let aborted = active_turn.take();
            event_tx
                .send(Event::new(
                    submission.id,
                    EventMsg::TurnAborted(TurnAbortedEvent {
                        turn_id: aborted,
                        reason: "interrupted by client".into(),
                    }),
                ))
                .await
                .map_err(|_| CoreError::ChannelClosed)?;
        }
        Op::StartTurn {
            user_message,
            turn_id,
        } => {
            if !*configured {
                return Err(CoreError::NotConfigured);
            }
            if active_turn.is_some() {
                return Err(CoreError::TurnInProgress);
            }

            let turn_id = turn_id.unwrap_or_else(TurnId::new);
            *active_turn = Some(turn_id);

            let run_result = agent
                .run_turn(submission.id, turn_id, user_message, config, event_tx)
                .await;

            active_turn.take();
            run_result?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tapestry_protocol::EventMsg;

    use super::*;
    use crate::model::MockModelClient;
    use crate::tool::{EchoTool, ToolRegistry};

    fn test_tools() -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        Arc::new(registry)
    }

    #[tokio::test]
    async fn thread_configure_and_turn() {
        let (_thread, handle) = TapestryThread::spawn(Arc::new(MockModelClient), test_tools());

        let config_id = handle
            .submit(Op::Configure {
                model: "mock-model".into(),
                system_prompt: Some("test".into()),
            })
            .await
            .unwrap();

        let config_events = handle.collect_events_for(config_id).await;
        assert!(matches!(
            config_events.last().map(|e| &e.msg),
            Some(EventMsg::SessionConfigured(_))
        ));

        let turn_id = handle
            .submit(Op::StartTurn {
                user_message: "hello".into(),
                turn_id: None,
            })
            .await
            .unwrap();

        let turn_events = handle.collect_events_for(turn_id).await;
        assert!(turn_events
            .iter()
            .any(|e| matches!(e.msg, EventMsg::AgentMessage(_))));
        assert!(turn_events
            .iter()
            .any(|e| matches!(e.msg, EventMsg::TurnComplete(_))));
    }

    #[tokio::test]
    async fn thread_rejects_turn_before_configure() {
        let (_thread, handle) = TapestryThread::spawn(Arc::new(MockModelClient), test_tools());

        let turn_id = handle
            .submit(Op::StartTurn {
                user_message: "hello".into(),
                turn_id: None,
            })
            .await
            .unwrap();

        // Give the worker a moment to emit the error event.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let events = handle.collect_events_for(turn_id).await;
        assert!(events.iter().any(|e| matches!(e.msg, EventMsg::Error(_))));
    }
}
