use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use xiaoman_cli::adb::{
    Adb, AdbError, AdbOutput, RECEIVER, RealAdb, SpawnedChild, broadcast_command,
};
use xiaoman_cli::cli::{GlobalOptions, run};
use xiaoman_cli::commands::new_run_id;

const SKILLS: &str = xiaoman_cli::adb::REMOTE_SKILLS;
const TRACES: &str = xiaoman_cli::adb::REMOTE_TRACES;

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn skill_fixture() -> PathBuf {
    repo("cli/tests/fixtures/skills/ride-home")
}

fn traces_fixture() -> PathBuf {
    repo("cli/tests/fixtures/traces")
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

type Call = (String, Vec<String>);

#[derive(Clone)]
struct ProcRecord {
    terminated: Rc<Cell<bool>>,
    waited: Rc<Cell<bool>>,
}

struct FakeState {
    lines: Vec<String>,
    calls: RefCell<Vec<Call>>,
    procs: RefCell<Vec<ProcRecord>>,
    write_trace_on_pull: Cell<bool>,
}

#[derive(Clone)]
struct FakeAdb {
    state: Rc<FakeState>,
    results: HashMap<String, AdbOutput>,
    adb: String,
    serial: Option<String>,
}

impl FakeAdb {
    fn new(lines: &[&str]) -> Self {
        Self {
            state: Rc::new(FakeState {
                lines: lines.iter().map(|line| line.to_string()).collect(),
                calls: RefCell::new(Vec::new()),
                procs: RefCell::new(Vec::new()),
                write_trace_on_pull: Cell::new(false),
            }),
            results: HashMap::new(),
            adb: "adb".to_string(),
            serial: None,
        }
    }

    fn with_results(lines: &[&str], results: &[(&str, i32, &str)]) -> Self {
        let mut fake = Self::new(lines);
        for (key, status, output) in results {
            fake.results.insert(
                key.to_string(),
                AdbOutput {
                    status: *status,
                    output: output.to_string(),
                },
            );
        }
        fake
    }
}

impl Adb for FakeAdb {
    fn adb_path(&self) -> &str {
        &self.adb
    }

    fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    fn exec(&self, argv: &[String]) -> Result<AdbOutput, AdbError> {
        self.state
            .calls
            .borrow_mut()
            .push(("run".to_string(), argv.to_vec()));
        if self.state.write_trace_on_pull.get() && argv.get(1).map(String::as_str) == Some("pull") {
            let out_dir = argv.last().expect("pull destination");
            let run_id = argv
                .get(2)
                .and_then(|remote| Path::new(remote).file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let directory = Path::new(out_dir).join(run_id);
            std::fs::create_dir_all(&directory).expect("create trace directory");
            std::fs::write(
                directory.join("trace.jsonl"),
                "{\"time\": 0, \"kind\": \"Failed\", \"text\": \"boom\"}\n",
            )
            .expect("write trace");
        }
        let joined = argv.join(" ");
        for (needle, result) in &self.results {
            if joined.contains(needle) {
                return Ok(result.clone());
            }
        }
        Ok(AdbOutput {
            status: 0,
            output: "Broadcast completed: result=0\n".to_string(),
        })
    }

    fn start(&self, argv: &[String]) -> Result<Box<dyn SpawnedChild>, AdbError> {
        self.state
            .calls
            .borrow_mut()
            .push(("spawn".to_string(), argv.to_vec()));
        let record = ProcRecord {
            terminated: Rc::new(Cell::new(false)),
            waited: Rc::new(Cell::new(false)),
        };
        self.state.procs.borrow_mut().push(record.clone());
        Ok(Box::new(FakeChild {
            lines: self.state.lines.clone().into_iter(),
            terminated: record.terminated,
            waited: record.waited,
        }))
    }
}

struct FakeChild {
    lines: std::vec::IntoIter<String>,
    terminated: Rc<Cell<bool>>,
    waited: Rc<Cell<bool>>,
}

impl SpawnedChild for FakeChild {
    fn read_line(&mut self) -> io::Result<Option<String>> {
        Ok(self.lines.next())
    }

    fn try_wait(&mut self) -> io::Result<Option<i32>> {
        Ok(None)
    }

    fn terminate(&mut self) {
        self.terminated.set(true);
    }

    fn wait(&mut self) {
        self.waited.set(true);
    }
}

fn run_main(fake: &FakeAdb, argv: &[&str]) -> (i32, String, String) {
    let base = fake.clone();
    let factory = move |options: &GlobalOptions| -> Box<dyn Adb> {
        let mut client = base.clone();
        client.adb = options.adb.clone().unwrap_or_else(|| "adb".to_string());
        client.serial = options.serial.clone();
        Box::new(client)
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run(argv.iter().copied(), &factory, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).expect("utf-8 stdout"),
        String::from_utf8(err).expect("utf-8 stderr"),
    )
}

/// How the device shell will split the single remote command argument.
fn remote_words(argv: &[String]) -> Vec<String> {
    assert_eq!(argv[argv.len() - 2], "shell", "{argv:?}");
    shlex_split(&argv[argv.len() - 1])
}

fn shlex_split(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                in_word = true;
                for inner in chars.by_ref() {
                    if inner == '\'' {
                        break;
                    }
                    current.push(inner);
                }
            }
            '"' => {
                in_word = true;
                while let Some(inner) = chars.next() {
                    if inner == '"' {
                        break;
                    }
                    if inner == '\\' {
                        if let Some(escaped) = chars.next() {
                            current.push(escaped);
                        }
                    } else {
                        current.push(inner);
                    }
                }
            }
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            _ => {
                in_word = true;
                current.push(ch);
            }
        }
    }
    if in_word {
        words.push(current);
    }
    words
}

#[test]
fn quoting_roundtrip() {
    let goal = "打开 设置 里的 \"WLAN\"; rm -rf / $(x) 'q'";
    let command = broadcast_command(
        "dev.xiaoman.debug.RUN",
        &[
            ("goal", Some(goal)),
            ("skill", None),
            ("run_id", Some("r1")),
        ],
    );
    assert_eq!(
        shlex_split(&command),
        strings(&[
            "am",
            "broadcast",
            "-n",
            RECEIVER,
            "-a",
            "dev.xiaoman.debug.RUN",
            "--es",
            "goal",
            goal,
            "--es",
            "run_id",
            "r1",
        ])
    );
}

#[test]
fn run_id_format() {
    let run_id = new_run_id();
    let parts: Vec<&str> = run_id.split('-').collect();
    assert_eq!(parts.len(), 4, "{run_id}");
    assert_eq!(parts[0], "cli");
    assert!(
        parts[1].len() == 8 && parts[1].chars().all(|ch| ch.is_ascii_digit()),
        "{run_id}"
    );
    assert!(
        parts[2].len() == 6 && parts[2].chars().all(|ch| ch.is_ascii_digit()),
        "{run_id}"
    );
    assert!(
        parts[3].len() == 4
            && parts[3]
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit()),
        "{run_id}"
    );
}

#[test]
fn run_command() {
    let fake = FakeAdb::new(&[]);
    let (code, out, _) = run_main(
        &fake,
        &[
            "--serial",
            "emu1",
            "run",
            "打开 设置",
            "--skill",
            "open-settings-wifi",
            "--run-id",
            "r1",
        ],
    );
    assert_eq!(code, 0);
    let calls = fake.state.calls.borrow();
    assert_eq!(calls.len(), 1);
    let (kind, argv) = &calls[0];
    assert_eq!(kind, "run");
    assert_eq!(argv[..4].to_vec(), strings(&["adb", "-s", "emu1", "shell"]));
    assert_eq!(argv.len(), 5);
    assert_eq!(
        remote_words(argv),
        strings(&[
            "am",
            "broadcast",
            "-n",
            RECEIVER,
            "-a",
            "dev.xiaoman.debug.RUN",
            "--es",
            "goal",
            "打开 设置",
            "--es",
            "skill",
            "open-settings-wifi",
            "--es",
            "run_id",
            "r1",
        ])
    );
    assert!(out.contains("started run r1"));
}

#[test]
fn run_generates_id_and_options_after_subcommand() {
    let fake = FakeAdb::new(&[]);
    let (code, _, _) = run_main(&fake, &["run", "go", "--adb", "/opt/adb", "-s", "X"]);
    assert_eq!(code, 0);
    let calls = fake.state.calls.borrow();
    let argv = &calls[0].1;
    assert_eq!(argv[..3].to_vec(), strings(&["/opt/adb", "-s", "X"]));
    let words = remote_words(argv);
    assert!(!words.contains(&"skill".to_string()));
    let index = words
        .iter()
        .position(|word| word == "run_id")
        .expect("run_id");
    assert!(words[index + 1].starts_with("cli-"));
}

#[test]
fn adb_env_default() {
    let old = std::env::var_os("ADB");
    // SAFETY: no other test in this crate reads ADB while this test runs.
    unsafe { std::env::set_var("ADB", "/env/adb") };
    let adb = RealAdb::new(None, None);
    assert_eq!(adb.argv(&["devices"]), strings(&["/env/adb", "devices"]));
    match old {
        Some(value) => unsafe { std::env::set_var("ADB", value) },
        None => unsafe { std::env::remove_var("ADB") },
    }
}

#[test]
fn run_rejects_bad_run_id() {
    let fake = FakeAdb::new(&[]);
    let (code, _, _) = run_main(&fake, &["run", "go", "--run-id", "a b/c"]);
    assert_eq!(code, 2);
    assert!(fake.state.calls.borrow().is_empty());
}

#[test]
fn run_follow() {
    let lines = [
        "--------- beginning of main",
        "10-31 12:00:00.000  1  1 I Xiaoman: old StepPlanned {\"step\": 9}",
        "10-31 12:00:01.000  1  1 I Xiaoman: r9 PhoneTaskStarted {\"goal\": \"打开设置\"}",
        "10-31 12:00:02.000  1  1 I Xiaoman: r9 StepPlanned {\"step\": 0, \"action\": {\"type\": \"home\"}}",
        "10-31 12:00:03.000  1  1 I Xiaoman: r9 PhoneTaskFinished {\"success\": true, \"summary\": \"ok\"}",
        "10-31 12:00:04.000  1  1 I Xiaoman: r9 StepPlanned {\"step\": 99}",
    ];
    let fake = FakeAdb::new(&lines);
    let (code, out, _) = run_main(&fake, &["run", "打开设置", "--run-id", "r9", "--follow"]);
    assert_eq!(code, 0);
    let calls = fake.state.calls.borrow();
    assert_eq!(
        calls[0],
        (
            "spawn".to_string(),
            strings(&["adb", "logcat", "-v", "threadtime", "-s", "Xiaoman"])
        )
    );
    assert_eq!(calls[1].0, "run");
    let shown: Vec<&str> = out.trim().lines().collect();
    assert_eq!(shown.len(), 3);
    assert!(!out.contains("old"));
    assert!(!out.contains("#99"));
    assert!(fake.state.procs.borrow()[0].terminated.get());
}

#[test]
fn run_follow_failure_exit_code() {
    let fake = FakeAdb::new(&["I Xiaoman: r9 Failed {\"message\": \"x\"}"]);
    let (code, _, _) = run_main(&fake, &["run", "g", "--run-id", "r9", "-f"]);
    assert_eq!(code, 1);

    let fake = FakeAdb::new(&[]);
    let (code, _, err) = run_main(&fake, &["run", "g", "--run-id", "r9", "-f"]);
    assert_eq!(code, 1);
    assert!(err.contains("logcat ended"));
}

#[test]
fn stop() {
    let fake = FakeAdb::new(&[]);
    assert_eq!(run_main(&fake, &["stop"]).0, 0);
    let calls = fake.state.calls.borrow();
    assert_eq!(
        remote_words(&calls[0].1),
        strings(&[
            "am",
            "broadcast",
            "-n",
            RECEIVER,
            "-a",
            "dev.xiaoman.debug.STOP"
        ])
    );
}

#[test]
fn broadcast_error_output() {
    let fake = FakeAdb::with_results(&[], &[("STOP", 0, "Error: Bad component name")]);
    let (code, _, err) = run_main(&fake, &["stop"]);
    assert_eq!(code, 1);
    assert!(err.contains("Bad component name"));
}

#[test]
fn adb_failure() {
    let fake = FakeAdb::with_results(&[], &[("STOP", 1, "error: no devices/emulators found")]);
    let (code, _, err) = run_main(&fake, &["stop"]);
    assert_eq!(code, 1);
    assert!(err.contains("no devices"));
}

#[test]
fn adb_missing() {
    let factory = |_options: &GlobalOptions| -> Box<dyn Adb> {
        Box::new(RealAdb::new(Some("xiaoman-cli-missing-adb".to_string()), None).with_echo(false))
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run(["stop"], &factory, &mut out, &mut err);
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&err).contains("--adb"));
}

#[test]
fn logs() {
    let lines = [
        "I Xiaoman: r1 StepPlanned {\"step\": 1}",
        "I Xiaoman: r2 StepPlanned {\"step\": 2}",
        "I Xiaoman: skills reloaded: 1 problem",
    ];
    let (code, out, _) = run_main(&FakeAdb::new(&lines), &["logs"]);
    assert_eq!(code, 0);
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        vec![
            "r1 StepPlanned  #1",
            "r2 StepPlanned  #2",
            "skills reloaded: 1 problem"
        ]
    );

    let (code, out, _) = run_main(&FakeAdb::new(&lines), &["logs", "--run-id", "r2"]);
    assert_eq!(code, 0);
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["r2 StepPlanned  #2"]);
}

#[test]
fn push() {
    let fixture = skill_fixture();
    let fake = FakeAdb::new(&[]);
    let (code, out, _) = run_main(&fake, &["push", fixture.to_str().unwrap(), "--serial", "S"]);
    assert_eq!(code, 0, "{out}");
    let calls = fake.state.calls.borrow();
    let runs: Vec<&Vec<String>> = calls.iter().map(|(_, argv)| argv).collect();
    let remote = format!("{SKILLS}/ride-home");
    assert_eq!(
        remote_words(runs[0]),
        strings(&["rm", "-rf", &remote, "&&", "mkdir", "-p", &remote])
    );
    let skill_md = fixture.join("SKILL.md");
    assert_eq!(
        runs[1],
        &strings(&[
            "adb",
            "-s",
            "S",
            "push",
            skill_md.to_str().unwrap(),
            &format!("{remote}/SKILL.md")
        ])
    );
    let flow = fixture.join("flow.yaml");
    assert_eq!(
        runs[2],
        &strings(&[
            "adb",
            "-s",
            "S",
            "push",
            flow.to_str().unwrap(),
            &format!("{remote}/flow.yaml")
        ])
    );
    assert_eq!(
        remote_words(runs[3]).last().unwrap(),
        "dev.xiaoman.debug.RELOAD_SKILLS"
    );
    assert_eq!(runs.len(), 4);
}

#[test]
fn push_refuses_invalid() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let directory = tmp.path().join("bad");
    std::fs::create_dir_all(&directory).expect("mkdir");
    std::fs::write(directory.join("SKILL.md"), "---\nname: bad\n---\n").expect("write SKILL.md");
    let fake = FakeAdb::new(&[]);
    let (code, out, _) = run_main(&fake, &["push", directory.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(
        out.contains("SKILL.md:1: missing required key 'description'"),
        "{out}"
    );
    assert!(fake.state.calls.borrow().is_empty());
}

#[test]
fn lint_command() {
    let fixture = skill_fixture();
    let (code, out, err) = run_main(&FakeAdb::new(&[]), &["lint", fixture.to_str().unwrap()]);
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(err.contains("1 skill(s) checked, 1 ok"));
}

#[test]
fn traces_pull() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path().join("out");
    let fake = FakeAdb::new(&[]);
    let (code, out, _) = run_main(
        &fake,
        &["traces", "pull", "r1", "--out", out_dir.to_str().unwrap()],
    );
    assert_eq!(code, 0);
    let remote = format!("{TRACES}/r1");
    {
        let calls = fake.state.calls.borrow();
        assert_eq!(
            calls[0].1,
            strings(&["adb", "pull", &remote, out_dir.to_str().unwrap()])
        );
    }
    assert!(out_dir.is_dir());
    assert!(out.contains(&out_dir.join("r1").display().to_string()));
}

#[test]
fn traces_show_local() {
    let fake = FakeAdb::new(&[]);
    let fixture = traces_fixture();
    let (code, out, _) = run_main(
        &fake,
        &["traces", "show", "r1", "--dir", fixture.to_str().unwrap()],
    );
    assert_eq!(code, 0);
    assert!(out.contains("StepBlocked"));
    assert!(out.contains("finished: success"));
    assert!(fake.state.calls.borrow().is_empty());
}

#[test]
fn traces_show_pulls_missing_run() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fake = FakeAdb::new(&[]);
    fake.state.write_trace_on_pull.set(true);
    let (code, out, _) = run_main(
        &fake,
        &[
            "traces",
            "show",
            "r7",
            "--dir",
            tmp.path().to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("boom"));
    assert_eq!(fake.state.calls.borrow()[0].1[1], "pull");
}
