use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tempfile::TempDir;
use workbook_domain::{Content, Mode, Phase, QuestionType};
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

    fn save(
        &self,
        book: &str,
        model: &workbook_domain::Workbook,
    ) -> workbook_service::SavedWorkbook {
        self.service.save_workbook(Some(book), model, true).unwrap()
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

fn two_note_workbook() -> serde_json::Value {
    serde_json::json!({
        "format": "workbook",
        "schemaVersion": 2,
        "id": "move-note",
        "revision": 1,
        "title": "Note移動",
        "description": "",
        "language": "ja",
        "tags": [],
        "notes": [
            {
                "id": "n1",
                "title": "移動元",
                "knowledgeCard": "カード",
                "questions": [
                    {"id":"q1","revision":1,"type":"true_false","prompt":"移動する問題","answer":true,"explanation":"解説"}
                ]
            },
            {
                "id": "n2",
                "title": "移動先",
                "knowledgeCard": "カード",
                "questions": [
                    {"id":"q2","revision":1,"type":"true_false","prompt":"残る問題","answer":true,"explanation":"解説"}
                ]
            }
        ]
    })
}

fn flashcard_workbook() -> serde_json::Value {
    serde_json::json!({
        "format": "workbook",
        "schemaVersion": 2,
        "id": "flashcards",
        "revision": 1,
        "title": "単語",
        "description": "",
        "language": "ja",
        "tags": [],
        "notes": [{
            "id": "n1",
            "title": "カード",
            "knowledgeCard": "カード",
            "questions": [
                {"id":"f1","revision":1,"type":"flashcard","front":"apple","back":"りんご","explanation":"果物"}
            ]
        }]
    })
}

#[test]
fn editing_text_and_topics_preserves_question_ids_fsrs_state_and_attempts() {
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
    question.explanation = Content::Text("補足した解説".to_string());
    question.hint = Some(Content::Text("改善したヒント".to_string()));
    question.topics = vec!["数学/二次関数".to_string()];
    let saved = harness.save(&book, &model);
    assert_eq!(saved.local_id, book);
    assert_eq!(saved.workbook.revision, 2);
    assert_eq!(saved.workbook.notes[0].questions[0].revision, 1);
    assert!(saved.reset_questions.is_empty());
    assert_eq!(harness.count("SELECT COUNT(*) FROM review_states"), 1);
    assert_eq!(harness.count("SELECT COUNT(*) FROM attempts"), attempts);
    let reloaded = harness.service.workbook(&book).unwrap();
    assert_eq!(reloaded.title, "編集後の教材");
    assert_eq!(
        reloaded.notes[0].questions[0].explanation,
        Content::Text("補足した解説".to_string())
    );
    assert_eq!(reloaded.notes[0].questions[0].topics, ["数学/二次関数"]);
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
fn moving_a_learned_question_to_another_note_keeps_its_fsrs_state_and_attempts() {
    let harness = Harness::new();
    let book = harness.import(&two_note_workbook().to_string());
    harness.learn_one(&book);
    assert_eq!(
        harness.count("SELECT COUNT(*) FROM review_states WHERE question_id='q1'"),
        1
    );
    let attempts = harness.count("SELECT COUNT(*) FROM attempts");
    let mut model = harness.service.workbook(&book).unwrap();
    // q1 moves into n2. n1 is then removed entirely.
    let question = model.notes[0].questions.remove(0);
    assert_eq!(question.id, "q1");
    model.notes.remove(0);
    model.notes[0].questions.push(question);
    let saved = harness.save(&book, &model);
    assert!(saved.reset_questions.is_empty());
    assert_eq!(
        harness.count("SELECT COUNT(*) FROM review_states WHERE question_id='q1'"),
        1
    );
    assert_eq!(harness.count("SELECT COUNT(*) FROM attempts"), attempts);
    assert_eq!(
        harness.count("SELECT COUNT(*) FROM notes WHERE note_id='n1'"),
        0
    );
    let note_id: String = {
        let connection =
            rusqlite::Connection::open(harness._dir.path().join("workbook.sqlite")).unwrap();
        connection
            .query_row(
                "SELECT note_id FROM questions WHERE question_id='q1'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(note_id, "n2");
    let reloaded = harness.service.workbook(&book).unwrap();
    assert_eq!(reloaded.notes.len(), 1);
    assert_eq!(reloaded.notes[0].id, "n2");
    assert!(
        reloaded.notes[0]
            .questions
            .iter()
            .any(|question| question.id == "q1")
    );
}

#[test]
fn moving_a_question_does_not_hide_a_semantic_edit() {
    let harness = Harness::new();
    let book = harness.import(&two_note_workbook().to_string());
    harness.learn_one(&book);
    let mut model = harness.service.workbook(&book).unwrap();
    let mut question = model.notes[0].questions.remove(0);
    question.answer = Some(serde_json::json!(false));
    model.notes.remove(0);
    model.notes[0].questions.push(question);
    let plan = harness
        .service
        .preview_workbook_save(&book, &model)
        .unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].question_id, "q1");
    let saved = harness.save(&book, &model);
    assert_eq!(saved.reset_questions.len(), 1);
    assert_eq!(
        harness.count("SELECT COUNT(*) FROM review_states WHERE question_id='q1'"),
        0
    );
    assert_eq!(
        harness.count("SELECT revision FROM questions WHERE question_id='q1'"),
        2
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
    harness.save(&book, &model);
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
fn changing_the_answer_requires_confirmation_and_resets_fsrs_state() {
    let harness = Harness::new();
    let book = harness.import(&workbook().to_string());
    harness.learn_one(&book);
    let attempts = harness.count("SELECT COUNT(*) FROM attempts");
    assert_eq!(
        harness.count("SELECT question_revision FROM attempts LIMIT 1"),
        1
    );
    let mut model = harness.service.workbook(&book).unwrap();
    let question = model.notes[0]
        .questions
        .iter_mut()
        .find(|question| question.id == "q1")
        .unwrap();
    question.answer = Some(serde_json::json!(false));
    let error = harness
        .service
        .save_workbook(Some(&book), &model, false)
        .unwrap_err();
    assert!(matches!(
        error,
        workbook_service::ServiceError::ResetRequired(_)
    ));
    // Nothing changed before confirmation.
    assert_eq!(
        harness.count("SELECT COUNT(*) FROM review_states WHERE question_id='q1'"),
        1
    );
    let plan = harness
        .service
        .preview_workbook_save(&book, &model)
        .unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].question_id, "q1");
    assert_eq!(plan[0].previous_revision, 1);
    assert_eq!(plan[0].next_revision, 2);

    let saved = harness.save(&book, &model);
    assert_eq!(saved.reset_questions.len(), 1);
    assert_eq!(
        harness.count("SELECT COUNT(*) FROM review_states WHERE question_id='q1'"),
        0
    );
    assert_eq!(harness.count("SELECT COUNT(*) FROM attempts"), attempts);
    assert_eq!(
        harness.count("SELECT question_revision FROM attempts LIMIT 1"),
        1
    );
    assert_eq!(
        harness.count("SELECT revision FROM questions WHERE question_id='q1'"),
        2
    );
    let reloaded = harness.service.workbook(&book).unwrap();
    let question = reloaded.notes[0]
        .questions
        .iter()
        .find(|question| question.id == "q1")
        .unwrap();
    assert_eq!(question.revision, 2);
    assert_eq!(question.answer, Some(serde_json::json!(false)));
}

#[test]
fn changing_question_format_or_flashcard_target_resets_old_state() {
    let harness = Harness::new();
    let book = harness.import(&workbook().to_string());
    harness.learn_one(&book);
    let mut model = harness.service.workbook(&book).unwrap();
    let question = model.notes[0]
        .questions
        .iter_mut()
        .find(|question| question.id == "q1")
        .unwrap();
    question.kind = QuestionType::Flashcard;
    question.prompt = None;
    question.answer = None;
    question.front = Some(Content::Text("表".to_string()));
    question.back = Some(Content::Text("裏".to_string()));
    let saved = harness.save(&book, &model);
    assert_eq!(saved.reset_questions.len(), 1);
    assert_eq!(
        harness.count("SELECT COUNT(*) FROM review_states WHERE question_id='q1'"),
        0
    );

    let flash_harness = Harness::new();
    let flash_book = flash_harness.import(&flashcard_workbook().to_string());
    flash_harness.learn_flashcard(&flash_book);
    assert_eq!(
        flash_harness.count("SELECT COUNT(*) FROM review_states WHERE question_id='f1'"),
        1
    );
    let mut model = flash_harness.service.workbook(&flash_book).unwrap();
    model.notes[0].questions[0].back = Some(Content::Text("果物（英語）".to_string()));
    let saved = flash_harness.save(&flash_book, &model);
    assert_eq!(saved.reset_questions.len(), 1);
    assert_eq!(
        flash_harness.count("SELECT COUNT(*) FROM review_states WHERE question_id='f1'"),
        0
    );
}

impl Harness {
    fn learn_flashcard(&self, book: &str) {
        self.service.start_session(Mode::New, Some(book)).unwrap();
        let mut view = self.service.active_view().unwrap().unwrap();
        if view.phase == Phase::Intro {
            view = self
                .service
                .begin_question(&view.session_id, view.revision)
                .unwrap();
        }
        let revealed = self
            .service
            .save_draft(&view.session_id, None, false, true, view.revision)
            .unwrap();
        let evaluated = self
            .service
            .evaluate_answer(
                &revealed.session_id,
                Some(&workbook_domain::Answer::Bool(true)),
                false,
                Some(true),
                false,
                revealed.revision,
            )
            .unwrap();
        let committed = self
            .service
            .commit_rating(
                &evaluated.session_id,
                "flashcard-attempt-1",
                workbook_domain::Rating::Good,
                evaluated.revision,
            )
            .unwrap();
        self.service
            .finish(&committed.session_id, committed.revision)
            .unwrap();
    }
}

#[test]
fn a_failed_edit_rolls_back_notes_questions_and_fsrs_state() {
    let harness = Harness::new();
    let book = harness.import(&workbook().to_string());
    harness.learn_one(&book);
    let attempts = harness.count("SELECT COUNT(*) FROM attempts");
    {
        let connection =
            rusqlite::Connection::open(harness._dir.path().join("workbook.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_topic_assign BEFORE INSERT ON question_topics
                 BEGIN SELECT RAISE(ABORT,'injected failure'); END;",
            )
            .unwrap();
    }
    let mut model = harness.service.workbook(&book).unwrap();
    model.title = "失敗する編集".to_string();
    model.notes[0].questions[0].prompt = Some(Content::Text("変更".to_string()));
    let error = harness
        .service
        .save_workbook(Some(&book), &model, true)
        .unwrap_err();
    assert!(error.to_string().contains("SQLite") || error.to_string().contains("injected"));
    let reloaded = harness.service.workbook(&book).unwrap();
    assert_eq!(reloaded.title, "編集テスト");
    assert_eq!(
        harness.count("SELECT COUNT(*) FROM review_states WHERE question_id='q1'"),
        1
    );
    assert_eq!(harness.count("SELECT COUNT(*) FROM attempts"), attempts);
    assert_eq!(
        harness.count("SELECT revision FROM questions WHERE question_id='q1'"),
        1
    );
}

#[test]
fn creating_a_new_workbook_adds_it_to_the_library() {
    let harness = Harness::new();
    let mut model = workbook_domain::parse_workbook(&workbook().to_string()).unwrap();
    model.id = "new-book".to_string();
    model.title = "新しい教材".to_string();
    let saved = harness.service.save_workbook(None, &model, false).unwrap();
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
    assert!(value["topicCatalog"].as_array().is_some_and(|catalog| {
        catalog
            .iter()
            .any(|entry| entry["name"] == "一次関数" && entry["id"].is_string())
    }));
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
        .save_workbook(Some(&book), &model, false)
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
    harness.save(&book, &model);
    let after = harness.service.snapshot().unwrap();
    let after_last = after
        .books
        .iter()
        .find(|item| item.local_id == book)
        .and_then(|item| item.last_used_at);
    assert_eq!(before_last, after_last);
}
