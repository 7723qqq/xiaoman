//! Command implementations shared by the binary and the tests.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::adb::{self, Adb, AdbError, SpawnedChild};
use crate::lint::{self, SkillInfo};
use crate::logcat::{format_event, message_of, parse_line};
use crate::traces::{self, Zone};

static RUN_ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]+$").unwrap());

fn err_line(err: &mut dyn Write, message: impl AsRef<str>) {
    let _ = writeln!(err, "xiaoman: {}", message.as_ref());
}

/// `cli-<date>-<time>-<4 random characters>`, local time.
pub fn new_run_id() -> String {
    let now = chrono::Local::now();
    format!("cli-{}-{}", now.format("%Y%m%d-%H%M%S"), random_suffix())
}

fn random_suffix() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default();
    let mut state = (nanos ^ ((std::process::id() as u64) << 32)) | 1;
    let mut suffix = String::new();
    for _ in 0..4 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        suffix.push(ALPHABET[(state % ALPHABET.len() as u64) as usize] as char);
    }
    suffix
}

/// Prints problems and the summary; returns the exit code and the lint infos.
fn lint_and_print(
    dirs: &[PathBuf],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> (i32, Vec<SkillInfo>) {
    let infos = lint::lint_paths(dirs);
    let mut total = 0;
    for info in &infos {
        for problem in &info.problems {
            let _ = writeln!(out, "{problem}");
        }
        total += info.problems.len();
    }
    let mut names: Vec<(String, PathBuf)> = Vec::new();
    for info in &infos {
        let Some(name) = &info.name else { continue };
        if let Some((_, first)) = names.iter().find(|(seen, _)| seen == name) {
            let _ = writeln!(
                out,
                "{}: skill name '{}' is also used by {}",
                info.directory.display(),
                name,
                first.display()
            );
            total += 1;
        } else {
            names.push((name.clone(), info.directory.clone()));
        }
    }
    let ok = infos.iter().filter(|info| info.ok()).count();
    let _ = writeln!(
        err,
        "{} skill(s) checked, {} ok, {} problem(s)",
        infos.len(),
        ok,
        total
    );
    (if total > 0 { 1 } else { 0 }, infos)
}

pub fn cmd_lint(dirs: &[PathBuf], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    lint_and_print(dirs, out, err).0
}

pub fn cmd_push(
    dirs: &[PathBuf],
    adb: &dyn Adb,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<i32, AdbError> {
    let (code, infos) = lint_and_print(dirs, out, err);
    if code != 0 {
        err_line(err, "not pushing: fix the problems above first");
        return Ok(1);
    }
    for info in &infos {
        let name = info.name.as_deref().unwrap_or_default();
        let (remote, files) = adb.push_skill(&info.directory, name)?;
        let _ = writeln!(
            out,
            "pushed {} -> {} ({})",
            info.directory.display(),
            remote,
            files.join(", ")
        );
    }
    adb.broadcast(adb::ACTION_RELOAD, &[])?;
    let _ = writeln!(
        out,
        "skills reloaded; problems found by the app are logged under tag {} (xiaoman logs)",
        adb::LOG_TAG
    );
    Ok(0)
}

pub fn cmd_run(
    goal: &str,
    skill: Option<&str>,
    run_id: Option<&str>,
    follow: bool,
    adb: &dyn Adb,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<i32, AdbError> {
    let run_id = run_id.map(str::to_string).unwrap_or_else(new_run_id);
    if !RUN_ID_RE.is_match(&run_id) {
        err_line(
            err,
            "run id may only contain letters, digits, '.', '_' and '-'",
        );
        return Ok(2);
    }
    if goal.trim().is_empty() {
        err_line(err, "goal must not be empty");
        return Ok(2);
    }
    let extras: [(&str, Option<&str>); 3] = [
        ("goal", Some(goal)),
        ("skill", skill),
        ("run_id", Some(run_id.as_str())),
    ];
    if !follow {
        adb.broadcast(adb::ACTION_RUN, &extras)?;
        let _ = writeln!(out, "started run {run_id}");
        let _ = writeln!(
            out,
            "follow with: xiaoman logs --run-id {run_id}   trace: xiaoman traces show {run_id}"
        );
        return Ok(0);
    }
    // start logcat before the broadcast so no event is missed
    let mut process = spawn_logcat(adb)?;
    let result = (|| -> Result<i32, AdbError> {
        adb.broadcast(adb::ACTION_RUN, &extras)?;
        let _ = writeln!(err, "started run {run_id}");
        Ok(follow_events(
            process.as_mut(),
            Some(&run_id),
            true,
            out,
            err,
        ))
    })();
    close(process.as_mut());
    result
}

/// Prints events (optionally for one run); returns the exit code.
fn follow_events(
    process: &mut dyn SpawnedChild,
    run_id: Option<&str>,
    stop_on_terminal: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    while let Ok(Some(line)) = process.read_line() {
        match parse_line(&line) {
            None => {
                if run_id.is_none() {
                    if let Some(message) = message_of(&line) {
                        let _ = writeln!(out, "{}", message.trim_end());
                    }
                }
            }
            Some(event) => {
                if let Some(run_id) = run_id {
                    if event.run_id != run_id {
                        continue;
                    }
                }
                let _ = writeln!(out, "{}", format_event(&event));
                let _ = out.flush();
                if stop_on_terminal && event.terminal() {
                    return if event.success() == Some(true) { 0 } else { 1 };
                }
            }
        }
    }
    if stop_on_terminal {
        err_line(err, "logcat ended before the run finished");
        1
    } else {
        0
    }
}

fn close(process: &mut dyn SpawnedChild) {
    if process.try_wait().ok().flatten().is_none() {
        process.terminate();
    }
    process.wait();
}

pub fn cmd_stop(adb: &dyn Adb, out: &mut dyn Write) -> Result<i32, AdbError> {
    adb.broadcast(adb::ACTION_STOP, &[])?;
    let _ = writeln!(out, "stop requested");
    Ok(0)
}

pub fn cmd_logs(
    run_id: Option<&str>,
    adb: &dyn Adb,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<i32, AdbError> {
    if let Some(run_id) = run_id {
        if !RUN_ID_RE.is_match(run_id) {
            err_line(err, "invalid run id");
            return Ok(2);
        }
    }
    let mut process = spawn_logcat(adb)?;
    let result = follow_events(process.as_mut(), run_id, false, out, err);
    close(process.as_mut());
    Ok(result)
}

fn spawn_logcat(adb: &dyn Adb) -> Result<Box<dyn SpawnedChild>, AdbError> {
    let logcat = adb.logcat_argv();
    let args: Vec<&str> = logcat.iter().map(String::as_str).collect();
    adb.spawn(&args)
}

pub fn cmd_traces_pull(
    run_id: &str,
    out_dir: &Path,
    adb: &dyn Adb,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<i32, AdbError> {
    if !RUN_ID_RE.is_match(run_id) {
        err_line(err, "invalid run id");
        return Ok(2);
    }
    let local = adb.pull_trace(run_id, out_dir)?;
    let _ = writeln!(out, "pulled trace to {}", local.display());
    Ok(0)
}

pub fn cmd_traces_show(
    target: &str,
    dir: &Path,
    full: bool,
    adb: &dyn Adb,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<i32, AdbError> {
    let mut path = traces::resolve_trace(target, dir);
    if path.is_none() {
        if Path::new(target).exists() || !RUN_ID_RE.is_match(target) {
            err_line(err, format!("no trace.jsonl at {target}"));
            return Ok(1);
        }
        let _ = writeln!(
            err,
            "xiaoman: trace {target} not found under {}/, pulling it from the device",
            dir.display()
        );
        adb.pull_trace(target, dir)?;
        path = traces::resolve_trace(target, dir);
        if path.is_none() {
            err_line(err, "the pulled trace has no trace.jsonl");
            return Ok(1);
        }
    }
    let Some(path) = path else {
        return Ok(1);
    };
    let events = traces::load_trace(&path)
        .map_err(|error| AdbError::new(format!("cannot read {}: {error}", path.display())))?;
    let _ = writeln!(out, "{}", traces::render_table(&events, Zone::Local, full));
    let _ = writeln!(out, "{}", traces::trace_footer(&events, path.parent()));
    Ok(0)
}
