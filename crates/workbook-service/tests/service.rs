use std::sync::Arc;

use chrono::{TimeZone, Utc};
use rusqlite::Connection;
use tempfile::TempDir;
use uuid::Uuid;
use workbook_domain::{Answer, Mode, Phase, Rating, RatingSource};
use workbook_scheduler::{Clock, FixedClock};
use workbook_service::{SAMPLE_WORKBOOK_JSON, ServiceError, StudyView, WorkbookService};

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
        // The legacy scenarios in this file exercise the explicit manual
        // rating command. Automatic-rating scenarios opt in with `new_auto`.
        settings.automatic_rating = false;
        service.update_settings(settings).unwrap();
        Self {
            _dir: dir,
            clock,
            service: Some(service),
        }
    }

    fn new_auto() -> Self {
        let harness = Self::new();
        let mut settings = harness.service().settings().unwrap();
        settings.automatic_rating = true;
        harness.service().update_settings(settings).unwrap();
        harness
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

    fn start(&self, mode: Mode, book: Option<&str>) -> StudyView {
        self.service().start_session(mode, book).unwrap();
        self.service().active_view().unwrap().unwrap()
    }

    fn begin(&self, view: &StudyView) -> StudyView {
        if view.phase == Phase::Intro {
            self.service()
                .begin_question(&view.session_id, view.revision)
                .unwrap()
        } else {
            view.clone()
        }
    }

    fn evaluate(&self, view: &StudyView, answer: Option<Answer>) -> StudyView {
        self.evaluate_with_hint(view, answer, false)
    }

    fn evaluate_with_hint(
        &self,
        view: &StudyView,
        answer: Option<Answer>,
        hint: bool,
    ) -> StudyView {
        self.service()
            .evaluate_answer(
                &view.session_id,
                answer.as_ref(),
                false,
                None,
                hint,
                view.revision,
            )
            .unwrap()
    }

    fn commit(&self, view: &StudyView, rating: Rating) -> StudyView {
        self.service()
            .commit_rating(
                &view.session_id,
                &Uuid::new_v4().to_string(),
                rating,
                view.revision,
            )
            .unwrap()
    }

    fn answer(&self, view: &StudyView, answer: Option<Answer>, rating: Rating) -> StudyView {
        let view = self.begin(view);
        let evaluated = self.evaluate(&view, answer);
        assert!(!evaluated.feedback.as_ref().unwrap().committed);
        self.commit(&evaluated, rating)
    }

    fn next(&self, view: &StudyView) -> StudyView {
        self.service()
            .next(&view.session_id, view.revision)
            .unwrap()
    }

    fn finish(&self, view: &StudyView) {
        self.service()
            .finish(&view.session_id, view.revision)
            .unwrap();
    }

    fn history(&self) -> Vec<workbook_service::HistoryEntry> {
        self.service().history(20).unwrap()
    }

    fn evaluate_unknown(&self, view: &StudyView) -> StudyView {
        self.service()
            .evaluate_answer(&view.session_id, None, true, None, false, view.revision)
            .unwrap()
    }

    fn attempt_rows(&self) -> Vec<(String, u8, Option<String>, String)> {
        let connection = Connection::open(self._dir.path().join("workbook.sqlite")).unwrap();
        let mut statement = connection
            .prepare(
                "SELECT rating_source,rating,after_state_json,answered_at
                 FROM attempts ORDER BY answered_at,rowid",
            )
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? as u8,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .unwrap();
        rows.map(|row| row.unwrap()).collect()
    }

    fn review_state_count(&self) -> i64 {
        let connection = Connection::open(self._dir.path().join("workbook.sqlite")).unwrap();
        connection
            .query_row("SELECT COUNT(*) FROM review_states", [], |row| row.get(0))
            .unwrap()
    }
}

fn one_question(id: &str) -> String {
    format!(
        r#"{{
          "format":"workbook","schemaVersion":1,"id":"{id}","revision":1,
          "title":"Tiny {id}","description":"","language":"ja","tags":[],
          "notes":[{{"id":"n-{id}","title":"Note","knowledgeCard":"card","questions":[
            {{"id":"q-{id}","revision":1,"type":"true_false","prompt":"true?",
              "answer":true,"explanation":"yes"}}
          ]}}]
        }}"#
    )
}

fn sibling_workbook(id: &str) -> String {
    format!(
        r#"{{
          "format":"workbook","schemaVersion":1,"id":"{id}","revision":1,
          "title":"Sibling","description":"","language":"ja","tags":[],
          "notes":[{{"id":"n","title":"Note","knowledgeCard":"card","questions":[
            {{"id":"q1","revision":1,"type":"true_false","prompt":"one?","answer":true,"explanation":"yes"}},
            {{"id":"q2","revision":1,"type":"true_false","prompt":"two?","answer":true,"explanation":"yes"}}
          ]}}]
        }}"#
    )
}

fn flashcard_workbook(id: &str) -> String {
    format!(
        r#"{{
          "format":"workbook","schemaVersion":1,"id":"{id}","revision":1,
          "title":"Flash","description":"","language":"ja","tags":[],
          "notes":[{{"id":"n-{id}","title":"Note","knowledgeCard":"card","questions":[
            {{"id":"q-{id}","revision":1,"type":"flashcard","front":"front","back":"back",
              "explanation":"because"}}
          ]}}]
        }}"#
    )
}

#[test]
fn consecutive_automatic_answers_advance_without_manual_rating() {
    let h = Harness::new_auto();
    let book = h.import(&sibling_workbook("auto-consecutive"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let first = h.evaluate(&view, Some(Answer::Bool(true)));
    assert!(first.feedback.as_ref().unwrap().committed);
    let second_view = h.next(&first);
    let second_view = h.begin(&second_view);
    let second = h.evaluate(&second_view, Some(Answer::Bool(true)));
    assert!(second.feedback.as_ref().unwrap().committed);
    h.finish(&second);
    let rows = h.attempt_rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.0 == "automatic"));
    assert_eq!(h.history()[0].stats.attempts, 2);
}

#[test]
fn blank_answer_is_not_auto_rated_as_incorrect() {
    let h = Harness::new_auto();
    let book = h.import(SAMPLE_WORKBOOK_JSON);
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let error = h
        .service()
        .evaluate_answer(&view.session_id, None, false, None, false, view.revision)
        .unwrap_err();
    assert!(matches!(error, ServiceError::Invalid(_)));
    assert!(h.attempt_rows().is_empty());
    let active = h.service().active_view().unwrap().unwrap();
    assert_eq!(active.phase, Phase::Question);
    assert_eq!(active.revision, view.revision);
}

#[test]
fn automatic_rating_commits_incorrect_and_unknown_as_again() {
    let h = Harness::new_auto();
    let book = h.import(&one_question("auto-again"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let evaluated = h.evaluate(&view, Some(Answer::Bool(false)));
    let feedback = evaluated.feedback.as_ref().unwrap();
    assert!(feedback.committed);
    assert_eq!(feedback.correct, Some(false));
    assert_eq!(feedback.rating, Some(Rating::Again));
    assert_eq!(feedback.rating_source, Some(RatingSource::Automatic));
    assert!(feedback.attempt_id.is_some());
    h.finish(&evaluated);

    let h = Harness::new_auto();
    let book = h.import(&one_question("auto-unknown"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let evaluated = h.evaluate_unknown(&view);
    let feedback = evaluated.feedback.as_ref().unwrap();
    assert!(feedback.committed);
    assert!(feedback.unknown);
    assert_eq!(feedback.rating, Some(Rating::Again));
    assert_eq!(feedback.rating_source, Some(RatingSource::Automatic));
    h.finish(&evaluated);
}

#[test]
fn automatic_rating_uses_hard_for_hinted_correct_and_good_without_hint() {
    let h = Harness::new_auto();
    let book = h.import(&one_question("auto-hard"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let evaluated = h.evaluate_with_hint(&view, Some(Answer::Bool(true)), true);
    let feedback = evaluated.feedback.as_ref().unwrap();
    assert!(feedback.hint);
    assert_eq!(feedback.rating, Some(Rating::Hard));
    assert_eq!(feedback.rating_source, Some(RatingSource::Automatic));
    h.finish(&evaluated);

    let h = Harness::new_auto();
    let book = h.import(&one_question("auto-good"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let evaluated = h.evaluate(&view, Some(Answer::Bool(true)));
    let feedback = evaluated.feedback.as_ref().unwrap();
    assert_eq!(feedback.rating, Some(Rating::Good));
    assert_ne!(feedback.rating, Some(Rating::Easy));
    assert_eq!(feedback.rating_source, Some(RatingSource::Automatic));
    h.finish(&evaluated);
}

#[test]
fn automatic_rating_falls_back_to_manual_for_subjective_flashcards() {
    let h = Harness::new_auto();
    let book = h.import(&flashcard_workbook("fc"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let revealed = h
        .service()
        .save_draft(&view.session_id, None, false, true, view.revision)
        .unwrap();
    let evaluated = h
        .service()
        .evaluate_answer(
            &revealed.session_id,
            Some(&Answer::Bool(true)),
            false,
            Some(true),
            false,
            revealed.revision,
        )
        .unwrap();
    let feedback = evaluated.feedback.as_ref().unwrap();
    assert!(!feedback.committed);
    assert_eq!(feedback.suggested_rating, None);
    assert_eq!(feedback.rating, None);
    let committed = h.commit(&evaluated, Rating::Good);
    assert!(committed.feedback.as_ref().unwrap().committed);
    assert_eq!(
        committed.feedback.as_ref().unwrap().rating_source,
        Some(RatingSource::Manual)
    );
    h.finish(&committed);
}

#[test]
fn automatic_and_manual_ratings_produce_identical_fsrs_state() {
    let auto = Harness::new_auto();
    let auto_book = auto.import(&one_question("same-fsrs"));
    let auto_view = auto.start(Mode::New, Some(&auto_book));
    let auto_view = auto.begin(&auto_view);
    let auto_evaluated = auto.evaluate(&auto_view, Some(Answer::Bool(true)));
    assert!(auto_evaluated.feedback.as_ref().unwrap().committed);
    let auto_row = auto.attempt_rows().remove(0);

    let manual = Harness::new();
    let manual_book = manual.import(&one_question("same-fsrs"));
    let manual_view = manual.start(Mode::New, Some(&manual_book));
    let manual_view = manual.begin(&manual_view);
    let manual_evaluated = manual.evaluate(&manual_view, Some(Answer::Bool(true)));
    let manual_committed = manual.commit(&manual_evaluated, Rating::Good);
    assert!(manual_committed.feedback.as_ref().unwrap().committed);
    let manual_row = manual.attempt_rows().remove(0);

    assert_eq!(auto_row.1, manual_row.1);
    assert_eq!(auto_row.2, manual_row.2);
    assert_eq!(auto_row.3, manual_row.3);
    assert_eq!(auto_row.0, "automatic");
    assert_eq!(manual_row.0, "manual");
}

#[test]
fn automatic_correction_updates_the_same_attempt_and_recomputes_fsrs() {
    let h = Harness::new_auto();
    let book = h.import(&one_question("auto-correct"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let evaluated = h.evaluate(&view, Some(Answer::Bool(true)));
    let attempt_id = evaluated
        .feedback
        .as_ref()
        .unwrap()
        .attempt_id
        .clone()
        .unwrap();
    let before = h.attempt_rows().remove(0);
    assert_eq!(before.1, Rating::Good.value());
    let corrected = h
        .service()
        .correct_attempt(
            &evaluated.session_id,
            &attempt_id,
            None,
            Some(Rating::Hard),
            evaluated.revision,
        )
        .unwrap();
    let feedback = corrected.feedback.as_ref().unwrap();
    assert_eq!(feedback.rating, Some(Rating::Hard));
    assert_eq!(feedback.rating_source, Some(RatingSource::Manual));
    let after = h.attempt_rows();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].1, Rating::Hard.value());
    assert_ne!(before.2, after[0].2);
    h.finish(&corrected);
    assert_eq!(h.history()[0].stats.attempts, 1);

    // The corrected state is recomputed from the saved before-state, not from
    // the automatic Good state. It must equal a direct manual Hard commit.
    let manual = Harness::new();
    let manual_book = manual.import(&one_question("auto-correct"));
    let manual_view = manual.start(Mode::New, Some(&manual_book));
    let manual_view = manual.begin(&manual_view);
    let manual_evaluated = manual.evaluate(&manual_view, Some(Answer::Bool(true)));
    let manual_committed = manual.commit(&manual_evaluated, Rating::Hard);
    assert!(manual_committed.feedback.as_ref().unwrap().committed);
    let manual_row = manual.attempt_rows().remove(0);
    assert_eq!(after[0].2, manual_row.2);
}

#[test]
fn automatic_double_submit_is_rejected_and_keeps_one_attempt() {
    let h = Harness::new_auto();
    let book = h.import(&one_question("auto-double"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let evaluated = h.evaluate(&view, Some(Answer::Bool(true)));
    let attempt_id = evaluated
        .feedback
        .as_ref()
        .unwrap()
        .attempt_id
        .clone()
        .unwrap();
    let error = h
        .service()
        .evaluate_answer(
            &evaluated.session_id,
            Some(&Answer::Bool(true)),
            false,
            None,
            false,
            view.revision,
        )
        .unwrap_err();
    assert!(matches!(error, ServiceError::Conflict(_)));
    let idempotent = h
        .service()
        .commit_rating(
            &evaluated.session_id,
            &attempt_id,
            Rating::Good,
            evaluated.revision,
        )
        .unwrap();
    assert_eq!(
        idempotent.feedback.as_ref().unwrap().attempt_id.as_deref(),
        Some(attempt_id.as_str())
    );
    assert_eq!(h.attempt_rows().len(), 1);
    h.finish(&idempotent);
}

#[test]
fn automatic_commit_survives_restart_without_duplicating_attempts() {
    let mut h = Harness::new_auto();
    let book = h.import(&one_question("auto-restart"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let _evaluated = h.evaluate(&view, Some(Answer::Bool(true)));
    h.reopen();
    let feedback = h.service().active_view().unwrap().unwrap();
    assert_eq!(feedback.phase, Phase::Feedback);
    let feedback_view = feedback.feedback.as_ref().unwrap();
    assert!(feedback_view.committed);
    assert_eq!(feedback_view.rating_source, Some(RatingSource::Automatic));
    h.finish(&feedback);
    assert_eq!(h.attempt_rows().len(), 1);
    assert_eq!(h.history()[0].stats.attempts, 1);
}

#[test]
fn automatic_practice_records_attempt_without_fsrs_update() {
    let h = Harness::new_auto();
    let book = h.import(&one_question("auto-practice"));
    let view = h.start(Mode::Practice, Some(&book));
    let view = h.begin(&view);
    let evaluated = h.evaluate(&view, Some(Answer::Bool(true)));
    assert!(evaluated.feedback.as_ref().unwrap().committed);
    assert_eq!(
        evaluated.feedback.as_ref().unwrap().rating_source,
        Some(RatingSource::Automatic)
    );
    assert_eq!(h.review_state_count(), 0);
    h.finish(&evaluated);
    assert_eq!(h.attempt_rows().len(), 1);
}

#[test]
fn automatic_setting_can_be_toggled_mid_session() {
    let h = Harness::new_auto();
    let book = h.import(&sibling_workbook("toggle"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let auto = h.evaluate(&view, Some(Answer::Bool(true)));
    assert!(auto.feedback.as_ref().unwrap().committed);
    let mut settings = h.service().settings().unwrap();
    settings.automatic_rating = false;
    h.service().update_settings(settings).unwrap();
    let next = h.next(&auto);
    let next = h.begin(&next);
    let manual = h.evaluate(&next, Some(Answer::Bool(true)));
    assert!(!manual.feedback.as_ref().unwrap().committed);
    assert_eq!(
        manual.feedback.as_ref().unwrap().suggested_rating,
        Some(Rating::Good)
    );
    let committed = h.commit(&manual, Rating::Good);
    h.finish(&committed);
    let rows = h.attempt_rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "automatic");
    assert_eq!(rows[1].0, "manual");
}

#[test]
fn new_session_shows_intro_once_and_saves_answer() {
    let h = Harness::new();
    let book = h.import(SAMPLE_WORKBOOK_JSON);
    let view = h.start(Mode::New, Some(&book));
    assert_eq!(view.phase, Phase::Intro);
    assert_eq!(view.progress.total, 6);
    let view = h.begin(&view);
    assert_eq!(view.phase, Phase::Question);
    let view = h.evaluate(&view, Some(Answer::Text("b".to_string())));
    let feedback = view.feedback.as_ref().unwrap();
    assert_eq!(feedback.correct, Some(true));
    assert_eq!(feedback.suggested_rating, Some(Rating::Good));
    let view = h.commit(&view, Rating::Good);
    assert_eq!(view.phase, Phase::Feedback);
    assert!(view.feedback.as_ref().unwrap().committed);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::Intro);
    let view = h.begin(&view);
    assert_eq!(view.phase, Phase::Question);
    assert_eq!(view.progress.position, 2);
    h.finish(&view);
    assert_eq!(h.history().len(), 1);
}

#[test]
fn new_limit_counts_distinct_questions_and_skip_does_not_write_fsrs() {
    let h = Harness::new();
    let book = h.import(SAMPLE_WORKBOOK_JSON);
    let mut settings = h.service().settings().unwrap();
    settings.new_limit = 2;
    h.service().update_settings(settings).unwrap();
    let view = h.start(Mode::New, Some(&book));
    assert_eq!(view.progress.total, 2);
    let skipped = h.service().skip(&view.session_id, view.revision).unwrap();
    h.finish(&skipped);
    // The skipped question was not introduced and remains available.
    let again = h.start(Mode::New, Some(&book));
    assert_eq!(again.progress.total, 2);
    h.finish(&again);
}

#[test]
fn one_question_session_reissues_immediately_and_keeps_each_attempt() {
    let h = Harness::new();
    let book = h.import(&one_question("one"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::Question);
    assert_eq!(view.item_kind, "short_term");
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Again);
    let view = h.next(&view);
    assert_eq!(view.item_kind, "short_term");
    h.finish(&view);
    let history = h.history();
    let stats = history[0].stats;
    assert_eq!(stats.questions, 1);
    assert_eq!(stats.answered, 1);
    assert_eq!(stats.attempts, 2);
    assert_eq!(stats.short_term, 1);
    assert_eq!(stats.first_pass_correct, 1);
}

#[test]
fn duplicate_attempt_id_is_idempotent_and_corrections_do_not_add_attempts() {
    let h = Harness::new();
    let book = h.import(&one_question("idem"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let evaluated = h.evaluate(&view, Some(Answer::Bool(false)));
    assert_eq!(evaluated.feedback.as_ref().unwrap().correct, Some(false));
    let attempt_id = Uuid::new_v4().to_string();
    let committed = h
        .service()
        .commit_rating(
            &evaluated.session_id,
            &attempt_id,
            Rating::Again,
            evaluated.revision,
        )
        .unwrap();
    let again = h
        .service()
        .commit_rating(
            &evaluated.session_id,
            &attempt_id,
            Rating::Again,
            committed.revision,
        )
        .unwrap();
    assert_eq!(
        again.feedback.as_ref().unwrap().attempt_id.as_deref(),
        Some(attempt_id.as_str())
    );
    let corrected = h
        .service()
        .correct_attempt(
            &again.session_id,
            &attempt_id,
            Some(true),
            None,
            again.revision,
        )
        .unwrap();
    assert_eq!(corrected.feedback.as_ref().unwrap().correct, Some(true));
    assert_eq!(
        corrected.feedback.as_ref().unwrap().rating,
        Some(Rating::Good)
    );
    let corrected = h
        .service()
        .correct_attempt(
            &corrected.session_id,
            &attempt_id,
            None,
            Some(Rating::Hard),
            corrected.revision,
        )
        .unwrap();
    assert_eq!(
        corrected.feedback.as_ref().unwrap().rating,
        Some(Rating::Hard)
    );
    h.finish(&corrected);
    let stats = h.history()[0].stats;
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.first_pass_correct, 1);
}

#[test]
fn stale_revision_is_rejected() {
    let h = Harness::new();
    let book = h.import(&one_question("stale"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let evaluated = h.evaluate(&view, Some(Answer::Bool(true)));
    let error = h
        .service()
        .evaluate_answer(
            &evaluated.session_id,
            Some(&Answer::Bool(true)),
            false,
            None,
            false,
            view.revision,
        )
        .unwrap_err();
    assert!(matches!(error, ServiceError::Conflict(_)));
}

#[test]
fn sibling_questions_are_suppressed_in_review_but_same_question_repeats() {
    let h = Harness::new();
    let book = h.import(&sibling_workbook("sib"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    h.finish(&view);
    h.clock.advance(chrono::Duration::minutes(11));
    let review = h.start(Mode::Review, Some(&book));
    assert_eq!(review.progress.total, 1);
    let review = h.answer(&review, Some(Answer::Bool(false)), Rating::Again);
    let repeated = h.next(&review);
    assert_eq!(repeated.item_kind, "short_term");
    assert_eq!(repeated.progress.total, 1);
    h.finish(&repeated);
}

#[test]
fn multi_workbook_review_stops_at_each_result_and_requires_continue() {
    let h = Harness::new();
    let a = h.import(&one_question("a"));
    let b = h.import(&one_question("b"));
    for book in [&a, &b] {
        let view = h.start(Mode::New, Some(book));
        let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
        let view = h.next(&view);
        let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
        let view = h.next(&view);
        assert_eq!(view.phase, Phase::WorkbookResult);
        h.finish(&view);
    }
    h.clock.advance(chrono::Duration::days(11));
    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.today.due, 2);
    assert_eq!(
        snapshot
            .today
            .books
            .iter()
            .filter(|book| book.due == 1)
            .count(),
        2
    );
    let view = h.start(Mode::Review, None);
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    assert_eq!(h.service().snapshot().unwrap().today.due, 1);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::WorkbookResult);
    assert!(
        view.workbook_result
            .as_ref()
            .unwrap()
            .next_workbook_title
            .is_some()
    );
    let view = h
        .service()
        .continue_session(&view.session_id, view.revision)
        .unwrap();
    assert_eq!(view.phase, Phase::Question);
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::WorkbookResult);
    assert!(
        view.workbook_result
            .as_ref()
            .unwrap()
            .next_workbook_title
            .is_none()
    );
    let view = h
        .service()
        .continue_session(&view.session_id, view.revision)
        .unwrap();
    assert_eq!(view.phase, Phase::OverallResult);
    assert!(h.service().snapshot().unwrap().active.is_none());
}

#[test]
fn restart_recovery_preserves_feedback_and_excludes_closed_time() {
    let mut h = Harness::new();
    let book = h.import(&one_question("restart"));
    let view = h.start(Mode::New, Some(&book));
    let _view = h.begin(&view);
    h.clock.advance(chrono::Duration::hours(1));
    h.reopen();
    let view = h.service().active_view().unwrap().unwrap();
    assert_eq!(view.phase, Phase::Question);
    let evaluated = h.evaluate(&view, Some(Answer::Bool(true)));
    assert_eq!(evaluated.feedback.as_ref().unwrap().elapsed_ms, 0);
    h.clock.advance(chrono::Duration::seconds(2));
    h.reopen();
    let feedback = h.service().active_view().unwrap().unwrap();
    assert_eq!(feedback.phase, Phase::Feedback);
    assert!(!feedback.feedback.as_ref().unwrap().committed);
    let committed = h.commit(&feedback, Rating::Good);
    h.finish(&committed);
    assert_eq!(h.history()[0].stats.attempts, 1);
}

#[test]
fn learn_ahead_does_not_cross_the_four_am_boundary() {
    let h = Harness::new();
    h.clock
        .set(Utc.with_ymd_and_hms(2026, 1, 1, 3, 55, 0).unwrap());
    let book = h.import(&one_question("boundary"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::WorkbookResult);
    h.finish(&view);
}

#[test]
fn independent_copies_and_safe_delete() {
    let h = Harness::new();
    let a = h.import(&one_question("copy"));
    let b = h.import(&one_question("copy"));
    assert_ne!(a, b);
    h.service().delete_workbook(&a).unwrap();
    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.books.len(), 1);
    assert_eq!(snapshot.books[0].local_id, b);
}

#[test]
fn backup_restore_and_corrupt_backup_rejection() {
    let h = Harness::new();
    h.import(&one_question("backup"));
    let backup = h._dir.path().join("manual.sqlite");
    h.service().backup_to(&backup).unwrap();
    h.import(&one_question("later"));
    assert_eq!(h.service().snapshot().unwrap().books.len(), 2);
    h.service().restore_from(&backup).unwrap();
    assert_eq!(h.service().snapshot().unwrap().books.len(), 1);
    let corrupt = h._dir.path().join("corrupt.sqlite");
    std::fs::write(&corrupt, b"not a database").unwrap();
    assert!(h.service().restore_from(&corrupt).is_err());
    assert_eq!(h.service().snapshot().unwrap().books.len(), 1);
}

fn create_legacy_db(path: &std::path::Path) -> String {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            r#"
            PRAGMA application_id=1463962443;
            PRAGMA user_version=1;
            CREATE TABLE workbooks(id TEXT PRIMARY KEY, content TEXT NOT NULL, imported_at TEXT NOT NULL, last_used TEXT);
            CREATE TABLE notes(workbook_id TEXT NOT NULL, id TEXT NOT NULL, introduced INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(workbook_id,id));
            CREATE TABLE questions(workbook_id TEXT NOT NULL, note_id TEXT NOT NULL, id TEXT NOT NULL, buried_until TEXT, PRIMARY KEY(workbook_id,id));
            CREATE TABLE review_states(workbook_id TEXT NOT NULL, question_id TEXT NOT NULL, card TEXT NOT NULL, due TEXT NOT NULL, PRIMARY KEY(workbook_id,question_id));
            CREATE TABLE sessions(id TEXT PRIMARY KEY, data TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1);
            CREATE TABLE review_logs(session_id TEXT NOT NULL, workbook_id TEXT NOT NULL, question_id TEXT NOT NULL, outcome TEXT NOT NULL, PRIMARY KEY(session_id,workbook_id,question_id));
            CREATE TABLE practice_attempts(session_id TEXT NOT NULL, workbook_id TEXT NOT NULL, question_id TEXT NOT NULL, outcome TEXT NOT NULL, PRIMARY KEY(session_id,workbook_id,question_id));
            CREATE TABLE settings(id INTEGER PRIMARY KEY CHECK(id=1), data TEXT NOT NULL);
            "#,
        )
        .unwrap();
    let workbook = workbook_domain::parse_workbook(SAMPLE_WORKBOOK_JSON).unwrap();
    let legacy_id = "legacy-book";
    connection
        .execute(
            "INSERT INTO workbooks VALUES(?1,?2,?3,NULL)",
            rusqlite::params![legacy_id, SAMPLE_WORKBOOK_JSON, "2025-12-01T00:00:00.000Z"],
        )
        .unwrap();
    for note in &workbook.notes {
        connection
            .execute(
                "INSERT INTO notes(workbook_id,id,introduced) VALUES(?1,?2,1)",
                rusqlite::params![legacy_id, note.id],
            )
            .unwrap();
        for question in &note.questions {
            connection
                .execute(
                    "INSERT INTO questions(workbook_id,note_id,id,buried_until) VALUES(?1,?2,?3,NULL)",
                    rusqlite::params![legacy_id, note.id, question.id],
                )
                .unwrap();
        }
    }
    let card = serde_json::json!({
        "stability": 5.0, "stabilityFast": 20.0, "difficulty": 4.0, "state": 2,
        "dueAt": "2025-12-31T00:00:00.000Z", "lastReviewAt": "2025-12-20T00:00:00.000Z",
        "cardId": "legacy-card", "reps": 3, "lapses": 0, "scheduledDays": 11.0,
        "learningStep": 0, "scheduleStatus": "review"
    });
    connection
        .execute(
            "INSERT INTO review_states VALUES(?1,'q1',?2,'2025-12-31T00:00:00.000Z')",
            rusqlite::params![legacy_id, card.to_string()],
        )
        .unwrap();
    let outcome = serde_json::json!({
        "correct": true, "unknown": false, "hint": false, "rating": 3,
        "answer": "b", "elapsedMs": 1234, "at": "2025-12-20T00:00:00.000Z",
        "before": null
    });
    let mut outcomes = serde_json::Map::new();
    outcomes.insert(format!("{legacy_id}/q1"), outcome.clone());
    let mut session = serde_json::json!({
        "id":"legacy-session","mode":"new",
        "queue":[{"workbookId":legacy_id,"noteId":"promise","questionId":"q1"}],
        "index":1,"phase":"workbookResult","draft":null,"hint":false,"revealed":false,
        "startedAt":"2025-12-20T00:00:00.000Z","questionStartedAt":"2025-12-20T00:00:00.000Z",
        "questionElapsedMs":1234,"paused":false,
        "outcomes":{},"skipped":[],
        "endedAt":"2025-12-20T00:10:00.000Z"
    });
    session["outcomes"] = serde_json::Value::Object(outcomes);
    connection
        .execute(
            "INSERT INTO sessions VALUES('legacy-session',?,0)",
            rusqlite::params![session.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO review_logs VALUES('legacy-session',?1,'q1',?2)",
            rusqlite::params![legacy_id, outcome.to_string()],
        )
        .unwrap();
    let active = serde_json::json!({
        "id":"legacy-active","mode":"review",
        "queue":[{"workbookId":legacy_id,"noteId":"async","questionId":"q2"}],
        "index":0,"phase":"question","draft":null,"hint":false,"revealed":false,
        "startedAt":"2025-12-31T00:00:00.000Z","questionStartedAt":"2025-12-31T00:00:00.000Z",
        "questionElapsedMs":0,"paused":false,"outcomes":{},"skipped":[],"endedAt":null
    });
    connection
        .execute(
            "INSERT INTO sessions VALUES('legacy-active',?,1)",
            rusqlite::params![active.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO settings VALUES(1,?)",
            rusqlite::params![
                serde_json::json!({
                    "theme":"system","newLimit":10,"automaticRating":true,
                    "retention":0.9,"externalImages":true,"autoBackup":true
                })
                .to_string()
            ],
        )
        .unwrap();
    workbook.title
}

#[test]
fn legacy_v1_migration_preserves_cards_and_single_attempt_history() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("legacy.sqlite");
    let title = create_legacy_db(&legacy);
    let clock = Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    ));
    let service = WorkbookService::open(dir.path().join("new-data").as_path(), clock).unwrap();
    let preview = WorkbookService::legacy_preview(&legacy).unwrap();
    assert_eq!(preview.workbook_count, 1);
    assert_eq!(preview.review_state_count, 1);
    assert_eq!(preview.attempt_count, 1);
    assert!(preview.active_session);
    let imported = service.import_legacy(&legacy).unwrap();
    assert_eq!(imported.attempt_count, 1);
    let snapshot = service.snapshot().unwrap();
    assert_eq!(snapshot.books.len(), 1);
    assert_eq!(snapshot.books[0].title, title);
    assert_eq!(snapshot.books[0].learned, 1);
    assert_eq!(snapshot.books[0].due, 1);
    let history = service.history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].stats.attempts, 1);
    assert_eq!(history[0].stats.first_pass_correct, 1);
    let active = service
        .active_view()
        .unwrap()
        .expect("migrated active session");
    assert_eq!(active.phase, Phase::Question);
    assert_eq!(active.note_title.as_deref(), Some("async関数"));
}

#[test]
fn legacy_invalid_database_is_rejected_without_touching_new_data() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.sqlite");
    let connection = Connection::open(&bad).unwrap();
    connection
        .execute_batch(
            "PRAGMA application_id=42; PRAGMA user_version=1; CREATE TABLE workbooks(id TEXT);",
        )
        .unwrap();
    drop(connection);
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let service = WorkbookService::open(dir.path().join("new-data").as_path(), clock).unwrap();
    service
        .import_workbook_json(&one_question("existing"))
        .unwrap();
    assert!(service.import_legacy(&bad).is_err());
    assert_eq!(service.snapshot().unwrap().books.len(), 1);
}

#[test]
fn scenario_a_new_again_learn_ahead_good_transitions_to_review() {
    let h = Harness::new();
    let book = h.import(&one_question("scenario-a"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(false)), Rating::Again);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::Question);
    assert_eq!(view.item_kind, "short_term");
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    assert_eq!(view.item_kind, "short_term");
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::WorkbookResult);
    h.finish(&view);
    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.books[0].learned, 1);
    assert_eq!(snapshot.books[0].due, 0);
    let stats = h.history()[0].stats;
    assert_eq!(stats.attempts, 3);
    assert_eq!(stats.short_term, 2);
    assert_eq!(stats.first_pass_graded, 1);
    h.clock.advance(chrono::Duration::days(11));
    assert_eq!(h.service().snapshot().unwrap().books[0].due, 1);
}

#[test]
fn scenario_b_review_again_relearning_then_new_review_due() {
    let h = Harness::new();
    let book = h.import(&one_question("scenario-b"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    h.finish(&view);
    h.clock.advance(chrono::Duration::days(11));

    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.today.due, 1);
    let review = h.start(Mode::Review, None);
    assert_eq!(review.phase, Phase::Question);
    let review = h.answer(&review, Some(Answer::Bool(false)), Rating::Again);
    let review = h.next(&review);
    assert_eq!(review.item_kind, "short_term");
    let review = h.answer(&review, Some(Answer::Bool(true)), Rating::Good);
    h.finish(&review);
    assert_eq!(h.service().snapshot().unwrap().today.due, 0);
    h.clock.advance(chrono::Duration::days(2));
    assert_eq!(h.service().snapshot().unwrap().today.due, 1);
}

#[test]
fn scenario_c_restart_uses_saved_due_without_duplicate_attempts() {
    let mut h = Harness::new();
    let book = h.import(&one_question("scenario-c"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    h.finish(&view);
    h.reopen();
    h.clock.advance(chrono::Duration::days(11));
    assert_eq!(h.service().snapshot().unwrap().today.due, 1);
    let review = h.start(Mode::Review, None);
    let review = h.answer(&review, Some(Answer::Bool(true)), Rating::Good);
    h.finish(&review);
    let attempts: usize = h.history().iter().map(|entry| entry.stats.attempts).sum();
    assert_eq!(attempts, 3);
    h.reopen();
    let attempts: usize = h.history().iter().map(|entry| entry.stats.attempts).sum();
    assert_eq!(attempts, 3);
    assert_eq!(h.service().snapshot().unwrap().today.due, 0);
}

#[test]
fn today_reports_upcoming_short_term_instead_of_completion() {
    let h = Harness::new();
    let book = h.import(&one_question("upcoming"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(false)), Rating::Again);
    h.finish(&view);
    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.today.due, 0);
    assert_eq!(snapshot.today.short_term_pending, 1);
    assert!(snapshot.today.next_due_at.is_some());
    let review = h.start(Mode::Review, None);
    assert_eq!(review.phase, Phase::Question);
    assert_eq!(review.item_kind, "initial");
    let review = h.answer(&review, Some(Answer::Bool(true)), Rating::Good);
    let next = h.next(&review);
    assert_eq!(next.item_kind, "short_term");
    h.finish(&next);
    let review_stats = h
        .history()
        .into_iter()
        .find(|entry| entry.mode == Mode::Review)
        .unwrap()
        .stats;
    assert_eq!(review_stats.attempts, 1);
    assert_eq!(review_stats.short_term, 1);
}

#[test]
fn next_wake_at_tracks_the_next_saved_due() {
    let h = Harness::new();
    let book = h.import(&one_question("wake"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(false)), Rating::Again);
    h.finish(&view);
    let next = h.service().next_wake_at().unwrap().expect("next due");
    assert!(next > h.clock.now());
    assert!(next <= h.clock.now() + chrono::Duration::minutes(1));
}

#[test]
fn restart_after_commit_before_next_does_not_duplicate_attempt() {
    let mut h = Harness::new();
    let book = h.import(&one_question("commit-restart"));
    let view = h.start(Mode::New, Some(&book));
    let _view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    // Crash with the committed feedback still on screen.
    h.reopen();
    let feedback = h.service().active_view().unwrap().unwrap();
    assert_eq!(feedback.phase, Phase::Feedback);
    let feedback_view = feedback.feedback.as_ref().unwrap();
    assert!(feedback_view.committed);
    assert!(feedback_view.attempt_id.is_some());
    h.finish(&feedback);
    assert_eq!(h.history()[0].stats.attempts, 1);
}

#[test]
fn next_wake_also_schedules_the_four_am_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 1, 1, 3, 55, 0).unwrap(),
    ));
    let service = WorkbookService::open(dir.path(), clock).unwrap();
    let mut settings = service.settings().unwrap();
    settings.timezone = "UTC".to_string();
    service.update_settings(settings).unwrap();
    let next = service.next_wake_at().unwrap().unwrap();
    assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 1, 4, 0, 0).unwrap());
}

#[test]
fn daily_new_limit_counts_commits_and_resets_after_four_am() {
    let h = Harness::new();
    let book = h.import(SAMPLE_WORKBOOK_JSON);
    let mut settings = h.service().settings().unwrap();
    settings.new_limit = 1;
    h.service().update_settings(settings).unwrap();
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Text("b".to_string())), Rating::Good);
    h.finish(&view);
    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.today.new_remaining_today, 0);
    let error = h
        .service()
        .start_session(Mode::New, Some(&book))
        .unwrap_err();
    assert!(matches!(error, ServiceError::Invalid(_)));
    h.clock.advance(chrono::Duration::hours(5));
    assert_eq!(h.service().snapshot().unwrap().today.new_remaining_today, 1);
    h.service().start_session(Mode::New, Some(&book)).unwrap();
}

fn pre_import_backups(h: &Harness) -> Vec<std::path::PathBuf> {
    let directory = h._dir.path().join("backups");
    let mut files: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("pre-import-"))
        })
        .collect();
    files.sort();
    files
}

#[test]
fn normal_import_creates_a_valid_pre_import_backup() {
    let h = Harness::new();
    assert!(pre_import_backups(&h).is_empty());
    h.import(&one_question("first"));
    let backups = pre_import_backups(&h);
    assert_eq!(backups.len(), 1);
    WorkbookService::validate_backup(&backups[0]).unwrap();
}

#[test]
fn invalid_workbooks_do_not_create_a_backup() {
    let h = Harness::new();
    assert!(
        h.service()
            .import_workbook_json("{ definitely not json")
            .is_err()
    );
    assert!(pre_import_backups(&h).is_empty());
}

#[test]
fn a_missing_asset_file_does_not_create_a_backup() {
    let h = Harness::new();
    let source = tempfile::tempdir().unwrap();
    let path = source.path().join("workbook.json");
    std::fs::write(
        &path,
        r#"{
          "format":"workbook","schemaVersion":2,"id":"missing-asset","revision":1,
          "title":"T","description":"","language":"ja","tags":[],
          "assets":[{"id":"img","alt":"図の説明","mediaType":"image/png","file":"missing.png"}],
          "notes":[{"id":"n","title":"N","knowledgeCard":"card","questions":[
            {"id":"q","revision":1,"type":"true_false","prompt":"true?","answer":true,"explanation":"yes"}
          ]}]
        }"#,
    )
    .unwrap();
    assert!(h.service().import_workbook_file(&path).is_err());
    assert!(pre_import_backups(&h).is_empty());
}

#[test]
fn backup_failure_aborts_the_import_without_changing_the_database() {
    let h = Harness::new();
    h.import(&one_question("existing"));
    let before = h.service().snapshot().unwrap().books.len();

    let backups = h._dir.path().join("backups");
    std::fs::remove_dir_all(&backups).unwrap();
    std::fs::write(&backups, b"not a directory").unwrap();

    assert!(
        h.service()
            .import_workbook_json(&one_question("never-imported"))
            .is_err()
    );
    assert_eq!(h.service().snapshot().unwrap().books.len(), before);
    assert!(
        h.service()
            .snapshot()
            .unwrap()
            .books
            .iter()
            .all(|book| !book.title.contains("never-imported"))
    );
}

#[test]
fn consecutive_imports_at_the_same_instant_never_overwrite_a_backup() {
    let h = Harness::new();
    for index in 0..3 {
        h.import(&one_question(&format!("same-instant-{index}")));
    }
    let backups = pre_import_backups(&h);
    assert_eq!(backups.len(), 3);
    for backup in &backups {
        WorkbookService::validate_backup(backup).unwrap();
    }
}

#[test]
fn an_unrelated_existing_backup_file_is_not_overwritten() {
    let h = Harness::new();
    let existing = h._dir.path().join("backups").join("keep-me.sqlite");
    std::fs::write(&existing, b"do not overwrite").unwrap();
    h.import(&one_question("protect-existing"));
    assert_eq!(std::fs::read(&existing).unwrap(), b"do not overwrite");
    assert_eq!(pre_import_backups(&h).len(), 1);
}

#[test]
fn import_backup_can_be_restored_and_survives_a_restart() {
    let mut h = Harness::new();
    h.import(&one_question("first"));
    let before_second = pre_import_backups(&h);
    h.import(&one_question("second"));
    assert_eq!(h.service().snapshot().unwrap().books.len(), 2);

    let mut after_second = pre_import_backups(&h);
    after_second.retain(|path| !before_second.contains(path));
    assert_eq!(after_second.len(), 1);
    let backup = after_second.remove(0);

    h.reopen();
    assert_eq!(h.service().snapshot().unwrap().books.len(), 2);

    h.service().restore_from(&backup).unwrap();
    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.books.len(), 1);
    assert!(snapshot.books[0].title.contains("first"));
}
