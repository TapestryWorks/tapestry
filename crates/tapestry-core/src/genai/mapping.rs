use genai::chat::{
    ChatMessage, ChatRequest, ChatResponse, ContentPart, MessageContent, Tool,
    ToolCall as GenaiToolCall, ToolResponse,
};

use crate::error::CoreError;
use crate::history::{render_user_content, HistoryItem, ToolCall};
use crate::model::{ModelRequest, ModelResponse};
use crate::tool::ToolSpec;

/// Convert a Tapestry [`ModelRequest`] into a genai [`ChatRequest`].
///
/// User names are rendered as a `"name: "` text prefix; tool call ids are
/// sent as the Tapestry [`ToolCallId`] string so calls and results correlate.
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
            ChatMessage::user(render_user_content(name.as_deref(), content))
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
                    call_id: call.id.to_string(),
                    fn_name: call.name.clone(),
                    fn_arguments: call.arguments.clone(),
                    thought_signatures: None,
                })
            }));
            ChatMessage::assistant(MessageContent::from_parts(parts))
        }
        HistoryItem::ToolResult {
            call_id,
            tool_name,
            output,
            ..
        } => ChatMessage::from(
            ToolResponse::new(call_id.to_string(), output.clone()).with_fn_name(tool_name.clone()),
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
        .map(|call| ToolCall::new(call.fn_name.clone(), call.fn_arguments.clone()))
        .collect();

    let usage = &response.usage;
    ModelResponse {
        content,
        tool_calls,
        input_tokens: usage.prompt_tokens.unwrap_or(0).max(0) as u32,
        output_tokens: usage.completion_tokens.unwrap_or(0).max(0) as u32,
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

    use tapestry_protocol::ToolCallStatus;

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
    fn maps_tool_calls_and_results() {
        let call = ToolCall::new("echo", serde_json::json!({"input": "x"}));
        let chat_req = model_request_to_chat_request(&request(
            vec![
                HistoryItem::user(None, "go"),
                HistoryItem::assistant(Some("calling".into()), vec![call.clone()]),
                HistoryItem::tool_result(call.id, "echo", "x", ToolCallStatus::Completed),
                HistoryItem::assistant(Some("done".into()), vec![]),
            ],
            vec![],
        ));
        assert_eq!(chat_req.messages.len(), 4);

        let calls = chat_req.messages[1].content.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, call.id.to_string());
        assert_eq!(calls[0].fn_name, "echo");
        assert_eq!(chat_req.messages[1].content.first_text(), Some("calling"));

        let responses = chat_req.messages[2].content.tool_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].call_id, call.id.to_string());
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

    #[test]
    fn maps_usage_and_tool_calls_from_response() {
        use genai::chat::{ChatResponse, MessageContent, Usage};
        let response = ChatResponse {
            content: MessageContent::from_parts(vec![
                ContentPart::Text("hi".into()),
                ContentPart::ToolCall(GenaiToolCall {
                    call_id: "c1".into(),
                    fn_name: "echo".into(),
                    fn_arguments: serde_json::json!({"input": "a"}),
                    thought_signatures: None,
                }),
            ]),
            reasoning_content: None,
            model_iden: genai::ModelIden::new(genai::adapter::AdapterKind::OpenAI, "m"),
            provider_model_iden: genai::ModelIden::new(genai::adapter::AdapterKind::OpenAI, "m"),
            stop_reason: None,
            usage: Usage {
                prompt_tokens: Some(7),
                completion_tokens: Some(3),
                ..Default::default()
            },
            captured_raw_body: None,
            response_id: None,
        };
        let mapped = chat_response_to_model_response(response);
        assert_eq!(mapped.content.as_deref(), Some("hi"));
        assert_eq!(mapped.input_tokens, 7);
        assert_eq!(mapped.output_tokens, 3);
        assert_eq!(mapped.tool_calls.len(), 1);
        assert_eq!(mapped.tool_calls[0].name, "echo");
    }

    #[test]
    fn maps_simple_chat_request() {
        let chat_req = simple_chat_request(Some("sys"), "user msg");
        assert_eq!(chat_req.system.as_deref(), Some("sys"));
        assert_eq!(chat_req.messages.len(), 1);
    }
}
