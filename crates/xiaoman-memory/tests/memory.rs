use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use xiaoman_api::{MemoryStore, Note, Observation, Relation};
use xiaoman_memory::store::FileMemoryStore;
use xiaoman_memory::{format::BasicMemoryFormat, from_title, normalize};

fn observation(category: &str, content: &str) -> Observation {
    Observation {
        category: category.into(),
        content: content.into(),
        tags: vec![],
        context: None,
    }
}

fn observation_with(
    category: &str,
    content: &str,
    tags: &[&str],
    context: Option<&str>,
) -> Observation {
    Observation {
        category: category.into(),
        content: content.into(),
        tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
        context: context.map(str::to_string),
    }
}

fn relation(kind: &str, target: &str, context: Option<&str>) -> Relation {
    Relation {
        relation_type: kind.into(),
        target: target.into(),
        context: context.map(str::to_string),
    }
}

fn documented_example() -> &'static str {
    "---\ntitle: 家\ntype: place\npermalink: places/家\ntags: [地址]\n---\n# 家\n\n## Observations\n- [address] 北京市海淀区某某路 1 号 #常用 (用户 2026-10-30 告知)\n\n## Relations\n- lives_in [[北京]]\n"
}

#[test]
fn parses_documented_example() {
    let note = BasicMemoryFormat::parse(documented_example(), "places/家", 42);
    assert_eq!(note.title, "家");
    assert_eq!(note.note_type, "place");
    assert_eq!(note.permalink, "places/家");
    assert_eq!(note.tags, vec!["地址"]);
    assert!(note.extra_frontmatter.is_empty());
    assert_eq!(note.updated_at_millis, 42);
    assert_eq!(
        note.observations,
        vec![observation_with(
            "address",
            "北京市海淀区某某路 1 号",
            &["常用"],
            Some("用户 2026-10-30 告知")
        )]
    );
    assert_eq!(note.relations, vec![relation("lives_in", "北京", None)]);
    assert!(note.body.starts_with("# 家\n"), "{}", note.body);
}

#[test]
fn round_trips_generated_note_with_cjk() {
    let mut note = Note::new("车型", "偏好/车型");
    note.note_type = "preference".into();
    note.tags = vec!["打车".into(), "taxi".into()];
    note.observations = vec![
        observation_with("car", "优先 快车，其次 专车", &["常用", "taxi"], None),
        observation_with("note", "不要 拼车", &[], Some("用户 说过 (两次)")),
    ];
    note.relations = vec![relation("used_by", "taxi-home", Some("skill"))];

    let text = BasicMemoryFormat::render(&note);
    let parsed = BasicMemoryFormat::parse(&text, &note.permalink, 7);
    assert_eq!(parsed.title, note.title);
    assert_eq!(parsed.note_type, note.note_type);
    assert_eq!(parsed.tags, note.tags);
    assert_eq!(parsed.observations, note.observations);
    assert_eq!(parsed.relations, note.relations);
    assert!(
        parsed
            .body
            .contains("## Observations\n- [car] 优先 快车，其次 专车 #常用 #taxi\n"),
        "{}",
        parsed.body
    );
    assert!(
        parsed
            .body
            .contains("- [note] 不要 拼车 (用户 说过 (两次))"),
        "{}",
        parsed.body
    );
    assert!(
        parsed
            .body
            .contains("## Relations\n- used_by [[taxi-home]] (skill)\n"),
        "{}",
        parsed.body
    );
    // Rendering the parsed note again is stable.
    assert_eq!(text, BasicMemoryFormat::render(&parsed));
}

#[test]
fn preserves_extra_frontmatter() {
    let text = "---\ntitle: Alice\ntype: person\npermalink: people/alice\ntags: friend, work\ncreated: 2026-10-30\naliases: [Ally, 爱丽丝]\nmeta:\n  source: obsidian\n  rank: 3\n---\nFree text mentioning [[Bob|her colleague]] and [[Carol#Work]].\n";
    let note = BasicMemoryFormat::parse(text, "people/alice", 0);
    assert_eq!(note.tags, vec!["friend", "work"]);
    let mut keys: Vec<&str> = note.extra_frontmatter.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, vec!["aliases", "created", "meta"]);
    assert_eq!(
        note.extra_frontmatter["aliases"],
        serde_json::json!(["Ally", "爱丽丝"])
    );
    assert_eq!(
        note.extra_frontmatter["meta"],
        serde_json::json!({"source": "obsidian", "rank": 3})
    );
    assert_eq!(
        note.relations,
        vec![
            relation("links_to", "Bob", None),
            relation("links_to", "Carol", None)
        ]
    );

    let reparsed = BasicMemoryFormat::parse(&BasicMemoryFormat::render(&note), "people/alice", 0);
    assert_eq!(note.extra_frontmatter, reparsed.extra_frontmatter);
    assert_eq!(note.body, reparsed.body);
    assert_eq!(note.relations, reparsed.relations);
}

#[test]
fn ignores_checkboxes_links_and_code() {
    let body = "# T\n- [ ] todo item\n- [x] done item\n- [docs](https://example.com)\n```\n- [fake] inside code [[Nope]]\n```\n- [fact] real one\nInline `[[NotALink]]` text.\n";
    let (observations, relations) = xiaoman_memory::format::parse_body(body);
    assert_eq!(observations, vec![observation("fact", "real one")]);
    assert!(relations.is_empty(), "{relations:?}");
}

#[test]
fn given_body_gets_missing_observations_and_relations() {
    let body = "# 家\n\n前言文字。\n\n## Observations\n- [address] 旧地址\n\n## Notes\n其它\n";
    let mut note = Note::new("家", "家");
    note.body = body.into();
    note.observations = vec![
        observation("address", "旧地址"),
        observation_with("phone", "123", &["常用"], None),
    ];
    note.relations = vec![relation("lives_in", "北京", None)];

    let parsed = BasicMemoryFormat::parse(&BasicMemoryFormat::render(&note), "家", 0);
    assert_eq!(parsed.observations, note.observations);
    assert_eq!(parsed.relations, note.relations);
    assert!(
        parsed
            .body
            .contains("## Observations\n- [address] 旧地址\n- [phone] 123 #常用\n\n## Notes"),
        "{}",
        parsed.body
    );
    assert!(
        parsed
            .body
            .trim_end()
            .ends_with("## Relations\n- lives_in [[北京]]"),
        "{}",
        parsed.body
    );
    assert!(
        parsed.body.starts_with("# 家\n\n前言文字。"),
        "{}",
        parsed.body
    );
}

#[test]
fn observations_section_created_before_relations() {
    let mut note = Note::new("X", "x");
    note.body = "# X\n\n## Relations\n- knows [[Y]]\n".into();
    note.observations = vec![observation("fact", "new")];
    let body = BasicMemoryFormat::parse(&BasicMemoryFormat::render(&note), "x", 0).body;
    assert_eq!(
        body,
        "# X\n\n## Observations\n- [fact] new\n\n## Relations\n- knows [[Y]]\n"
    );
}

#[test]
fn falls_back_to_heading_and_permalink_for_title() {
    assert_eq!(
        BasicMemoryFormat::parse("# Heading\ntext\n", "a/b", 0).title,
        "Heading"
    );
    let bare = BasicMemoryFormat::parse("just text\n", "a/b", 0);
    assert_eq!(bare.title, "b");
    assert_eq!(bare.note_type, "note");
    assert_eq!(bare.body, "just text\n");
}

#[test]
fn parses_observation_parts() {
    let parsed =
        BasicMemoryFormat::parse_observation("- [tech] Uses f(x) for #math and #ml (from paper)")
            .unwrap();
    assert_eq!(
        parsed,
        observation_with(
            "tech",
            "Uses f(x) for and",
            &["math", "ml"],
            Some("from paper")
        )
    );
    assert!(BasicMemoryFormat::parse_observation("plain line").is_none());
    assert_eq!(
        BasicMemoryFormat::parse_relation("- depends on [[库]] (v2)"),
        Some(relation("depends on", "库", Some("v2")))
    );
}

struct Fixture {
    dir: tempfile::TempDir,
    now: Arc<AtomicI64>,
    store: FileMemoryStore,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let now = Arc::new(AtomicI64::new(1_700_000_000_000));
    let clock = now.clone();
    let store = FileMemoryStore::with_clock(
        dir.path().to_path_buf(),
        Arc::new(move || clock.load(Ordering::SeqCst)),
    );
    Fixture { dir, now, store }
}

impl Fixture {
    fn tick(&self) {
        self.now.fetch_add(10_000, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn write_and_read_by_permalink_and_title() {
    let fixture = fixture();
    let mut note = Note::new("家", "places/家");
    note.note_type = "place".into();
    note.tags = vec!["地址".into()];
    note.observations = vec![observation_with(
        "address",
        "北京市海淀区某某路 1 号",
        &["常用"],
        None,
    )];
    note.relations = vec![relation("lives_in", "北京", None)];

    let stored = fixture.store.write(note).await.unwrap();
    assert_eq!(stored.updated_at_millis, fixture.now.load(Ordering::SeqCst));
    let file = fixture.dir.path().join("places/家.md");
    assert!(file.is_file());
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("- [address] 北京市海淀区某某路 1 号 #常用"),
        "{text}"
    );

    assert_eq!(
        fixture.store.read("places/家").await.unwrap(),
        Some(stored.clone())
    );
    assert_eq!(
        fixture.store.read("places/家.md").await.unwrap(),
        Some(stored.clone())
    );
    assert_eq!(fixture.store.read("家").await.unwrap(), Some(stored));
    assert_eq!(fixture.store.read("nowhere").await.unwrap(), None);
}

#[tokio::test]
async fn title_lookup_is_case_insensitive_for_latin() {
    let fixture = fixture();
    fixture
        .store
        .write(Note::new("Home Address", ""))
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .read("home address")
            .await
            .unwrap()
            .map(|note| note.permalink),
        Some("home-address".to_string())
    );
    assert_eq!(
        fixture
            .store
            .read("HOME ADDRESS")
            .await
            .unwrap()
            .map(|note| note.title),
        Some("Home Address".to_string())
    );
}

#[tokio::test]
async fn generates_permalinks_from_titles() {
    let fixture = fixture();
    assert_eq!(
        fixture
            .store
            .write(Note::new("家的地址", ""))
            .await
            .unwrap()
            .permalink,
        "家的地址"
    );
    assert_eq!(
        fixture
            .store
            .write(Note::new("Taxi: Home  v2!", ""))
            .await
            .unwrap()
            .permalink,
        "taxi-home-v2"
    );
    // Same slug, different title: does not overwrite.
    assert_eq!(
        fixture
            .store
            .write(Note::new("taxi home v2", ""))
            .await
            .unwrap()
            .permalink,
        "taxi-home-v2-2"
    );
    // Same title again: replaces in place.
    assert_eq!(
        fixture
            .store
            .write(Note::new("Taxi: Home  v2!", ""))
            .await
            .unwrap()
            .permalink,
        "taxi-home-v2"
    );
    assert_eq!(fixture.store.list(None).await.unwrap().len(), 3);
}

#[tokio::test]
async fn rejects_path_traversal() {
    let fixture = fixture();
    for bad in [
        "../escape",
        "a/../../b",
        "/etc/passwd",
        "C:/x",
        "a//b",
        "a/./b",
        "a/",
        ".hidden/x",
        "..\\x",
    ] {
        assert!(normalize(bad).is_err(), "{bad}");
        assert!(
            fixture.store.write(Note::new("x", bad)).await.is_err(),
            "{bad}"
        );
    }
    assert!(
        fixture
            .store
            .append_observation("../x", observation("a", "b"))
            .await
            .is_err()
    );
    assert!(
        !fixture
            .dir
            .path()
            .parent()
            .unwrap()
            .join("escape.md")
            .exists()
    );
    assert_eq!(fixture.store.read("../escape").await.unwrap(), None);
}

#[tokio::test]
async fn append_observation_creates_note() {
    let fixture = fixture();
    let note = fixture
        .store
        .append_observation("偏好/车型", observation("car", "快车"))
        .await
        .unwrap();
    assert_eq!(note.title, "车型");
    assert_eq!(note.note_type, "note");
    assert_eq!(note.permalink, "偏好/车型");
    assert_eq!(note.observations, vec![observation("car", "快车")]);
    assert!(note.body.starts_with("# 车型\n"), "{}", note.body);
}

#[tokio::test]
async fn append_observation_keeps_existing_body_and_frontmatter() {
    let fixture = fixture();
    std::fs::create_dir_all(fixture.dir.path().join("people")).unwrap();
    std::fs::write(
        fixture.dir.path().join("people/alice.md"),
        "---\ntitle: Alice\ntype: person\naliases: [Ally]\n---\n# Alice\n\nMet at work.\n\n## Observations\n- [role] engineer\n",
    )
    .unwrap();

    let updated = fixture
        .store
        .append_observation("Alice", observation_with("likes", "tea", &["drink"], None))
        .await
        .unwrap();
    assert_eq!(updated.permalink, "people/alice");
    assert_eq!(
        updated.extra_frontmatter["aliases"],
        serde_json::json!(["Ally"])
    );
    assert_eq!(
        updated.observations,
        vec![
            observation("role", "engineer"),
            observation_with("likes", "tea", &["drink"], None)
        ]
    );
    assert!(updated.body.contains("Met at work."), "{}", updated.body);
    assert_eq!(
        fixture.store.read("people/alice").await.unwrap(),
        Some(updated)
    );
}

#[tokio::test]
async fn chinese_search_finds_observation() {
    let fixture = fixture();
    let mut home = Note::new("住址", "places/home");
    home.observations = vec![observation("fact", "家 地址 北京市海淀区")];
    fixture.store.write(home).await.unwrap();

    let mut work = Note::new("公司", "places/work");
    work.observations = vec![observation("fact", "公司 在 上海")];
    fixture.store.write(work).await.unwrap();

    let mut diary = Note::new("日记", "diary");
    diary.body = "今天的天气很好，我们的心情也好。".into();
    fixture.store.write(diary).await.unwrap();

    let hits = fixture.store.search("家的地址", 10).await.unwrap();
    assert_eq!(hits.first().unwrap().permalink, "places/home");
    assert_eq!(hits.first().unwrap().snippet, "[fact] 家 地址 北京市海淀区");
    assert!(
        hits.iter().all(|hit| hit.permalink != "diary"),
        "weak 的 matches are dropped: {hits:?}"
    );

    assert_eq!(
        fixture.store.search("家", 10).await.unwrap()[0].permalink,
        "places/home"
    );
    assert_eq!(
        fixture.store.search("上海", 10).await.unwrap()[0].permalink,
        "places/work"
    );
    assert!(
        fixture.store.search("你好", 10).await.unwrap().is_empty(),
        "single shared characters alone are not a match"
    );
}

#[tokio::test]
async fn title_matches_are_boosted() {
    let fixture = fixture();
    let mut coffee = Note::new("Coffee", "coffee");
    coffee.observations = vec![observation("fact", "brewed daily")];
    fixture.store.write(coffee).await.unwrap();
    let mut morning = Note::new("Morning", "morning");
    morning.observations = vec![observation("fact", "drink coffee then tea")];
    fixture.store.write(morning).await.unwrap();

    let hits = fixture.store.search("coffee", 10).await.unwrap();
    assert_eq!(
        hits.iter()
            .map(|hit| hit.permalink.as_str())
            .collect::<Vec<_>>(),
        vec!["coffee", "morning"]
    );
    assert!(hits[0].score > hits[1].score);
    assert_eq!(fixture.store.search("coffee", 1).await.unwrap().len(), 1);
}

#[tokio::test]
async fn snippets_are_clipped_around_the_match() {
    let fixture = fixture();
    let long = format!("{} 目标词 {}", "填充".repeat(100), "尾巴".repeat(100));
    let mut note = Note::new("长", "long");
    note.observations = vec![observation("x", &long)];
    fixture.store.write(note).await.unwrap();

    let snippet = fixture
        .store
        .search("目标词", 10)
        .await
        .unwrap()
        .remove(0)
        .snippet;
    assert!(
        snippet.chars().count() <= xiaoman_memory::index::Snippets::MAX_LENGTH + 2,
        "{snippet}"
    );
    assert!(snippet.contains("目标词"), "{snippet}");
}

#[tokio::test]
async fn delete_removes_file_and_search_entries() {
    let fixture = fixture();
    let mut note = Note::new("秘密", "secrets/秘密");
    note.observations = vec![observation("pin", "银行 密码 提示")];
    fixture.store.write(note).await.unwrap();

    assert_eq!(fixture.store.search("密码", 10).await.unwrap().len(), 1);
    assert!(fixture.store.delete("秘密").await.unwrap());
    assert!(!fixture.dir.path().join("secrets/秘密.md").exists());
    assert!(
        !fixture.dir.path().join("secrets").exists(),
        "empty folder is pruned"
    );
    assert!(fixture.store.search("密码", 10).await.unwrap().is_empty());
    assert_eq!(fixture.store.read("secrets/秘密").await.unwrap(), None);
    assert!(!fixture.store.delete("secrets/秘密").await.unwrap());
}

#[tokio::test]
async fn list_filters_by_folder() {
    let fixture = fixture();
    for (title, permalink) in [
        ("a", "howto/a"),
        ("b", "howto/sub/b"),
        ("c", "howtox/c"),
        ("d", "d"),
    ] {
        fixture
            .store
            .write(Note::new(title, permalink))
            .await
            .unwrap();
    }
    let folder = |notes: Vec<Note>| {
        notes
            .into_iter()
            .map(|note| note.permalink)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        folder(fixture.store.list(Some("howto")).await.unwrap()),
        vec!["howto/a", "howto/sub/b"]
    );
    assert_eq!(
        folder(fixture.store.list(Some("/howto/")).await.unwrap()),
        vec!["howto/a", "howto/sub/b"]
    );
    assert_eq!(fixture.store.list(None).await.unwrap().len(), 4);
    assert_eq!(fixture.store.list(Some("")).await.unwrap().len(), 4);
}

#[tokio::test]
async fn external_changes_are_seen() {
    let fixture = fixture();
    let mut old = Note::new("旧", "old");
    old.observations = vec![observation("x", "苹果")];
    fixture.store.write(old).await.unwrap();
    assert!(fixture.store.search("香蕉", 10).await.unwrap().is_empty());

    // Added by adb / Obsidian.
    std::fs::create_dir_all(fixture.dir.path().join("inbox")).unwrap();
    std::fs::write(
        fixture.dir.path().join("inbox/水果.md"),
        "---\ntitle: 水果\n---\n# 水果\n\n- [fruit] 香蕉 很甜\n",
    )
    .unwrap();
    assert_eq!(
        fixture
            .store
            .search("香蕉", 10)
            .await
            .unwrap()
            .remove(0)
            .permalink,
        "inbox/水果"
    );

    // Edited externally (different size and mtime).
    std::fs::write(
        fixture.dir.path().join("old.md"),
        "---\ntitle: 旧\n---\n- [x2] 葡萄 酸\n",
    )
    .unwrap();
    assert_eq!(
        fixture
            .store
            .search("葡萄", 10)
            .await
            .unwrap()
            .remove(0)
            .permalink,
        "old"
    );
    assert!(
        fixture
            .store
            .search("苹果", 10)
            .await
            .unwrap()
            .iter()
            .all(|hit| hit.permalink != "old"),
        "old content is gone"
    );

    // Removed externally; hidden folders are ignored.
    std::fs::remove_file(fixture.dir.path().join("inbox/水果.md")).unwrap();
    std::fs::create_dir_all(fixture.dir.path().join(".obsidian")).unwrap();
    std::fs::write(
        fixture.dir.path().join(".obsidian/香蕉.md"),
        "- [fruit] 香蕉\n",
    )
    .unwrap();
    assert!(fixture.store.search("香蕉", 10).await.unwrap().is_empty());
    assert_eq!(
        fixture
            .store
            .list(None)
            .await
            .unwrap()
            .into_iter()
            .map(|note| note.permalink)
            .collect::<Vec<_>>(),
        vec!["old"]
    );
}

#[tokio::test]
async fn empty_query_returns_recent_notes() {
    let fixture = fixture();
    let mut one = Note::new("one", "one");
    one.observations = vec![observation("x", "first")];
    fixture.store.write(one).await.unwrap();
    fixture.tick();
    fixture.store.write(Note::new("two", "two")).await.unwrap();
    fixture.tick();
    fixture
        .store
        .write(Note::new("three", "three"))
        .await
        .unwrap();

    let titles = |notes: Vec<xiaoman_api::SearchHit>| {
        notes
            .into_iter()
            .map(|hit| hit.permalink)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        titles(fixture.store.search("", 10).await.unwrap()),
        vec!["three", "two", "one"]
    );
    assert_eq!(
        titles(fixture.store.search("  ", 2).await.unwrap()),
        vec!["three", "two"]
    );
    assert_eq!(
        fixture
            .store
            .search("", 10)
            .await
            .unwrap()
            .last()
            .unwrap()
            .snippet,
        "[x] first"
    );
    assert!(fixture.store.search("！？", 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn write_with_body_reflects_observations() {
    let fixture = fixture();
    let mut note = Note::new("T", "t");
    note.body = "# T\n\nIntro [[Other]].\n".into();
    note.observations = vec![observation("fact", "added")];

    let stored = fixture.store.write(note).await.unwrap();
    assert_eq!(stored.observations, vec![observation("fact", "added")]);
    assert_eq!(stored.relations, vec![relation("links_to", "Other", None)]);
    assert!(
        stored
            .body
            .starts_with("# T\n\nIntro [[Other]].\n\n## Observations\n- [fact] added"),
        "{}",
        stored.body
    );
}

#[test]
fn slugs_match_the_kotlin_implementation() {
    assert_eq!(from_title("家的地址"), "家的地址");
    assert_eq!(from_title("Taxi: Home  v2!"), "taxi-home-v2");
}
