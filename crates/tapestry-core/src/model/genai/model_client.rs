use async_trait::async_trait;
use futures_util::StreamExt;
use genai::adapter::AdapterKind;
use genai::chat::{ChatOptions, ChatStream};
use std::sync::Arc;

use super::config::{parse_model_name, GenaiProviderConfig};
use super::mapping::{
    chat_response_to_model_response, chat_stream_event_to_item, map_genai_error,
    model_request_to_chat_request_with_formatter,
};
use super::service::GenaiChatService;
use crate::error::CoreError;
use crate::formatter::{Formatter, OpenAiFormatter};
use crate::model::{ChatModel, ModelStream, ModelStreamSource};
use tapestry_protocol::{ModelRequest, ModelResponse, ModelStreamItem};

/// [`ChatModel`] backed by the [`genai`](https://crates.io/crates/genai) crate.
#[derive(Clone)]
pub struct GenaiModelClient {
    service: GenaiChatService,
    formatter: Arc<dyn Formatter>,
}

impl GenaiModelClient {
    pub fn from_config(config: &GenaiProviderConfig) -> Result<Self, CoreError> {
        Ok(Self {
            service: GenaiChatService::from_config(config)?,
            formatter: Arc::new(OpenAiFormatter),
        })
    }

    pub fn from_model(model: impl Into<String>) -> Result<Self, CoreError> {
        Ok(Self {
            service: GenaiChatService::from_model(model)?,
            formatter: Arc::new(OpenAiFormatter),
        })
    }

    /// Use a custom event-to-message strategy for every request.
    pub fn with_formatter(mut self, formatter: Arc<dyn Formatter>) -> Self {
        self.formatter = formatter;
        self
    }

    pub(crate) fn from_fixed_adapter(
        config: &GenaiProviderConfig,
        adapter: AdapterKind,
    ) -> Result<Self, CoreError> {
        let parsed = parse_model_name(&config.model);
        let namespace = adapter.as_lower_str();
        if parsed.name.is_empty() || parsed.name.contains("::") {
            return Err(CoreError::Model("invalid OpenAI model name".into()));
        }
        if config.adapter.is_some_and(|kind| kind != adapter)
            || parsed
                .namespace
                .as_deref()
                .is_some_and(|ns| ns != namespace)
        {
            return Err(CoreError::Model(format!(
                "model configuration must use the {namespace} adapter"
            )));
        }

        // A namespace also lets genai resolve model names it cannot infer.
        let mut config = config.clone().with_adapter(adapter);
        config.model = format!("{namespace}::{}", parsed.name);
        Self::from_config(&config)
    }

    pub fn service(&self) -> &GenaiChatService {
        &self.service
    }
}

#[async_trait]
impl ChatModel for GenaiModelClient {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
        let chat_req =
            model_request_to_chat_request_with_formatter(&request, self.formatter.as_ref());
        let response = self
            .service
            .client()
            .exec_chat(self.service.model(), chat_req, None)
            .await
            .map_err(map_genai_error)?;
        Ok(chat_response_to_model_response(response))
    }

    /// Streams text chunks as deltas; the captured end event supplies the full
    /// content, tool calls and usage.
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, CoreError> {
        let chat_req =
            model_request_to_chat_request_with_formatter(&request, self.formatter.as_ref());
        let options = ChatOptions::default()
            .with_capture_content(true)
            .with_capture_tool_calls(true)
            .with_capture_usage(true);
        let response = self
            .service
            .client()
            .exec_chat_stream(self.service.model(), chat_req, Some(&options))
            .await
            .map_err(map_genai_error)?;
        Ok(ModelStream::from_source(Box::new(GenaiStreamSource {
            stream: response.stream,
        })))
    }
}

struct GenaiStreamSource {
    stream: ChatStream,
}

#[async_trait]
impl ModelStreamSource for GenaiStreamSource {
    async fn next(&mut self) -> Option<Result<ModelStreamItem, CoreError>> {
        loop {
            let Some(event) = self.stream.next().await else {
                return Some(Err(CoreError::Model(
                    "genai stream ended before its end event".into(),
                )));
            };
            match event {
                Ok(event) => {
                    if let Some(item) = chat_stream_event_to_item(event) {
                        return Some(Ok(item));
                    }
                }
                Err(error) => return Some(Err(map_genai_error(error))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tapestry_protocol::{Event, EventMsg, SubmissionId, ThreadId, UserMessageEvent};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;
    use tokio::time::timeout;

    use super::*;
    use crate::formatter::Formatter;
    use crate::{OpenAiChatModel, OpenAiResponseModel, ResponseModel};

    fn request() -> ModelRequest {
        ModelRequest {
            system_prompt: None,
            events: vec![Event {
                id: SubmissionId::new(),
                log_position: 1,
                thread_id: ThreadId::new(),
                turn_id: None,
                timestamp_ms: 0,
                msg: EventMsg::UserMessage(UserMessageEvent {
                    name: None,
                    content: "hello".into(),
                }),
            }],
            tools: vec![],
            iteration: 0,
        }
    }

    async fn serve_once(
        path: &'static str,
        input_field: &'static str,
        content_type: &'static str,
        body: &'static str,
        expected_input: &'static str,
    ) -> (GenaiProviderConfig, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            let mut line = String::new();
            socket.read_line(&mut line).await.unwrap();
            assert_eq!(line.trim_end(), format!("POST {path} HTTP/1.1"));

            let mut content_length = None;
            loop {
                line.clear();
                assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        content_length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
            }
            let mut request_body = vec![0; content_length.unwrap()];
            socket.read_exact(&mut request_body).await.unwrap();
            let payload: serde_json::Value = serde_json::from_slice(&request_body).unwrap();
            assert!(payload[input_field].is_array(), "{payload}");
            assert_eq!(payload["model"], "test-model");
            assert!(
                payload[input_field].to_string().contains(expected_input),
                "{payload}"
            );
            let http = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                    body.len()
                                );
            socket.get_mut().write_all(http.as_bytes()).await.unwrap();
        });

        let config = GenaiProviderConfig::new("test-model")
            .with_endpoint(format!("http://{address}/v1"))
            .with_api_key("test-key");
        (config, server)
    }

    struct MarkerFormatter;

    impl Formatter for MarkerFormatter {
        fn format(&self, _events: &[Event]) -> Vec<genai::chat::ChatMessage> {
            vec![genai::chat::ChatMessage::user("custom formatter marker")]
        }
    }

    #[test]
    fn model_client_clone_shares_the_injected_formatter() {
        let config = GenaiProviderConfig::new("test-model")
            .with_endpoint("http://127.0.0.1:1/v1")
            .with_api_key("test-key");
        let client = GenaiModelClient::from_config(&config)
            .unwrap()
            .with_formatter(Arc::new(MarkerFormatter));
        let clone = client.clone();
        assert!(Arc::ptr_eq(&client.formatter, &clone.formatter));
    }

    #[tokio::test]
    async fn openai_chat_uses_chat_completions_endpoint() {
        let (config, server) = serve_once(
                                "/v1/chat/completions",
                                "messages",
                                "application/json",
                                r#"{"model":"test-model","choices":[{"message":{"role":"assistant","content":"hello from chat"}}]}"#,
                                "hello",
                            )
                            .await;
        let model = OpenAiChatModel::from_config(&config).unwrap();
        let response = timeout(Duration::from_secs(5), model.complete(request()))
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(
            response.unwrap().content.as_deref(),
            Some("hello from chat")
        );
    }

    #[tokio::test]
    async fn openai_response_uses_responses_endpoint() {
        let (config, server) = serve_once(
            "/v1/responses",
            "input",
            "application/json",
            r#"{"id":"resp_test","model":"test-model","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello from responses"}]}]}"#,
            "hello",
        )
        .await;
        let model = OpenAiResponseModel::from_config(&config).unwrap();
        let response = timeout(Duration::from_secs(5), model.respond(request()))
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(
            response.unwrap().content.as_deref(),
            Some("hello from responses")
        );
    }

    #[tokio::test]
    async fn openai_response_stream_uses_responses_endpoint() {
        let (config, server) = serve_once(
                                "/v1/responses",
                                "input",
                                "text/event-stream",
                                concat!(
                                    "event: response.output_text.delta\n",
                                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello from responses\"}\n\n",
                                    "event: response.completed\n",
                                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"model\":\"test-model\",\"status\":\"completed\"}}\n\n",
                                ),
                                "hello",
                            )
                            .await;
        let model = OpenAiResponseModel::from_config(&config).unwrap();
        let stream = timeout(Duration::from_secs(5), model.stream(request()))
            .await
            .unwrap();
        timeout(Duration::from_secs(5), async {
            let mut stream = stream.unwrap();
            assert_eq!(
                stream.next().await.unwrap().unwrap(),
                ModelStreamItem::TextDelta {
                    text: "hello from responses".into(),
                }
            );
            let ModelStreamItem::Completed { response } = stream.next().await.unwrap().unwrap()
            else {
                panic!("expected completed response");
            };
            assert_eq!(response.content.as_deref(), Some("hello from responses"));
            assert!(stream.next().await.is_none());
            server.await.unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn custom_formatter_is_used_for_chat_completion_and_stream() {
        let body = r#"{"model":"test-model","choices":[{"message":{"role":"assistant","content":"chat marker response"}}]}"#;
        let (config, server) = serve_once(
            "/v1/chat/completions",
            "messages",
            "application/json",
            body,
            "custom formatter marker",
        )
        .await;
        let model = OpenAiChatModel::from_config(&config)
            .unwrap()
            .with_formatter(Arc::new(MarkerFormatter));
        let response = model.complete(request()).await.unwrap();
        server.await.unwrap();
        assert_eq!(response.content.as_deref(), Some("chat marker response"));

        let (config, server) = serve_once(
            "/v1/chat/completions",
            "messages",
            "text/event-stream",
            concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"chat stream marker\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n",
            ),
            "custom formatter marker",
        )
        .await;
        let model = OpenAiChatModel::from_config(&config)
            .unwrap()
            .with_formatter(Arc::new(MarkerFormatter));
        let mut stream = model.stream(request()).await.unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ModelStreamItem::TextDelta {
                text: "chat stream marker".into()
            }
        );
        let ModelStreamItem::Completed { response } = stream.next().await.unwrap().unwrap() else {
            panic!("expected completed response");
        };
        assert_eq!(response.content.as_deref(), Some("chat stream marker"));
        assert!(stream.next().await.is_none());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn custom_formatter_is_used_for_responses_completion_and_stream() {
        let (config, server) = serve_once(
            "/v1/responses",
            "input",
            "application/json",
            r#"{"id":"resp_custom","model":"test-model","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"responses marker response"}]}]}"#,
            "custom formatter marker",
        )
        .await;
        let model = OpenAiResponseModel::from_config(&config)
            .unwrap()
            .with_formatter(Arc::new(MarkerFormatter));
        let response = model.respond(request()).await.unwrap();
        server.await.unwrap();
        assert_eq!(
            response.content.as_deref(),
            Some("responses marker response")
        );

        let (config, server) = serve_once(
            "/v1/responses",
            "input",
            "text/event-stream",
            concat!(
                "event: response.output_text.delta\n",
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"responses stream marker\"}\n\n",
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_custom\",\"model\":\"test-model\",\"status\":\"completed\"}}\n\n",
            ),
            "custom formatter marker",
        )
        .await;
        let model = OpenAiResponseModel::from_config(&config)
            .unwrap()
            .with_formatter(Arc::new(MarkerFormatter));
        let mut stream = model.stream(request()).await.unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ModelStreamItem::TextDelta {
                text: "responses stream marker".into()
            }
        );
        let ModelStreamItem::Completed { response } = stream.next().await.unwrap().unwrap() else {
            panic!("expected completed response");
        };
        assert_eq!(response.content.as_deref(), Some("responses stream marker"));
        assert!(stream.next().await.is_none());
        server.await.unwrap();
    }

    #[test]
    fn openai_models_reject_conflicting_routes() {
        assert!(OpenAiChatModel::from_model("openai_resp::gpt-5-mini").is_err());
        assert!(OpenAiResponseModel::from_model("openai_resp::gpt-5-mini").is_ok());
        let config = GenaiProviderConfig::new("gpt-4o-mini").with_adapter(AdapterKind::OpenAI);
        assert!(OpenAiResponseModel::from_config(&config).is_err());
        assert!(OpenAiChatModel::from_config(&config).is_ok());
    }
}
