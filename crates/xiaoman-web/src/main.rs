//! `web-server`: the command line the Kotlin `Main.kt` provided, over the same options.
//!
//! `--local` is the classroom mode: the port falls back when it is taken, an access code is
//! generated when none was given, the LAN links and the `/connect` QR page are offered, and the
//! page opens in the default browser.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use xiaoman_web::local_launch;
use xiaoman_web::options::WebOptions;
use xiaoman_web::server::{ServerError, WebServer};
use xiaoman_web::tokens_admin;

struct Launch {
    options: WebOptions,
    timezone: Option<String>,
    local: bool,
    open_browser: bool,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("tokens") {
        let clock: xiaoman_web::memory::Clock = std::sync::Arc::new(xiaoman_api::now_millis);
        let mut out = std::io::stdout();
        let mut err = std::io::stderr();
        let code = tokens_admin::run(
            &args[1..],
            &mut out,
            &mut err,
            &|name| std::env::var(name).ok(),
            clock,
        );
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{}", usage());
        return ExitCode::SUCCESS;
    }

    let launch = match parse_options(&args) {
        Ok(launch) => launch,
        Err(message) => {
            eprintln!("错误：{message}\n\n{}", usage());
            return ExitCode::from(2);
        }
    };

    // `ReminderTimes` parses the device-local time, so the configured zone must be the process
    // zone before the first `chrono::Local` call. Linux and macOS honour TZ; Windows does not,
    // which is why the deploy unit sets it as well.
    if let Some(zone) = &launch.timezone {
        // Safety: called before the runtime starts and before any other thread exists.
        unsafe { std::env::set_var("TZ", zone) };
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("无法启动运行时：{error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run(launch))
}

fn parse_options(args: &[String]) -> Result<Launch, String> {
    let env = |name: &str| std::env::var(name).ok();
    let mut class_tokens: Option<PathBuf> = None;
    let mut local = false;
    let mut open_browser = true;
    let mut stdio_mode = "auto".to_string();
    let mut zone: Option<String> = None;
    let mut host: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut static_root: Option<PathBuf> = None;
    let mut skills: Option<PathBuf> = None;
    let mut access_code: Option<String> = None;
    let mut allow_private = false;
    let mut session_hours: Option<f64> = None;
    let mut max_stdio: Option<usize> = None;

    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str().to_string();
        index += 1;
        let mut value = || -> Result<String, String> {
            let value = args
                .get(index)
                .ok_or_else(|| format!("{flag} 需要一个值"))?;
            index += 1;
            Ok(value.clone())
        };
        match flag.as_str() {
            "--host" => host = Some(value()?),
            "--port" => {
                port = Some(
                    value()?
                        .parse::<u16>()
                        .map_err(|_| "端口必须是 0-65535".to_string())?,
                )
            }
            "--static" => static_root = Some(PathBuf::from(value()?)),
            "--skills" => skills = Some(PathBuf::from(value()?)),
            "--access-code" => access_code = Some(value()?),
            "--allow-private" | "--allow-private-mcp" => allow_private = true,
            "--timezone" => {
                let name = value()?;
                xiaoman_web::class_tokens::zone_offset(&name)
                    .ok_or_else(|| format!("不支持的时区 {name}"))?;
                zone = Some(name);
            }
            "--session-hours" => {
                let hours = value()?
                    .parse::<f64>()
                    .map_err(|_| "--session-hours 需要数字".to_string())?;
                if hours <= 0.0 {
                    return Err("--session-hours 必须大于 0".to_string());
                }
                session_hours = Some(hours);
            }
            "--stdio-mcp" => {
                let mode = value()?;
                if !matches!(mode.as_str(), "auto" | "on" | "off") {
                    return Err("--stdio-mcp 只能是 auto、on 或 off".to_string());
                }
                stdio_mode = mode;
            }
            "--max-stdio" => {
                let count = value()?
                    .parse::<usize>()
                    .map_err(|_| "--max-stdio 需要数字".to_string())?;
                if count == 0 {
                    return Err("--max-stdio 必须大于 0".to_string());
                }
                max_stdio = Some(count);
            }
            "--class-tokens" => class_tokens = Some(PathBuf::from(value()?)),
            "--local" => local = true,
            "--no-browser" => open_browser = false,
            other => return Err(format!("参数无效：{other}")),
        }
    }

    // `from_env_with` also reads XIAOMAN_CLASS_* and XIAOMAN_CLAIM_*; `--class-tokens` replaces
    // only the token file those variables point at.
    let mut options = WebOptions::from_env_with(&env, class_tokens)?;
    if let Some(host) = host {
        options.host = host;
    }
    if let Some(port) = port {
        options.port = port;
    }
    if let Some(root) = static_root {
        options.static_root = Some(root);
    }
    if let Some(root) = skills {
        options.skills_root = Some(root);
    }
    if let Some(code) = access_code {
        options.access_code = Some(code);
    }
    if allow_private {
        options.allow_private = true;
    }
    if let Some(hours) = session_hours {
        options.session_millis = (hours * 3_600_000.0) as i64;
    }
    if let Some(name) = &zone {
        options.timezone_offset_minutes = xiaoman_web::class_tokens::zone_offset(name)
            .ok_or_else(|| format!("不支持的时区 {name}"))?;
    }
    if local && options.skills_root.is_none() {
        options.skills_root = local_launch::local_skills_root();
    }
    options.local_connect = local;
    let node = command_on_path("node") && command_on_path("npx");
    options.stdio_mcp = match stdio_mode.as_str() {
        "on" => true,
        "off" => false,
        _ => node,
    };
    if local
        && options
            .access_code
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
    {
        options.access_code = Some(local_launch::access_code(6));
    }
    if let Some(count) = max_stdio {
        options.max_stdio = count;
    }
    Ok(Launch {
        options,
        timezone: zone,
        local,
        open_browser,
    })
}

async fn run(launch: Launch) -> ExitCode {
    let options = launch.options;
    let tries: u16 = if launch.local { 20 } else { 1 };
    let start = options.port;
    let mut server = None;
    let mut last_error: Option<ServerError> = None;
    for offset in 0..tries {
        let mut attempt = options.clone();
        attempt.port = start.saturating_add(offset);
        match WebServer::new(attempt).bind().await {
            Ok(bound) => {
                server = Some(bound);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let Some(server) = server else {
        eprintln!(
            "启动失败：{}",
            last_error
                .map(|error| error.to_string())
                .unwrap_or_default()
        );
        return ExitCode::FAILURE;
    };

    println!("小满已监听 http://{}:{}", options.host, server.port());
    println!(
        "{}",
        if options.stdio_mcp {
            format!(
                "12306 火车票（stdio）：已开启，最多 {} 个进程",
                options.max_stdio
            )
        } else {
            "12306 火车票（stdio）：已关闭（网页上显示为不可用）".to_string()
        }
    );
    println!(
        "课堂令牌：{}",
        if options.class.is_some() {
            "已开启"
        } else {
            "未开启"
        }
    );
    if options.class.is_some() {
        println!(
            "自助领取：{}",
            if options.claim.is_some() {
                "已开启，页面 /claim"
            } else {
                "未开启"
            }
        );
    }

    if launch.local {
        if server.port() != options.port {
            println!("端口 {} 已被占用，改用 {}", options.port, server.port());
        }
        let mut stdout = std::io::stdout();
        local_launch::announce(server.port(), options.access_code.as_deref(), &mut stdout);
        let _ = stdout.flush();
        if launch.open_browser {
            local_launch::open_browser(&format!("http://127.0.0.1:{}/connect", server.port()));
        }
    }

    if tokio::signal::ctrl_c().await.is_err() {
        eprintln!("无法监听 Ctrl+C，服务继续运行");
    }
    server.shutdown().await;
    ExitCode::SUCCESS
}

/// True when `name` resolves through PATH (with `.exe`/`.cmd` on Windows).
fn command_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let mut names = vec![name.to_string()];
    if cfg!(windows) {
        names.push(format!("{name}.exe"));
        names.push(format!("{name}.cmd"));
    }
    std::env::split_paths(&path).any(|dir| names.iter().any(|name| dir.join(name).is_file()))
}

fn usage() -> String {
    "小满网页版后端\n\
用法：web-server [--host 127.0.0.1] [--port 8787] [--static DIR]\n\
  [--access-code CODE] [--allow-private] [--timezone Asia/Shanghai] [--session-hours 2]\n\
  [--skills DIR] [--stdio-mcp auto|on|off] [--max-stdio 20] [--class-tokens FILE] [--local [--no-browser]]\n\
       web-server tokens …   管理课堂令牌\n\
访问码也可通过 XIAOMAN_WEB_ACCESS_CODE 设置。默认仅监听本机。\n\
--skills DIR 加载聊天工作流，也可用 XIAOMAN_WEB_SKILLS 设置，命令行优先。\n\
--local 学员在自己电脑上运行：端口被占用时自动往后换，没给访问码就随机生成，\n\
  打印局域网地址，并开启只能在本机打开的扫码页 /connect（自动用浏览器打开）。\n\
--stdio-mcp 控制需要 Node.js 的目录服务（12306 火车票）：auto（默认）在 PATH 里找到 node 和 npx 时开启。\n\
--max-stdio 同时在跑的 stdio MCP 子进程上限（默认 20，含所有会话）。\n\
记忆保存在学员浏览器里，随每次对话发来，服务器不落盘。\n\
课堂令牌（默认关闭）：设置 XIAOMAN_CLASS_* 环境变量并用 web-server tokens generate 生成令牌文件后开启。\n\
自助领取（默认关闭，需要先开启课堂令牌）：设置 XIAOMAN_CLAIM_*，页面 /claim。\n"
        .to_string()
}
