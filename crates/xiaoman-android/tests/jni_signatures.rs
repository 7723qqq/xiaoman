//! Guards the Kotlin <-> Rust seam of the Android host.
//!
//! 1. Every `external fun` must have a `Java_<package>_<class>_<method>` export with exactly two
//!    extra parameters (`JNIEnv`, `jobject`), so a Kotlin signature change cannot silently
//!    misalign the JNI call.
//! 2. Every method Rust invokes on the Kotlin `NotifierBridge` must exist in the host.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const HOSTS: [(&str, &str); 1] = [(
    "gpui",
    "../xiaoman-gpui/android/app/src/main/kotlin/dev/xiaoman",
)];

/// `(relative path under the host, package, class)` — the four classes that declare natives.
const NATIVE_CLASSES: [(&str, &str, &str); 4] = [
    ("app/RustSession.kt", "dev.xiaoman.app", "RustSession"),
    ("app/AgentService.kt", "dev.xiaoman.app", "AgentService"),
    (
        "hands/XiaomanAccessibilityService.kt",
        "dev.xiaoman.hands",
        "XiaomanAccessibilityService",
    ),
    ("hands/Smoke.kt", "dev.xiaoman.hands", "Smoke"),
];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The text between the parenthesis at `open` and its match.
fn paren_content(text: &str, open: usize) -> &str {
    let mut depth = 0i32;
    for (offset, ch) in text[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced parentheses in:\n{}", &text[open..]);
}

/// Splits a parameter list on top-level commas, dropping an empty trailing segment.
fn split_top_level(content: &str) -> Vec<String> {
    let mut depth = 0i32;
    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in content.chars() {
        match ch {
            '(' | '<' | '[' => {
                depth += 1;
                current.push(ch);
            }
            ')' | '>' | ']' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
            }
            other => current.push(other),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

/// The declared type of a Kotlin or Rust parameter: everything after `:`, minus `?` and generics.
fn parameter_type(parameter: &str) -> String {
    let Some((_, declared)) = parameter.split_once(':') else {
        panic!("parameter without a type: `{parameter}`");
    };
    let declared = declared.trim().trim_end_matches('?').trim();
    match declared.find('<') {
        Some(index) => declared[..index].trim().to_string(),
        None => declared.to_string(),
    }
}

/// The Rust JNI types that can carry one Kotlin argument type.
fn accepted_rust_types(kotlin: &str) -> Vec<&'static str> {
    match kotlin {
        // A nullable Java String is a JObject at the JNI seam; both spellings are fine.
        "String" => vec!["JString", "JObject"],
        "Long" => vec!["jlong"],
        "Int" => vec!["jint"],
        "Boolean" => vec!["jboolean"],
        "Any" => vec!["JObject"],
        other => panic!("no JNI mapping for Kotlin type `{other}`; extend the test"),
    }
}

fn kotlin_natives(path: &Path) -> Vec<(String, Vec<String>)> {
    let text =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(index) = rest.find("external fun ") {
        let after = &rest[index + "external fun ".len()..];
        let Some(name_end) = after.find('(') else {
            panic!("{}: external fun without arguments", path.display());
        };
        let name = after[..name_end].trim().to_string();
        let open = name_end;
        let types = split_top_level(paren_content(after, open))
            .iter()
            .map(|parameter| parameter_type(parameter))
            .collect();
        out.push((name, types));
        rest = &after[open..];
    }
    out
}

fn rust_exports(path: &Path) -> Vec<(String, Vec<String>)> {
    let text =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(index) = rest.find("fn Java_") {
        let after = &rest[index + 3..];
        let Some(name_end) = after.find('(') else {
            panic!("{}: export without arguments", path.display());
        };
        let name = after[..name_end].trim().to_string();
        let types = split_top_level(paren_content(after, name_end))
            .iter()
            .map(|parameter| parameter_type(parameter))
            .collect();
        out.push((name, types));
        rest = &after[name_end..];
    }
    out
}

#[test]
fn every_kotlin_native_matches_its_export() {
    let exports: Vec<(String, Vec<String>)> = [
        crate_dir().join("src/session.rs"),
        crate_dir().join("src/jni_exports.rs"),
    ]
    .iter()
    .flat_map(|path| rust_exports(path))
    .collect();

    let mut checked = 0usize;
    for (host, root) in HOSTS {
        for (relative, package, class) in NATIVE_CLASSES {
            let path = crate_dir().join(root).join(relative);
            let natives = kotlin_natives(&path);
            assert!(!natives.is_empty(), "{host}/{relative}: no external fun");
            for (name, kotlin_types) in natives {
                let symbol = format!("Java_{}_{}_{}", package.replace('.', "_"), class, name);
                let Some((_, rust_types)) = exports.iter().find(|(export, _)| *export == symbol)
                else {
                    panic!("{host}/{relative}: `{name}` has no Rust export `{symbol}`");
                };
                let jni_types: Vec<&String> = rust_types.iter().skip(2).collect();
                assert_eq!(
                    jni_types.len(),
                    kotlin_types.len(),
                    "{host}/{relative}: `{name}` takes {} Java arguments, `{symbol}` takes {} JNI \
                     parameters after `JNIEnv`/`jobject`",
                    kotlin_types.len(),
                    jni_types.len()
                );
                for (index, (kotlin, rust)) in kotlin_types.iter().zip(jni_types).enumerate() {
                    let accepted = accepted_rust_types(kotlin);
                    assert!(
                        accepted.contains(&rust.as_str()),
                        "{host}/{relative}: `{name}` argument {index} is `{kotlin}` in Kotlin but \
                         `{rust}` in `{symbol}` (expected {accepted:?})"
                    );
                }
                checked += 1;
            }
        }
    }
    assert!(checked >= 29, "only {checked} natives checked");
}

/// Method names Rust calls on the Kotlin notifier object.
fn notifier_methods(session_rs: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for marker in ["call_notifier(\"", "call_notifier_bool(\""] {
        let mut rest = session_rs;
        while let Some(index) = rest.find(marker) {
            let tail = &rest[index + marker.len()..];
            let end = tail.find('"').expect("method name");
            out.insert(tail[..end].to_string());
            rest = &tail[end..];
        }
    }
    let mut rest = session_rs;
    while let Some(index) = rest.find("notifier.as_obj(),") {
        let tail = &rest[index + "notifier.as_obj(),".len()..];
        let after = tail.trim_start();
        if let Some(stripped) = after.strip_prefix('"') {
            if let Some(end) = stripped.find('"') {
                out.insert(stripped[..end].to_string());
            }
        }
        rest = tail;
    }
    out
}

#[test]
fn the_host_implements_the_notifier_contract() {
    let session = fs::read_to_string(crate_dir().join("src/session.rs")).expect("session.rs");
    let methods = notifier_methods(&session);
    assert!(methods.contains("readSecret") && methods.contains("writeSecret"));
    assert!(
        methods.len() >= 15,
        "only {} notifier methods found",
        methods.len()
    );

    for (host, root) in HOSTS {
        let path = crate_dir().join(root).join("app/NotifierBridge.kt");
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{host}: {}: {error}", path.display()));
        for method in &methods {
            assert!(
                text.contains(&format!("fun {method}(")),
                "{host}: NotifierBridge has no `fun {method}(`, but Rust calls it"
            );
        }
    }
}
