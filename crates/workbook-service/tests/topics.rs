use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use tempfile::TempDir;
use workbook_domain::{ExamScope, ExamScopeGroup, Mode, Phase};
use workbook_scheduler::FixedClock;
use workbook_service::WorkbookService;

struct Harness {
    _dir: TempDir,
    clock: Arc<FixedClock>,
    service: WorkbookService,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FixedClock::new(
            Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        ));
        let service = WorkbookService::open(dir.path(), clock.clone()).unwrap();
        let mut settings = service.settings().unwrap();
        settings.timezone = "UTC".to_string();
        settings.automatic_rating = true;
        settings.new_limit = 100;
        service.update_settings(settings).unwrap();
        Self {
            _dir: dir,
            clock,
            service,
        }
    }

    fn import(&self, id: &str, title: &str, questions: serde_json::Value) -> String {
        let workbook = serde_json::json!({
            "format": "workbook",
            "schemaVersion": 2,
            "id": id,
            "revision": 1,
            "title": title,
            "description": "",
            "language": "ja",
            "tags": ["共有タグ"],
            "notes": questions
        });
        self.service
            .import_workbook_json(&workbook.to_string())
            .unwrap()
    }

    fn import_two_books(&self) -> (String, String) {
        let a = self.import(
            "book-a",
            "数学と英語",
            serde_json::json!([
                {
                    "id": "n1",
                    "title": "関数",
                    "knowledgeCard": "カード",
                    "questions": [
                        {"id":"q1","revision":1,"type":"true_false","prompt":"一次関数","answer":true,"explanation":"e","topics":["数学/一次関数"]},
                        {"id":"q2","revision":1,"type":"true_false","prompt":"二次関数","answer":true,"explanation":"e","topics":["数学/二次関数"]}
                    ]
                },
                {
                    "id": "n2",
                    "title": "文法",
                    "knowledgeCard": "カード",
                    "questions": [
                        {"id":"q3","revision":1,"type":"true_false","prompt":"文法","answer":true,"explanation":"e","topics":["英語/文法"]}
                    ]
                }
            ]),
        );
        let b = self.import(
            "book-b",
            "数学の続き",
            serde_json::json!([
                {
                    "id": "n3",
                    "title": "一次関数",
                    "knowledgeCard": "カード",
                    "questions": [
                        {"id":"q4","revision":1,"type":"true_false","prompt":"傾き","answer":true,"explanation":"e","topics":["数学/一次関数"]},
                        {"id":"q5","revision":1,"type":"true_false","prompt":"タグなし","answer":true,"explanation":"e"}
                    ]
                }
            ]),
        );
        (a, b)
    }

    fn math_scope(&self) -> ExamScope {
        let topics = self.service.topics().unwrap();
        let math = topics
            .iter()
            .find(|topic| topic.path == "数学")
            .expect("math topic");
        ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: Vec::new(),
                tags: Vec::new(),
                topics: vec![math.id.clone()],
            }],
            ..Default::default()
        }
    }
}

#[test]
fn imported_topic_paths_are_shared_across_workbooks_and_hierarchical() {
    let harness = Harness::new();
    let (a, b) = harness.import_two_books();
    assert_ne!(a, b);
    let topics = harness.service.topics().unwrap();
    let paths: Vec<&str> = topics.iter().map(|topic| topic.path.as_str()).collect();
    assert!(paths.contains(&"数学"));
    assert!(paths.contains(&"数学 / 一次関数"));
    assert!(paths.contains(&"数学 / 二次関数"));
    assert!(paths.contains(&"英語 / 文法"));
    let linear = topics
        .iter()
        .find(|topic| topic.path == "数学 / 一次関数")
        .unwrap();
    // q1 in book A and q4 in book B share one topic ID.
    assert_eq!(linear.question_count, 2);
    let math = topics.iter().find(|topic| topic.path == "数学").unwrap();
    assert_eq!(math.question_count, 3);
}

#[test]
fn selecting_a_parent_topic_resolves_all_descendants_in_the_scope() {
    let harness = Harness::new();
    harness.import_two_books();
    let scope = harness.math_scope();
    let preview = harness.service.scope_preview(&scope).unwrap();
    assert_eq!(preview.total, 3);
    assert_eq!(preview.studied, 0);
    assert_eq!(preview.unstudied, 3);
    assert_eq!(preview.workbooks.len(), 2);
    assert_eq!(preview.topics.len(), 3);
    let preview_questions = harness
        .service
        .preview_exam_scope(&scope)
        .unwrap()
        .into_iter()
        .map(|question| question.question_id)
        .collect::<Vec<_>>();
    let mut sorted = preview_questions.clone();
    sorted.sort();
    assert_eq!(sorted, ["q1", "q2", "q4"]);
}

#[test]
fn exclusions_are_applied_after_topic_inclusion() {
    let harness = Harness::new();
    harness.import_two_books();
    let topics = harness.service.topics().unwrap();
    let quadratic = topics
        .iter()
        .find(|topic| topic.path == "数学 / 二次関数")
        .unwrap();
    let mut scope = harness.math_scope();
    scope.exclude_topics = vec![quadratic.id.clone()];
    let questions = harness
        .service
        .preview_exam_scope(&scope)
        .unwrap()
        .into_iter()
        .map(|question| question.question_id)
        .collect::<Vec<_>>();
    let mut sorted = questions.clone();
    sorted.sort();
    assert_eq!(sorted, ["q1", "q4"]);
}

#[test]
fn duplicate_topic_names_conflict_only_under_the_same_parent() {
    let harness = Harness::new();
    let math = harness
        .service
        .create_topic("数学", None, "")
        .expect("root topic");
    let _linear = harness
        .service
        .create_topic("一次関数", Some(&math), "")
        .expect("child");
    let error = harness
        .service
        .create_topic(" 一次関数 ", Some(&math), "")
        .unwrap_err();
    assert!(error.to_string().contains("同名"));
    // The same name under another parent is a distinct topic.
    let english = harness.service.create_topic("英語", None, "").unwrap();
    let other = harness
        .service
        .create_topic("一次関数", Some(&english), "")
        .unwrap();
    assert_ne!(other, _linear);
}

#[test]
fn deleting_a_parent_topic_is_rejected_until_children_are_moved() {
    let harness = Harness::new();
    let math = harness.service.create_topic("数学", None, "").unwrap();
    let child = harness
        .service
        .create_topic("一次関数", Some(&math), "")
        .unwrap();
    assert!(harness.service.delete_topic(&math).is_err());
    harness
        .service
        .update_topic(&child, "一次関数", None, "")
        .unwrap();
    harness.service.delete_topic(&math).unwrap();
}

#[test]
fn individual_question_topic_assignment_changes_the_next_resolution() {
    let harness = Harness::new();
    let (_a, b) = harness.import_two_books();
    let before = harness
        .service
        .scope_preview(&harness.math_scope())
        .unwrap();
    assert_eq!(before.total, 3);
    let linear = harness
        .service
        .topics()
        .unwrap()
        .into_iter()
        .find(|topic| topic.path == "数学 / 一次関数")
        .unwrap();
    harness
        .service
        .set_question_topics(&b, "q5", &[linear.id])
        .unwrap();
    let after = harness
        .service
        .scope_preview(&harness.math_scope())
        .unwrap();
    assert_eq!(after.total, 4);
}

#[test]
fn a_cross_workbook_scope_plan_starts_and_keeps_ids_shared() {
    let harness = Harness::new();
    harness.import_two_books();
    let scope = harness.math_scope();
    harness.service.start_scope_plan(&scope, Some(600)).unwrap();
    let view = harness.service.active_view().unwrap().unwrap();
    assert_eq!(view.mode, Mode::Plan);
    assert!(view.progress.total > 0);
    assert_eq!(view.progress.total, 3);
    assert_eq!(view.budget.unwrap().target_seconds, 600);
    assert!(matches!(view.phase, Phase::Intro | Phase::Question));
}

#[test]
fn a_new_matching_workbook_is_reflected_next_time_but_not_in_a_running_session() {
    let harness = Harness::new();
    harness.import_two_books();
    let scope = harness.math_scope();
    harness.service.start_scope_plan(&scope, Some(600)).unwrap();
    let active = harness.service.active_view().unwrap().unwrap();
    let fixed_total = active.progress.total;
    harness.import(
        "book-c",
        "追加の数学",
        serde_json::json!([{
            "id": "n4",
            "title": "追加",
            "knowledgeCard": "カード",
            "questions": [
                {"id":"q6","revision":1,"type":"true_false","prompt":"追加","answer":true,"explanation":"e","topics":["数学/一次関数"]}
            ]
        }]),
    );
    let after_preview = harness.service.scope_preview(&scope).unwrap();
    assert_eq!(after_preview.total, 4);
    let active_again = harness.service.active_view().unwrap().unwrap();
    assert_eq!(active_again.progress.total, fixed_total);
}

#[test]
fn a_due_short_term_card_can_be_reviewed_inside_a_scope() {
    let harness = Harness::new();
    harness.import_two_books();
    let scope = harness.math_scope();
    harness.service.start_scope_plan(&scope, Some(600)).unwrap();
    let mut view = harness.service.active_view().unwrap().unwrap();
    if view.phase == Phase::Intro {
        view = harness
            .service
            .begin_question(&view.session_id, view.revision)
            .unwrap();
    }
    let evaluated = harness
        .service
        .evaluate_answer(
            &view.session_id,
            Some(&workbook_domain::Answer::Bool(true)),
            false,
            None,
            false,
            view.revision,
        )
        .unwrap();
    assert!(evaluated.feedback.as_ref().is_some_and(|f| f.committed));
    harness
        .service
        .finish(&evaluated.session_id, evaluated.revision)
        .unwrap();
    harness.clock.advance(Duration::minutes(11));
    harness
        .service
        .start_scope_review(&scope, Some(180))
        .unwrap();
    let review = harness.service.active_view().unwrap().unwrap();
    assert_eq!(review.mode, Mode::Review);
    assert!(review.progress.total >= 1);
}
