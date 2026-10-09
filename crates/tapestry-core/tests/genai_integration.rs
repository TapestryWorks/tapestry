//! Live integration tests for genai providers. Skipped unless env vars are set.

use tapestry_core::{GenaiChatService, GenaiProviderConfig};

#[tokio::test]
#[ignore = "requires OPENAI_API_KEY"]
async fn openai_live_chat() {
    if std::env::var("OPENAI_API_KEY").is_err() {
        eprintln!("skipping: OPENAI_API_KEY not set");
        return;
    }

    let service =
        GenaiChatService::from_config(&GenaiProviderConfig::new("gpt-4o-mini")).expect("client");
    let turn = service
        .chat_simple(Some("Reply briefly."), "Say hello in one word.")
        .await
        .expect("chat");
    assert!(turn.content.is_some());
}

#[tokio::test]
#[ignore = "requires local Ollama"]
async fn ollama_live_chat() {
    use genai::adapter::AdapterKind;

    let endpoint = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let service = GenaiChatService::from_config(
        &GenaiProviderConfig::new("ollama::llama3.2")
            .with_adapter(AdapterKind::Ollama)
            .with_endpoint(endpoint),
    )
    .expect("client");

    let turn = service
        .chat_simple(Some("Reply briefly."), "Say hello in one word.")
        .await
        .expect("chat");
    assert!(turn.content.is_some());
}
