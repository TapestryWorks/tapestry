//! Queue of system reminders waiting to be recorded as events.

use std::sync::Mutex;

use tapestry_protocol::SystemReminder;

/// Reminders added by an agent or middleware hooks during a turn. Core drains
/// it at safe points and records one `SystemReminder` event per entry.
#[derive(Debug, Default)]
pub(crate) struct ReminderQueue {
    pending: Mutex<Vec<SystemReminder>>,
}

impl ReminderQueue {
    pub(crate) fn push(&self, reminder: SystemReminder) {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(reminder);
    }

    pub(crate) fn drain(&self) -> Vec<SystemReminder> {
        std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}
