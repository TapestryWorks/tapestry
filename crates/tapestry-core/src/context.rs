//! Decides when an [`EnvironmentContext`] entry is appended to history.
//!
//! The baseline is the last `EnvironmentContext` already in history, so
//! rewriting history needs no extra bookkeeping. Every entry is a complete
//! snapshot.

use tapestry_protocol::{ConversationItem, EnvironmentContext};

use crate::clock::Clock;

/// Return the entry to append this turn, if any.
pub fn next_context_entry(
    history: &[ConversationItem],
    enabled: bool,
    clock: &dyn Clock,
) -> Option<EnvironmentContext> {
    let baseline = history.iter().rev().find_map(|item| match item {
        ConversationItem::EnvironmentContext(context) => Some(context),
        _ => None,
    });

    if !enabled {
        return match baseline {
            Some(EnvironmentContext::Available { .. }) => Some(EnvironmentContext::Unavailable),
            _ => None,
        };
    }

    match clock.now() {
        Ok(reading) => {
            let current = EnvironmentContext::Available {
                date: reading.local_date,
                timezone: reading.timezone,
            };
            (baseline != Some(&current)).then_some(current)
        }
        Err(_) => match baseline {
            Some(EnvironmentContext::Unavailable) => None,
            _ => Some(EnvironmentContext::Unavailable),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FixedClock;

    fn available(date: &str, tz: &str) -> EnvironmentContext {
        EnvironmentContext::Available {
            date: date.into(),
            timezone: tz.into(),
        }
    }

    fn with(context: Option<EnvironmentContext>) -> Vec<ConversationItem> {
        let mut history = vec![ConversationItem::user(None, "earlier")];
        history.extend(context.map(ConversationItem::from));
        history.push(ConversationItem::user(None, "later"));
        history
    }

    fn working() -> FixedClock {
        FixedClock::new("2026-11-05", "UTC")
    }

    fn failing() -> FixedClock {
        let clock = working();
        clock.fail();
        clock
    }

    fn next(
        baseline: Option<EnvironmentContext>,
        enabled: bool,
        clock: &FixedClock,
    ) -> Option<EnvironmentContext> {
        next_context_entry(&with(baseline), enabled, clock)
    }

    #[test]
    fn no_baseline() {
        assert_eq!(next(None, false, &failing()), None);
        assert_eq!(
            next(None, true, &working()),
            Some(available("2026-11-05", "UTC"))
        );
        assert_eq!(
            next(None, true, &failing()),
            Some(EnvironmentContext::Unavailable)
        );
    }

    #[test]
    fn available_baseline() {
        let base = || Some(available("2026-11-05", "UTC"));
        assert_eq!(
            next(base(), false, &working()),
            Some(EnvironmentContext::Unavailable)
        );
        assert_eq!(next(base(), true, &working()), None);
        assert_eq!(
            next(base(), true, &FixedClock::new("2026-11-06", "UTC")),
            Some(available("2026-11-06", "UTC"))
        );
        assert_eq!(
            next(
                base(),
                true,
                &FixedClock::new("2026-11-05", "Asia/Shanghai")
            ),
            Some(available("2026-11-05", "Asia/Shanghai"))
        );
        assert_eq!(
            next(base(), true, &failing()),
            Some(EnvironmentContext::Unavailable)
        );
    }

    #[test]
    fn unavailable_baseline() {
        let base = || Some(EnvironmentContext::Unavailable);
        assert_eq!(next(base(), false, &working()), None);
        assert_eq!(
            next(base(), true, &working()),
            Some(available("2026-11-05", "UTC"))
        );
        assert_eq!(next(base(), true, &failing()), None);
    }

    #[test]
    fn last_entry_is_the_baseline() {
        let history = vec![
            available("2026-11-04", "UTC").into(),
            available("2026-11-05", "UTC").into(),
            ConversationItem::user(None, "x"),
        ];
        assert_eq!(next_context_entry(&history, true, &working()), None);
    }

    #[test]
    fn replaced_history_without_context_resends_in_full() {
        assert_eq!(
            next_context_entry(&[], true, &working()),
            Some(available("2026-11-05", "UTC"))
        );
    }

    #[test]
    fn replaced_history_with_context_does_not_resend() {
        let history = vec![available("2026-11-05", "UTC").into()];
        assert_eq!(next_context_entry(&history, true, &working()), None);
    }

    #[test]
    fn new_day_sends_full_snapshot() {
        let history = vec![available("2026-11-04", "Asia/Shanghai").into()];
        let clock = FixedClock::new("2026-11-05", "Asia/Shanghai");
        assert_eq!(
            next_context_entry(&history, true, &clock),
            Some(available("2026-11-05", "Asia/Shanghai"))
        );
    }

    #[test]
    fn disabled_skips_the_clock() {
        // A failing clock must not matter when disabled.
        assert_eq!(next(None, false, &failing()), None);
    }
}
