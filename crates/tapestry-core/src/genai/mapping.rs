use genai::chat::{
    ChatMessage, ChatRequest, ChatResponse, ContentPart, MessageContent, Tool,
    ToolCall as GenaiToolCall, ToolResponse, Usage,
};

use crate::error::CoreError;
use crate::prompt::{render_environment_context, render_system_reminder, render_user};
use tapestry_protocol::{
    HistoryItem, ModelRequest, ModelResponse, TokenUsage, ToolCall, ToolCallId, ToolSpec,
};

/// Convert a Tapestry [`ModelRequest`] into a genai [`ChatRequest`].
///
/// User names are rendered as a `"name: "` text prefix. Environment context and
/// system reminders become unnamed user messages. Tool call ids are sent back
/// exactly as the provider issued them.
pub fn model_request_to_chat_request(request: &ModelRequest) -> ChatRequest {
    let messages: Vec<ChatMessage> = request
        .history
        .iter()
        .map(history_item_to_message)
        .collect();

    let mut chat_req = ChatRequest::new(messages);
    if let Some(system) = &request.system_prompt {
        chat_req = chat_req.with_system(system.clone());
    }
    if !request.tools.is_empty() {
        chat_req = chat_req.with_tools(request.tools.iter().map(tool_spec_to_tool));
    }
    chat_req
}

fn history_item_to_message(item: &HistoryItem) -> ChatMessage {
    match item {
        HistoryItem::User { name, content } => {
            ChatMessage::user(render_user(name.as_deref(), content))
        }
        HistoryItem::EnvironmentContext(context) => {
            ChatMessage::user(render_environment_context(context))
        }
        HistoryItem::SystemReminder(reminder) => {
            ChatMessage::user(render_system_reminder(reminder))
        }
        HistoryItem::Assistant {
            content,
            tool_calls,
        } => {
            if tool_calls.is_empty() {
                return ChatMessage::assistant(content.clone().unwrap_or_default());
            }
            let mut parts = Vec::new();
            if let Some(text) = content.as_ref().filter(|t| !t.is_empty()) {
                parts.push(ContentPart::Text(text.clone()));
            }
            parts.extend(tool_calls.iter().map(|call| {
                ContentPart::ToolCall(GenaiToolCall {
                    call_id: call.id.as_str().to_string(),
                    fn_name: call.name.clone(),
                    fn_arguments: call.arguments.clone(),
                    thought_signatures: None,
                })
            }));
            ChatMessage::assistant(MessageContent::from_parts(parts))
        }
        HistoryItem::ToolResult(result) => ChatMessage::from(
            ToolResponse::new(result.call_id.as_str().to_string(), result.output.clone())
                .with_fn_name(result.tool_name.clone()),
        ),
    }
}

/// Convert a [`ToolSpec`] into a genai [`Tool`].
pub fn tool_spec_to_tool(spec: &ToolSpec) -> Tool {
    Tool::new(spec.name.clone())
        .with_description(spec.description.clone())
        .with_schema(spec.parameters.clone())
}

/// Convert a genai [`ChatResponse`] into Tapestry's [`ModelResponse`].
pub fn chat_response_to_model_response(response: ChatResponse) -> ModelResponse {
    let content = response.first_text().map(str::to_string);
    let tool_calls = response
        .tool_calls()
        .into_iter()
        .map(|call| {
            let id = if call.call_id.is_empty() {
                ToolCallId::generate()
            } else {
                ToolCallId::from(call.call_id.as_str())
            };
            ToolCall::new(id, call.fn_name.clone(), call.fn_arguments.clone())
        })
        .collect();

    ModelResponse {
        content,
        tool_calls,
        usage: usage_to_token_usage(&response.usage),
    }
}

fn non_negative(value: Option<i32>) -> u64 {
    value.unwrap_or(0).max(0) as u64
}

/// Normalize genai usage: genai's `prompt_tokens` includes cache reads and
/// writes, which are split out; `completion_tokens` already includes reasoning.
pub fn usage_to_token_usage(usage: &Usage) -> TokenUsage {
    let details = usage.prompt_tokens_details.as_ref();
    let cache_read = non_negative(details.and_then(|d| d.cached_tokens));
    let cache_write = non_negative(details.and_then(|d| d.cache_creation_tokens));
    TokenUsage {
        input_tokens: non_negative(usage.prompt_tokens)
            .saturating_sub(cache_read)
            .saturating_sub(cache_write),
        output_tokens: non_negative(usage.completion_tokens),
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
    }
}

/// Map genai errors into Tapestry [`CoreError::Model`].
pub fn map_genai_error(error: genai::Error) -> CoreError {
    CoreError::Model(error.to_string())
}

/// Build a simple user/system chat request for direct service calls.
pub fn simple_chat_request(system: Option<&str>, user: &str) -> ChatRequest {
    let mut chat_req = ChatRequest::new(vec![ChatMessage::user(user)]);
    if let Some(system) = system {
        chat_req = chat_req.with_system(system);
    }
    chat_req
}

/// Extract assistant text and token usage from a non-streaming chat response.
pub fn chat_text_and_tokens(response: &ChatResponse) -> (Option<String>, u32, u32) {
    let usage = &response.usage;
    (
        response.first_text().map(str::to_string),
        usage.prompt_tokens.unwrap_or(0).max(0) as u32,
        usage.completion_tokens.unwrap_or(0).max(0) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool as _;

    use genai::chat::PromptTokensDetails;
    use tapestry_protocol::{EnvironmentContext, SystemReminder, ToolResult};

    fn request(history: Vec<HistoryItem>, tools: Vec<ToolSpec>) -> ModelRequest {
        ModelRequest {
            system_prompt: Some("You are helpful.".into()),
            history,
            tools,
            iteration: 0,
        }
    }

    #[test]
    fn maps_system_prompt_and_named_user_prefix() {
        let chat_req = model_request_to_chat_request(&request(
            vec![
                HistoryItem::user(Some("alice".into()), "hi"),
                HistoryItem::user(Some("bob".into()), "yo"),
                HistoryItem::user(None, "plain"),
            ],
            vec![],
        ));
        assert_eq!(chat_req.system.as_deref(), Some("You are helpful."));
        assert_eq!(chat_req.messages.len(), 3);
        assert_eq!(chat_req.messages[0].content.first_text(), Some("alice: hi"));
        assert_eq!(chat_req.messages[1].content.first_text(), Some("bob: yo"));
        assert_eq!(chat_req.messages[2].content.first_text(), Some("plain"));
        assert!(chat_req.tools.is_none());
    }

    #[test]
    fn structured_items_map_to_unnamed_user_messages() {
        let context = EnvironmentContext::Available {
            date: "2026-11-05".into(),
            timezone: "UTC".into(),
        };
        let chat_req = model_request_to_chat_request(&request(
            vec![
                context.clone().into(),
                HistoryItem::system_reminder(SystemReminder::ToolLimitReached),
            ],
            vec![],
        ));
        assert_eq!(chat_req.messages.len(), 2);
        assert!(chat_req
            .messages
            .iter()
            .all(|m| matches!(m.role, genai::chat::ChatRole::User)));
        assert_eq!(
            chat_req.messages[0].content.first_text(),
            Some(render_environment_context(&context).as_str())
        );
        assert_eq!(
            chat_req.messages[1].content.first_text(),
            Some(crate::prompt::FORCED_FINISH_REMINDER)
        );
    }

    #[test]
    fn maps_tool_calls_and_results() {
        let call = ToolCall::new(
            ToolCallId::from("provider_call_9"),
            "echo",
            serde_json::json!({"input": "x"}),
        );
        let chat_req = model_request_to_chat_request(&request(
            vec![
                HistoryItem::user(None, "go"),
                HistoryItem::assistant(Some("calling".into()), vec![call.clone()]),
                ToolResult::completed(&call, "x").into(),
                HistoryItem::assistant(Some("done".into()), vec![]),
            ],
            vec![],
        ));
        assert_eq!(chat_req.messages.len(), 4);

        let calls = chat_req.messages[1].content.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "provider_call_9");
        assert_eq!(calls[0].fn_name, "echo");
        assert_eq!(chat_req.messages[1].content.first_text(), Some("calling"));

        let responses = chat_req.messages[2].content.tool_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].call_id, "provider_call_9");
        assert_eq!(responses[0].content, "x");

        assert_eq!(chat_req.messages[3].content.first_text(), Some("done"));
    }

    #[test]
    fn maps_tool_specs() {
        let spec = crate::tool::EchoTool.spec();
        let chat_req = model_request_to_chat_request(&request(
            vec![HistoryItem::user(None, "go")],
            vec![spec.clone()],
        ));
        let tools = chat_req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].description.as_deref(),
            Some(spec.description.as_str())
        );
        assert_eq!(tools[0].schema, Some(spec.parameters));
    }

    fn response(calls: Vec<(&str, &str)>, usage: Usage) -> ChatResponse {
        let mut parts = vec![ContentPart::Text("hi".into())];
        parts.extend(calls.into_iter().map(|(id, name)| {
            ContentPart::ToolCall(GenaiToolCall {
                call_id: id.into(),
                fn_name: name.into(),
                fn_arguments: serde_json::json!({"input": "a"}),
                thought_signatures: None,
            })
        }));
        ChatResponse {
            content: MessageContent::from_parts(parts),
            reasoning_content: None,
            model_iden: genai::ModelIden::new(genai::adapter::AdapterKind::OpenAI, "m"),
            provider_model_iden: genai::ModelIden::new(genai::adapter::AdapterKind::OpenAI, "m"),
            stop_reason: None,
            usage,
            captured_raw_body: None,
            response_id: None,
        }
    }

    fn usage(prompt: i32, completion: i32, read: Option<i32>, write: Option<i32>) -> Usage {
        Usage {
            prompt_tokens: Some(prompt),
            completion_tokens: Some(completion),
            prompt_tokens_details: (read.is_some() || write.is_some()).then(|| {
                PromptTokensDetails {
                    cached_tokens: read,
                    cache_creation_tokens: write,
                    ..Default::default()
                }
            }),
            ..Default::default()
        }
    }

    #[test]
    fn provider_call_id_is_reused() {
        let mapped = chat_response_to_model_response(response(
            vec![("c1", "echo")],
            usage(7, 3, None, None),
        ));
        assert_eq!(mapped.content.as_deref(), Some("hi"));
        assert_eq!(mapped.tool_calls.len(), 1);
        assert_eq!(mapped.tool_calls[0].id.as_str(), "c1");
        assert_eq!(mapped.tool_calls[0].name, "echo");
    }

    #[test]
    fn empty_provider_call_id_is_generated() {
        let mapped = chat_response_to_model_response(response(
            vec![("", "echo"), ("", "echo")],
            usage(1, 1, None, None),
        ));
        let ids: Vec<_> = mapped.tool_calls.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.iter().all(|id| id.starts_with("call_")));
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn total_input_includes_cache_read() {
        let mapped =
            chat_response_to_model_response(response(vec![], usage(100, 20, Some(60), Some(10))));
        assert_eq!(
            mapped.usage,
            TokenUsage {
                input_tokens: 30,
                output_tokens: 20,
                cache_read_tokens: 60,
                cache_write_tokens: 10,
            }
        );
    }

    #[test]
    fn provider_without_cache_reports_zero_cache_tokens() {
        let mapped = chat_response_to_model_response(response(vec![], usage(7, 3, None, None)));
        assert_eq!(
            mapped.usage,
            TokenUsage {
                input_tokens: 7,
                output_tokens: 3,
                ..Default::default()
            }
        );
    }

    #[test]
    fn output_tokens_keep_reasoning_and_bad_values_become_zero() {
        // genai already folds reasoning into `completion_tokens`.
        let mapped = chat_response_to_model_response(response(vec![], usage(5, 42, None, None)));
        assert_eq!(mapped.usage.output_tokens, 42);

        let odd = usage_to_token_usage(&Usage {
            prompt_tokens: Some(-4),
            completion_tokens: None,
            ..Default::default()
        });
        assert_eq!(odd, TokenUsage::default());

        let over = usage_to_token_usage(&usage(5, 0, Some(9), None));
        assert_eq!(over.input_tokens, 0);
    }

    #[test]
    fn maps_simple_chat_request() {
        let chat_req = simple_chat_request(Some("sys"), "user msg");
        assert_eq!(chat_req.system.as_deref(), Some("sys"));
        assert_eq!(chat_req.messages.len(), 1);
    }
}
