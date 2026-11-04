//! Time context injected into history as a persistent `<environment_context>` entry.
//!
//! Unlike the per-request `reminder`, the entry is written into history and only
//! re-sent when the date or time zone changes.

use serde::{Deserialize, Serialize};

use crate::clock::{Clock, ClockReading};

/// What the model has already been told about the date and time zone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ContextState {
    /// Nothing sent yet, or history was rewritten: the next send is complete.
    #[default]
    Unsent,
    Sent {
        date: String,
        timezone: String,
    },
    /// The model was told the date is unavailable.
    Unavailable,
}

/// Decide whether a context entry is needed this turn.
///
/// Returns the text to append to history (if any) and updates `state` accordingly.
pub fn next_context_entry(
    state: &mut ContextState,
    enabled: bool,
    clock: &dyn Clock,
) -> Option<String> {
    if !enabled {
        return match state {
            ContextState::Sent { .. } => {
                *state = ContextState::Unavailable;
                Some(render_unavailable())
            }
            _ => None,
        };
    }

    match clock.now() {
        Ok(reading) => diff_reading(state, reading),
        Err(_) => match state {
            ContextState::Unavailable => None,
            _ => {
                *state = ContextState::Unavailable;
                Some(render_unavailable())
            }
        },
    }
}

fn diff_reading(state: &mut ContextState, reading: ClockReading) -> Option<String> {
    let ClockReading {
        local_date,
        timezone,
    } = reading;
    let (date, tz) = match state {
        ContextState::Sent {
            date: old_date,
            timezone: old_tz,
        } => {
            let date_changed = *old_date != local_date;
            let tz_changed = *old_tz != timezone;
            if !date_changed && !tz_changed {
                return None;
            }
            (
                date_changed.then_some(local_date.as_str()),
                tz_changed.then_some(timezone.as_str()),
            )
        }
        _ => (Some(local_date.as_str()), Some(timezone.as_str())),
    };
    let text = render_context(date, tz);
    *state = ContextState::Sent {
        date: local_date,
        timezone,
    };
    Some(text)
}

/// Render an `<environment_context>` fragment with only the given fields.
pub fn render_context(date: Option<&str>, timezone: Option<&str>) -> String {
    let mut out = String::from("<environment_context>\n");
    if let Some(date) = date {
        out.push_str(&format!("  <current_date>{date}</current_date>\n"));
    }
    if let Some(timezone) = timezone {
        out.push_str(&format!("  <timezone>{timezone}</timezone>\n"));
    }
    out.push_str("</environment_context>");
    out
}

pub fn render_unavailable() -> String {
    "<environment_context>\n  <current_date status=\"unavailable\" />\n</environment_context>"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FixedClock;

    fn step(state: &mut ContextState, enabled: bool, clock: &FixedClock) -> Option<String> {
        next_context_entry(state, enabled, clock)
    }

    #[test]
    fn first_send_is_complete_and_exact() {
        let clock = FixedClock::new("2026-11-05", "Asia/Shanghai");
        let mut state = ContextState::Unsent;
        let text = step(&mut state, true, &clock).unwrap();
        assert_eq!(
            text,
            "<environment_context>\n  <current_date>2026-11-05</current_date>\n  <timezone>Asia/Shanghai</timezone>\n</environment_context>"
        );
        assert_eq!(
            state,
            ContextState::Sent {
                date: "2026-11-05".into(),
                timezone: "Asia/Shanghai".into()
            }
        );
    }

    #[test]
    fn same_day_is_not_resent() {
        let clock = FixedClock::new("2026-11-05", "UTC");
        let mut state = ContextState::Unsent;
        assert!(step(&mut state, true, &clock).is_some());
        assert!(step(&mut state, true, &clock).is_none());
    }

    #[test]
    fn new_day_sends_only_date() {
        let clock = FixedClock::new("2026-11-04", "UTC");
        let mut state = ContextState::Unsent;
        step(&mut state, true, &clock);
        clock.set("2026-11-05", "UTC");
        let text = step(&mut state, true, &clock).unwrap();
        assert!(text.contains("<current_date>2026-11-05</current_date>"));
        assert!(!text.contains("timezone"));
    }

    #[test]
    fn timezone_change_sends_only_timezone() {
        let clock = FixedClock::new("2026-11-05", "UTC");
        let mut state = ContextState::Unsent;
        step(&mut state, true, &clock);
        clock.set("2026-11-05", "Asia/Shanghai");
        let text = step(&mut state, true, &clock).unwrap();
        assert!(text.contains("<timezone>Asia/Shanghai</timezone>"));
        assert!(!text.contains("current_date"));
    }

    #[test]
    fn unavailable_is_deduplicated_and_recovers_with_full_send() {
        let clock = FixedClock::new("2026-11-05", "UTC");
        let mut state = ContextState::Unsent;
        step(&mut state, true, &clock);
        clock.fail();
        let text = step(&mut state, true, &clock).unwrap();
        assert_eq!(text, render_unavailable());
        assert!(text.contains(r#"<current_date status="unavailable" />"#));
        assert!(step(&mut state, true, &clock).is_none());
        clock.set("2026-11-05", "UTC");
        let text = step(&mut state, true, &clock).unwrap();
        assert!(text.contains("current_date") && text.contains("timezone"));
    }

    #[test]
    fn clock_failure_on_first_turn_sends_unavailable() {
        let clock = FixedClock::new("2026-11-05", "UTC");
        clock.fail();
        let mut state = ContextState::Unsent;
        assert_eq!(step(&mut state, true, &clock), Some(render_unavailable()));
        assert_eq!(state, ContextState::Unavailable);
    }

    #[test]
    fn disabled_never_sent_emits_nothing_and_skips_clock() {
        let clock = FixedClock::new("2026-11-05", "UTC");
        clock.fail();
        let mut state = ContextState::Unsent;
        assert!(step(&mut state, false, &clock).is_none());
        assert_eq!(state, ContextState::Unsent);
    }

    #[test]
    fn disabled_after_sent_emits_unavailable_once() {
        let clock = FixedClock::new("2026-11-05", "UTC");
        let mut state = ContextState::Unsent;
        step(&mut state, true, &clock);
        assert_eq!(step(&mut state, false, &clock), Some(render_unavailable()));
        assert!(step(&mut state, false, &clock).is_none());
    }

    #[test]
    fn state_serde_roundtrip() {
        for state in [
            ContextState::Unsent,
            ContextState::Unavailable,
            ContextState::Sent {
                date: "2026-11-05".into(),
                timezone: "UTC".into(),
            },
        ] {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(serde_json::from_str::<ContextState>(&json).unwrap(), state);
        }
    }
}
