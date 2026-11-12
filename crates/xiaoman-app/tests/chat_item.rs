//! Ports of `app/src/test/kotlin/dev/xiaoman/app/ui/ChatPresentationTest.kt`, plus the chat text
//! and status rules of `ChatItem.kt`.

use xiaoman_api::{
    Action, ActionResult, AgentEvent, ApprovalAnswer, ApprovalScope, Decision, Point,
};
use xiaoman_app::chat_item::{
    ApprovalTexts, ChatItem, TimelineRow, approval_line, chat_rich_text, chat_timeline, step_text,
};

fn progress(id: i64, text: &str, ok: Option<bool>) -> ChatItem {
    ChatItem::Progress {
        id,
        text: text.to_string(),
        ok,
    }
}

#[test]
fn folding_diagnostics_keeps_question_error_and_final_answer_visible() {
    let original = vec![
        ChatItem::User {
            id: 1,
            text: "选本笔记本".to_string(),
        },
        progress(2, "开始", None),
        progress(3, "1. 搜索", Some(true)),
        ChatItem::Question {
            id: 4,
            text: "选哪种？".to_string(),
            takeover: false,
        },
        progress(5, "已确认", Some(true)),
        ChatItem::Error {
            id: 6,
            text: "断网".to_string(),
        },
        ChatItem::Assistant {
            id: 7,
            text: "尚未购买".to_string(),
        },
    ];
    let rows = chat_timeline(&original);
    assert_eq!(rows.len(), 6);
    match &rows[1] {
        TimelineRow::Process(entries) => {
            assert_eq!(
                entries.iter().map(ChatItem::id).collect::<Vec<_>>(),
                vec![2, 3]
            );
        }
        other => panic!("expected a folded progress row, got {other:?}"),
    }
    assert!(matches!(
        &rows[2],
        TimelineRow::Message(ChatItem::Question { .. })
    ));
    assert!(matches!(
        &rows[4],
        TimelineRow::Message(ChatItem::Error { .. })
    ));

    let restored: Vec<ChatItem> = rows.iter().flat_map(TimelineRow::items).collect();
    assert_eq!(restored, original);
}

#[test]
fn new_progress_keeps_card_identity_stable() {
    let first = progress(12, "搜索", None);
    let single = chat_timeline(std::slice::from_ref(&first));
    assert_eq!(single.len(), 1);
    let grown = chat_timeline(&[first, progress(13, "进入详情", None)]);
    assert_eq!(single[0].id(), grown[0].id());
    assert!(chat_timeline(&[]).is_empty());
}

#[test]
fn bold_price_and_lists_render_without_changing_their_meaning() {
    let styled = chat_rich_text("- **合计**：**2.40元**\n尚未付款");
    assert_eq!(styled.text, "• 合计：2.40元\n尚未付款");
    assert_eq!(styled.spans.len(), 2);
    assert_eq!(
        &styled.text[styled.spans[0].start..styled.spans[0].end],
        "合计"
    );
    assert_eq!(
        &styled.text[styled.spans[1].start..styled.spans[1].end],
        "2.40元"
    );
}

#[test]
fn unmatched_markers_and_literal_math_stay_intact() {
    let styled = chat_rich_text("2 * 3 = 6，**未结束");
    assert_eq!(styled.text, "2 * 3 = 6，**未结束");
    assert!(styled.spans.is_empty());
}

#[test]
fn headings_and_bullets_lose_only_their_markers() {
    let styled = chat_rich_text("### 标题\n- 一\n* 二\n#没有空格\n\n正文");
    assert_eq!(styled.text, "标题\n• 一\n• 二\n#没有空格\n\n正文");
    assert!(styled.spans.is_empty());
}

#[test]
fn bold_never_crosses_a_line_break() {
    let unwrapped = chat_rich_text("**bold\nplain**");
    assert_eq!(unwrapped.text, "**bold\nplain**");
    assert!(unwrapped.spans.is_empty());

    // Kotlin's `(.+?)` cannot cross the break, so the pair on the second line wins and the outer
    // asterisks stay literal.
    let styled = chat_rich_text("**a\nb** c **d**");
    assert_eq!(styled.text, "**a\nb c d**");
    assert_eq!(styled.spans.len(), 1);
    assert_eq!(
        &styled.text[styled.spans[0].start..styled.spans[0].end],
        " c "
    );
}

#[test]
fn step_text_names_every_action() {
    let tap = Action::Tap {
        point: None,
        node_id: Some(12),
        target: Some("目的地输入框".to_string()),
        reason: Some("先输入目的地".to_string()),
    };
    assert_eq!(step_text::action(&tap), "点击 目的地输入框");
    assert_eq!(
        step_text::action(&Action::Tap {
            point: None,
            node_id: Some(4),
            target: None,
            reason: None
        }),
        "点击 #4"
    );
    assert_eq!(
        step_text::action(&Action::Tap {
            point: Some(Point::new(10, 20).unwrap()),
            node_id: None,
            target: Some("  ".to_string()),
            reason: None,
        }),
        "点击"
    );
    assert_eq!(
        step_text::action(&Action::LongPress {
            point: None,
            node_id: None,
            duration_ms: 800,
            target: Some("图标".to_string()),
            reason: None,
        }),
        "长按 图标"
    );
    assert_eq!(
        step_text::action(&Action::Swipe {
            from: Point::new(0, 0).unwrap(),
            to: Point::new(100, 100).unwrap(),
            duration_ms: 300,
            target: Some("列表".to_string()),
            reason: None,
        }),
        "滑动 列表"
    );
    assert_eq!(
        step_text::action(&Action::TypeText {
            text: "家".to_string(),
            node_id: None,
            submit: false,
            target: Some("输入框".to_string()),
            reason: None,
        }),
        "输入「家」 输入框"
    );
    assert_eq!(
        step_text::action(&Action::Back {
            target: None,
            reason: None
        }),
        "返回"
    );
    assert_eq!(
        step_text::action(&Action::Home {
            target: None,
            reason: None
        }),
        "回到桌面"
    );
    assert_eq!(
        step_text::action(&Action::LaunchApp {
            package_name: "com.android.settings".to_string(),
            target: None,
            reason: None,
        }),
        "打开 com.android.settings"
    );
    assert_eq!(
        step_text::action(&Action::OpenDeeplink {
            uri: "https://x.test/a?b=1".to_string(),
            package_name: None,
            target: None,
            reason: None,
        }),
        "打开链接 https://x.test/a"
    );
    assert_eq!(
        step_text::action(&Action::Wait {
            millis: 1000,
            target: None,
            reason: None
        }),
        "等待 1000 毫秒"
    );
    assert_eq!(
        step_text::action(&Action::AskUser {
            question: "哪种？".to_string(),
            takeover: false,
            target: None,
            reason: None,
        }),
        "询问：哪种？"
    );
    assert_eq!(
        step_text::action(&Action::Finish {
            success: true,
            summary: "买好了".to_string(),
            target: None,
            reason: None,
        }),
        "结束：买好了"
    );
}

#[test]
fn status_only_changes_for_the_events_it_names() {
    assert_eq!(
        step_text::status(&AgentEvent::PhoneTaskStarted {
            task_id: "r1".to_string(),
            goal: "打车回家".to_string(),
            skill_name: None,
        }),
        Some("开始：打车回家".to_string())
    );
    assert_eq!(
        step_text::status(&AgentEvent::StepPlanned {
            task_id: "r1".to_string(),
            step_index: 0,
            action: Action::Home {
                target: None,
                reason: None
            },
        }),
        Some("第 1 步：回到桌面".to_string())
    );
    assert_eq!(
        step_text::status(&AgentEvent::StepBlocked {
            task_id: "r1".to_string(),
            step_index: 1,
            action: Action::Back {
                target: None,
                reason: None
            },
            decision: Decision::Deny {
                reason: "no".to_string()
            },
        }),
        Some("第 2 步已拦截".to_string())
    );
    assert_eq!(
        step_text::status(&AgentEvent::NeedsUser {
            task_id: "r1".to_string(),
            question: "验证码？".to_string(),
            takeover: false,
        }),
        Some("等待你回答：验证码？".to_string())
    );
    assert_eq!(
        step_text::status(&AgentEvent::NeedsUser {
            task_id: "r1".to_string(),
            question: "登录".to_string(),
            takeover: true,
        }),
        Some("请接管手机，完成后回到小满回复".to_string())
    );
    assert_eq!(
        step_text::status(&AgentEvent::PhoneTaskFinished {
            task_id: "r1".to_string(),
            success: true,
            summary: "ok".to_string(),
            steps: 1,
        }),
        Some("已完成".to_string())
    );
    assert_eq!(
        step_text::status(&AgentEvent::Reply {
            task_id: "r1".to_string(),
            text: "hi".to_string()
        }),
        None
    );
}

#[test]
fn chat_rows_cover_every_event() {
    let reply = AgentEvent::Reply {
        task_id: "r1".to_string(),
        text: "你好".to_string(),
    };
    assert_eq!(
        step_text::chat_item(&reply, 1),
        Some(ChatItem::Assistant {
            id: 1,
            text: "你好".to_string()
        })
    );
    assert_eq!(
        step_text::chat_item(
            &AgentEvent::SkillChosen {
                task_id: "r1".to_string(),
                skill_name: "外卖".to_string()
            },
            2,
        ),
        Some(progress(2, "使用技能 外卖", None))
    );
    assert_eq!(
        step_text::chat_item(
            &AgentEvent::PhoneTaskStarted {
                task_id: "r1".to_string(),
                goal: "打车回家".to_string(),
                skill_name: None,
            },
            3,
        ),
        Some(progress(3, "开始手机任务：打车回家", None))
    );
    assert_eq!(
        step_text::chat_item(
            &AgentEvent::NeedsUser {
                task_id: "r1".to_string(),
                question: "选哪种？".to_string(),
                takeover: false,
            },
            4,
        ),
        Some(ChatItem::Question {
            id: 4,
            text: "选哪种？".to_string(),
            takeover: false
        })
    );
    assert_eq!(
        step_text::chat_item(
            &AgentEvent::ToolCalled {
                task_id: "r1".to_string(),
                tool: "memory_search".to_string(),
                ok: false,
                summary: "没找到".to_string(),
            },
            5,
        ),
        Some(progress(5, "没找到", Some(false)))
    );
    let planned = AgentEvent::StepPlanned {
        task_id: "r1".to_string(),
        step_index: 0,
        action: Action::Back {
            target: None,
            reason: None,
        },
    };
    assert_eq!(
        step_text::chat_item(&planned, 6),
        None,
        "a plan only updates the status"
    );
    assert_eq!(
        step_text::chat_item(
            &AgentEvent::StepExecuted {
                task_id: "r1".to_string(),
                step_index: 0,
                action: Action::Back {
                    target: None,
                    reason: None
                },
                result: ActionResult::failed("页面没响应"),
            },
            3,
        ),
        Some(progress(3, "1. 返回（页面没响应）", Some(false)))
    );
    assert_eq!(
        step_text::chat_item(
            &AgentEvent::StepBlocked {
                task_id: "r1".to_string(),
                step_index: 1,
                action: Action::Back {
                    target: None,
                    reason: None
                },
                decision: Decision::Ask {
                    prompt: "确认？".to_string(),
                    reason: "confirmation word '确认'".to_string(),
                },
            },
            4,
        ),
        Some(progress(
            4,
            "2. 返回：已拦截（confirmation word '确认'）",
            Some(false)
        ))
    );
    assert_eq!(
        step_text::chat_item(
            &AgentEvent::PhoneTaskFinished {
                task_id: "r1".to_string(),
                success: true,
                summary: "买好了".to_string(),
                steps: 3,
            },
            5,
        ),
        Some(progress(5, "任务完成（3 步）：买好了", Some(true)))
    );
    assert_eq!(
        step_text::chat_item(
            &AgentEvent::MemoryUpdated {
                task_id: "r1".to_string(),
                permalink: "memory/x.md".to_string()
            },
            6,
        ),
        Some(progress(6, "已更新记忆 memory/x.md", None))
    );
    assert_eq!(
        step_text::chat_item(
            &AgentEvent::Failed {
                task_id: "r1".to_string(),
                message: "断网".to_string()
            },
            7,
        ),
        Some(ChatItem::Error {
            id: 7,
            text: "断网".to_string()
        })
    );
}

#[test]
fn answering_the_approval_card_leaves_one_progress_row() {
    let texts = ApprovalTexts::default();
    assert_eq!(
        approval_line(
            9,
            "支付 12 元",
            ApprovalAnswer {
                approved: true,
                scope: ApprovalScope::Once
            },
            &texts
        ),
        progress(9, "已允许：支付 12 元", Some(true))
    );
    assert_eq!(
        approval_line(
            10,
            "支付 12 元",
            ApprovalAnswer {
                approved: true,
                scope: ApprovalScope::Task
            },
            &texts
        ),
        progress(10, "本次任务都允许：支付 12 元", Some(true))
    );
    assert_eq!(
        approval_line(11, "支付 12 元", ApprovalAnswer::once(false), &texts),
        progress(11, "已拒绝：支付 12 元", Some(false))
    );
}
