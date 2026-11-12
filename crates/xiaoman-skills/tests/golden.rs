//! The skills fixtures are shared with the Kotlin `GoldenTest`: both parse the same SKILL.md and
//! flow.yaml and compare the problem list field for field.

use std::fs;
use std::path::Path;

use serde_json::Value;
use xiaoman_skills::SkillParser;

#[test]
fn every_fixture_matches_field_for_field() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/skills");
    let mut cases = 0;
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .expect("fixtures/skills")
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    for path in paths {
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .to_string();
        let Some(case) = name.strip_suffix(".skill.md") else {
            continue;
        };
        let skill_md = fs::read_to_string(&path).expect("skill");
        let flow = fs::read_to_string(dir.join(format!("{case}.flow.yaml"))).ok();
        let (_, problems) = SkillParser::parse_folder(case, &skill_md, flow.as_deref(), case);
        let expected =
            fs::read_to_string(dir.join(format!("{case}.problems.json"))).expect("problems.json");
        let expected: Value = serde_json::from_str(&expected).expect("JSON");
        let expected = expected.as_array().expect("array");
        assert_eq!(expected.len(), problems.len(), "{case}: {problems:?}");
        for (index, expected) in expected.iter().enumerate() {
            let problem = &problems[index];
            assert_eq!(
                expected["source"].as_str(),
                Some(problem.source.as_str()),
                "{case}[{index}] source"
            );
            assert_eq!(
                expected["line"].as_u64().map(|line| line as usize),
                problem.line,
                "{case}[{index}] line"
            );
            assert_eq!(
                expected["message"].as_str(),
                Some(problem.message.as_str()),
                "{case}[{index}] message"
            );
        }
        cases += 1;
    }
    assert!(
        cases >= 8,
        "expected the shared skills fixtures, found {cases}"
    );
}
