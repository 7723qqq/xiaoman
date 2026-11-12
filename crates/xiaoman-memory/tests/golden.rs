//! The memory fixtures are shared with the Kotlin `GoldenTest`: both parse the same input and
//! render it byte for byte, so a change on one side breaks the other.

use std::fs;
use std::path::Path;

use xiaoman_memory::BasicMemoryFormat;

fn permalink_of(input: &str) -> Option<String> {
    let mut lines = input.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix("permalink:") {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

#[test]
fn every_fixture_renders_byte_for_byte() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/memory");
    let mut cases = 0;
    for entry in fs::read_dir(&dir).expect("fixtures/memory") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .to_string();
        let Some(case) = name.strip_suffix(".input.md") else {
            continue;
        };
        let input = fs::read_to_string(&path).expect("input");
        let expected = fs::read_to_string(dir.join(format!("{case}.rendered.md")))
            .unwrap_or_else(|_| panic!("{case}.rendered.md"));
        let permalink = permalink_of(&input).unwrap_or_else(|| case.to_string());
        let note = BasicMemoryFormat::parse(&input, &permalink, 0);
        assert_eq!(expected, BasicMemoryFormat::render(&note), "{case}");
        cases += 1;
    }
    assert!(
        cases >= 4,
        "expected the shared memory fixtures, found {cases}"
    );
}
