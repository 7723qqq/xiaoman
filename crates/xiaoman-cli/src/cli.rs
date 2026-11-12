//! The clap command line: names, flags and Chinese help text of the Python CLI.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::adb::{Adb, RealAdb};
use crate::commands;

/// Global options, also accepted after the subcommand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalOptions {
    pub adb: Option<String>,
    pub serial: Option<String>,
}

#[derive(Debug, Parser)]
#[command(
    name = "xiaoman",
    about = "xiaoman：小满技能开发者 CLI（校验、推送、运行、日志、轨迹）。"
)]
pub struct Cli {
    /// adb 可执行文件（默认：$ADB 或 PATH 上的 adb）
    #[arg(long, global = true, value_name = "PATH")]
    pub adb: Option<String>,

    /// 设备序列号（默认由 adb 决定，或取 $ANDROID_SERIAL）
    #[arg(long, short = 's', global = true, value_name = "S")]
    pub serial: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 按 App 的规则校验技能文件夹
    Lint {
        #[arg(required = true, value_name = "skill-dir")]
        skill_dirs: Vec<PathBuf>,
    },

    /// 校验并推送技能文件夹到手机，然后重新加载
    Push {
        #[arg(required = true, value_name = "skill-dir")]
        skill_dirs: Vec<PathBuf>,
    },

    /// 通过调试桥启动一个手机任务
    Run {
        /// 任务目标
        goal: String,

        /// 要使用的技能名
        #[arg(long)]
        skill: Option<String>,

        /// run id（默认自动生成）；配合 --follow 时请用新的 id
        #[arg(long = "run-id")]
        run_id: Option<String>,

        /// 持续输出本次运行的事件，直到 PhoneTaskFinished/Failed；成功时退出码为 0
        #[arg(long, short = 'f')]
        follow: bool,
    },

    /// 停止正在运行的任务
    Stop,

    /// 从 logcat 持续读取小满事件
    Logs {
        /// 只显示该次运行的事件
        #[arg(long = "run-id")]
        run_id: Option<String>,
    },

    /// 拉取并查看运行轨迹
    Traces {
        #[command(subcommand)]
        command: TracesCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum TracesCommand {
    /// adb pull traces/<run_id>
    Pull {
        run_id: String,

        /// 本地目录（默认：./traces）
        #[arg(long, default_value = "traces")]
        out: PathBuf,
    },

    /// 把 trace.jsonl 渲染成步骤表
    Show {
        /// trace.jsonl、其所在目录或 run id（在 --dir 中查找，本地没有则先拉取）
        #[arg(value_name = "path-or-run_id")]
        target: String,

        /// 按 run id 查找时使用的本地轨迹目录（默认：./traces）
        #[arg(long, default_value = "traces")]
        dir: PathBuf,

        /// 不截断过长的单元格
        #[arg(long)]
        full: bool,
    },
}

/// Parses `argv` (without the program name) and runs it with an injectable adb factory.
pub fn run<I, T>(
    argv: I,
    adb_factory: &dyn Fn(&GlobalOptions) -> Box<dyn Adb>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut args: Vec<OsString> = vec![OsString::from("xiaoman")];
    args.extend(argv.into_iter().map(Into::into));
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            if error.use_stderr() {
                let _ = write!(err, "{error}");
            } else {
                let _ = write!(out, "{error}");
            }
            return error.exit_code();
        }
    };
    let options = GlobalOptions {
        adb: cli.adb.clone(),
        serial: cli.serial.clone(),
    };
    let adb = adb_factory(&options);
    let result = match cli.command {
        Command::Lint { skill_dirs } => Ok(commands::cmd_lint(&skill_dirs, out, err)),
        Command::Push { skill_dirs } => commands::cmd_push(&skill_dirs, adb.as_ref(), out, err),
        Command::Run {
            goal,
            skill,
            run_id,
            follow,
        } => commands::cmd_run(
            &goal,
            skill.as_deref(),
            run_id.as_deref(),
            follow,
            adb.as_ref(),
            out,
            err,
        ),
        Command::Stop => commands::cmd_stop(adb.as_ref(), out),
        Command::Logs { run_id } => commands::cmd_logs(run_id.as_deref(), adb.as_ref(), out, err),
        Command::Traces { command } => match command {
            TracesCommand::Pull {
                run_id,
                out: out_dir,
            } => commands::cmd_traces_pull(&run_id, &out_dir, adb.as_ref(), out, err),
            TracesCommand::Show { target, dir, full } => {
                commands::cmd_traces_show(&target, &dir, full, adb.as_ref(), out, err)
            }
        },
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(err, "xiaoman: {error}");
            1
        }
    }
}

/// Runs the CLI against the process's stdout/stderr and the real adb.
pub fn main_with_args<I, T>(argv: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    let factory = |options: &GlobalOptions| -> Box<dyn Adb> {
        Box::new(RealAdb::new(options.adb.clone(), options.serial.clone()))
    };
    run(argv, &factory, &mut out, &mut err)
}
