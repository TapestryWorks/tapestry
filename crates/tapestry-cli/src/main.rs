mod config;

use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use tapestry_core::{
    EventSubscription, GenaiProviderConfig, OpenAiChatModel, TapestryThread, ToolRegistry,
};
use tapestry_protocol::{Command, EventMsg, SubmissionId, ThreadSettings};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Agent name in .tapestry/agents.
    #[arg(short, long)]
    agent: String,

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
    let (provider, system_prompt) = config::load(&cwd, home.as_deref(), &args.agent)?;
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
            print_events(&mut events, configure_id, output).await?,
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
            print_events(&mut events, turn_id, output).await?,
            EventMsg::TurnComplete(_)
        ),
        "agent did not complete the turn"
    );
    Ok(())
}

async fn print_events(
    events: &mut EventSubscription,
    submission: SubmissionId,
    output: &mut impl Write,
) -> Result<EventMsg> {
    while let Some(event) = events.next().await {
        let event = event.context("read agent event")?;
        if event.id != submission {
            continue;
        }
        serde_json::to_writer(&mut *output, &event).context("write agent event")?;
        writeln!(output).context("write event newline")?;
        output.flush().context("flush agent event")?;

        match event.msg {
            EventMsg::SessionConfigured(_) | EventMsg::TurnComplete(_) => return Ok(event.msg),
            EventMsg::Error(_) => bail!("agent reported an error; see the error event"),
            EventMsg::TurnAborted(_) => bail!("agent turn aborted; see the turn_aborted event"),
            _ => {}
        }
    }
    bail!("agent event stream closed before the submission finished")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;
    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn streams_one_prompt_to_the_configured_endpoint_and_prints_events() {
        let args =
            Args::try_parse_from(["tapestry-cli", "-a", "reviewer", "-p", "review this"]).unwrap();
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
        let (provider, system_prompt) = config::load(cwd.path(), None, &args.agent).unwrap();
        let mut output = Vec::new();

        let (_, ()) = timeout(Duration::from_secs(10), async {
            tokio::join!(
                async {
                    run_turn(provider, system_prompt, args.prompt, &mut output)
                        .await
                        .unwrap();
                },
                serve_completion(listener)
            )
        })
        .await
        .unwrap();

        let events: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events[0]["msg"]["type"], "session_configured");
        assert!(events.iter().any(|event| {
            event["msg"]["type"] == "assistant_message_delta" && event["msg"]["delta"] == "approved"
        }));
        assert_eq!(events.last().unwrap()["msg"]["type"], "turn_complete");
    }

    async fn serve_completion(listener: TcpListener) {
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
            message["role"] == "system" && message["content"] == "act as a reviewer"
        }));
        assert!(messages
            .iter()
            .any(|message| { message["role"] == "user" && message["content"] == "review this" }));

        let response = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"approved\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let http = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
            response.len()
        );
        socket.get_mut().write_all(http.as_bytes()).await.unwrap();
    }
}
