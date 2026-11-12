//! Approvals and questions on the terminal.
//!
//! Non-interactive runs (no console, or `--non-interactive`) deny every approval and answer
//! questions with a fixed "nobody is here" reply, so a scripted run can never pay, send or delete
//! on its own.

use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoman_api::{
    ApprovalAnswer, ApprovalError, ApprovalGateway, ApprovalRequest, ApprovalScope,
    ToolApprovalGateway, ToolApprovalRequest, UserChannel,
};

/// What a question gets when nobody can answer it.
pub const NOBODY_HERE: &str =
    "用户不在场，无法回答。请不要继续需要用户的步骤，用 finish 说明当前状态。";

/// Shared stdin reader, so the chat prompt and the approval prompts use the same stream.
pub type Input = Arc<Mutex<Box<dyn BufRead + Send>>>;
/// Shared stdout writer.
pub type Output = Arc<Mutex<Box<dyn Write + Send>>>;

/// Approvals and questions on the terminal.
pub struct ConsoleChannels {
    input: Input,
    out: Output,
    interactive: bool,
}

impl ConsoleChannels {
    pub fn new(
        input: Box<dyn BufRead + Send>,
        out: Box<dyn Write + Send>,
        interactive: bool,
    ) -> Self {
        Self {
            input: Arc::new(Mutex::new(input)),
            out: Arc::new(Mutex::new(out)),
            interactive,
        }
    }

    /// Channels over handles the caller already owns.
    pub fn from_shared(input: Input, out: Output, interactive: bool) -> Self {
        Self {
            input,
            out,
            interactive,
        }
    }

    /// The process terminal; `interactive` should be false when stdin or stdout is redirected.
    pub fn stdio(interactive: bool) -> Self {
        Self::new(
            Box::new(BufReader::new(std::io::stdin())),
            Box::new(std::io::stdout()),
            interactive,
        )
    }

    pub fn interactive(&self) -> bool {
        self.interactive
    }

    /// Shared writer, for the other console-only pieces of the runner.
    pub fn out_handle(&self) -> Output {
        self.out.clone()
    }

    pub fn line(&self, text: &str) {
        write_line(&self.out, text);
    }

    fn prompt(&self, text: &str) {
        if let Ok(mut out) = self.out.lock() {
            let _ = out.write_all(text.as_bytes());
            let _ = out.flush();
        }
    }

    /// One line without its terminator; `None` at end of input.
    async fn read_line(&self) -> Option<String> {
        let input = self.input.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = input.lock().ok()?;
            let mut line = String::new();
            match guard.read_line(&mut line) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
            }
        })
        .await
        .ok()
        .flatten()
    }
}

#[async_trait]
impl ApprovalGateway for ConsoleChannels {
    async fn request(&self, request: ApprovalRequest) -> Result<ApprovalAnswer, ApprovalError> {
        self.line(&format!(
            "需要确认：{}（{}）",
            request.prompt, request.reason
        ));
        if !self.interactive {
            self.line("已自动拒绝（非交互模式）。");
            return Ok(ApprovalAnswer::once(false));
        }
        self.prompt("[y=允许一次 / t=本任务内允许 / 其他=拒绝] > ");
        match self
            .read_line()
            .await
            .map(|line| line.trim().to_lowercase())
        {
            Some(answer) if answer == "y" || answer == "yes" => Ok(ApprovalAnswer::once(true)),
            Some(answer) if answer == "t" => Ok(ApprovalAnswer {
                approved: true,
                scope: ApprovalScope::Task,
            }),
            _ => {
                self.line("已拒绝。");
                Ok(ApprovalAnswer::once(false))
            }
        }
    }
}

#[async_trait]
impl ToolApprovalGateway for ConsoleChannels {
    async fn approve(&self, request: ToolApprovalRequest) -> bool {
        let arguments = truncate(&request.arguments_json, 300);
        let title = request.title.as_deref().unwrap_or(&request.tool);
        self.line(&format!(
            "需要确认：调用「{}」（{}），参数 {}（{}）",
            title, request.server, arguments, request.reason
        ));
        if !self.interactive {
            self.line("已自动拒绝（非交互模式）。");
            return false;
        }
        self.prompt("[y=允许 / 其他=拒绝] > ");
        let approved = matches!(
            self.read_line().await.map(|line| line.trim().to_lowercase()),
            Some(answer) if answer == "y" || answer == "yes"
        );
        if !approved {
            self.line("已拒绝。");
        }
        approved
    }
}

#[async_trait]
impl UserChannel for ConsoleChannels {
    async fn ask(&self, _task_id: &str, question: &str, takeover: bool) -> String {
        self.line(&format!("小满问你：{question}"));
        if !self.interactive {
            return NOBODY_HERE.to_string();
        }
        self.prompt(if takeover {
            "请在手机上完成这一步，完成后按回车 > "
        } else {
            "你的回答 > "
        });
        self.read_line()
            .await
            .unwrap_or_else(|| NOBODY_HERE.to_string())
    }
}

fn write_line(out: &Output, text: &str) {
    if let Ok(mut out) = out.lock() {
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut shortened: String = text.chars().take(max).collect();
    shortened.push('…');
    shortened
}
