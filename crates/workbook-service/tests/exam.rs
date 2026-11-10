use std::sync::Arc;

use chrono::{Duration, NaiveDate, TimeZone, Utc};
use rusqlite::OptionalExtension;
use tempfile::TempDir;
use workbook_domain::{
    Answer, ExamDraft, ExamScope, ExamScopeGroup, MockOptions, MockOrder, Mode, Phase,
    PracticeOptions, PracticeStrategy, QuestionRef,
};
use workbook_scheduler::FixedClock;
use workbook_service::{MockItemStatus, WorkbookService};

struct Harness {
    _dir: TempDir,
    clock: Arc<FixedClock>,
    service: Option<WorkbookService>,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FixedClock::new(
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        ));
        let service = WorkbookService::open(dir.path(), clock.clone()).unwrap();
        let mut settings = service.settings().unwrap();
        settings.timezone = "UTC".to_string();
        settings.automatic_rating = true;
        service.update_settings(settings).unwrap();
        Self {
            _dir: dir,
            clock,
            service: Some(service),
        }
    }

    fn service(&self) -> &WorkbookService {
        self.service.as_ref().unwrap()
    }

    fn reopen(&mut self) {
        let clock = self.clock.clone();
        let dir = self._dir.path().to_path_buf();
        drop(self.service.take());
        self.service = Some(WorkbookService::open(&dir, clock).unwrap());
    }

    fn import(&self, raw: &str) -> String {
        self.service().import_workbook_json(raw).unwrap()
    }

    fn state_count(&self) -> i64 {
        let connection =
            rusqlite::Connection::open(self._dir.path().join("workbook.sqlite")).unwrap();
        connection
            .query_row("SELECT COUNT(*) FROM review_states", [], |row| row.get(0))
            .unwrap()
    }

    fn reps(&self, workbook_id: &str, question_id: &str) -> Option<u32> {
        let connection =
            rusqlite::Connection::open(self._dir.path().join("workbook.sqlite")).unwrap();
        connection
            .query_row(
                "SELECT reps FROM review_states WHERE workbook_id=?1 AND question_id=?2",
                [workbook_id, question_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .unwrap()
            .map(|value| value as u32)
    }

    fn learn(&self, workbook_id: &str) {
        self.service()
            .start_session(Mode::New, Some(workbook_id))
            .unwrap();
        for _ in 0..20 {
            let Some(view) = self.service().active_view().unwrap() else {
                return;
            };
            if view.phase == Phase::WorkbookResult || view.phase == Phase::OverallResult {
                self.service()
                    .finish(&view.session_id, view.revision)
                    .unwrap();
                return;
            }
            let view = if view.phase == Phase::Intro {
                self.service()
                    .begin_question(&view.session_id, view.revision)
                    .unwrap()
            } else {
                view
            };
            let view = self
                .service()
                .evaluate_answer(
                    &view.session_id,
                    Some(&Answer::Bool(true)),
                    false,
                    None,
                    false,
                    view.revision,
                )
                .unwrap();
            let view = if view
                .feedback
                .as_ref()
                .is_some_and(|feedback| !feedback.committed)
            {
                let rating = view
                    .feedback
                    .as_ref()
                    .and_then(|feedback| feedback.suggested_rating)
                    .unwrap();
                self.service()
                    .commit_rating(
                        &view.session_id,
                        &uuid::Uuid::new_v4().to_string(),
                        rating,
                        view.revision,
                    )
                    .unwrap()
            } else {
                view
            };
            let view = self
                .service()
                .next(&view.session_id, view.revision)
                .unwrap();
            if view.phase == Phase::OverallResult {
                return;
            }
        }
    }

    fn complete_active(&self) {
        for _ in 0..300 {
            let Some(view) = self.service().active_view().unwrap() else {
                return;
            };
            if matches!(view.phase, Phase::OverallResult | Phase::WorkbookResult) {
                let _ = self.service().finish(&view.session_id, view.revision);
                return;
            }
            let view = if view.phase == Phase::Intro {
                self.service()
                    .begin_question(&view.session_id, view.revision)
                    .unwrap()
            } else {
                view
            };
            let view = if view.phase == Phase::Feedback {
                view
            } else {
                self.service()
                    .evaluate_answer(
                        &view.session_id,
                        Some(&Answer::Bool(true)),
                        false,
                        None,
                        false,
                        view.revision,
                    )
                    .unwrap()
            };
            let view = if view
                .feedback
                .as_ref()
                .is_some_and(|feedback| !feedback.committed)
            {
                let rating = view
                    .feedback
                    .as_ref()
                    .and_then(|feedback| feedback.suggested_rating)
                    .unwrap();
                self.service()
                    .commit_rating(
                        &view.session_id,
                        &uuid::Uuid::new_v4().to_string(),
                        rating,
                        view.revision,
                    )
                    .unwrap()
            } else {
                view
            };
            let view = self
                .service()
                .next(&view.session_id, view.revision)
                .unwrap();
            if view.phase == Phase::OverallResult {
                let _ = self.service().finish(&view.session_id, view.revision);
                return;
            }
        }
        panic!("session did not finish");
    }
}

fn workbook(id: &str, tags: &[&str], questions: &[(&str, bool)]) -> String {
    let questions = questions
        .iter()
        .map(|(qid, answer)| {
            format!(
                r#"{{"id":"{qid}","revision":1,"type":"true_false","prompt":"{qid}?",
                    "answer":{answer},"explanation":"because {qid}"}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let tags = tags
        .iter()
        .map(|tag| format!("\"{tag}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{
          "format":"workbook","schemaVersion":1,"id":"{id}","revision":1,
          "title":"Book {id}","description":"","language":"ja","tags":[{tags}],
          "notes":[{{"id":"n-{id}","title":"Note {id}","knowledgeCard":"card",
            "questions":[{questions}]}}]
        }}"#
    )
}

fn exam_draft(title: &str, workbook_ids: &[&str], tags: &[&str]) -> ExamDraft {
    ExamDraft {
        title: title.to_string(),
        description: None,
        target_date: None,
        target_score: None,
        daily_minutes: 30,
        scope: ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: workbook_ids.iter().map(|id| id.to_string()).collect(),
                tags: tags.iter().map(|tag| tag.to_string()).collect(),
                topics: Vec::new(),
            }],
            ..Default::default()
        },
    }
}

#[test]
fn scope_resolution_and_preview_track_imports_and_reclassification() {
    let h = Harness::new();
    let a = h.import(&workbook(
        "A",
        &["electricity"],
        &[("q1", true), ("q2", false)],
    ));
    let b = h.import(&workbook("B", &["chemistry"], &[("q3", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Science", &[], &["electricity"]))
        .unwrap();
    assert_eq!(
        h.service()
            .preview_exam_scope(&h.service().exam(&exam_id).unwrap().scope)
            .unwrap()
            .len(),
        2
    );

    // A newly imported matching workbook becomes eligible for the next session.
    let c = h.import(&workbook("C", &["electricity"], &[("q4", true)]));
    assert_eq!(
        h.service()
            .preview_exam_scope(&h.service().exam(&exam_id).unwrap().scope)
            .unwrap()
            .len(),
        3
    );

    // Reclassification is reflected without copying questions.
    let mut scope = h.service().exam(&exam_id).unwrap().scope;
    scope.groups[0].tags = vec!["chemistry".into()];
    h.service()
        .update_exam(
            &exam_id,
            ExamDraft {
                title: "Science".into(),
                description: None,
                target_date: None,
                target_score: None,
                daily_minutes: 30,
                scope: scope.clone(),
            },
        )
        .unwrap();
    let preview = h.service().preview_exam_scope(&scope).unwrap();
    assert_eq!(preview.len(), 1);
    assert_eq!(preview[0].workbook_id, b);
    assert_ne!(preview[0].workbook_id, a);
    assert_ne!(preview[0].workbook_id, c);
}

#[test]
fn exam_practice_filters_strategies_and_never_touches_fsrs() {
    let h = Harness::new();
    let wb = h.import(&workbook("P", &["tag"], &[("q1", true), ("q2", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Practice", &[&wb], &[]))
        .unwrap();

    let before = h.state_count();
    h.service()
        .start_exam_practice(
            &exam_id,
            &PracticeOptions {
                strategy: PracticeStrategy::Unstudied,
                question_limit: Some(1),
            },
        )
        .unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    assert_eq!(view.mode, Mode::Practice);
    assert_eq!(view.progress.total, 1);
    let question = view.question.clone().unwrap();
    let answer = Answer::Bool(
        question
            .answer
            .as_ref()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    );
    let view = h
        .service()
        .evaluate_answer(
            &view.session_id,
            Some(&answer),
            false,
            None,
            false,
            view.revision,
        )
        .unwrap();
    let view = if view.feedback.as_ref().is_some_and(|f| !f.committed) {
        let rating = view.feedback.as_ref().unwrap().suggested_rating.unwrap();
        h.service()
            .commit_rating(
                &view.session_id,
                &uuid::Uuid::new_v4().to_string(),
                rating,
                view.revision,
            )
            .unwrap()
    } else {
        view
    };
    let _ = view;
    assert_eq!(h.state_count(), before);
    h.service().finish(&view.session_id, view.revision).unwrap();

    // The same exam can be practiced again; history is persistent but FSRS is
    // still untouched.
    h.service()
        .start_exam_practice(&exam_id, &PracticeOptions::default())
        .unwrap();
    let second = h.service().active_view().unwrap().unwrap();
    assert_eq!(second.progress.total, 2);
    assert_eq!(h.state_count(), before);
}

#[test]
fn exam_review_only_selects_scope_questions_and_shares_fsrs_state() {
    let h = Harness::new();
    let in_scope = h.import(&workbook("R1", &["scope"], &[("q1", true)]));
    let _outside = h.import(&workbook("R2", &["other"], &[("q2", true)]));
    h.learn(&in_scope);
    h.learn(&_outside);
    let exam_id = h
        .service()
        .create_exam(exam_draft("Review", &[&in_scope], &[]))
        .unwrap();
    h.clock.advance(Duration::days(5));
    let before = h.reps(&in_scope, "q1");
    let outside_before = h.reps(&_outside, "q2");

    h.service().start_exam_review(&exam_id).unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    assert_eq!(view.mode, Mode::Review);
    assert_eq!(view.exam_id.as_deref(), Some(exam_id.as_str()));
    assert_eq!(view.progress.total, 1);
    assert_eq!(view.note_id.as_deref(), Some("n-R1"));
    let view = h
        .service()
        .evaluate_answer(
            &view.session_id,
            Some(&Answer::Bool(true)),
            false,
            None,
            false,
            view.revision,
        )
        .unwrap();
    let view = if view.feedback.as_ref().is_some_and(|f| !f.committed) {
        let rating = view.feedback.as_ref().unwrap().suggested_rating.unwrap();
        h.service()
            .commit_rating(
                &view.session_id,
                &uuid::Uuid::new_v4().to_string(),
                rating,
                view.revision,
            )
            .unwrap()
    } else {
        view
    };
    let after = h.reps(&in_scope, "q1").unwrap();
    assert_eq!(after, before.unwrap() + 1);
    h.service().finish(&view.session_id, view.revision).unwrap();
    assert_eq!(h.reps(&_outside, "q2"), outside_before);
}

#[test]
fn exam_review_keeps_note_sibling_suppression() {
    let h = Harness::new();
    let wb = h.import(&workbook("SIB", &["sib"], &[("q1", true), ("q2", true)]));
    h.learn(&wb);
    h.clock.advance(Duration::days(5));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Sibling", &[&wb], &[]))
        .unwrap();
    h.service().start_exam_review(&exam_id).unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    // Both questions belong to one note, so only one is selected at a time.
    assert_eq!(view.progress.total, 1);
    let view = h
        .service()
        .evaluate_answer(
            &view.session_id,
            Some(&Answer::Bool(true)),
            false,
            None,
            false,
            view.revision,
        )
        .unwrap();
    let view = h.service().next(&view.session_id, view.revision).unwrap();
    assert_eq!(view.phase, Phase::OverallResult);
}

#[test]
fn exam_sessions_flow_across_multiple_workbooks() {
    let h = Harness::new();
    let a = h.import(&workbook("A1", &["multi"], &[("q1", true)]));
    let b = h.import(&workbook("B1", &["multi"], &[("q2", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Multi", &[&a, &b], &[]))
        .unwrap();
    h.service()
        .start_exam_practice(&exam_id, &PracticeOptions::default())
        .unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    assert_eq!(view.progress.total, 2);
    let first_workbook = view.workbook_id.clone().unwrap();
    let answer = Answer::Bool(true);
    let view = h
        .service()
        .evaluate_answer(
            &view.session_id,
            Some(&answer),
            false,
            None,
            false,
            view.revision,
        )
        .unwrap();
    let view = if view.feedback.as_ref().is_some_and(|f| !f.committed) {
        let rating = view.feedback.as_ref().unwrap().suggested_rating.unwrap();
        h.service()
            .commit_rating(
                &view.session_id,
                &uuid::Uuid::new_v4().to_string(),
                rating,
                view.revision,
            )
            .unwrap()
    } else {
        view
    };
    let view = h.service().next(&view.session_id, view.revision).unwrap();
    assert_eq!(view.phase, Phase::Question);
    assert_ne!(view.workbook_id.as_deref(), Some(first_workbook.as_str()));
    assert_eq!(view.progress.total, 2);
    h.service().finish(&view.session_id, view.revision).unwrap();
}

#[test]
fn practice_previously_incorrect_selects_only_wrong_questions() {
    let h = Harness::new();
    let wb = h.import(&workbook("W", &["weak"], &[("q1", true), ("q2", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Weak", &[&wb], &[]))
        .unwrap();
    h.service()
        .start_exam_practice(&exam_id, &PracticeOptions::default())
        .unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    let first_id = view.question.as_ref().unwrap().id.clone();
    let answer = Answer::Bool(false);
    let view = h
        .service()
        .evaluate_answer(
            &view.session_id,
            Some(&answer),
            false,
            None,
            false,
            view.revision,
        )
        .unwrap();
    let view = if view.feedback.as_ref().is_some_and(|f| !f.committed) {
        let rating = view.feedback.as_ref().unwrap().suggested_rating.unwrap();
        h.service()
            .commit_rating(
                &view.session_id,
                &uuid::Uuid::new_v4().to_string(),
                rating,
                view.revision,
            )
            .unwrap()
    } else {
        view
    };
    h.service().finish(&view.session_id, view.revision).unwrap();

    h.service()
        .start_exam_practice(
            &exam_id,
            &PracticeOptions {
                strategy: PracticeStrategy::PreviouslyIncorrect,
                question_limit: None,
            },
        )
        .unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    assert_eq!(view.progress.total, 1);
    assert_eq!(view.question.as_ref().unwrap().id, first_id);
    h.service().finish(&view.session_id, view.revision).unwrap();
}

#[test]
fn mock_selection_is_fixed_and_grading_separates_unanswered_and_ungraded() {
    let h = Harness::new();
    let wb = h.import(&workbook("M", &["mock"], &[("q1", true), ("q2", false)]));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Mock", &[&wb], &[]))
        .unwrap();
    let before = h.state_count();
    h.service()
        .start_exam_mock(
            &exam_id,
            &MockOptions {
                question_limit: Some(2),
                time_limit_seconds: Some(600),
                order: MockOrder::Fixed,
            },
        )
        .unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    assert_eq!(view.mode, Mode::Mock);
    assert_eq!(view.progress.total, 2);
    assert_eq!(view.mock.as_ref().unwrap().total, 2);
    assert!(view.mock.as_ref().unwrap().remaining_ms.unwrap() > 0);
    let session_id = view.session_id.clone();
    let first = view.question.clone().unwrap();
    let answer = Answer::Bool(
        first
            .answer
            .as_ref()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    );
    let view = h
        .service()
        .mock_answer(&session_id, Some(&answer), view.revision)
        .unwrap();
    // A newly imported matching question must not appear in the fixed set.
    h.import(&workbook("M2", &["mock"], &[("q3", true)]));
    assert_eq!(
        h.service().active_view().unwrap().unwrap().progress.total,
        2
    );
    assert_eq!(view.mock.as_ref().unwrap().answered, 1);

    // Submit with one question unanswered.
    let view = h.service().submit_mock(&session_id, view.revision).unwrap();
    let result = view.mock_result.as_ref().unwrap();
    assert_eq!(result.total, 2);
    assert_eq!(result.correct + result.incorrect, 1);
    assert_eq!(result.unanswered, 1);
    assert_eq!(result.ungraded, 0);
    assert_eq!(h.state_count(), before);
    assert!(
        h.service()
            .exam_summary(&exam_id)
            .unwrap()
            .mock_history
            .iter()
            .any(|entry| entry.session_id == session_id)
    );
}

#[test]
fn mock_ungraded_flashcard_does_not_fabricate_a_score() {
    let h = Harness::new();
    let raw = r#"{
      "format":"workbook","schemaVersion":1,"id":"F","revision":1,
      "title":"Flash","description":"","language":"ja","tags":["flash"],
      "notes":[{"id":"n","title":"Note","knowledgeCard":"card","questions":[
        {"id":"q","revision":1,"type":"flashcard","front":"front","back":"back",
          "explanation":"because"}
      ]}]
    }"#;
    let wb = h.import(raw);
    let exam_id = h
        .service()
        .create_exam(exam_draft("Flash mock", &[&wb], &[]))
        .unwrap();
    h.service()
        .start_exam_mock(&exam_id, &MockOptions::default())
        .unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    let view = h
        .service()
        .mock_answer(&view.session_id, Some(&Answer::Bool(true)), view.revision)
        .unwrap();
    let view = h
        .service()
        .submit_mock(&view.session_id, view.revision)
        .unwrap();
    let result = view.mock_result.as_ref().unwrap();
    assert_eq!(result.ungraded, 1);
    assert_eq!(result.score_percent, None);
    assert_eq!(result.items[0].status, MockItemStatus::Ungraded);
}

#[test]
fn mock_rejects_answers_after_the_time_limit() {
    let h = Harness::new();
    let wb = h.import(&workbook("T", &["time"], &[("q1", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Timed", &[&wb], &[]))
        .unwrap();
    h.service()
        .start_exam_mock(
            &exam_id,
            &MockOptions {
                question_limit: None,
                time_limit_seconds: Some(60),
                order: MockOrder::Fixed,
            },
        )
        .unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    h.clock.advance(Duration::seconds(61));
    let error = h
        .service()
        .mock_answer(&view.session_id, Some(&Answer::Bool(true)), view.revision)
        .unwrap_err();
    assert!(error.to_string().contains("制限時間"));
}

#[test]
fn newly_imported_matching_questions_appear_in_the_next_session_only() {
    let h = Harness::new();
    let _a = h.import(&workbook("N1", &["dynamic"], &[("q1", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Dynamic", &[], &["dynamic"]))
        .unwrap();
    h.service()
        .start_exam_practice(&exam_id, &PracticeOptions::default())
        .unwrap();
    let first = h.service().active_view().unwrap().unwrap();
    assert_eq!(first.progress.total, 1);
    let session_id = first.session_id.clone();

    let _b = h.import(&workbook("N2", &["dynamic"], &[("q2", true)]));
    // The active session keeps its original fixed selection.
    assert_eq!(
        h.service().active_view().unwrap().unwrap().progress.total,
        1
    );
    h.service().finish(&session_id, first.revision).unwrap();

    h.service()
        .start_exam_practice(&exam_id, &PracticeOptions::default())
        .unwrap();
    assert_eq!(
        h.service().active_view().unwrap().unwrap().progress.total,
        2
    );
    let view = h.service().active_view().unwrap().unwrap();
    h.service().finish(&view.session_id, view.revision).unwrap();
}

#[test]
fn archiving_keeps_the_exam_and_can_be_reversed() {
    let h = Harness::new();
    let wb = h.import(&workbook("AR", &["archive"], &[("q1", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Archive", &[&wb], &[]))
        .unwrap();
    h.service().archive_exam(&exam_id, true).unwrap();
    assert!(h.service().exams(false).unwrap().is_empty());
    let archived = h.service().exams(true).unwrap();
    assert_eq!(archived.len(), 1);
    assert!(archived[0].exam.is_archived());
    h.service().archive_exam(&exam_id, false).unwrap();
    assert_eq!(h.service().exams(false).unwrap().len(), 1);
    assert!(!h.service().exam(&exam_id).unwrap().is_archived());
}

#[test]
fn exam_backup_and_restore_keep_scope_and_history() {
    let h = Harness::new();
    let wb = h.import(&workbook("BK", &["backup"], &[("q1", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Backup", &[&wb], &[]))
        .unwrap();
    let path = h._dir.path().join("exam-backup.sqlite");
    h.service().backup_to(&path).unwrap();
    WorkbookService::validate_backup(&path).unwrap();

    h.service().delete_exam(&exam_id).unwrap();
    assert!(h.service().exam(&exam_id).is_err());

    h.service().restore_from(&path).unwrap();
    let exam = h.service().exam(&exam_id).unwrap();
    assert_eq!(exam.title, "Backup");
    let summary = h.service().exam_summary(&exam_id).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.exam.scope, exam.scope);
}

#[test]
fn mock_flows_across_workbooks_and_scores_the_fixed_set() {
    let h = Harness::new();
    let a = h.import(&workbook("MA", &["mock-multi"], &[("q1", true)]));
    let b = h.import(&workbook("MB", &["mock-multi"], &[("q2", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Mock multi", &[&a, &b], &[]))
        .unwrap();
    h.service()
        .start_exam_mock(
            &exam_id,
            &MockOptions {
                question_limit: None,
                time_limit_seconds: None,
                order: MockOrder::Fixed,
            },
        )
        .unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    assert_eq!(view.progress.total, 2);
    let first_workbook = view.workbook_id.clone().unwrap();
    let view = h
        .service()
        .mock_answer(&view.session_id, Some(&Answer::Bool(true)), view.revision)
        .unwrap();
    assert_eq!(view.phase, Phase::Question);
    assert_ne!(view.workbook_id.as_deref(), Some(first_workbook.as_str()));
    let view = h
        .service()
        .mock_answer(&view.session_id, Some(&Answer::Bool(true)), view.revision)
        .unwrap();
    assert_eq!(view.phase, Phase::WorkbookResult);
    let view = h
        .service()
        .submit_mock(&view.session_id, view.revision)
        .unwrap();
    let result = view.mock_result.as_ref().unwrap();
    assert_eq!(result.total, 2);
    assert_eq!(result.correct, 2);
    assert_eq!(result.incorrect, 0);
    assert_eq!(result.unanswered, 0);
    assert_eq!(result.score_percent, Some(100.0));
}

#[test]
fn deleting_an_exam_keeps_workbooks_and_fsrs_but_removes_mock_history() {
    let h = Harness::new();
    let wb = h.import(&workbook("D", &["delete"], &[("q1", true)]));
    h.learn(&wb);
    let before = h.state_count();
    let exam_id = h
        .service()
        .create_exam(exam_draft("Delete", &[&wb], &[]))
        .unwrap();
    h.service()
        .start_exam_mock(&exam_id, &MockOptions::default())
        .unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    let view = h
        .service()
        .mock_answer(&view.session_id, Some(&Answer::Bool(true)), view.revision)
        .unwrap();
    h.service()
        .submit_mock(&view.session_id, view.revision)
        .unwrap();
    h.service().delete_exam(&exam_id).unwrap();
    assert!(h.service().exam(&exam_id).is_err());
    assert!(h.service().workbook(&wb).is_ok());
    assert_eq!(h.state_count(), before);
    assert!(
        h.service()
            .exams(true)
            .unwrap()
            .iter()
            .all(|summary| summary.exam.id != exam_id)
    );
}

#[test]
fn exam_data_survives_reopen_and_question_identity_is_stable() {
    let mut h = Harness::new();
    let wb = h.import(&workbook("S", &["persist"], &[("q1", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_draft("Persisted", &[&wb], &[]))
        .unwrap();
    let reference = QuestionRef::new(wb.clone(), "q1".to_string());
    h.reopen();
    let exam = h.service().exam(&exam_id).unwrap();
    assert_eq!(exam.title, "Persisted");
    assert_eq!(exam.scope.include_questions, Vec::<QuestionRef>::new());
    let summary = h.service().exam_summary(&exam_id).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.studied, 0);
    assert_eq!(
        h.service()
            .preview_exam_scope(&exam.scope)
            .unwrap()
            .iter()
            .map(|q| QuestionRef::new(q.workbook_id.clone(), q.question_id.clone()))
            .collect::<Vec<_>>(),
        vec![reference]
    );
}

fn exam_dated(
    title: &str,
    workbook_id: &str,
    target_date: Option<NaiveDate>,
    daily_minutes: u32,
) -> ExamDraft {
    ExamDraft {
        title: title.to_string(),
        description: None,
        target_date,
        target_score: None,
        daily_minutes,
        scope: ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: vec![workbook_id.to_string()],
                tags: vec![],
                topics: Vec::new(),
            }],
            ..Default::default()
        },
    }
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

#[test]
fn exam_plan_introduces_new_questions_and_shares_fsrs_with_normal_study() {
    let h = Harness::new();
    let wb = h.import(&workbook("A", &[], &[("q1", true), ("q2", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_dated("Plan", &wb, Some(date(2026, 1, 3)), 10))
        .unwrap();
    assert_eq!(
        h.service()
            .exam_summary(&exam_id)
            .unwrap()
            .plan
            .coverage
            .selected,
        2
    );
    h.service().start_exam_plan(&exam_id).unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    assert_eq!(view.mode, Mode::Plan);
    assert_eq!(view.phase, Phase::Intro);
    h.complete_active();
    assert_eq!(h.state_count(), 2);

    // The questions are no longer new for ordinary study: the same history is
    // shared, not copied into an exam-only store.
    assert!(h.service().start_session(Mode::New, Some(&wb)).is_err());
    h.clock.advance(Duration::days(7));
    h.service().start_session(Mode::Review, Some(&wb)).unwrap();
    assert!(h.service().active_view().unwrap().is_some());
}

#[test]
fn exam_plan_respects_zero_minutes_and_recomputes_after_a_settings_change() {
    let h = Harness::new();
    let wb = h.import(&workbook(
        "B",
        &[],
        &[("q1", true), ("q2", true), ("q3", true)],
    ));
    let exam_id = h
        .service()
        .create_exam(exam_dated("Rest", &wb, Some(date(2026, 1, 2)), 0))
        .unwrap();
    let summary = h.service().exam_summary(&exam_id).unwrap();
    assert_eq!(summary.plan.coverage.selected, 0);
    assert!(
        summary
            .plan
            .warnings
            .iter()
            .any(|warning| matches!(warning, workbook_scheduler::PlanWarning::NoTime))
    );
    assert!(h.service().start_exam_plan(&exam_id).is_err());

    let mut draft = exam_dated("Rest", &wb, Some(date(2026, 1, 2)), 10);
    draft.daily_minutes = 10;
    h.service().update_exam(&exam_id, draft).unwrap();
    let summary = h.service().exam_summary(&exam_id).unwrap();
    assert!(summary.plan.coverage.selected > 0);
    h.service().start_exam_plan(&exam_id).unwrap();
    let view = h.service().active_view().unwrap().unwrap();
    assert_eq!(view.progress.total, summary.plan.coverage.selected);
}

#[test]
fn exam_plan_is_deadline_aware_and_expires_across_the_four_am_boundary() {
    let h = Harness::new();
    let mut settings = h.service().settings().unwrap();
    settings.timezone = "Asia/Tokyo".to_string();
    h.service().update_settings(settings).unwrap();
    h.clock
        .set(Utc.with_ymd_and_hms(2026, 1, 1, 16, 0, 0).unwrap()); // Jan 2 01:00 JST
    let wb = h.import(&workbook("T", &[], &[("q1", true)]));
    let exam_id = h
        .service()
        .create_exam(exam_dated("Tokyo", &wb, Some(date(2026, 1, 2)), 10))
        .unwrap();
    let summary = h.service().exam_summary(&exam_id).unwrap();
    assert_eq!(summary.days_remaining, Some(1));
    assert!(!summary.expired);

    h.clock
        .set(Utc.with_ymd_and_hms(2026, 1, 2, 20, 0, 0).unwrap()); // Jan 3 05:00 JST
    let summary = h.service().exam_summary(&exam_id).unwrap();
    assert!(summary.expired);
    assert_eq!(summary.days_remaining, Some(0));

    // Moving the exam date later recomputes the plan from the same history.
    let mut draft = exam_dated("Tokyo", &wb, Some(date(2026, 1, 4)), 10);
    draft.daily_minutes = 10;
    h.service().update_exam(&exam_id, draft).unwrap();
    let summary = h.service().exam_summary(&exam_id).unwrap();
    assert!(!summary.expired);
    assert_eq!(summary.days_remaining, Some(1));
    assert!(
        !summary
            .plan
            .warnings
            .iter()
            .any(|warning| matches!(warning, workbook_scheduler::PlanWarning::Expired))
    );
}

#[test]
fn two_exams_share_one_fsrs_history() {
    let h = Harness::new();
    let wb = h.import(&workbook("S", &[], &[("q1", true), ("q2", true)]));
    let first = h
        .service()
        .create_exam(exam_dated("First", &wb, Some(date(2026, 1, 2)), 10))
        .unwrap();
    let second = h
        .service()
        .create_exam(exam_dated("Second", &wb, Some(date(2026, 1, 3)), 10))
        .unwrap();
    h.service().start_exam_plan(&first).unwrap();
    h.complete_active();
    let summary = h.service().exam_summary(&second).unwrap();
    assert_eq!(summary.studied, 2);
    assert_eq!(summary.plan.coverage.selected_new, 0);
    assert_eq!(h.state_count(), 2);
}

#[test]
fn plan_introductions_count_toward_the_daily_new_limit() {
    let h = Harness::new();
    let mut settings = h.service().settings().unwrap();
    settings.new_limit = 1;
    h.service().update_settings(settings).unwrap();
    let wb = h.import(&workbook(
        "L",
        &[],
        &[("q1", true), ("q2", true), ("q3", true)],
    ));
    let exam_id = h
        .service()
        .create_exam(exam_dated("Limit", &wb, Some(date(2026, 1, 2)), 30))
        .unwrap();
    let summary = h.service().exam_summary(&exam_id).unwrap();
    assert_eq!(summary.plan.coverage.selected_new, 1);
    h.service().start_exam_plan(&exam_id).unwrap();
    h.complete_active();

    let summary = h.service().exam_summary(&exam_id).unwrap();
    assert_eq!(summary.plan.coverage.selected_new, 0);
    assert_eq!(h.state_count(), 1);
}

#[test]
fn infeasible_deadlines_report_the_shortfall_without_promising_coverage() {
    let h = Harness::new();
    let questions: Vec<_> = (0..20).map(|index| (format!("q{index}"), true)).collect();
    let borrowed: Vec<_> = questions
        .iter()
        .map(|(id, answer)| (id.as_str(), *answer))
        .collect();
    let wb = h.import(&workbook("F", &[], &borrowed));
    let exam_id = h
        .service()
        .create_exam(exam_dated("Short", &wb, Some(date(2026, 1, 1)), 1))
        .unwrap();
    let summary = h.service().exam_summary(&exam_id).unwrap();
    assert_eq!(
        summary.plan.feasibility,
        workbook_scheduler::PlanFeasibility::Insufficient
    );
    assert!(summary.plan.warnings.iter().any(|warning| matches!(
        warning,
        workbook_scheduler::PlanWarning::InsufficientTime { .. }
    )));
    assert!(summary.plan.coverage.selected > 0);
    assert!(summary.plan.planned_seconds <= 60);
    assert!(summary.plan.coverage.selected < summary.plan.coverage.total);
}
