//! Shared text rendering helpers used when preparing model messages.

use tapestry_protocol::{EnvironmentContext, SystemReminder};

/// Render a user message: `"{name}: {content}"`, or the bare content.
pub fn render_user(name: Option<&str>, content: &str) -> String {
    match name {
        Some(name) => format!("{name}: {content}"),
        None => content.to_string(),
    }
}

/// Render an `<environment_context>` fragment.
pub fn render_environment_context(context: &EnvironmentContext) -> String {
    match context {
        EnvironmentContext::Available { date, timezone } => format!(
            "<environment_context>\n  <current_date>{date}</current_date>\n  <timezone>{timezone}</timezone>\n</environment_context>"
        ),
        EnvironmentContext::Unavailable => {
            "<environment_context>\n  <current_date status=\"unavailable\" />\n</environment_context>"
                .to_string()
        }
    }
}

/// Text of [`SystemReminder::ToolLimitReached`].
pub const FORCED_FINISH_REMINDER: &str =
    "The tool call limit has been reached. Do not call any tools; summarize your progress and answer now.";

pub fn render_system_reminder(reminder: &SystemReminder) -> String {
    match reminder {
        SystemReminder::ToolLimitReached => FORCED_FINISH_REMINDER.to_string(),
        SystemReminder::Custom { text } => text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_with_and_without_name() {
        assert_eq!(render_user(Some("alice"), "hi"), "alice: hi");
        assert_eq!(render_user(None, "hi"), "hi");
        assert_eq!(render_user(None, ""), "");
        assert_eq!(render_user(Some("bob"), ""), "bob: ");
    }

    #[test]
    fn available_context_has_date_and_timezone() {
        let text = render_environment_context(&EnvironmentContext::Available {
            date: "2026-11-05".into(),
            timezone: "Asia/Shanghai".into(),
        });
        assert_eq!(
            text,
            "<environment_context>\n  <current_date>2026-11-05</current_date>\n  <timezone>Asia/Shanghai</timezone>\n</environment_context>"
        );
    }

    #[test]
    fn unavailable_context_marks_date_unavailable() {
        let text = render_environment_context(&EnvironmentContext::Unavailable);
        assert_eq!(
            text,
            "<environment_context>\n  <current_date status=\"unavailable\" />\n</environment_context>"
        );
    }

    #[test]
    fn tool_limit_reminder_text() {
        assert_eq!(
            render_system_reminder(&SystemReminder::ToolLimitReached),
            "The tool call limit has been reached. Do not call any tools; summarize your progress and answer now."
        );
    }

    #[test]
    fn custom_reminder_is_rendered_verbatim() {
        assert_eq!(
            render_system_reminder(&SystemReminder::custom("be brief")),
            "be brief"
        );
    }
}
