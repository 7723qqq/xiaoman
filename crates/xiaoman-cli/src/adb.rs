//! adb invocation: argument-list subprocess calls, never a host shell.
//!
//! Remote commands for `adb shell` are passed as one argument, quoted so the device's
//! shell sees each value (Chinese, spaces, quotes) intact.

use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub const APP_PACKAGE: &str = "dev.xiaoman.host";
/// `/sdcard/Android/data/<APP_PACKAGE>/files`, the host's `getExternalFilesDir(null)`.
pub const EXTERNAL_FILES: &str = "/sdcard/Android/data/dev.xiaoman.host/files";
pub const REMOTE_SKILLS: &str = "/sdcard/Android/data/dev.xiaoman.host/files/skills";
pub const REMOTE_TRACES: &str = "/sdcard/Android/data/dev.xiaoman.host/files/traces";
pub const RECEIVER: &str = "dev.xiaoman.host/dev.xiaoman.app.DebugReceiver";
pub const ACTION_RUN: &str = "dev.xiaoman.debug.RUN";
pub const ACTION_STOP: &str = "dev.xiaoman.debug.STOP";
pub const ACTION_RELOAD: &str = "dev.xiaoman.debug.RELOAD_SKILLS";
pub const LOG_TAG: &str = "Xiaoman";

/// An adb invocation failure, reported as `xiaoman: <message>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbError {
    pub message: String,
}

impl AdbError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AdbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AdbError {}

/// Exit status plus combined output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbOutput {
    pub status: i32,
    pub output: String,
}

/// A spawned adb process, normally `adb logcat`.
pub trait SpawnedChild {
    /// The next line (newline removed), or `None` at end of output.
    fn read_line(&mut self) -> io::Result<Option<String>>;
    fn try_wait(&mut self) -> io::Result<Option<i32>>;
    fn terminate(&mut self);
    fn wait(&mut self);
}

/// The adb surface the commands use; tests provide a fake implementation.
pub trait Adb {
    fn adb_path(&self) -> &str;
    fn serial(&self) -> Option<&str>;

    /// Whether adb output is echoed to stderr.
    fn echo(&self) -> bool {
        false
    }

    /// `adb [-s serial] <args...>`.
    fn argv(&self, args: &[&str]) -> Vec<String> {
        let mut argv = vec![self.adb_path().to_string()];
        if let Some(serial) = self.serial() {
            argv.push("-s".to_string());
            argv.push(serial.to_string());
        }
        argv.extend(args.iter().map(|arg| arg.to_string()));
        argv
    }

    fn exec(&self, argv: &[String]) -> Result<AdbOutput, AdbError>;

    /// Starts a full argv (adb path and serial already included).
    fn start(&self, argv: &[String]) -> Result<Box<dyn SpawnedChild>, AdbError>;

    /// `adb [-s serial] <args...>`, spawned with piped stdout.
    fn spawn(&self, args: &[&str]) -> Result<Box<dyn SpawnedChild>, AdbError> {
        self.start(&self.argv(args))
    }

    fn run(&self, args: &[&str], check: bool) -> Result<AdbOutput, AdbError> {
        let argv = self.argv(args);
        let result = self.exec(&argv)?;
        if self.echo() && !result.output.trim().is_empty() {
            eprintln!("{}", result.output.trim_end_matches('\n'));
        }
        if check && result.status != 0 {
            let suffix = result
                .output
                .trim()
                .lines()
                .last()
                .map(|line| format!(": {line}"))
                .unwrap_or_default();
            return Err(AdbError::new(format!(
                "{} failed with exit code {}{suffix}",
                shell_join(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
                result.status
            )));
        }
        Ok(result)
    }

    fn shell(&self, remote_command: &str, check: bool) -> Result<AdbOutput, AdbError> {
        self.run(&["shell", remote_command], check)
    }

    fn broadcast(
        &self,
        action: &str,
        extras: &[(&str, Option<&str>)],
    ) -> Result<AdbOutput, AdbError> {
        let result = self.shell(&broadcast_command(action, extras), true)?;
        // `am broadcast` exits 0 even when nothing received it
        if result.output.contains("Error") || result.output.contains("Exception") {
            return Err(AdbError::new(format!(
                "broadcast {action} failed: {}",
                result.output.trim()
            )));
        }
        Ok(result)
    }

    /// Replaces `/sdcard/.../skills/<name>/` with the files of `local_dir`.
    fn push_skill(&self, local_dir: &Path, name: &str) -> Result<(String, Vec<String>), AdbError> {
        let remote = format!("{REMOTE_SKILLS}/{name}");
        let command = format!(
            "{} && {}",
            shell_join(&["rm", "-rf", remote.as_str()]),
            shell_join(&["mkdir", "-p", remote.as_str()])
        );
        self.shell(&command, true)?;
        let mut pushed = Vec::new();
        for relative in skill_files(local_dir)? {
            let relative_text = relative.to_string_lossy().replace('\\', "/");
            let remote_path = format!("{remote}/{relative_text}");
            if let Some((parent, _)) = remote_path.rsplit_once('/') {
                if parent != remote {
                    self.shell(&shell_join(&["mkdir", "-p", parent]), true)?;
                }
            }
            let local_path = local_dir.join(&relative);
            self.run(&["push", &local_path.to_string_lossy(), &remote_path], true)?;
            pushed.push(relative_text);
        }
        Ok((remote, pushed))
    }

    fn logcat_argv(&self) -> Vec<String> {
        vec![
            "logcat".to_string(),
            "-v".to_string(),
            "threadtime".to_string(),
            "-s".to_string(),
            LOG_TAG.to_string(),
        ]
    }

    fn pull_trace(&self, run_id: &str, out_dir: &Path) -> Result<PathBuf, AdbError> {
        std::fs::create_dir_all(out_dir).map_err(|error| {
            AdbError::new(format!("cannot create {}: {error}", out_dir.display()))
        })?;
        let remote = format!("{REMOTE_TRACES}/{run_id}");
        self.run(&["pull", &remote, &out_dir.to_string_lossy()], true)?;
        Ok(out_dir.join(run_id))
    }
}

/// The real implementation, shelling out to the `adb` executable.
#[derive(Debug, Clone)]
pub struct RealAdb {
    adb: String,
    serial: Option<String>,
    echo: bool,
}

impl RealAdb {
    /// `adb` defaults to `$ADB`, then to `adb` on `PATH`.
    pub fn new(adb: Option<String>, serial: Option<String>) -> Self {
        Self {
            adb: adb.unwrap_or_else(default_adb_path),
            serial,
            echo: true,
        }
    }

    pub fn with_echo(mut self, echo: bool) -> Self {
        self.echo = echo;
        self
    }
}

/// `$ADB`, or `adb` when it is unset or empty.
pub fn default_adb_path() -> String {
    match std::env::var("ADB") {
        Ok(value) if !value.is_empty() => value,
        _ => "adb".to_string(),
    }
}

impl Adb for RealAdb {
    fn adb_path(&self) -> &str {
        &self.adb
    }

    fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    fn echo(&self) -> bool {
        self.echo
    }

    fn exec(&self, argv: &[String]) -> Result<AdbOutput, AdbError> {
        let Some((program, args)) = argv.split_first() else {
            return Err(AdbError::new("empty adb command"));
        };
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|error| spawn_error(program, error))?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.stderr.is_empty() {
            text.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        Ok(AdbOutput {
            status: output.status.code().unwrap_or(-1),
            output: text,
        })
    }

    fn start(&self, argv: &[String]) -> Result<Box<dyn SpawnedChild>, AdbError> {
        let Some((program, args)) = argv.split_first() else {
            return Err(AdbError::new("empty adb command"));
        };
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| spawn_error(program, error))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AdbError::new("adb produced no output"))?;
        Ok(Box::new(RealChild {
            child,
            reader: BufReader::new(stdout),
        }))
    }
}

fn spawn_error(program: &str, error: io::Error) -> AdbError {
    if error.kind() == io::ErrorKind::NotFound {
        AdbError::new(format!(
            "adb not found at '{program}'; pass --adb PATH or set ADB"
        ))
    } else {
        AdbError::new(format!("{program}: {error}"))
    }
}

struct RealChild {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
}

impl SpawnedChild for RealChild {
    fn read_line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        match self.reader.read_line(&mut line)? {
            0 => Ok(None),
            _ => Ok(Some(line)),
        }
    }

    fn try_wait(&mut self) -> io::Result<Option<i32>> {
        Ok(self
            .child
            .try_wait()?
            .map(|status| status.code().unwrap_or(-1)))
    }

    fn terminate(&mut self) {
        let _ = self.child.kill();
    }

    fn wait(&mut self) {
        let _ = self.child.wait();
    }
}

/// Quotes each part for the device's POSIX shell and joins them.
pub fn shell_join(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| shell_quote(part))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `shlex.quote`: safe words pass through, the rest are single-quoted.
pub fn shell_quote(part: &str) -> String {
    let safe = !part.is_empty()
        && part
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_@%+=:,./-".contains(ch));
    if safe {
        part.to_string()
    } else {
        format!("'{}'", part.replace('\'', "'\"'\"'"))
    }
}

/// Remote command string for `am broadcast` to the debug receiver.
pub fn broadcast_command(action: &str, extras: &[(&str, Option<&str>)]) -> String {
    let mut parts: Vec<String> = vec![
        "am".to_string(),
        "broadcast".to_string(),
        "-n".to_string(),
        RECEIVER.to_string(),
        "-a".to_string(),
        action.to_string(),
    ];
    for (key, value) in extras {
        if let Some(value) = value {
            parts.push("--es".to_string());
            parts.push((*key).to_string());
            parts.push((*value).to_string());
        }
    }
    shell_join(&parts.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Relative paths of files to push, skipping hidden files and folders.
pub fn skill_files(local_dir: &Path) -> Result<Vec<PathBuf>, AdbError> {
    let mut result = Vec::new();
    walk(local_dir, Path::new(""), &mut result)?;
    Ok(result)
}

fn walk(base: &Path, relative: &Path, result: &mut Vec<PathBuf>) -> Result<(), AdbError> {
    let directory = base.join(relative);
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| AdbError::new(format!("cannot read {}: {error}", directory.display())))?;
    let mut directories = Vec::new();
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            AdbError::new(format!("cannot read {}: {error}", directory.display()))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() {
            if !name.starts_with('.') {
                directories.push(name);
            }
        } else if !name.starts_with('.') && !name.ends_with('~') {
            files.push(name);
        }
    }
    directories.sort();
    files.sort();
    for file in files {
        result.push(relative.join(file));
    }
    for name in directories {
        walk(base, &relative.join(name), result)?;
    }
    Ok(())
}
