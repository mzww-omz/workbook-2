use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tempfile::TempDir;
use workbook_domain::{
    Answer, GradeStatus, HotspotShape, Mode, Phase, Question, QuestionType, Rating,
};
use workbook_scheduler::FixedClock;
use workbook_service::{SAMPLE_WORKBOOK_V2_JSON, StudyView, WorkbookService};

struct Harness {
    dir: TempDir,
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
            dir,
            clock,
            service: Some(service),
        }
    }

    fn service(&self) -> &WorkbookService {
        self.service.as_ref().unwrap()
    }

    fn reopen(&mut self) {
        let clock = self.clock.clone();
        let dir = self.dir.path().to_path_buf();
        drop(self.service.take());
        self.service = Some(WorkbookService::open(&dir, clock).unwrap());
    }

    fn import(&self) -> String {
        self.service()
            .import_workbook_json(SAMPLE_WORKBOOK_V2_JSON)
            .unwrap()
    }

    fn start(&self, mode: Mode, book: &str) -> StudyView {
        self.service().start_session(mode, Some(book)).unwrap();
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

    fn evaluate(&self, view: &StudyView, answer: Answer) -> StudyView {
        self.service()
            .evaluate_answer(
                &view.session_id,
                Some(&answer),
                false,
                None,
                false,
                view.revision,
            )
            .unwrap()
    }

    fn next(&self, view: &StudyView) -> StudyView {
        self.service()
            .next(&view.session_id, view.revision)
            .unwrap()
    }

    fn attempt_count(&self) -> i64 {
        let connection =
            rusqlite::Connection::open(self.dir.path().join("workbook.sqlite")).unwrap();
        connection
            .query_row("SELECT COUNT(*) FROM attempts", [], |row| row.get(0))
            .unwrap()
    }

    fn attempt_assist(&self) -> (i64, Option<String>) {
        let connection =
            rusqlite::Connection::open(self.dir.path().join("workbook.sqlite")).unwrap();
        connection
            .query_row("SELECT rating, assist_json FROM attempts", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap()
    }
}

fn correct_answer(question: &Question) -> Answer {
    match question.kind {
        QuestionType::SingleChoice => {
            Answer::Text(question.single_choice_answer().unwrap().to_string())
        }
        QuestionType::TrueFalse => {
            Answer::Bool(question.answer.as_ref().unwrap().as_bool().unwrap())
        }
        QuestionType::MultipleChoice => Answer::Choices(question.multiple_choice_answers()),
        QuestionType::ShortText => {
            Answer::Text(question.short_text_answers().first().unwrap().clone())
        }
        QuestionType::Cloze => Answer::Blanks(
            question
                .cloze_blanks()
                .into_iter()
                .map(|(id, values)| (id, values.first().unwrap().clone()))
                .collect(),
        ),
        QuestionType::Flashcard => Answer::Bool(true),
        QuestionType::Numeric => {
            let spec = question.numeric_spec().unwrap();
            let value = spec.value.value().unwrap();
            let text = match spec.significant_figures {
                Some(sig) => {
                    let decimals = sig.saturating_sub(1) as usize;
                    format!("{:.*}{}", decimals, value, spec.unit.unwrap_or_default())
                }
                None => format!("{}{}", value, spec.unit.unwrap_or_default()),
            };
            Answer::Text(text)
        }
        QuestionType::Ordering => Answer::Ordering {
            order: question.ordering_spec().unwrap().orders[0].clone(),
        },
        QuestionType::Matching => Answer::Matching {
            matches: question
                .matching_spec()
                .unwrap()
                .pairs
                .into_iter()
                .map(|pair| (pair.left, pair.right))
                .collect(),
        },
        QuestionType::Hotspot => {
            let hotspot = question.hotspot.as_ref().unwrap();
            let accepted = question.hotspot_spec().unwrap().region_ids[0].clone();
            let region = hotspot
                .regions
                .iter()
                .find(|region| region.id == accepted)
                .unwrap();
            let (x, y) = match &region.shape {
                HotspotShape::Circle { cx, cy, .. } => (*cx, *cy),
                HotspotShape::Rect { x, y, w, h } => (x + w / 2.0, y + h / 2.0),
                HotspotShape::Polygon { points } => {
                    let count = points.len() as f64;
                    (
                        points.iter().map(|point| point[0]).sum::<f64>() / count,
                        points.iter().map(|point| point[1]).sum::<f64>() / count,
                    )
                }
            };
            Answer::Hotspot { x, y }
        }
        QuestionType::Labeling => Answer::Labels {
            labels: question.labeling_spec().unwrap().pairs,
        },
    }
}

#[test]
fn expanded_sample_answers_every_type_and_commits_fsrs() {
    let h = Harness::new();
    let book = h.import();
    let mut view = h.start(Mode::New, &book);
    let mut seen = Vec::new();
    for _ in 0..20 {
        view = h.begin(&view);
        if view.phase != Phase::Question {
            break;
        }
        let question = view.question.clone().unwrap();
        seen.push(question.kind);
        if matches!(
            question.kind,
            QuestionType::Hotspot | QuestionType::Labeling
        ) {
            assert!(!view.assets.is_empty(), "image asset must be served");
            let bytes = &view.assets[0].bytes;
            let is_png = bytes.starts_with(b"\x89PNG");
            let is_svg = bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml");
            assert!(is_png || is_svg, "unexpected asset bytes");
        }
        let evaluated = h.evaluate(&view, correct_answer(&question));
        let feedback = evaluated.feedback.as_ref().unwrap();
        assert!(feedback.committed, "{:?}", question.kind);
        assert_eq!(feedback.status, GradeStatus::Correct);
        assert_eq!(feedback.rating, Some(Rating::Good));
        if seen.len() == 6 {
            break;
        }
        let next = h.next(&evaluated);
        if matches!(next.phase, Phase::WorkbookResult | Phase::OverallResult) {
            break;
        }
        view = next;
    }
    assert_eq!(seen.len(), 6);
    for kind in [
        QuestionType::Numeric,
        QuestionType::Ordering,
        QuestionType::Matching,
        QuestionType::Hotspot,
        QuestionType::Labeling,
    ] {
        assert!(seen.contains(&kind), "missing {kind:?}");
    }
    assert_eq!(h.attempt_count(), 6);
    let connection = rusqlite::Connection::open(h.dir.path().join("workbook.sqlite")).unwrap();
    let states: i64 = connection
        .query_row("SELECT COUNT(*) FROM review_states", [], |row| row.get(0))
        .unwrap();
    assert_eq!(states, 6);
}

#[test]
fn near_miss_retries_then_commits_once_as_hard() {
    let mut h = Harness::new();
    let book = h.import();
    let view = h.start(Mode::New, &book);
    let view = h.begin(&view);
    assert_eq!(view.question.as_ref().unwrap().kind, QuestionType::Numeric);
    let near = h.evaluate(&view, Answer::Text("3A".to_string()));
    let feedback = near.feedback.as_ref().unwrap();
    assert_eq!(feedback.status, GradeStatus::NearMiss);
    assert!(!feedback.committed);
    assert_eq!(
        feedback.near_miss_reasons,
        vec![workbook_domain::NearMissReason::SignificantFigures]
    );
    assert_eq!(h.attempt_count(), 0, "near miss must not write an attempt");
    assert!(!feedback.near_miss_messages.is_empty());

    // Restart proves the near-miss state and draft survive.
    h.reopen();
    let resumed = h.service().active_view().unwrap().unwrap();
    assert_eq!(resumed.phase, Phase::Feedback);
    assert_eq!(
        resumed.feedback.as_ref().unwrap().status,
        GradeStatus::NearMiss
    );
    assert_eq!(resumed.draft, Some(Answer::Text("3A".to_string())));

    let retried = h
        .service()
        .retry_answer(&resumed.session_id, resumed.revision)
        .unwrap();
    assert_eq!(retried.phase, Phase::Question);
    assert_eq!(retried.draft, Some(Answer::Text("3A".to_string())));

    let committed = h.evaluate(&retried, Answer::Text("3.0A".to_string()));
    let feedback = committed.feedback.as_ref().unwrap();
    assert!(feedback.committed);
    assert_eq!(feedback.status, GradeStatus::Correct);
    assert_eq!(feedback.rating, Some(Rating::Hard));
    assert!(feedback.assisted);
    assert_eq!(feedback.retry_count, 1);
    assert_eq!(h.attempt_count(), 1);
    let (rating, assist) = h.attempt_assist();
    assert_eq!(rating, 2);
    let assist: serde_json::Value = serde_json::from_str(&assist.unwrap()).unwrap();
    assert_eq!(assist["history"].as_array().unwrap().len(), 1);
    assert_eq!(
        assist["history"][0]["result"]["reasons"][0],
        "significant_figures"
    );
}

#[test]
fn near_miss_cannot_be_committed_without_retrying() {
    let h = Harness::new();
    let book = h.import();
    let view = h.begin(&h.start(Mode::New, &book));
    let near = h.evaluate(&view, Answer::Text("3A".to_string()));
    let feedback = near.feedback.as_ref().unwrap();
    assert_eq!(feedback.status, GradeStatus::NearMiss);
    let error = h
        .service()
        .commit_rating(
            &near.session_id,
            &uuid::Uuid::new_v4().to_string(),
            Rating::Good,
            near.revision,
        )
        .unwrap_err();
    assert!(error.to_string().contains("もう一度"));
    assert_eq!(h.attempt_count(), 0);
}

#[test]
fn backup_and_restore_keep_assets_and_assist_history() {
    let h = Harness::new();
    let book = h.import();
    let view = h.begin(&h.start(Mode::New, &book));
    let near = h.evaluate(&view, Answer::Text("3A".to_string()));
    let retried = h
        .service()
        .retry_answer(&near.session_id, near.revision)
        .unwrap();
    let committed = h.evaluate(&retried, Answer::Text("3.0A".to_string()));
    assert!(committed.feedback.as_ref().unwrap().committed);

    let backup = h.dir.path().join("backup.sqlite");
    h.service().backup_to(&backup).unwrap();
    WorkbookService::validate_backup(&backup).unwrap();

    let target = tempfile::tempdir().unwrap();
    let clock = Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    ));
    let restored = WorkbookService::open(target.path(), clock).unwrap();
    restored.restore_from(&backup).unwrap();
    let (rating, assist) = {
        let connection = rusqlite::Connection::open(target.path().join("workbook.sqlite")).unwrap();
        connection
            .query_row("SELECT rating, assist_json FROM attempts", [], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .unwrap()
    };
    assert_eq!(rating, 2);
    assert!(assist.unwrap().contains("significant_figures"));
    let workbook = restored.workbook(&book).unwrap();
    assert_eq!(workbook.assets.len(), 2);
}

#[test]
fn matching_diagnostic_is_persisted_with_the_attempt() {
    let mut h = Harness::new();
    let book = h.import();
    let mut view = h.start(Mode::Practice, &book);
    loop {
        view = h.begin(&view);
        if view.question.as_ref().map(|question| question.kind) == Some(QuestionType::Matching) {
            break;
        }
        view = h.service().skip(&view.session_id, view.revision).unwrap();
    }
    let wrong = Answer::Matching {
        matches: BTreeMap::from([
            ("h".to_string(), "oxygen".to_string()),
            ("o".to_string(), "hydrogen".to_string()),
            ("na".to_string(), "sodium".to_string()),
            ("cl".to_string(), "chlorine".to_string()),
        ]),
    };
    let evaluated = h.evaluate(&view, wrong);
    let feedback = evaluated.feedback.as_ref().unwrap();
    assert!(feedback.committed);
    assert_eq!(feedback.status, GradeStatus::Incorrect);
    assert_eq!(feedback.rating, Some(Rating::Again));
    let diagnostic = feedback.diagnostic.as_ref().unwrap();
    assert_eq!((diagnostic.matched, diagnostic.total), (2, 4));

    let attempt_id = feedback.attempt_id.clone().unwrap();
    let corrected = h
        .service()
        .correct_attempt(
            &evaluated.session_id,
            &attempt_id,
            Some(true),
            None,
            evaluated.revision,
        )
        .unwrap();
    let corrected = corrected.feedback.as_ref().unwrap();
    assert_eq!(corrected.status, GradeStatus::Correct);
    assert!(corrected.diagnostic.is_none());

    h.reopen();
    let resumed = h.service().session_view(&evaluated.session_id).unwrap();
    let feedback = resumed.feedback.as_ref().unwrap();
    assert_eq!(feedback.status, GradeStatus::Correct);
    assert!(feedback.diagnostic.is_none());
    assert_eq!(feedback.correct, Some(true));
}

#[test]
fn ordering_matching_and_hotspot_drafts_survive_restart() {
    let mut h = Harness::new();
    let book = h.import();
    let mut view = h.start(Mode::Practice, &book);
    for target in [
        QuestionType::Ordering,
        QuestionType::Matching,
        QuestionType::Hotspot,
        QuestionType::Labeling,
    ] {
        view = h.begin(&view);
        while view.question.as_ref().map(|question| question.kind) != Some(target) {
            view = h.service().skip(&view.session_id, view.revision).unwrap();
        }
        let draft = match target {
            QuestionType::Ordering => Answer::Ordering {
                order: view
                    .question
                    .as_ref()
                    .unwrap()
                    .display_order()
                    .into_iter()
                    .rev()
                    .collect(),
            },
            QuestionType::Matching => Answer::Matching {
                matches: BTreeMap::from([
                    ("h".to_string(), "hydrogen".to_string()),
                    ("o".to_string(), "oxygen".to_string()),
                ]),
            },
            QuestionType::Hotspot => Answer::Hotspot { x: 0.65, y: 0.6 },
            QuestionType::Labeling => Answer::Labels {
                labels: BTreeMap::from([
                    ("nucleus".to_string(), "l-nucleus".to_string()),
                    ("wall".to_string(), "l-wall".to_string()),
                ]),
            },
            _ => unreachable!(),
        };
        let _saved = h
            .service()
            .save_draft(&view.session_id, Some(&draft), false, false, view.revision)
            .unwrap();
        h.reopen();
        let resumed = h.service().active_view().unwrap().unwrap();
        assert_eq!(resumed.draft, Some(draft), "{target:?}");
        // Move on from the restored state for the next target.
        let resumed = h
            .service()
            .skip(&resumed.session_id, resumed.revision)
            .unwrap();
        view = resumed;
    }
}

#[test]
fn deleting_a_workbook_removes_its_assets() {
    let h = Harness::new();
    let book = h.import();
    let connection = rusqlite::Connection::open(h.dir.path().join("workbook.sqlite")).unwrap();
    let before: i64 = connection
        .query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(before, 2);
    h.service().delete_workbook(&book).unwrap();
    let after: i64 = connection
        .query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after, 0);
}

#[test]
fn practice_answers_new_types_without_fsrs_state() {
    let h = Harness::new();
    let book = h.import();
    let mut view = h.start(Mode::Practice, &book);
    let mut seen = 0;
    for _ in 0..20 {
        view = h.begin(&view);
        if view.phase != Phase::Question {
            break;
        }
        let question = view.question.clone().unwrap();
        let evaluated = h.evaluate(&view, correct_answer(&question));
        assert!(evaluated.feedback.as_ref().unwrap().committed);
        seen += 1;
        let next = h.next(&evaluated);
        if next.phase != Phase::Question {
            break;
        }
        view = next;
    }
    assert_eq!(seen, 6);
    let connection = rusqlite::Connection::open(h.dir.path().join("workbook.sqlite")).unwrap();
    let states: i64 = connection
        .query_row("SELECT COUNT(*) FROM review_states", [], |row| row.get(0))
        .unwrap();
    assert_eq!(states, 0);
}
