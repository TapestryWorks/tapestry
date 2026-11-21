mod config;

use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use tapestry_adapter_cli::{CliEventAdapter, CliOutput};
use tapestry_core::{
    EventSubscription, GenaiProviderConfig, OpenAiChatModel, TapestryThread, ToolRegistry,
};
use tapestry_protocol::{Command, EventAdapter, EventMsg, SubmissionId, ThreadSettings};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Agent name in .tapestry/agents or .agents. Without it, a built-in
    /// helpful-assistant system prompt is used.
    #[arg(short, long)]
    agent: Option<String>,

    /// User instruction for a single turn.
    #[arg(short, long)]
    prompt: String,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tapestry-cli: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<()> {
    ensure!(!args.prompt.trim().is_empty(), "prompt must not be empty");
    let cwd = std::env::current_dir().context("determine current working directory")?;
    let home = std::env::home_dir();
    let (provider, system_prompt) = config::load(&cwd, home.as_deref(), args.agent.as_deref())?;
    run_turn(
        provider,
        system_prompt,
        args.prompt,
        &mut io::stdout().lock(),
    )
    .await
}

async fn run_turn(
    provider: GenaiProviderConfig,
    system_prompt: String,
    prompt: String,
    output: &mut impl Write,
) -> Result<()> {
    let client = OpenAiChatModel::from_config(&provider).context("initialize chat model")?;
    let (_thread, handle) = TapestryThread::spawn(Arc::new(client), Arc::new(ToolRegistry::new()));
    let mut events = handle.subscribe(0).await?;
    let mut adapter = CliEventAdapter::new();

    let configure_id = handle
        .submit(Command::UpdateSettings {
            settings: ThreadSettings {
                model: Some(provider.model),
                system_prompt: Some(system_prompt),
                ..Default::default()
            },
        })
        .await?;
    ensure!(
        matches!(
            print_events(&mut events, &mut adapter, configure_id, output).await?,
            EventMsg::SessionConfigured(_)
        ),
        "agent did not acknowledge configuration"
    );

    let turn_id = handle
        .submit(Command::StartTurn {
            content: prompt,
            name: None,
            turn_id: None,
        })
        .await?;
    ensure!(
        matches!(
            print_events(&mut events, &mut adapter, turn_id, output).await?,
            EventMsg::TurnComplete(_)
        ),
        "agent did not complete the turn"
    );
    write_events(output, adapter.finish())
}

async fn print_events(
    events: &mut EventSubscription,
    adapter: &mut CliEventAdapter,
    submission: SubmissionId,
    output: &mut impl Write,
) -> Result<EventMsg> {
    while let Some(event) = events.next().await {
        let event = event.context("read agent event")?;
        if event.id != submission {
            continue;
        }
        write_events(output, adapter.adapt(&event))?;

        match event.msg {
            EventMsg::SessionConfigured(_) | EventMsg::TurnComplete(_) => return Ok(event.msg),
            EventMsg::Error(error) => bail!("agent reported an error: {}", error.message),
            EventMsg::TurnAborted(aborted) => match aborted.message {
                Some(message) => bail!("agent turn aborted ({:?}): {message}", aborted.reason),
                None => bail!("agent turn aborted ({:?})", aborted.reason),
            },
            _ => {}
        }
    }
    bail!("agent event stream closed before the submission finished")
}

fn write_events(output: &mut impl Write, entries: Vec<CliOutput>) -> Result<()> {
    for entry in entries {
        writeln!(output, "{entry}").context("write agent output")?;
    }
    output.flush().context("flush agent output")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;
    use tokio::time::timeout;

    use super::*;

    async fn run_against(responses: Vec<String>) -> (Result<()>, Vec<String>) {
        run_with_args(&["-a", "reviewer"], "act as a reviewer", responses).await
    }

    async fn run_with_args(
        extra_args: &[&str],
        expected_system_prompt: &'static str,
        responses: Vec<String>,
    ) -> (Result<()>, Vec<String>) {
        let args = Args::try_parse_from(
            ["tapestry-cli", "-p", "review this"]
                .into_iter()
                .chain(extra_args.iter().copied()),
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let config_dir = cwd.path().join(".tapestry");
        std::fs::create_dir_all(config_dir.join("agents")).unwrap();
        std::fs::write(
            config_dir.join("config.yaml"),
            format!(
                "base_url: http://{}/v1\napi_key: test-key\nmodel: test-model",
                listener.local_addr().unwrap()
            ),
        )
        .unwrap();
        std::fs::write(
            config_dir.join("agents/reviewer.yaml"),
            "system_prompt: act as a reviewer",
        )
        .unwrap();
        let (provider, system_prompt) =
            config::load(cwd.path(), None, args.agent.as_deref()).unwrap();
        let mut output = Vec::new();

        let (result, ()) = timeout(Duration::from_secs(10), async {
            tokio::join!(
                run_turn(provider, system_prompt, args.prompt, &mut output),
                serve_completion(listener, responses, expected_system_prompt)
            )
        })
        .await
        .unwrap();

        let events = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        (result, events)
    }

    fn http(status: &str, content_type: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn streams_one_prompt_and_prints_only_the_final_assistant_message() {
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"looks \"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"approved\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let (result, events) = run_against(vec![http("200 OK", "text/event-stream", body)]).await;
        result.unwrap();

        assert_eq!(events, ["looks approved"]);
    }

    #[tokio::test]
    async fn uses_the_default_system_prompt_without_an_agent() {
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let (result, events) = run_with_args(
            &[],
            config::DEFAULT_SYSTEM_PROMPT,
            vec![http("200 OK", "text/event-stream", body)],
        )
        .await;
        result.unwrap();
        assert_eq!(events, ["hi"]);
    }

    #[tokio::test]
    async fn prints_tool_use_tool_result_and_assistant_messages() {
        let tool_call = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let answer = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let (result, events) = run_against(vec![
            http("200 OK", "text/event-stream", tool_call),
            http("200 OK", "text/event-stream", answer),
        ])
        .await;
        result.unwrap();

        assert_eq!(events.len(), 3, "{events:?}");
        assert_eq!(events[0], "tool call: lookup()");
        assert!(
            events[1].starts_with("tool result: lookup (not found): "),
            "{events:?}"
        );
        assert_eq!(events[2], "done");
    }

    #[tokio::test]
    async fn model_errors_fail_the_command_with_the_error_message() {
        let (result, events) = run_against(vec![http(
            "400 Bad Request",
            "application/json",
            "{\"error\":{\"message\":\"bad request\"}}",
        )])
        .await;

        let error = result.unwrap_err().to_string();
        assert!(error.starts_with("agent reported an error: "), "{error}");
        assert!(events.is_empty());
    }

    async fn serve_completion(
        listener: TcpListener,
        responses: Vec<String>,
        expected_system_prompt: &str,
    ) {
        for http in responses {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            let mut line = String::new();
            socket.read_line(&mut line).await.unwrap();
            assert_eq!(line, "POST /v1/chat/completions HTTP/1.1\r\n");
            let mut content_length = None;
            let mut authorization = None;
            loop {
                line.clear();
                assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap();
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => content_length = Some(value.trim().parse().unwrap()),
                    "authorization" => authorization = Some(value.trim().to_owned()),
                    _ => {}
                }
            }
            assert_eq!(authorization.as_deref(), Some("Bearer test-key"));
            let mut body = vec![0; content_length.unwrap()];
            socket.read_exact(&mut body).await.unwrap();
            let payload: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(payload["model"], "test-model");
            assert_eq!(payload["stream"], true);
            let messages = payload["messages"].as_array().unwrap();
            assert!(messages.iter().any(|message| {
                message["role"] == "system" && message["content"] == expected_system_prompt
            }));
            assert!(messages.iter().any(|message| {
                message["role"] == "user" && message["content"] == "review this"
            }));

            socket.get_mut().write_all(http.as_bytes()).await.unwrap();
        }
    }
}
