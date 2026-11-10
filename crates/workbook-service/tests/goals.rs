use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use tempfile::TempDir;
use workbook_domain::{
    ExamDraft, ExamScope, ExamScopeGroup, GoalDraft, GoalSource, Mode, Phase, QuestionRef,
};
use workbook_scheduler::{FixedClock, PlanFeasibility};
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
            Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
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

    fn import(&self, questions: usize) -> String {
        let workbook = serde_json::json!({
            "format": "workbook",
            "schemaVersion": 2,
            "id": "goal-book",
            "revision": 1,
            "title": "目標の教材",
            "description": "",
            "language": "ja",
            "tags": [],
            "notes": [{
                "id": "n1", "title": "単元", "knowledgeCard": "カード",
                "questions": (0..questions).map(|index| serde_json::json!({
                    "id": format!("q{index}"),
                    "revision": 1,
                    "type": "true_false",
                    "prompt": format!("問 {index}"),
                    "answer": true,
                    "explanation": "e",
                    "topics": [if index % 2 == 0 { "数学/一次関数" } else { "数学/二次関数" }]
                })).collect::<Vec<_>>()
            }]
        });
        self.service
            .import_workbook_json(&workbook.to_string())
            .unwrap()
    }

    fn scope(&self, workbook_id: &str) -> ExamScope {
        ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: vec![workbook_id.to_string()],
                tags: Vec::new(),
                topics: Vec::new(),
            }],
            ..Default::default()
        }
    }

    fn goal_draft(&self, scope: ExamScope, date: Option<chrono::NaiveDate>) -> GoalDraft {
        GoalDraft {
            title: "毎日の数学".to_string(),
            description: None,
            target_date: date,
            target_score: None,
            daily_minutes: 30,
            scope,
        }
    }
}

#[test]
fn a_goal_without_a_deadline_reports_real_measured_counts() {
    let harness = Harness::new();
    let book = harness.import(8);
    let id = harness
        .service
        .create_goal(harness.goal_draft(harness.scope(&book), None))
        .unwrap();
    let summary = harness.service.goal_summary(&id).unwrap();
    assert_eq!(summary.source, GoalSource::Goal);
    assert_eq!(summary.total, 8);
    assert_eq!(summary.studied, 0);
    assert_eq!(summary.unstudied, 8);
    assert!(summary.days_remaining.is_none());
    assert!(!summary.expired);
    assert!(summary.topics.len() >= 3);
    assert_eq!(summary.first_pass_graded, 0);
    assert!(summary.first_pass_accuracy.is_none());
    assert_eq!(
        summary
            .topics
            .iter()
            .map(|topic| topic.total)
            .sum::<usize>(),
        16
    );
}

#[test]
fn ongoing_answers_feed_first_pass_accuracy_but_never_a_mastery_score() {
    let harness = Harness::new();
    let book = harness.import(3);
    let id = harness
        .service
        .create_goal(harness.goal_draft(harness.scope(&book), None))
        .unwrap();
    harness.service.start_goal_plan(&id).unwrap();
    let mut view = harness.service.active_view().unwrap().unwrap();
    assert_eq!(view.goal_id.as_deref(), Some(id.as_str()));
    assert_eq!(view.goal_title.as_deref(), Some("毎日の数学"));
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
    harness
        .service
        .finish(&evaluated.session_id, evaluated.revision)
        .unwrap();
    let summary = harness.service.goal_summary(&id).unwrap();
    assert_eq!(summary.first_pass_graded, 1);
    assert_eq!(summary.first_pass_correct, 1);
    assert_eq!(summary.first_pass_accuracy, Some(1.0));
    assert!(summary.studied >= 1);
}

#[test]
fn an_exam_is_visible_as_a_goal_without_duplicating_exam_data() {
    let harness = Harness::new();
    let book = harness.import(3);
    let exam_id = harness
        .service
        .create_exam(ExamDraft {
            title: "資格試験".to_string(),
            description: None,
            target_date: None,
            target_score: Some(80.0),
            daily_minutes: 20,
            scope: harness.scope(&book),
        })
        .unwrap();
    let goals = harness.service.goals(false).unwrap();
    let exam_goals: Vec<_> = goals
        .iter()
        .filter(|goal| goal.source == GoalSource::Exam)
        .collect();
    assert_eq!(exam_goals.len(), 1);
    assert_eq!(exam_goals[0].goal.id, exam_id);
    assert_eq!(
        exam_goals[0].goal.exam_id.as_deref(),
        Some(exam_id.as_str())
    );
    assert!(exam_goals[0].goal.target_score == Some(80.0));
    // The exam row itself is untouched and still editable through the exam API.
    assert_eq!(harness.service.exam(&exam_id).unwrap().daily_minutes, 20);
}

#[test]
fn archiving_and_deleting_a_goal_preserve_attempts_and_workbooks() {
    let harness = Harness::new();
    let book = harness.import(2);
    let id = harness
        .service
        .create_goal(harness.goal_draft(harness.scope(&book), None))
        .unwrap();
    harness.service.archive_goal(&id, true).unwrap();
    assert!(
        harness
            .service
            .goals(false)
            .unwrap()
            .iter()
            .all(|goal| goal.goal.id != id)
    );
    assert!(
        harness
            .service
            .goals(true)
            .unwrap()
            .iter()
            .any(|goal| goal.goal.id == id)
    );
    harness.service.archive_goal(&id, false).unwrap();
    harness.service.delete_goal(&id).unwrap();
    assert!(harness.service.goals(true).unwrap().is_empty());
    assert_eq!(harness.service.snapshot().unwrap().books.len(), 1);
}

#[test]
fn a_short_deadline_does_not_pretend_the_goal_is_achievable() {
    let harness = Harness::new();
    let book = harness.import(100);
    let today = chrono::NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
    let id = harness
        .service
        .create_goal(harness.goal_draft(harness.scope(&book), Some(today)))
        .unwrap();
    let summary = harness.service.goal_summary(&id).unwrap();
    assert_eq!(summary.plan.feasibility, PlanFeasibility::Insufficient);
    assert!(summary.plan.required_minutes > summary.plan.available_minutes);
    assert!(summary.plan.warnings.iter().any(|warning| {
        matches!(
            warning,
            workbook_scheduler::PlanWarning::InsufficientTime { .. }
        )
    }));
}

#[test]
fn today_lists_active_goal_progress_and_reflects_learning() {
    let harness = Harness::new();
    let book = harness.import(4);
    let id = harness
        .service
        .create_goal(harness.goal_draft(harness.scope(&book), None))
        .unwrap();
    let snapshot = harness.service.snapshot().unwrap();
    let brief = snapshot
        .goals
        .iter()
        .find(|goal| goal.id == id)
        .expect("goal brief");
    assert_eq!(brief.total, 4);
    assert_eq!(brief.due, 0);
    assert_eq!(brief.studied, 0);
}

#[test]
fn a_goal_with_an_individual_question_scope_works_the_same_way() {
    let harness = Harness::new();
    let book = harness.import(3);
    let scope = ExamScope {
        include_questions: vec![QuestionRef::new(book.clone(), "q1")],
        ..Default::default()
    };
    let id = harness
        .service
        .create_goal(harness.goal_draft(scope, None))
        .unwrap();
    let summary = harness.service.goal_summary(&id).unwrap();
    assert_eq!(summary.total, 1);
}

#[test]
fn goal_plan_respects_the_daily_minutes_and_advances_the_clock() {
    let harness = Harness::new();
    let book = harness.import(20);
    let mut draft = harness.goal_draft(harness.scope(&book), None);
    draft.daily_minutes = 2;
    let id = harness.service.create_goal(draft).unwrap();
    harness.service.start_goal_plan(&id).unwrap();
    let view = harness.service.active_view().unwrap().unwrap();
    assert_eq!(view.mode, Mode::Plan);
    assert!(view.progress.total <= 6);
    harness
        .service
        .finish(&view.session_id, view.revision)
        .unwrap();
    harness.clock.advance(Duration::hours(1));
    let snapshot = harness.service.snapshot().unwrap();
    assert!(snapshot.goals.iter().any(|goal| goal.id == id));
}
