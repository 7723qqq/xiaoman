//! Port of the Kotlin `ClassTokensTest`: what the token file stores, what the `tokens` command prints,
//! and how revocations, expiry and usage accounting behave across processes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use xiaoman_web::class_tokens::{ClassTokenCodec, ClassTokenStore};
use xiaoman_web::memory::Clock;

/// 2025-11-06 11:33:20 +08:00, the instant the Kotlin test pins.
const NOW: i64 = 1_762_400_000_000;
const ALPHABET: &str = "23456789abcdefghijkmnpqrstuvwxyz";

struct Run {
    code: i32,
    out: String,
    err: String,
}

fn clock(now: &Arc<AtomicI64>) -> Clock {
    let now = now.clone();
    Arc::new(move || now.load(Ordering::SeqCst))
}

fn temp_file() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("class-tokens.json");
    (dir, file)
}

fn admin(file: &Path, now: &Arc<AtomicI64>, args: &[&str]) -> Run {
    let mut argv: Vec<String> = args.iter().map(|argument| argument.to_string()).collect();
    argv.push("--file".to_string());
    argv.push(file.display().to_string());
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = xiaoman_web::tokens_admin::run(&argv, &mut out, &mut err, &|_| None, clock(now));
    Run {
        code,
        out: String::from_utf8(out).expect("utf-8 stdout"),
        err: String::from_utf8(err).expect("utf-8 stderr"),
    }
}

fn store(file: &Path, now: &Arc<AtomicI64>) -> ClassTokenStore {
    ClassTokenStore::new(file.to_path_buf(), 480, clock(now), 0)
}

/// `标签<TAB>明文` lines of a `generate` run.
fn generated(run: &Run) -> Vec<(String, String)> {
    run.out
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| match line.split_once('\t') {
            Some((label, plain)) => (label.to_string(), plain.to_string()),
            None => panic!("not a tab-separated token line: {line:?}"),
        })
        .collect()
}

fn looks_like_token(value: &str) -> bool {
    value.len() == 23
        && value.starts_with("xm-")
        && value[3..]
            .chars()
            .all(|character| ALPHABET.contains(character))
}

#[test]
fn generate_stores_only_hashes_and_prints_the_plaintext_once() {
    let (_dir, file) = temp_file();
    let now = Arc::new(AtomicI64::new(NOW));
    let run = admin(
        &file,
        &now,
        &["generate", "3", "--daily", "1000", "--total", "5000"],
    );
    assert_eq!(run.code, 0, "{}", run.err);
    let tokens = generated(&run);
    assert_eq!(
        tokens
            .iter()
            .map(|(label, _)| label.as_str())
            .collect::<Vec<_>>(),
        vec!["学员 01", "学员 02", "学员 03"]
    );
    for (_, plain) in &tokens {
        assert!(looks_like_token(plain), "{plain}");
    }
    let distinct: std::collections::HashSet<&String> =
        tokens.iter().map(|(_, plain)| plain).collect();
    assert_eq!(distinct.len(), 3);

    let text = std::fs::read_to_string(&file).expect("token file");
    for (_, plain) in &tokens {
        assert!(!text.contains(plain), "plaintext must not be stored");
        assert!(text.contains(&ClassTokenCodec::hash(plain)));
    }
    let store = store(&file, &now);
    let first = store
        .find(&tokens[0].1)
        .expect("the first token is in the file");
    assert_eq!(first.label, "学员 01");
    assert_eq!(first.prefix, tokens[0].1[..6].to_string());
    assert_eq!(first.daily_limit, Some(1000));
    assert_eq!(first.total_limit, Some(5000));
    assert_eq!(first.expires_at, Some(NOW + 3 * 86_400_000));
    assert!(store.find(&format!("xm-{}", "a".repeat(20))).is_none());

    // Numbering continues after the existing labels.
    let more = admin(&file, &now, &["generate", "2"]);
    assert_eq!(
        generated(&more)
            .iter()
            .map(|(label, _)| label.as_str())
            .collect::<Vec<_>>(),
        vec!["学员 04", "学员 05"]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o640, "the token file is rw-r-----");
    }
    assert!(admin(&file, &now, &["list"]).out.contains("学员 05"));
}

#[test]
fn csv_output_carries_what_students_need() {
    let (_dir, file) = temp_file();
    let now = Arc::new(AtomicI64::new(NOW));
    let run = admin(
        &file,
        &now,
        &[
            "generate",
            "2",
            "--csv",
            "--label",
            "A班",
            "--base-url",
            "https://x.example/v1",
            "--model",
            "xiaoman-class",
        ],
    );
    assert_eq!(run.code, 0, "{}", run.err);
    let text = run
        .out
        .strip_prefix('\u{feff}')
        .expect("the CSV starts with a byte-order mark");
    let lines: Vec<&str> = text.trim().lines().collect();
    assert_eq!(
        lines[0],
        "标签,令牌,过期时间,每日上限,总上限,Base URL,模型名"
    );
    assert_eq!(lines.len(), 3);
    assert!(lines[1].starts_with("A班 01,xm-"), "{}", lines[1]);
    assert!(
        lines[1].ends_with(",https://x.example/v1,xiaoman-class"),
        "{}",
        lines[1]
    );
}

#[test]
fn revoke_expiry_and_quota_are_enforced() {
    let (_dir, file) = temp_file();
    let now = Arc::new(AtomicI64::new(NOW));
    let tokens = generated(&admin(
        &file,
        &now,
        &[
            "generate", "3", "--daily", "100", "--total", "150", "--hours", "2",
        ],
    ));
    let store = store(&file, &now);
    let code = |index: usize| {
        store
            .find(&tokens[index].1)
            .and_then(|token| store.check(&token))
            .map(|denied| denied.code)
    };
    assert!(code(0).is_none());
    assert_eq!(
        admin(&file, &now, &["revoke", "学员01"]).code,
        0,
        "a label without the space also matches"
    );
    assert_eq!(code(0).as_deref(), Some("token_revoked"));
    assert_eq!(
        admin(&file, &now, &["restore", &tokens[0].1]).code,
        0,
        "a full token works as a selector"
    );
    assert!(code(0).is_none());

    store.record(&store.find(&tokens[1].1).expect("token 2").hash, 100);
    assert_eq!(code(1).as_deref(), Some("daily_quota_exceeded"));
    now.fetch_add(36 * 3_600_000, Ordering::SeqCst); // past the 2-hour expiry, and a new day
    assert_eq!(code(1).as_deref(), Some("token_expired"));
    assert_eq!(
        admin(&file, &now, &["limit", "学员 02", "--days", "5"]).code,
        0
    );
    assert!(code(1).is_none(), "a new day resets the daily usage");
    store.record(&store.find(&tokens[1].1).expect("token 2").hash, 60);
    assert_eq!(code(1).as_deref(), Some("total_quota_exceeded"));
    assert_eq!(
        admin(
            &file,
            &now,
            &["limit", "--all", "--total", "none", "--daily", "none"]
        )
        .code,
        0
    );
    assert!(code(1).is_none());

    assert_eq!(admin(&file, &now, &["revoke-all"]).code, 0);
    assert!((0..3).all(|index| code(index).as_deref() == Some("token_revoked")));
    assert_eq!(admin(&file, &now, &["revoke", "不存在"]).code, 2);
    assert_eq!(admin(&file, &now, &["generate", "0"]).code, 2);
}

#[test]
fn usage_flush_merges_with_admin_changes_and_survives_a_restart() {
    let (_dir, file) = temp_file();
    let now = Arc::new(AtomicI64::new(NOW));
    let plain = generated(&admin(&file, &now, &["generate", "1", "--daily", "1000"]))
        .single()
        .1;
    let server = store(&file, &now);
    let hash = server.find(&plain).expect("token").hash;
    server.record(&hash, 40);
    server.record(&hash, 2);
    assert_eq!(
        server.find(&plain).expect("token").used_total,
        42,
        "pending usage counts before it is written"
    );
    // The admin changes the limit while usage is pending; neither change may be lost.
    assert_eq!(
        admin(&file, &now, &["limit", "学员 01", "--daily", "77"]).code,
        0
    );
    server.flush().expect("flush");
    let reread = store(&file, &now).find(&plain).expect("token");
    assert_eq!(reread.daily_limit, Some(77));
    assert_eq!(reread.used_total, 42);
    assert_eq!(reread.used_today, 42);
    assert_eq!(reread.requests, 2);
    assert_eq!(
        server.find(&plain).expect("token").used_total,
        42,
        "no double counting after the flush"
    );
    assert!(admin(&file, &now, &["list"]).out.contains("42/77"));
}

#[test]
fn a_missing_file_means_off() {
    let (_dir, file) = temp_file();
    let now = Arc::new(AtomicI64::new(NOW));
    let store = store(&file, &now);
    assert!(!store.exists());
    assert!(store.find("xm-anything").is_none());
}

trait Single<T> {
    fn single(self) -> T;
}

impl<T> Single<T> for Vec<T> {
    fn single(mut self) -> T {
        assert_eq!(self.len(), 1, "expected exactly one item");
        self.remove(0)
    }
}
