use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tempfile::TempDir;
use workbook_domain::{Mode, Phase};
use workbook_scheduler::FixedClock;
use workbook_service::WorkbookService;

struct Harness {
    _dir: TempDir,
    service: WorkbookService,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FixedClock::new(
            Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
        ));
        let service = WorkbookService::open(dir.path(), clock.clone()).unwrap();
        let mut settings = service.settings().unwrap();
        settings.timezone = "UTC".to_string();
        settings.automatic_rating = true;
        settings.new_limit = 100;
        service.update_settings(settings).unwrap();
        Self { _dir: dir, service }
    }

    fn import(&self, raw: &str) -> String {
        self.service.import_workbook_json(raw).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        let connection =
            rusqlite::Connection::open(self._dir.path().join("workbook.sqlite")).unwrap();
        connection.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn learn_one(&self, book: &str) {
        self.service.start_session(Mode::New, Some(book)).unwrap();
        let mut view = self.service.active_view().unwrap().unwrap();
        if view.phase == Phase::Intro {
            view = self
                .service
                .begin_question(&view.session_id, view.revision)
                .unwrap();
        }
        let evaluated = self
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
        self.service
            .finish(&evaluated.session_id, evaluated.revision)
            .unwrap();
    }
}

fn workbook() -> serde_json::Value {
    serde_json::json!({
        "format": "workbook",
        "schemaVersion": 2,
        "id": "authoring",
        "revision": 1,
        "title": "編集テスト",
        "description": "説明",
        "language": "ja",
        "tags": ["tag"],
        "notes": [{
            "id": "n1",
            "title": "ノート",
            "knowledgeCard": "カード",
            "questions": [
                {"id":"q1","revision":1,"type":"true_false","prompt":"もとの問題","answer":true,"explanation":"解説","hint":"ヒント","topics":["数学/一次関数"]},
                {"id":"q2","revision":1,"type":"true_false","prompt":"二問目","answer":true,"explanation":"解説"}
            ]
        }]
    })
}

#[test]
fn editing_preserves_question_ids_fsrs_state_and_attempts() {
    let harness = Harness::new();
    let book = harness.import(&workbook().to_string());
    harness.learn_one(&book);
    assert_eq!(harness.count("SELECT COUNT(*) FROM review_states"), 1);
    let attempts = harness.count("SELECT COUNT(*) FROM attempts");
    let mut model = harness.service.workbook(&book).unwrap();
    model.title = "編集後の教材".to_string();
    let question = model.notes[0]
        .questions
        .iter_mut()
        .find(|question| question.id == "q1")
        .unwrap();
    question.prompt = Some(workbook_domain::Content::Text("更新した問題文".to_string()));
    let saved = harness.service.save_workbook(Some(&book), &model).unwrap();
    assert_eq!(saved.local_id, book);
    assert_eq!(saved.workbook.revision, 2);
    assert_eq!(harness.count("SELECT COUNT(*) FROM review_states"), 1);
    assert_eq!(harness.count("SELECT COUNT(*) FROM attempts"), attempts);
    let reloaded = harness.service.workbook(&book).unwrap();
    assert_eq!(reloaded.title, "編集後の教材");
    assert_eq!(
        reloaded.notes[0].questions[0].prompt_text(),
        "更新した問題文"
    );
    assert!(
        std::fs::read_dir(harness._dir.path().join("backups"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("pre-edit-"))
    );
}

#[test]
fn removing_a_question_deletes_its_fsrs_state_but_keeps_attempt_history() {
    let harness = Harness::new();
    let book = harness.import(&workbook().to_string());
    harness.learn_one(&book);
    let attempts = harness.count("SELECT COUNT(*) FROM attempts");
    let mut model = harness.service.workbook(&book).unwrap();
    model.notes[0]
        .questions
        .retain(|question| question.id != "q1");
    harness.service.save_workbook(Some(&book), &model).unwrap();
    assert_eq!(
        harness.count("SELECT COUNT(*) FROM review_states WHERE question_id='q1'"),
        0
    );
    assert_eq!(harness.count("SELECT COUNT(*) FROM attempts"), attempts);
    assert_eq!(
        harness.count("SELECT COUNT(*) FROM questions WHERE question_id='q1'"),
        0
    );
}

#[test]
fn creating_a_new_workbook_adds_it_to_the_library() {
    let harness = Harness::new();
    let mut model = workbook_domain::parse_workbook(&workbook().to_string()).unwrap();
    model.id = "new-book".to_string();
    model.title = "新しい教材".to_string();
    let saved = harness.service.save_workbook(None, &model).unwrap();
    assert_eq!(harness.service.snapshot().unwrap().books.len(), 1);
    assert_eq!(
        harness.service.workbook(&saved.local_id).unwrap().title,
        "新しい教材"
    );
}

#[test]
fn a_copy_has_independent_learning_state() {
    let harness = Harness::new();
    let book = harness.import(&workbook().to_string());
    harness.learn_one(&book);
    let copy = harness.service.copy_workbook(&book).unwrap();
    assert_ne!(copy, book);
    let snapshot = harness.service.snapshot().unwrap();
    let original = snapshot
        .books
        .iter()
        .find(|item| item.local_id == book)
        .unwrap();
    let copied = snapshot
        .books
        .iter()
        .find(|item| item.local_id == copy)
        .unwrap();
    assert_eq!(original.learned, 1);
    assert_eq!(copied.learned, 0);
    assert!(copied.title.ends_with("（コピー）"));
}

#[test]
fn export_embeds_assets_as_base64_and_restores_topic_paths() {
    let harness = Harness::new();
    let book = harness.import(&workbook().to_string());
    let exported = harness.service.export_workbook_json(&book).unwrap();
    let value: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(value["title"], "編集テスト");
    assert_eq!(
        value["notes"][0]["questions"][0]["topics"][0],
        "数学/一次関数"
    );
    // The exported JSON must be importable again with the same topic path.
    let reimported = harness.service.import_workbook_json(&exported).unwrap();
    assert_ne!(reimported, book);
    // A workbook that really has assets exports them as base64 data, not as a
    // machine-local file path.
    let with_assets = harness
        .service
        .import_workbook_json(workbook_service::SAMPLE_WORKBOOK_V2_JSON)
        .unwrap();
    let exported = harness.service.export_workbook_json(&with_assets).unwrap();
    let value: serde_json::Value = serde_json::from_str(&exported).unwrap();
    let assets = value["assets"].as_array().expect("assets array");
    assert!(!assets.is_empty());
    assert!(assets.iter().all(|asset| asset["data"].is_string()));
    assert!(assets.iter().all(|asset| asset.get("file").is_none()));
    let topics = harness.service.topics().unwrap();
    assert!(
        topics
            .iter()
            .any(|topic| topic.path == "数学 / 一次関数" && topic.question_count == 2)
    );
}

#[test]
fn workbook_reads_and_copies_preserve_database_topic_assignments() {
    let harness = Harness::new();
    let book = harness.import(&workbook().to_string());
    let linear = harness
        .service
        .topics()
        .unwrap()
        .into_iter()
        .find(|topic| topic.path == "数学 / 一次関数")
        .unwrap();
    harness
        .service
        .set_question_topics(&book, "q2", std::slice::from_ref(&linear.id))
        .unwrap();
    let model = harness.service.workbook(&book).unwrap();
    let q2 = model.notes[0]
        .questions
        .iter()
        .find(|question| question.id == "q2")
        .unwrap();
    assert_eq!(q2.topics, ["数学/一次関数"]);
    let copy = harness.service.copy_workbook(&book).unwrap();
    let copied = harness.service.workbook(&copy).unwrap();
    let copied_q2 = copied.notes[0]
        .questions
        .iter()
        .find(|question| question.id == "q2")
        .unwrap();
    assert_eq!(copied_q2.topics, ["数学/一次関数"]);
}

#[test]
fn lint_returns_concrete_paths_and_errors() {
    let harness = Harness::new();
    let issues = harness.service.lint_workbook_json(&workbook().to_string());
    assert!(issues.iter().all(|issue| !issue.is_error()));
    let broken = r#"{"format":"workbook","schemaVersion":2,"id":"x","revision":1,"title":"","description":"","language":"ja","tags":[],"notes":[]}"#;
    let issues = harness.service.lint_workbook_json(broken);
    assert!(issues.iter().any(|issue| issue.is_error()));
    assert!(issues.iter().any(|issue| !issue.path.is_empty()));
}

#[test]
fn manual_validation_rejects_broken_answers_with_a_path() {
    let harness = Harness::new();
    let mut model = workbook_domain::parse_workbook(&workbook().to_string()).unwrap();
    model.notes[0].questions[0].answer = Some(serde_json::json!({"optionId":"missing"}));
    let error = harness.service.validate_workbook_model(&model).unwrap_err();
    assert!(error.to_string().contains("prompt") || error.to_string().contains("answer"));
}

#[test]
fn editing_is_refused_while_that_workbook_has_an_active_session() {
    let harness = Harness::new();
    let book = harness.import(&workbook().to_string());
    harness
        .service
        .start_session(Mode::New, Some(&book))
        .unwrap();
    let mut model = harness.service.workbook(&book).unwrap();
    model.title = "学習中に変更".to_string();
    let error = harness
        .service
        .save_workbook(Some(&book), &model)
        .unwrap_err();
    assert!(error.to_string().contains("学習中"));
}

#[test]
fn editing_a_workbook_keeps_its_last_used_and_import_time() {
    let harness = Harness::new();
    let book = harness.import(&workbook().to_string());
    harness.learn_one(&book);
    let before = harness.service.snapshot().unwrap();
    let before_last = before
        .books
        .iter()
        .find(|item| item.local_id == book)
        .and_then(|item| item.last_used_at);
    let mut model = harness.service.workbook(&book).unwrap();
    model.title = "更新".to_string();
    harness.service.save_workbook(Some(&book), &model).unwrap();
    let after = harness.service.snapshot().unwrap();
    let after_last = after
        .books
        .iter()
        .find(|item| item.local_id == book)
        .and_then(|item| item.last_used_at);
    assert_eq!(before_last, after_last);
}
