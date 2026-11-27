//! Opt-in local tools. The working directory is a base path, not a sandbox.
//! These tools run with the host process's filesystem and execution permissions.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tapestry_protocol::{ToolCall, ToolResult, ToolSpec};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::{Tool, ToolRegistry};

const MAX_BYTES: usize = 1024 * 1024;

/// Library tools are disabled by default. Use `all` for CLI-style defaults,
/// or enable individual fields before registering them alongside custom tools.
#[derive(Debug, Clone, Default)]
pub struct BuiltinToolsConfig {
    pub working_directory: PathBuf,
    pub read_file: bool,
    pub write_file: bool,
    pub shell: bool,
}

impl BuiltinToolsConfig {
    pub fn all(working_directory: impl Into<PathBuf>) -> Self {
        Self {
            working_directory: working_directory.into(),
            read_file: true,
            write_file: true,
            shell: true,
        }
    }

    pub fn register(&self, registry: &mut ToolRegistry) {
        if self.read_file {
            registry.register(Arc::new(ReadFileTool::new(&self.working_directory)));
        }
        if self.write_file {
            registry.register(Arc::new(WriteFileTool::new(&self.working_directory)));
        }
        if self.shell {
            registry.register(Arc::new(ShellTool::new(&self.working_directory)));
        }
    }
}

fn base_path(path: PathBuf) -> PathBuf {
    if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path
    }
}

fn file_path(base: &Path, path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("path must not be empty".into());
    }
    Ok(base.join(path))
}

fn result(call: &ToolCall, output: Result<String, String>) -> ToolResult {
    match output {
        Ok(output) => ToolResult::completed(call, output),
        Err(error) => ToolResult::failed(call, error),
    }
}

/// Reads UTF-8 files, rejecting files larger than 1 MiB.
pub struct ReadFileTool {
    working_directory: PathBuf,
}

impl ReadFileTool {
    pub fn new(working_directory: impl Into<PathBuf>) -> Self {
        Self {
            working_directory: base_path(working_directory.into()),
        }
    }
}

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Read a UTF-8 file (maximum 1 MiB). Relative paths use the working directory; absolute paths are supported.".into(),
            parameters: json!({"type": "object", "properties": {
                "path": {"type": "string", "minLength": 1}
            }, "required": ["path"]}),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        result(
            call,
            async {
                let args: ReadArgs =
                    serde_json::from_value(call.arguments.clone()).map_err(|e| e.to_string())?;
                let path = file_path(&self.working_directory, &args.path)?;
                let file = tokio::fs::File::open(path)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                file.take((MAX_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|e| e.to_string())?;
                if bytes.len() > MAX_BYTES {
                    return Err("file exceeds the 1 MiB read limit".into());
                }
                String::from_utf8(bytes).map_err(|e| e.to_string())
            }
            .await,
        )
    }
}

/// Creates or overwrites a UTF-8 file. Parent directories must already exist.
pub struct WriteFileTool {
    working_directory: PathBuf,
}

impl WriteFileTool {
    pub fn new(working_directory: impl Into<PathBuf>) -> Self {
        Self {
            working_directory: base_path(working_directory.into()),
        }
    }
}

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Create or overwrite a UTF-8 file. Parent directories must exist. Relative paths use the working directory; absolute paths are supported.".into(),
            parameters: json!({"type": "object", "properties": {
                "path": {"type": "string", "minLength": 1},
                "content": {"type": "string"}
            }, "required": ["path", "content"]}),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        result(
            call,
            async {
                let args: WriteArgs =
                    serde_json::from_value(call.arguments.clone()).map_err(|e| e.to_string())?;
                let path = file_path(&self.working_directory, &args.path)?;
                tokio::fs::write(path, args.content.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "Wrote {} bytes to {}",
                    args.content.len(),
                    args.path
                ))
            }
            .await,
        )
    }
}

/// Runs `sh -c` on Unix or `cmd /D /S /C` on Windows.
/// Timeout/cancellation kills the direct child; descendant process cleanup is
/// platform dependent. This tool does not provide process isolation.
pub struct ShellTool {
    working_directory: PathBuf,
}

impl ShellTool {
    pub fn new(working_directory: impl Into<PathBuf>) -> Self {
        Self {
            working_directory: base_path(working_directory.into()),
        }
    }
}

#[derive(Deserialize)]
struct ShellArgs {
    command: String,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
}

fn default_timeout() -> u64 {
    30_000
}

// Drain both streams continuously while bounding retained output.
async fn capture(mut stream: impl AsyncRead + Unpin) -> std::io::Result<(String, bool)> {
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let n = stream.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        let keep = n.min(MAX_BYTES - output.len());
        output.extend_from_slice(&buffer[..keep]);
        truncated |= keep < n;
    }
    Ok((String::from_utf8_lossy(&output).into_owned(), truncated))
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().into(),
            description: "Execute a shell command in the working directory (sh on Unix, cmd on Windows). Returns JSON with stdout, stderr, exit_code and truncation flags; retains up to 1 MiB per stream. Default timeout is 30000 ms.".into(),
            parameters: json!({"type": "object", "properties": {
                "command": {"type": "string", "minLength": 1},
                "timeout_ms": {"type": "integer", "minimum": 1, "maximum": 300000, "default": 30000}
            }, "required": ["command"]}),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let args: ShellArgs = match serde_json::from_value(call.arguments.clone()) {
            Ok(args) => args,
            Err(error) => return ToolResult::failed(call, error.to_string()),
        };
        if args.command.trim().is_empty() || !(1..=300_000).contains(&args.timeout_ms) {
            return ToolResult::failed(
                call,
                "command must not be blank; timeout_ms must be 1..=300000",
            );
        }
        let mut command;
        #[cfg(windows)]
        {
            command = tokio::process::Command::new("cmd");
            command.args(["/D", "/S", "/C"]);
        }
        #[cfg(not(windows))]
        {
            command = tokio::process::Command::new("sh");
            command.arg("-c");
        }
        let execution = async {
            let mut child = command
                .arg(&args.command)
                .current_dir(&self.working_directory)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;
            let stdout = child.stdout.take().expect("piped stdout");
            let stderr = child.stderr.take().expect("piped stderr");
            tokio::try_join!(child.wait(), capture(stdout), capture(stderr))
        };
        match tokio::time::timeout(Duration::from_millis(args.timeout_ms), execution).await {
            Ok(Ok((status, (stdout, stdout_truncated), (stderr, stderr_truncated)))) => {
                let output = json!({"stdout": stdout, "stderr": stderr, "exit_code": status.code(),
                    "stdout_truncated": stdout_truncated, "stderr_truncated": stderr_truncated})
                .to_string();
                if status.success() {
                    ToolResult::completed(call, output)
                } else {
                    ToolResult::failed(call, output)
                }
            }
            Ok(Err(error)) => ToolResult::failed(call, error.to_string()),
            Err(_) => ToolResult::failed(
                call,
                format!("shell timed out after {} ms", args.timeout_ms),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapestry_protocol::ToolCallStatus;

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall::new("test-call".into(), name, args)
    }

    #[test]
    fn library_tools_are_opt_in_and_individually_selectable() {
        let mut registry = ToolRegistry::new();
        BuiltinToolsConfig::default().register(&mut registry);
        assert!(registry.specs().is_empty());
        BuiltinToolsConfig {
            read_file: true,
            ..Default::default()
        }
        .register(&mut registry);
        assert_eq!(registry.specs().len(), 1);
        assert!(registry.get("read_file").is_some());
        BuiltinToolsConfig::all(".").register(&mut registry);
        assert_eq!(
            registry
                .specs()
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["read_file", "shell", "write_file"]
        );
    }

    #[tokio::test]
    async fn files_round_trip_overwrite_and_report_failures() {
        let dir = tempfile::tempdir().unwrap();
        let read = ReadFileTool::new(dir.path());
        let write = WriteFileTool::new(dir.path());
        for content in ["你好\nworld", "short", ""] {
            let request = call(
                "write_file",
                json!({"path": "test.txt", "content": content, "description": "test"}),
            );
            let result = write.execute(&request).await;
            assert_eq!(result.status, ToolCallStatus::Completed);
            assert_eq!(result.call_id, request.id);
            let result = read
                .execute(&call(
                    "read_file",
                    json!({"path": dir.path().join("test.txt")}),
                ))
                .await;
            assert_eq!(result.status, ToolCallStatus::Completed);
            assert_eq!(result.output, content);
        }
        for args in [
            json!({}),
            json!({"path": 1}),
            json!({"path": ""}),
            json!({"path": "missing"}),
        ] {
            assert_eq!(
                read.execute(&call("read_file", args)).await.status,
                ToolCallStatus::Failed
            );
        }
        for args in [
            json!({"path": "test.txt"}),
            json!({"path": "missing/test", "content": "x"}),
        ] {
            assert_eq!(
                write.execute(&call("write_file", args)).await.status,
                ToolCallStatus::Failed
            );
        }
        std::fs::write(dir.path().join("binary"), [0xff]).unwrap();
        std::fs::write(dir.path().join("large"), vec![b'x'; MAX_BYTES + 1]).unwrap();
        for path in ["binary", "large"] {
            assert_eq!(
                read.execute(&call("read_file", json!({"path": path})))
                    .await
                    .status,
                ToolCallStatus::Failed
            );
        }
    }

    #[tokio::test]
    async fn shell_captures_streams_exit_status_and_validates_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let shell = ShellTool::new(dir.path());
        let result = shell
            .execute(&call("shell", json!({"command": "echo hello"})))
            .await;
        assert_eq!(result.status, ToolCallStatus::Completed);
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["stdout"].as_str().unwrap().trim(), "hello");
        assert_eq!(output["exit_code"], 0);
        let result = shell
            .execute(&call(
                "shell",
                json!({"command": "echo error >&2 & exit 7"}),
            ))
            .await;
        assert_eq!(result.status, ToolCallStatus::Failed);
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["exit_code"], 7);
        assert!(output["stderr"].as_str().unwrap().contains("error"));
        for args in [
            json!({}),
            json!({"command": " "}),
            json!({"command": "echo x", "timeout_ms": 0}),
            json!({"command": "echo x", "timeout_ms": 300001}),
        ] {
            assert_eq!(
                shell.execute(&call("shell", args)).await.status,
                ToolCallStatus::Failed
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_uses_working_directory_and_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let shell = ShellTool::new(dir.path());
        let result = shell
            .execute(&call("shell", json!({"command": "printf value > marker"})))
            .await;
        assert_eq!(result.status, ToolCallStatus::Completed);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("marker")).unwrap(),
            "value"
        );
        let result = shell
            .execute(&call(
                "shell",
                json!({"command": "exec sleep 10", "timeout_ms": 20}),
            ))
            .await;
        assert_eq!(result.status, ToolCallStatus::Failed);
        assert!(result.output.contains("timed out"));
    }

    #[tokio::test]
    async fn capture_bounds_output_and_marks_truncation() {
        let bytes = vec![b'x'; MAX_BYTES + 10];
        let (output, truncated) = capture(bytes.as_slice()).await.unwrap();
        assert_eq!(output.len(), MAX_BYTES);
        assert!(truncated);
    }
}
