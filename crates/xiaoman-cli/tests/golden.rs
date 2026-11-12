//! The CLI fixtures are run by both CLIs: the Python reference (`cli/xiaoman.py`) and this crate.
//! Only the parts that need no device are compared here (lint and traces show); push/run/stop/logs
//! are covered by each side's fake-adb tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn normalize(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

struct Run {
    code: i32,
    out: String,
    err: String,
}

fn run(mut command: Command, args: &[&str]) -> Run {
    let output = command
        .args(args)
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .unwrap_or_else(|error| panic!("cannot run {command:?}: {error}"));
    Run {
        code: output.status.code().unwrap_or(-1),
        out: normalize(&output.stdout),
        err: normalize(&output.stderr),
    }
}

fn python(args: &[&str]) -> Run {
    let mut command = Command::new("python3");
    command.current_dir(root()).arg("cli/xiaoman.py");
    run(command, args)
}

fn rust(args: &[&str]) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xiaoman-cli"));
    command.current_dir(root());
    run(command, args)
}

fn prepared_skills() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xiaoman-cli-golden-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let fixtures = root().join("fixtures/skills");
    for case in [
        "valid",
        "bad-name",
        "missing-description",
        "flow-unknown-step",
    ] {
        let target = dir.join(case);
        fs::create_dir_all(&target).expect("skill dir");
        fs::copy(
            fixtures.join(format!("{case}.skill.md")),
            target.join("SKILL.md"),
        )
        .expect("SKILL.md");
        let flow = fixtures.join(format!("{case}.flow.yaml"));
        if flow.is_file() {
            fs::copy(flow, target.join("flow.yaml")).expect("flow.yaml");
        }
    }
    dir
}

#[test]
fn lint_matches_the_python_cli() {
    let dir = prepared_skills();
    let dirs: Vec<String> = [
        "valid",
        "bad-name",
        "missing-description",
        "flow-unknown-step",
    ]
    .iter()
    .map(|case| dir.join(case).to_string_lossy().to_string())
    .collect();
    let args: Vec<&str> = std::iter::once("lint")
        .chain(dirs.iter().map(String::as_str))
        .collect();
    let python = python(&args);
    let rust = rust(&args);
    assert_eq!(python.code, rust.code, "exit code");
    assert_eq!(python.out, rust.out, "stdout");
    assert_eq!(python.err, rust.err, "stderr");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn traces_show_matches_the_python_cli() {
    let trace = root().join("fixtures/trace/lines.jsonl");
    let trace = trace.to_string_lossy().to_string();
    let args = ["traces", "show", trace.as_str()];
    let python = python(&args);
    let rust = rust(&args);
    assert_eq!(python.code, rust.code, "exit code");
    assert_eq!(python.out, rust.out, "stdout");
    assert_eq!(python.err, rust.err, "stderr");
}
