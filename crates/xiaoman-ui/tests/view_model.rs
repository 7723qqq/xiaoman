//! The screen logic, without a window: `cargo test -p xiaoman-ui --test view_model`.
//!
//! Ports the intent of `app/src/test/kotlin/dev/xiaoman/app/ui/ChatPresentationTest.kt` (folding,
//! card identity, rich text) and covers the rules the Compose screens own rather than the shared
//! `chat_item` rules: the approval card only for `Decision::Ask`, error rows that never fold, the
//! settings form round trip, and the memory/skill row mapping.

use xiaoman_api::{
    Action, ActionResult, AgentEvent, ApprovalScope, Decision, Flow, FlowStep, Note, SearchHit,
    Skill, SkillChannel, SkillLoadResult, SkillProblem,
};
use xiaoman_app::chat_item::{ChatItem, chat_rich_text, step_text};
use xiaoman_app::model_probe::{models_outcome, test_outcome};
use xiaoman_app::settings::{AppSettings, ModelSettings, SettingsForm};
use xiaoman_mcp::{CatalogKey, CatalogServer, ConfirmPolicy, McpSelection, ServerStatus};
use xiaoman_ui::view_model::chat::{ApprovalCard, ChatModel, ChatRow, MessageKind, Submit};
use xiaoman_ui::view_model::copy;
use xiaoman_ui::view_model::mcp::{McpModel, StatusRow};
use xiaoman_ui::view_model::memory::{MemoryModel, hit_rows, note_rows};
use xiaoman_ui::view_model::settings::{Endpoint, Field, SettingsModel};
use xiaoman_ui::view_model::skills::{ProblemRow, SkillsModel, skill_rows};

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

fn tap(target: &str) -> Action {
    Action::Tap {
        point: None,
        node_id: None,
        target: Some(target.to_string()),
        reason: None,
    }
}

fn back() -> Action {
    Action::Back {
        target: None,
        reason: None,
    }
}

fn sample_items() -> Vec<ChatItem> {
    vec![
        ChatItem::User {
            id: 1,
            text: "选本笔记本".to_string(),
        },
        ChatItem::Progress {
            id: 2,
            text: "开始".to_string(),
            ok: None,
        },
        ChatItem::Progress {
            id: 3,
            text: "1. 搜索".to_string(),
            ok: Some(true),
        },
        ChatItem::Question {
            id: 4,
            text: "选哪种？".to_string(),
            takeover: false,
        },
        ChatItem::Progress {
            id: 5,
            text: "已确认".to_string(),
            ok: Some(true),
        },
        ChatItem::Error {
            id: 6,
            text: "断网".to_string(),
        },
        ChatItem::Assistant {
            id: 7,
            text: "尚未购买".to_string(),
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Chat timeline
// ---------------------------------------------------------------------------------------------

#[test]
fn folding_keeps_question_error_and_final_answer_visible() {
    let rows = ChatModel::with_items(sample_items()).rows();

    assert_eq!(
        6,
        rows.len(),
        "user, process, question, process, error, assistant"
    );

    let ChatRow::Process(card) = &rows[1] else {
        panic!("row 1 is the folded progress card")
    };
    assert_eq!(2, card.entries.len());
    assert_eq!("开始", card.entries[0].text);
    assert_eq!("1. 搜索", card.entries[1].text);
    assert_eq!(copy::process_header(2), card.header);

    let ChatRow::Message(question) = &rows[2] else {
        panic!("row 2 is the question")
    };
    assert_eq!(MessageKind::Question { takeover: false }, question.kind);
    assert_eq!(Some(copy::CHAT_QUESTION_TITLE.to_string()), question.title);
    assert_eq!("选哪种？", question.body.text);

    let ChatRow::Process(confirmed) = &rows[3] else {
        panic!("row 3 is the second progress card")
    };
    assert_eq!(1, confirmed.entries.len());
    assert_eq!("已确认", confirmed.entries[0].text);
    assert_eq!(Some(true), confirmed.entries[0].ok);

    let ChatRow::Message(error) = &rows[4] else {
        panic!("row 4 is the error")
    };
    assert_eq!(MessageKind::Error, error.kind);
    assert_eq!(Some(copy::CHAT_ERROR_TITLE.to_string()), error.title);
    assert_eq!("断网", error.body.text);

    let ChatRow::Message(answer) = &rows[5] else {
        panic!("row 5 is the reply")
    };
    assert_eq!(7, answer.id);
    assert_eq!(MessageKind::Assistant, answer.kind);
    assert_eq!(Some(copy::CHAT_ASSISTANT_NAME.to_string()), answer.title);
}

#[test]
fn folding_then_unfolding_is_lossless() {
    let original = sample_items();
    let rows = ChatModel::with_items(original.clone()).rows();

    // Walk the original items alongside the rows: a message row consumes one item, a card
    // consumes exactly the run of progress items it folded.
    let mut cursor = 0;
    for row in &rows {
        match row {
            ChatRow::Message(message) => {
                assert_eq!(
                    original[cursor].id(),
                    message.id,
                    "message ids stay in chat order"
                );
                cursor += 1;
            }
            ChatRow::Process(card) => {
                for entry in &card.entries {
                    let ChatItem::Progress { id, text, .. } = &original[cursor] else {
                        panic!("a card only folds progress items")
                    };
                    assert_eq!(entry.text, *text);
                    assert_eq!(original[cursor].id(), *id);
                    cursor += 1;
                }
            }
        }
    }
    assert_eq!(original.len(), cursor, "every item was shown exactly once");

    let texts: Vec<String> = rows
        .iter()
        .flat_map(|row| match row {
            ChatRow::Message(message) => vec![message.body.text.clone()],
            ChatRow::Process(card) => card
                .entries
                .iter()
                .map(|entry| entry.text.clone())
                .collect(),
        })
        .collect();
    assert_eq!(original.len(), texts.len());
}

#[test]
fn progress_rows_collapse_and_keep_their_identity() {
    let first = ChatItem::Progress {
        id: 12,
        text: "搜索".to_string(),
        ok: None,
    };
    let mut model = ChatModel::with_items(vec![first]);

    let collapsed = model.rows();
    assert_eq!(1, collapsed.len());
    let ChatRow::Process(card) = &collapsed[0] else {
        panic!("a lone progress entry folds")
    };
    assert!(!card.expanded, "process cards start collapsed");
    let identity = collapsed[0].id();

    model.push_item(ChatItem::Progress {
        id: 13,
        text: "进入详情".to_string(),
        ok: Some(true),
    });
    let grown = model.rows();
    assert_eq!(1, grown.len(), "appending progress keeps one card");
    assert_eq!(identity, grown[0].id(), "and keeps the first entry's id");

    model.toggle(identity);
    let ChatRow::Process(card) = &model.rows()[0] else {
        panic!("still a process card")
    };
    assert!(card.expanded, "clicking the header opens the card");
    assert_eq!(2, card.entries.len());

    model.set_expanded(identity, false);
    let ChatRow::Process(card) = &model.rows()[0] else {
        panic!("still a process card")
    };
    assert!(!card.expanded);

    assert!(ChatModel::with_items(Vec::new()).rows().is_empty());
}

#[test]
fn error_rows_never_fold_into_the_process_card() {
    let model = ChatModel::with_items(vec![
        ChatItem::Progress {
            id: 1,
            text: "开始".to_string(),
            ok: None,
        },
        ChatItem::Error {
            id: 2,
            text: "断网".to_string(),
        },
        ChatItem::Progress {
            id: 3,
            text: "重试".to_string(),
            ok: None,
        },
    ]);
    let rows = model.rows();
    assert_eq!(3, rows.len(), "the error splits the run into two cards");
    assert!(matches!(rows[0], ChatRow::Process(_)));
    assert!(matches!(rows[1], ChatRow::Message(_)));
    assert!(matches!(rows[2], ChatRow::Process(_)));

    // A later reply still leaves the error where it was, in its own row.
    let mut model = ChatModel::with_items(vec![ChatItem::Error {
        id: 1,
        text: "断网".to_string(),
    }]);
    model.push_item(ChatItem::Assistant {
        id: 2,
        text: "稍后再试".to_string(),
    });
    let rows = model.rows();
    assert_eq!(2, rows.len());
    let ChatRow::Message(error) = &rows[0] else {
        panic!("the error keeps its row")
    };
    assert_eq!(1, error.id);
    assert_eq!(MessageKind::Error, error.kind);
}

#[test]
fn bold_price_and_lists_render_without_changing_their_meaning() {
    let body = chat_rich_text("- **合计**：**2.40元**\n尚未付款");
    assert_eq!("• 合计：2.40元\n尚未付款", body.text);
    assert_eq!(2, body.spans.len());
    assert_eq!(
        "2 * 3 = 6，**未结束",
        chat_rich_text("2 * 3 = 6，**未结束").text
    );

    // The message row carries that rendering, not the raw markers.
    let row = ChatModel::with_items(vec![ChatItem::Assistant {
        id: 1,
        text: "- **合计**：**2.40元**".to_string(),
    }])
    .rows()
    .remove(0);
    let ChatRow::Message(message) = row else {
        panic!("a reply is a message row")
    };
    assert_eq!("• 合计：2.40元", message.body.text);
    assert_eq!(2, message.body.spans.len());
}

// ---------------------------------------------------------------------------------------------
// Approval card
// ---------------------------------------------------------------------------------------------

#[test]
fn the_approval_card_waits_only_for_ask() {
    let ask = Decision::Ask {
        prompt: "要付款 2.40 元".to_string(),
        reason: "花钱".to_string(),
    };
    let card = ApprovalCard::for_decision(1, &ask).expect("Ask opens the card");
    assert_eq!("要付款 2.40 元", card.prompt);
    assert_eq!(copy::APPROVAL_TITLE, card.title);
    assert!(!card.responding);
    assert!(!card.more);

    assert!(
        ApprovalCard::for_decision(
            2,
            &Decision::Allow {
                reason: "读屏幕".to_string()
            }
        )
        .is_none()
    );
    assert!(
        ApprovalCard::for_decision(
            3,
            &Decision::Deny {
                reason: "支付".to_string()
            }
        )
        .is_none()
    );
}

#[test]
fn ask_opens_the_card_and_the_answer_leaves_one_progress_line() {
    let mut model = ChatModel::new();
    model.apply(&AgentEvent::PhoneTaskStarted {
        task_id: "run-1".to_string(),
        goal: "买笔记本".to_string(),
        skill_name: Some("taobao".to_string()),
    });
    model.apply(&AgentEvent::StepBlocked {
        task_id: "run-1".to_string(),
        step_index: 1,
        action: tap("付款"),
        decision: Decision::Ask {
            prompt: "要付款 2.40 元".to_string(),
            reason: "花钱".to_string(),
        },
    });

    let card = model.approval().expect("the Ask decision opens the card");
    assert_eq!("要付款 2.40 元", card.prompt);
    assert_eq!(Some("等待确认：要付款 2.40 元"), model.status());

    model.set_more(true);
    assert!(model.approval().expect("still open").more);

    model.approve(ApprovalScope::Once);
    assert!(model.approval().is_none(), "answering closes the card");
    assert_eq!(None, model.status(), "and restores the previous status");

    let rows = model.rows();
    let ChatRow::Process(card) = rows.last().expect("the answer leaves a line") else {
        panic!("the answer line folds with the step rows")
    };
    let answer = card.entries.last().expect("the answer is the last entry");
    assert_eq!("已允许：要付款 2.40 元", answer.text);
    assert_eq!(Some(true), answer.ok);
}

#[test]
fn a_denied_or_allowed_step_never_opens_the_card() {
    let mut model = ChatModel::new();
    model.apply(&AgentEvent::StepBlocked {
        task_id: "run-1".to_string(),
        step_index: 0,
        action: tap("支付"),
        decision: Decision::Deny {
            reason: "敏感操作".to_string(),
        },
    });
    assert!(model.approval().is_none());
    let ChatRow::Process(card) = &model.rows()[0] else {
        panic!("the blocked step is a progress row")
    };
    assert!(card.failed(), "a blocked step paints in the error color");
    assert!(card.entries[0].text.contains("已拦截"));
    assert!(card.entries[0].text.contains("敏感操作"));

    let mut model = ChatModel::new();
    model.apply(&AgentEvent::StepBlocked {
        task_id: "run-1".to_string(),
        step_index: 0,
        action: tap("读屏幕"),
        decision: Decision::Allow {
            reason: "只读".to_string(),
        },
    });
    assert!(model.approval().is_none(), "Allow never asks");
}

#[test]
fn the_task_scope_answer_says_so_in_the_transcript() {
    let mut model = ChatModel::new();
    assert!(model.request_approval(&Decision::Ask {
        prompt: "删除这条笔记".to_string(),
        reason: "写操作".to_string(),
    }));
    model.approve(ApprovalScope::Task);
    let ChatRow::Process(card) = &model.rows()[0] else {
        panic!("the answer is progress")
    };
    assert_eq!("本次任务都允许：删除这条笔记", card.entries[0].text);

    assert!(!model.request_approval(&Decision::Allow {
        reason: "只读".to_string()
    }));
}

// ---------------------------------------------------------------------------------------------
// Session behaviour
// ---------------------------------------------------------------------------------------------

#[test]
fn the_input_answers_a_question_before_it_starts_a_turn() {
    let mut model = ChatModel::new().configured(true);
    model.apply(&AgentEvent::NeedsUser {
        task_id: "run-1".to_string(),
        question: "买哪种颜色？".to_string(),
        takeover: false,
    });
    assert_eq!(
        Some("买哪种颜色？"),
        model.question().map(|question| question.text.as_str())
    );
    assert!(model.busy() || !model.busy(), "state is readable");
    assert_eq!(copy::CHAT_ANSWER_PLACEHOLDER, model.placeholder());
    assert!(
        model.can_send("红色"),
        "a pending question can be answered while busy"
    );

    assert_eq!(Submit::Answered, model.submit("红色"));
    assert!(model.question().is_none());
    assert_eq!(Submit::Empty, model.submit("   "));
    assert_eq!(copy::CHAT_PLACEHOLDER, model.placeholder());
    assert!(Submit::Answered.clears_draft());
    assert!(!Submit::Busy.clears_draft());
    assert!(
        Submit::NotConfigured.clears_draft(),
        "the refusal clears the draft like Compose"
    );
}

#[test]
fn an_unconfigured_session_shows_the_message_banner_and_error() {
    let mut model = ChatModel::new();
    let banners = model.banners();
    assert_eq!(1, banners.len());
    assert_eq!(copy::BANNER_MODELS, banners[0].text);
    assert_eq!(copy::BANNER_MODELS_ACTION, banners[0].action);

    assert_eq!(Submit::NotConfigured, model.submit("帮我买笔记本"));
    assert!(!model.can_send("   "));
    let rows = model.rows();
    assert_eq!(2, rows.len());
    let ChatRow::Message(error) = &rows[1] else {
        panic!("the refusal is an error row")
    };
    assert_eq!(MessageKind::Error, error.kind);
    assert_eq!(copy::SESSION_NOT_CONFIGURED, error.body.text);
    assert_eq!(copy::STATUS_UNCONFIGURED, model.header().status);

    let configured = ChatModel::new().configured(true);
    assert!(configured.banners().is_empty());
    assert_eq!(copy::STATUS_READY, configured.header().status);
}

#[test]
fn the_header_reports_attention_before_busy() {
    let mut model = ChatModel::new().configured(true);

    model.apply(&AgentEvent::PhoneTaskStarted {
        task_id: "run-1".to_string(),
        goal: "买笔记本".to_string(),
        skill_name: None,
    });
    assert!(model.busy());
    assert_eq!(copy::STATUS_BUSY, model.header().status);
    assert_eq!(
        Some("开始：买笔记本"),
        model.status_line(),
        "the last step text stands in"
    );
    assert!(model.shows_stop(), "a plain run offers Stop");

    // A run that has not produced a step yet falls back to 正在想办法…
    let mut quiet = ChatModel::new().configured(true);
    quiet.submit("买笔记本");
    assert_eq!(Some(copy::CHAT_THINKING), quiet.status_line());

    model.apply(&AgentEvent::NeedsUser {
        task_id: "run-1".to_string(),
        question: "选哪种？".to_string(),
        takeover: true,
    });
    assert_eq!(copy::STATUS_ATTENTION, model.header().status);
    assert!(model.header().attention);
    assert!(!model.shows_stop(), "answering replaces the Stop button");

    model.apply(&AgentEvent::PhoneTaskFinished {
        task_id: "run-1".to_string(),
        success: true,
        summary: "已下单".to_string(),
        steps: 3,
    });
    assert!(!model.busy(), "a finished run frees the session");
    assert_eq!(copy::STATUS_READY, model.header().status);
    assert_eq!(None, model.status_line());
    let rows = model.rows();
    let ChatRow::Process(card) = rows.last().expect("the result is a progress row") else {
        panic!("the task result folds into the card")
    };
    assert_eq!(Some(true), card.entries.last().expect("result entry").ok);
    assert!(
        card.entries
            .last()
            .expect("result entry")
            .text
            .contains("任务完成")
    );
}

#[test]
fn stop_releases_the_pending_prompts() {
    let mut model = ChatModel::new().configured(true);
    assert_eq!(Submit::Started, model.submit("买笔记本"));
    assert!(model.busy());

    model.request_approval(&Decision::Ask {
        prompt: "允许返回？".to_string(),
        reason: "导航".to_string(),
    });
    assert!(model.approval().is_some());

    model.stop();
    assert!(model.approval().is_none());
    assert_eq!(Some(copy::SESSION_STOPPED), model.status());
}

#[test]
fn a_busy_session_rejects_a_second_turn() {
    let mut model = ChatModel::new().configured(true);
    assert_eq!(Submit::Started, model.submit("买笔记本"));
    assert_eq!(Submit::Busy, model.submit("再买一个"));
    model.finish();
    assert_eq!(Submit::Started, model.submit("再买一个"));
}

#[test]
fn step_rows_match_the_shared_event_mapping() {
    let event = AgentEvent::StepExecuted {
        task_id: "run-1".to_string(),
        step_index: 2,
        action: tap("搜索"),
        result: ActionResult::failed("没有找到"),
    };
    let mut model = ChatModel::new().configured(true);
    model.apply(&event);

    let ChatRow::Process(card) = &model.rows()[0] else {
        panic!("a step folds into a card")
    };
    let ChatItem::Progress { text, ok, .. } = step_text::chat_item(&event, 2).expect("a row")
    else {
        panic!("a step is progress")
    };
    assert_eq!(text, card.entries[0].text);
    assert_eq!(ok, card.entries[0].ok);
    assert!(card.failed());
    assert!(card.entries[0].text.contains("没有找到"));

    let mut model = ChatModel::new().configured(true);
    model.apply(&AgentEvent::StepBlocked {
        task_id: "run-1".to_string(),
        step_index: 0,
        action: back(),
        decision: Decision::Allow {
            reason: "导航".to_string(),
        },
    });
    assert!(model.approval().is_none());
}

// ---------------------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------------------

fn configured_settings() -> AppSettings {
    AppSettings {
        chat: ModelSettings {
            base_url: "https://ark.example.com/api/v3".to_string(),
            model: "doubao-chat".to_string(),
            api_key: "sk-chat".to_string(),
            temperature: Some(0.7),
        },
        operator: ModelSettings {
            base_url: "https://ark.example.com/api/v3".to_string(),
            model: "doubao-vision".to_string(),
            api_key: "sk-operator".to_string(),
            temperature: None,
        },
        max_steps: 24,
        learn_from_success: true,
    }
}

#[test]
fn the_settings_form_round_trips_through_the_screen() {
    let settings = configured_settings();
    let mut model = SettingsModel::new(&settings);

    assert_eq!(
        "https://ark.example.com/api/v3",
        model.field(Endpoint::Chat, Field::BaseUrl)
    );
    assert_eq!(
        "doubao-vision",
        model.field(Endpoint::Operator, Field::Model)
    );
    assert_eq!("0.7", model.field(Endpoint::Chat, Field::Temperature));
    assert_eq!("", model.field(Endpoint::Operator, Field::Temperature));
    assert_eq!("24", model.max_steps());
    assert!(model.learn());
    assert!(model.can_save());
    assert_eq!(
        settings,
        model.to_settings().expect("an untouched form parses back")
    );

    model.set_field(Endpoint::Chat, Field::Model, "doubao-chat-2");
    model.set_field(Endpoint::Chat, Field::Temperature, "1");
    model.set_max_steps("40");
    model.set_learn(false);

    let edited = model.to_settings().expect("every field still parses");
    assert_eq!("doubao-chat-2", edited.chat.model);
    assert_eq!(
        Some(1.0),
        edited.chat.temperature,
        "an integer temperature keeps its point"
    );
    assert_eq!(40, edited.max_steps);
    assert!(!edited.learn_from_success);
    assert_eq!(
        "sk-operator", edited.operator.api_key,
        "the other endpoint is untouched"
    );

    let reopened = SettingsModel::new(&edited);
    assert_eq!(
        SettingsForm::of(&edited),
        *reopened.form(),
        "reopening shows what was saved, with the temperature as text"
    );
    assert_eq!("1.0", reopened.field(Endpoint::Chat, Field::Temperature));
    assert_eq!(
        edited,
        reopened.to_settings().expect("the saved form parses")
    );
}

#[test]
fn invalid_numbers_block_the_save_button() {
    let mut model = SettingsModel::new(&configured_settings());
    model.set_max_steps("0");
    assert!(!model.max_steps_valid());
    let rows = model.rows();
    assert!(rows.max_steps.invalid);
    assert_eq!(
        Some(copy::SETTINGS_INVALID_NUMBER.to_string()),
        rows.max_steps.help
    );
    assert!(!rows.save.enabled);
    assert!(model.save().is_none(), "an invalid form cannot be saved");

    model.set_max_steps("12");
    model.set_field(Endpoint::Chat, Field::Temperature, "3.5");
    assert!(!model.temperature_valid(Endpoint::Chat));
    assert!(model.rows().chat.temperature.invalid);
    assert_eq!(
        Some(copy::SETTINGS_INVALID_NUMBER.to_string()),
        model.rows().chat.temperature.help
    );
    assert!(!model.can_save());

    model.set_field(Endpoint::Chat, Field::Temperature, "");
    assert!(
        model.temperature_valid(Endpoint::Chat),
        "an empty temperature means unset"
    );
    assert_eq!(None, model.to_settings().expect("parses").chat.temperature);
    assert!(model.can_save());
}

#[test]
fn save_marks_the_note_and_copying_the_chat_endpoint_copies_address_and_key() {
    let mut model = SettingsModel::new(&AppSettings::default());
    assert_eq!(None, model.rows().saved_label);

    model.set_field(Endpoint::Chat, Field::BaseUrl, "https://api.example.com/v1");
    model.set_field(Endpoint::Chat, Field::ApiKey, "sk-new");
    model.copy_chat_to_operator();
    assert_eq!(
        "https://api.example.com/v1",
        model.field(Endpoint::Operator, Field::BaseUrl)
    );
    assert_eq!("sk-new", model.field(Endpoint::Operator, Field::ApiKey));
    assert_eq!(
        "",
        model.field(Endpoint::Operator, Field::Model),
        "the model name is not copied"
    );

    assert!(!model.saved());
    assert!(model.save().is_some());
    assert_eq!(
        Some(copy::SETTINGS_SAVED.to_string()),
        model.rows().saved_label
    );

    model.set_max_steps("30");
    assert_eq!(
        None,
        model.rows().saved_label,
        "an edit hides the note again"
    );
}

#[test]
fn the_api_key_is_masked_until_it_is_revealed() {
    let mut model = SettingsModel::new(&configured_settings());
    let rows = model.rows();
    assert_eq!(
        "sk-chat", rows.chat.api_key.value,
        "the raw value stays editable"
    );
    assert_eq!("•••••••", rows.chat.api_key.display());
    assert!(rows.chat.api_key.secret);
    assert!(!rows.chat.api_key.revealed);
    assert_eq!(copy::SETTINGS_CHAT_MODEL, rows.chat.title);
    assert_eq!(copy::SETTINGS_OPERATOR_MODEL, rows.operator.title);
    assert!(rows.learn.on);

    model.toggle_reveal(Endpoint::Chat);
    assert!(model.revealed(Endpoint::Chat));
    assert_eq!("sk-chat", model.rows().chat.api_key.display());
    assert_eq!("•••••••••••", model.rows().operator.api_key.display());

    let folders = SettingsModel::new(&configured_settings())
        .with_folders("/skills", "/memory", "/traces")
        .rows()
        .folders;
    assert_eq!(
        vec![
            copy::settings_folder_skills("/skills"),
            copy::settings_folder_memory("/memory"),
            copy::settings_folder_traces("/traces")
        ],
        folders
    );
}

// ---------------------------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------------------------

fn note(title: &str, permalink: &str, updated: i64) -> Note {
    let mut note = Note::new(title, permalink);
    note.updated_at_millis = updated;
    note
}

#[test]
fn memory_rows_are_newest_first_and_hits_flatten_their_snippet() {
    let notes = vec![
        note("家", "places/家", 100),
        note("口味", "prefs/口味", 300),
        note("公司", "places/公司", 200),
    ];
    let rows = note_rows(&notes);
    assert_eq!(
        vec!["prefs/口味", "places/公司", "places/家"],
        rows.iter()
            .map(|row| row.permalink.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        "prefs/口味", rows[0].detail,
        "a listed note shows its permalink"
    );

    let hits = vec![SearchHit {
        permalink: "places/家".to_string(),
        title: "家".to_string(),
        snippet: "北京市海淀区\n某某路 1 号".to_string(),
        score: 1.5,
    }];
    let rows = hit_rows(&hits);
    assert_eq!("北京市海淀区 某某路 1 号", rows[0].detail);
    assert_eq!("家", rows[0].title);
}

#[test]
fn the_memory_screen_walks_query_to_rows_to_note() {
    let mut model = MemoryModel::new();
    assert!(model.loading());
    assert_eq!(None, model.state().empty_hint(), "no hint while loading");

    model.set_notes(&[]);
    assert_eq!(Some(copy::MEMORY_EMPTY), model.state().empty_hint());

    model.set_query("笔记本");
    assert!(model.loading());
    model.set_hits(&[]);
    assert_eq!(Some(copy::MEMORY_NO_HITS), model.state().empty_hint());

    let notes = [note("家", "places/家", 1)];
    model.set_hits(&[SearchHit {
        permalink: "places/家".to_string(),
        title: "家".to_string(),
        snippet: "海淀".to_string(),
        score: 1.0,
    }]);
    assert_eq!(1, model.rows().len());
    assert_eq!(None, model.state().empty_hint());

    model.open_note(&notes[0]);
    assert_eq!("家", model.open().expect("the note is open").title);
    model.deleted();
    assert!(model.open().is_none());
    assert!(model.loading(), "deleting reloads the list");

    model.open_missing();
    assert_eq!(Some(copy::MEMORY_NOT_FOUND), model.state().not_found());
    model.close();
    assert_eq!(None, model.state().not_found());

    model.set_failed();
    assert_eq!(Some(copy::MEMORY_NO_HITS), model.state().empty_hint());
}

#[test]
fn the_note_body_drops_markdown_markers() {
    let mut note = note("家", "places/家", 1);
    note.body = "## Observations\n- [address] **北京市海淀区**".to_string();
    let mut model = MemoryModel::new();
    model.open_note(&note);
    let body = &model.open().expect("open").body;
    assert_eq!("Observations\n• [address] 北京市海淀区", body.text);
    assert_eq!(1, body.spans.len());
}

// ---------------------------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------------------------

fn skill(name: &str, apps: &[&str], steps: usize) -> Skill {
    Skill {
        name: name.to_string(),
        description: format!("{name}的描述"),
        apps: apps.iter().map(|app| (*app).to_string()).collect(),
        channel: SkillChannel::Gui,
        confirm_before: Vec::new(),
        memory_refs: Vec::new(),
        deeplinks: Vec::new(),
        instructions: String::new(),
        flow: (steps > 0).then(|| Flow {
            steps: (0..steps)
                .map(|index| FlowStep::Ai {
                    goal: format!("step {index}"),
                })
                .collect(),
        }),
        source: format!("/skills/{name}/SKILL.md"),
    }
}

#[test]
fn skill_rows_carry_their_apps_and_flow_indicator() {
    let rows = skill_rows(&[
        skill(
            "taxi-home",
            &["com.sdu.didi.psnger", "com.autonavi.minimap"],
            3,
        ),
        skill("note", &[], 0),
    ]);
    assert_eq!("taxi-home", rows[0].name);
    assert_eq!("taxi-home的描述", rows[0].description);
    assert_eq!(
        Some(copy::skills_apps(
            "com.sdu.didi.psnger, com.autonavi.minimap"
        )),
        rows[0].apps
    );
    assert_eq!(
        Some(copy::skills_flow(3)),
        rows[0].flow.as_ref().map(|flow| flow.label.clone())
    );
    assert_eq!("/skills/taxi-home/SKILL.md", rows[0].source);
    assert_eq!(None, rows[1].apps);
    assert_eq!(
        None, rows[1].flow,
        "a skill without flow.yaml shows no indicator"
    );
    assert_eq!("App：a", copy::skills_apps("a"));
}

#[test]
fn loading_problems_and_the_empty_hint() {
    let mut model = SkillsModel::new().with_folder("/skills");
    assert!(model.loading());
    assert_eq!(None, model.empty_hint());

    model.set_result(&SkillLoadResult::default());
    assert_eq!(Some(copy::skills_empty("/skills")), model.empty_hint());

    model.reload();
    assert!(model.loading());
    model.set_result(&SkillLoadResult {
        skills: vec![skill("note", &[], 0)],
        problems: vec![SkillProblem {
            source: "/skills/broken/SKILL.md".to_string(),
            message: "缺少 name".to_string(),
            line: Some(4),
        }],
    });
    assert!(!model.loading());
    assert_eq!(None, model.empty_hint());
    assert_eq!(
        vec![ProblemRow {
            source: "/skills/broken/SKILL.md:4".to_string(),
            message: "缺少 name".to_string(),
        }],
        model.problems().to_vec()
    );
    assert_eq!(copy::SKILLS_PROBLEMS, "加载问题");

    model.set_failed("磁盘不可读");
    assert_eq!(Some(copy::skills_empty("/skills")), model.empty_hint());
    assert_eq!("磁盘不可读", model.problems()[0].message);
    assert_eq!(
        "/skills",
        model.problems()[0].source,
        "no line number when unknown"
    );
}

// ---------------------------------------------------------------------------------------------
// MCP
// ---------------------------------------------------------------------------------------------

fn catalog_server(
    id: &str,
    title: &str,
    category: &str,
    official: bool,
    key: Option<&str>,
) -> CatalogServer {
    CatalogServer {
        id: id.to_string(),
        title: title.to_string(),
        category: category.to_string(),
        description: format!("{title}描述"),
        official,
        entry: serde_json::Map::new(),
        keys: key
            .map(|name| {
                vec![CatalogKey {
                    name: name.to_string(),
                    label: format!("{name}的key"),
                    help: Some("https://example.com/apply".to_string()),
                }]
            })
            .unwrap_or_default(),
        platforms: Default::default(),
    }
}

fn mcp_model() -> McpModel {
    McpModel::with_servers(
        McpSelection::default(),
        vec![
            catalog_server("amap", "高德地图", "出行", true, Some("AMAP_KEY")),
            catalog_server("aihot", "AI 热搜", "资讯", false, None),
        ],
    )
}

#[test]
fn mcp_sections_group_the_catalog_and_report_missing_keys() {
    let mut model = mcp_model();
    assert_eq!("已启用 0 个", model.enabled_count_label());

    let sections = model.sections();
    assert_eq!(2, sections.len());
    assert_eq!("出行", sections[0].category);
    assert_eq!("高德地图", sections[0].servers[0].title);
    assert_eq!("高德地图描述", sections[0].servers[0].description);
    assert_eq!(copy::MCP_OFFICIAL, sections[0].servers[0].badge);
    assert_eq!(copy::MCP_COMMUNITY, sections[1].servers[0].badge);
    assert_eq!(
        None, sections[0].servers[0].missing,
        "an unticked server asks for nothing"
    );

    model.set_enabled("amap", true);
    assert_eq!("已启用 1 个", model.enabled_count_label());
    let row = &model.sections()[0].servers[0];
    assert!(row.enabled);
    assert_eq!(
        Some(copy::mcp_missing_key("AMAP_KEY的key")),
        row.missing,
        "an enabled server with a blank key says what is missing"
    );

    model.set_key("AMAP_KEY", "  abc  ");
    let row = &model.sections()[0].servers[0];
    assert_eq!(None, row.missing, "a pasted key clears the warning");
    assert_eq!(
        "abc", row.keys[0].value,
        "keys are trimmed as they are pasted"
    );
    assert!(!model.saved());

    model.apply_recommended();
    assert_eq!("已启用 2 个", model.enabled_count_label());
    assert!(model.is_enabled("aihot"));

    model.toggle_enabled("aihot");
    assert!(!model.is_enabled("aihot"));
    assert_eq!("已启用 1 个", model.enabled_count_label());
}

#[test]
fn mcp_test_results_become_status_lines() {
    let mut model = mcp_model();
    model.set_testing(true);
    assert!(model.testing());

    model.record_statuses(vec![
        ServerStatus {
            name: "amap".to_string(),
            title: "高德地图".to_string(),
            connected: true,
            tool_count: 7,
            error: None,
        },
        ServerStatus {
            name: "aihot".to_string(),
            title: "AI 热搜".to_string(),
            connected: false,
            tool_count: 0,
            error: Some("超时".to_string()),
        },
    ]);
    assert!(!model.testing(), "the test ends");
    let connected = model.status("amap").expect("a connected line");
    assert_eq!(copy::mcp_connected(7), connected.label());
    assert!(connected.ok());
    let failed = model.status("aihot").expect("a failure is still a line");
    assert_eq!(copy::mcp_failed("超时"), failed.label());
    assert!(!failed.ok());
    assert_eq!(
        StatusRow::Failed {
            error: "超时".to_string(),
            label: copy::mcp_failed("超时")
        },
        *failed
    );

    let sections = model.sections();
    assert_eq!(
        Some(copy::mcp_connected(7)),
        sections[0].servers[0]
            .status
            .as_ref()
            .map(|s| s.label().to_string())
    );
}

#[test]
fn custom_servers_validate_their_url_and_clear_the_form() {
    let mut model = mcp_model();
    assert!(!model.can_add_custom(), "a blank address cannot be added");
    assert_eq!(None, model.url_problem(), "nor is a blank address an error");

    model.set_custom_url("http://example.com/mcp");
    assert_eq!(Some("只支持 https 地址"), model.url_problem());
    assert_eq!(
        Some(copy::mcp_custom_invalid("只支持 https 地址")),
        model.custom_form().problem
    );
    assert!(!model.add_custom());

    model.set_custom_url("not a url");
    assert_eq!(Some("地址无效"), model.url_problem());

    model.set_custom_url("https://example.com/mcp?token=1");
    model.set_custom_title("我的服务");
    model.set_custom_auth("Bearer t");
    model.set_custom_confirm(ConfirmPolicy::All);
    assert!(model.can_add_custom());
    let form = model.custom_form();
    assert_eq!(3, form.confirm_options.len());
    assert!(form.confirm_options[1].2, "每次都确认 is the selected chip");
    assert_eq!(copy::MCP_CUSTOM_ADD, form.add_label);
    assert_eq!("Bearer t", form.auth_value);

    assert!(model.add_custom());
    assert_eq!("已启用 1 个", model.enabled_count_label());
    let rows = model.custom_rows();
    assert_eq!("我的服务", rows[0].title);
    assert_eq!(
        "https://example.com/mcp", rows[0].detail,
        "the query string is hidden"
    );
    let added = &model.selection().custom[0];
    assert_eq!(ConfirmPolicy::All, added.confirm);
    assert_eq!("Bearer t", added.headers["Authorization"]);

    assert_eq!("", model.custom_form().url_value, "adding clears the form");
    assert_eq!(ConfirmPolicy::Writes, model.custom_confirm());
    assert!(!model.saved());

    model.remove_custom(0);
    assert!(model.custom_rows().is_empty());
    assert_eq!("已启用 0 个", model.enabled_count_label());
}

#[test]
fn an_untitled_custom_server_falls_back_to_its_url() {
    let mut model = mcp_model();
    model.set_custom_url("https://example.com/mcp");
    assert!(model.add_custom());
    assert_eq!("https://example.com/mcp", model.custom_rows()[0].title);
}

#[test]
fn the_mcp_screen_saves_the_selection_it_shows() {
    let mut model = mcp_model();
    model.set_enabled("aihot", true);
    assert!(!model.saved());
    let saved = model.save();
    assert!(model.saved());
    assert!(saved.enabled.contains("aihot"));
    assert_eq!(*model.selection(), saved);
}

// ---------------------------------------------------------------------------------------------
// The live session's snapshot
// ---------------------------------------------------------------------------------------------

fn session_snapshot() -> xiaoman_app::session::SessionSnapshot {
    xiaoman_app::session::SessionSnapshot {
        items: vec![
            ChatItem::User {
                id: 1,
                text: "记住我的偏好".to_string(),
            },
            ChatItem::Progress {
                id: 2,
                text: "已更新记忆 preferences/风格".to_string(),
                ok: None,
            },
            ChatItem::Assistant {
                id: 3,
                text: "记住了。".to_string(),
            },
        ],
        busy: true,
        status: Some("第 1 步：点击 确认".to_string()),
        question: None,
        approval: Some(xiaoman_app::session::PendingApproval {
            id: 4,
            task_id: "task-1".to_string(),
            prompt: "即将点击「确认呼叫」，是否继续？".to_string(),
            reason: "confirmation word '确认呼叫' in target".to_string(),
        }),
        skill_draft: None,
        configured: true,
    }
}

#[test]
fn a_session_snapshot_drives_the_chat_screen() {
    let mut model = ChatModel::new();
    model.apply_snapshot(&session_snapshot());

    assert_eq!(3, model.items().len(), "the transcript is the session's");
    assert!(model.busy());
    assert_eq!(Some("第 1 步：点击 确认"), model.status_line());
    let card = model.approval().expect("the approval card is open");
    assert_eq!(copy::APPROVAL_TITLE, card.title);
    assert!(card.reason.contains("confirmation word"));
    assert!(model.header().ready);
    assert!(model.header().attention);

    // The same card stays the same card: 更多选项 survives a refresh.
    model.set_more(true);
    model.apply_snapshot(&session_snapshot());
    assert!(model.approval().unwrap().more);

    // Answering closes the card and the session's own progress line carries the verdict.
    let mut answered = session_snapshot();
    answered.approval = None;
    answered.items.push(ChatItem::Progress {
        id: 5,
        text: "已允许：即将点击「确认呼叫」，是否继续？".to_string(),
        ok: Some(true),
    });
    model.apply_snapshot(&answered);
    assert!(model.approval().is_none());
    assert_eq!(4, model.items().len());

    // A finished run clears busy and the status line.
    let finished = xiaoman_app::session::SessionSnapshot {
        busy: false,
        status: None,
        ..answered
    };
    model.apply_snapshot(&finished);
    assert!(!model.busy());
    assert!(model.status_line().is_none());
}

#[test]
fn a_snapshot_with_a_question_fills_the_input_hint() {
    let mut snapshot = session_snapshot();
    snapshot.question = Some(xiaoman_app::session::PendingQuestion {
        id: 9,
        task_id: "task-1".to_string(),
        text: "选哪种？".to_string(),
        takeover: false,
    });
    snapshot.approval = None;
    let mut model = ChatModel::new();
    model.apply_snapshot(&snapshot);

    assert_eq!("选哪种？", model.question().unwrap().text);
    assert_eq!(copy::CHAT_ANSWER_PLACEHOLDER, model.placeholder());
    assert!(!model.shows_stop(), "answering a question is not a stop");
    assert!(model.can_send("A5"));
}

#[test]
fn a_fetch_fills_the_picker_and_a_test_leaves_it_alone() {
    let mut model = SettingsModel::new(&configured_settings());

    model.apply_probe(
        Endpoint::Chat,
        models_outcome(Ok(vec![
            "doubao-pro".to_string(),
            "doubao-lite".to_string(),
        ])),
    );
    let rows = model.rows();
    assert_eq!(rows.chat.models, ["doubao-pro", "doubao-lite"]);
    assert!(rows.chat.picker_open, "拉取成功后下拉自动展开");
    let probe = rows.chat.probe.expect("a probe line");
    assert_eq!(probe.message, "获取到 2 个模型");
    assert!(probe.ok);
    assert!(rows.operator.models.is_empty(), "另一个端点不受影响");
    assert!(!rows.operator.picker_open);

    // 测试成功：只换提示，不动列表，也不收起正在看的列表。
    model.apply_probe(Endpoint::Chat, test_outcome(Ok(())));
    assert_eq!(model.models(Endpoint::Chat), ["doubao-pro", "doubao-lite"]);
    assert!(model.rows().chat.picker_open);
    let probe = model.rows().chat.probe.expect("a probe line");
    assert_eq!(probe.message, "连接成功");
    assert!(probe.ok);

    // 测试失败：原文照出，列表还在。
    model.apply_probe(Endpoint::Chat, test_outcome(Err("连接超时".to_string())));
    assert_eq!(model.models(Endpoint::Chat).len(), 2);
    let probe = model.rows().chat.probe.expect("a probe line");
    assert_eq!(probe.message, "连接超时");
    assert!(!probe.ok);

    // 拉取失败：清空旧列表，下拉收起（没有可选的模型）。
    model.apply_probe(Endpoint::Chat, models_outcome(Err("HTTP 401".to_string())));
    assert!(model.models(Endpoint::Chat).is_empty());
    assert!(!model.rows().chat.picker_open);
    assert!(!model.rows().chat.probe.expect("a probe line").ok);
}

#[test]
fn picking_a_listed_model_writes_it_back_and_closes_the_picker() {
    let mut model = SettingsModel::new(&configured_settings());
    model.apply_probe(
        Endpoint::Operator,
        models_outcome(Ok(vec!["qwen-vl".to_string()])),
    );
    assert_eq!(model.rows().operator.models, ["qwen-vl"]);
    assert!(model.rows().operator.picker_open);

    model.pick_model(Endpoint::Operator, "qwen-vl");
    assert_eq!(model.field(Endpoint::Operator, Field::Model), "qwen-vl");
    assert!(!model.rows().operator.picker_open, "选完收起下拉");
    assert_eq!(
        model.models(Endpoint::Operator),
        ["qwen-vl"],
        "列表留着，还能再选一次"
    );
    assert_eq!(model.field(Endpoint::Chat, Field::Model), "doubao-chat");

    // 小箭头可以再打开。
    model.toggle_picker(Endpoint::Operator);
    assert!(model.rows().operator.picker_open);
    model.toggle_picker(Endpoint::Operator);
    assert!(!model.rows().operator.picker_open);
}
