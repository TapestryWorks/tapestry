use genai::chat::{ChatMessage, ChatRequest, ChatResponse};

use crate::error::CoreError;
use crate::model::{ModelRequest, ModelResponse, ModelToolCall};

/// Convert a Tapestry [`ModelRequest`] into a genai [`ChatRequest`].
pub fn model_request_to_chat_request(request: &ModelRequest) -> ChatRequest {
    let mut messages = Vec::new();

    if !request.tool_results.is_empty() {
        let summary = request
            .tool_results
            .iter()
            .map(|(name, output)| format!("{name}: {output}"))
            .collect::<Vec<_>>()
            .join("\n");
        messages.push(ChatMessage::assistant(format!("Tool results:\n{summary}")));
    }

    messages.push(ChatMessage::user(request.user_message.clone()));

    let mut chat_req = ChatRequest::new(messages);
    if let Some(system) = &request.system_prompt {
        chat_req = chat_req.with_system(system.clone());
    }
    chat_req
}

/// Convert a genai [`ChatResponse`] into Tapestry's [`ModelResponse`].
pub fn chat_response_to_model_response(response: ChatResponse) -> ModelResponse {
    let content = response.first_text().map(str::to_string);
    let tool_calls = response
        .tool_calls()
        .into_iter()
        .map(|call| ModelToolCall {
            name: call.fn_name.clone(),
            arguments: call.fn_arguments.clone(),
        })
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

    #[test]
    fn maps_model_request_with_system_and_tools() {
        let request = ModelRequest {
            system_prompt: Some("You are helpful.".into()),
            user_message: "hello".into(),
            tool_results: vec![("echo".into(), "hi".into())],
            iteration: 1,
        };

        let chat_req = model_request_to_chat_request(&request);
        assert_eq!(chat_req.system.as_deref(), Some("You are helpful."));
        assert_eq!(chat_req.messages.len(), 2);
        assert!(chat_req.messages[0]
            .content
            .first_text()
            .unwrap()
            .contains("echo: hi"));
        assert_eq!(chat_req.messages[1].content.first_text(), Some("hello"));
    }

    #[test]
    fn maps_simple_chat_request() {
        let chat_req = simple_chat_request(Some("sys"), "user msg");
        assert_eq!(chat_req.system.as_deref(), Some("sys"));
        assert_eq!(chat_req.messages.len(), 1);
    }
}
