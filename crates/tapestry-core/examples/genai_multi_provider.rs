//! Demonstrate switching between cloud and local providers via genai.
//!
//! Run (requires API keys in the environment for cloud models):
//! ```bash
//! cargo run -p tapestry-core --features genai --example genai_multi_provider
//! ```
//!
//! Environment variables:
//! - `OPENAI_API_KEY` — for `gpt-4o-mini`
//! - `OLLAMA_HOST` — optional; defaults to local Ollama at `http://127.0.0.1:11434`

use genai::adapter::AdapterKind;
use genai::chat::printer::{print_chat_stream, PrintChatStreamOptions};

use tapestry_core::{default_api_key_env, GenaiChatService, GenaiProviderConfig};

const QUESTION: &str = "Reply with one short sentence: what is Tapestry?";

struct ProviderDemo {
    label: &'static str,
    config: GenaiProviderConfig,
    env_gate: Option<&'static str>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let demos = [
        ProviderDemo {
            label: "OpenAI (cloud, simple model name)",
            config: GenaiProviderConfig::new("gpt-4o-mini"),
            env_gate: default_api_key_env("gpt-4o-mini"),
        },
        ProviderDemo {
            label: "OpenAI (cloud, namespace::model)",
            config: GenaiProviderConfig::new("openai::gpt-4o-mini"),
            env_gate: default_api_key_env("openai::gpt-4o-mini"),
        },
        ProviderDemo {
            label: "Ollama (local, namespace::model)",
            config: GenaiProviderConfig::new("ollama::llama3.2")
                .with_adapter(AdapterKind::Ollama)
                .with_endpoint(
                    std::env::var("OLLAMA_HOST")
                        .unwrap_or_else(|_| "http://127.0.0.1:11434".into()),
                ),
            env_gate: None,
        },
    ];

    let print_options = PrintChatStreamOptions::from_print_events(false);

    for demo in demos {
        if let Some(env_name) = demo.env_gate {
            if std::env::var(env_name).is_err() {
                println!(
                    "===== Skipping {} (env var not set: {}) =====\n",
                    demo.label, env_name
                );
                continue;
            }
        }

        println!("===== {} =====", demo.label);
        let service = GenaiChatService::from_config(&demo.config)?;

        println!("Model: {}", service.model());
        println!("Question: {QUESTION}");

        println!("\n--- Non-streaming ---");
        let turn = service
            .chat_simple(Some("Answer in one sentence."), QUESTION)
            .await?;
        println!("{}", turn.content.unwrap_or_else(|| "(no content)".into()));

        println!("\n--- Streaming ---");
        let stream = service
            .chat_simple_stream(Some("Answer in one sentence."), QUESTION)
            .await?;
        print_chat_stream(stream.into_inner(), Some(&print_options)).await?;
        println!();
    }

    Ok(())
}
