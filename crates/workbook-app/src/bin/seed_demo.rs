//! Development-only demo data seeder used for screenshots and manual QA.
//!
//! Usage:
//!   cargo run -p workbook-app --bin seed_demo -- /tmp/workbook2-demo today
//!   cargo run -p workbook-app --bin seed_demo -- /tmp/workbook2-demo feedback
//!
//! Profiles: today, library, history, settings, study, feedback, result,
//! v2-numeric, v2-ordering, v2-matching, v2-hotspot, v2-labeling, v2-near-miss,
//! v2-math.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Duration, Utc};
use workbook_domain::{Answer, Mode, Phase, Question, QuestionType, Rating};
use workbook_scheduler::FixedClock;
use workbook_service::{StudyView, WorkbookService};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let directory = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "/tmp/workbook2-demo".to_string()),
    );
    let profile = args.next().unwrap_or_else(|| "today".to_string());
    if directory.exists() {
        std::fs::remove_dir_all(&directory)?;
    }
    let start = if profile == "upcoming" {
        Utc::now()
    } else {
        Utc::now() - Duration::days(60)
    };
    let clock = Arc::new(FixedClock::new(start));
    let service = WorkbookService::open(&directory, clock.clone())?;
    if profile == "upcoming" {
        seed_upcoming(&service)?;
        println!("seeded profile={} at {}", profile, directory.display());
        return Ok(());
    }
    if profile == "v2-math" {
        seed_math(&service)?;
        println!("seeded profile={} at {}", profile, directory.display());
        return Ok(());
    }
    if let Some(target) = profile.strip_prefix("v2-") {
        seed_expanded(&service, target)?;
        println!("seeded profile={} at {}", profile, directory.display());
        return Ok(());
    }
    let book_id = service.import_workbook_json(workbook_service::SAMPLE_WORKBOOK_JSON)?;

    finish_new_session(&service, &book_id)?;
    clock.advance(Duration::days(45));

    match profile.as_str() {
        "study" => {
            let view = start_review(&service)?;
            wait_for_question(&service, view)?;
        }
        "feedback" => {
            let view = start_review(&service)?;
            let view = wait_for_question(&service, view)?;
            let view = prepare_answer(&service, &view)?;
            let (answer, self_correct) = correct_answer(view.question.as_ref().unwrap());
            let pending = service.evaluate_answer(
                &view.session_id,
                answer.as_ref(),
                false,
                self_correct,
                false,
                view.revision,
            )?;
            assert_eq!(pending.phase, Phase::Feedback);
        }
        "result" => {
            let mut view = start_review(&service)?;
            loop {
                match view.phase {
                    Phase::WorkbookResult | Phase::OverallResult => break,
                    Phase::Intro => {
                        view = service.begin_question(&view.session_id, view.revision)?;
                    }
                    Phase::Question => {
                        let prepared = prepare_answer(&service, &view)?;
                        let (answer, self_correct) =
                            correct_answer(prepared.question.as_ref().unwrap());
                        let pending = service.evaluate_answer(
                            &prepared.session_id,
                            answer.as_ref(),
                            false,
                            self_correct,
                            false,
                            prepared.revision,
                        )?;
                        view = ensure_committed(&service, pending, Rating::Good)?;
                    }
                    Phase::Feedback => {
                        view = service.next(&view.session_id, view.revision)?;
                    }
                }
            }
        }
        "today" | "library" | "history" | "settings" => {}
        other => anyhow::bail!("unknown profile: {other}"),
    }

    println!("seeded profile={} at {}", profile, directory.display());
    Ok(())
}

/// Math, inline math, image fit, and compact option rendering for manual QA
/// and the `docs/screenshots/math.png` capture.
fn seed_math(service: &WorkbookService) -> anyhow::Result<()> {
    let book = service.import_workbook_json(MATH_DEMO_JSON)?;
    service.start_session(Mode::Practice, Some(&book))?;
    let view = service.active_view()?.expect("active practice session");
    wait_for_question(service, view)?;
    Ok(())
}

const MATH_DEMO_JSON: &str = r#"{
  "format":"workbook","schemaVersion":2,"id":"workbook-math-demo","revision":1,
  "title":"数式と画像の表示確認","description":"数式の組版、インライン数式、contain と cover。",
  "language":"ja","tags":["確認"],
  "assets":[
    {"id":"wide-diagram","alt":"横長の見本画像","mediaType":"image/svg+xml",
     "data":"PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI4MDAiIGhlaWdodD0iMzAwIiB2aWV3Qm94PSIwIDAgODAwIDMwMCI+PHJlY3Qgd2lkdGg9IjgwMCIgaGVpZ2h0PSIzMDAiIGZpbGw9IiNlOGVkZjUiLz48cmVjdCB4PSIwIiB5PSIwIiB3aWR0aD0iNDAwIiBoZWlnaHQ9IjMwMCIgZmlsbD0iIzlkYjRkNiIvPjxjaXJjbGUgY3g9IjYwMCIgY3k9IjE1MCIgcj0iOTAiIGZpbGw9IiNjOTZiNWEiLz48cmVjdCB4PSI4MCIgeT0iMTEwIiB3aWR0aD0iMjQwIiBoZWlnaHQ9IjgwIiBmaWxsPSIjNGY3YTUyIi8+PC9zdmc+"}
  ],
  "notes":[{"id":"n-math","title":"数式の表示","knowledgeCard":[
    {"type":"math","text":"\\frac{1}{2}"},
    {"type":"math","text":"x^2 + 2x + 1"},
    {"type":"math","text":"\\sqrt{3}"}
  ],"questions":[
    {"id":"q-math","revision":1,"type":"single_choice",
     "prompt":[
       {"type":"math","text":"\\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}"},
       {"type":"math","text":"a_1 + a_2 + \\sum_{i=1}^{n} i"},
       {"type":"math","text":"\\sin\\theta + |x| + \\text{速さ}"},
       {"type":"markdown","text":"インライン数式: 速さは $v=\\frac{d}{t}$ です。"},
       {"type":"image","asset":"wide-diagram","alt":"横長画像（contain）","fit":"contain"},
       {"type":"image","asset":"wide-diagram","alt":"横長画像（cover）","fit":"cover"}
     ],
     "options":[
       {"id":"a","content":[{"type":"math","text":"\\frac{1}{2}"}]},
       {"id":"b","content":[{"type":"math","text":"x^2"}]},
       {"id":"c","content":[{"type":"math","text":"\\sqrt{3}"}]}
     ],
     "answer":{"optionId":"b"},
     "explanation":[
       {"type":"math","text":"\\int_0^1 x\\,dx = \\frac{1}{2}"}
     ]}
  ]}]
}"#;

fn seed_expanded(service: &WorkbookService, target: &str) -> anyhow::Result<()> {
    let book = service.import_workbook_json(workbook_service::SAMPLE_WORKBOOK_V2_JSON)?;
    service.start_session(Mode::Practice, Some(&book))?;
    let mut view = service.active_view()?.expect("active practice session");
    let wanted = match target {
        "numeric" | "near-miss" => QuestionType::Numeric,
        "ordering" => QuestionType::Ordering,
        "matching" => QuestionType::Matching,
        "hotspot" => QuestionType::Hotspot,
        "labeling" => QuestionType::Labeling,
        other => anyhow::bail!("unknown expanded profile: {other}"),
    };
    for _ in 0..100 {
        match view.phase {
            Phase::Intro => {
                view = service.begin_question(&view.session_id, view.revision)?;
            }
            Phase::Question => {
                let kind = view.question.as_ref().map(|question| question.kind);
                if kind == Some(wanted) {
                    if target == "near-miss" {
                        let pending = service.evaluate_answer(
                            &view.session_id,
                            Some(&Answer::Text("3A".to_string())),
                            false,
                            None,
                            false,
                            view.revision,
                        )?;
                        assert_eq!(pending.phase, Phase::Feedback);
                    }
                    return Ok(());
                }
                view = service.skip(&view.session_id, view.revision)?;
            }
            _ => anyhow::bail!("expanded target not found"),
        }
    }
    anyhow::bail!("expanded target not found")
}

fn seed_upcoming(service: &WorkbookService) -> anyhow::Result<()> {
    let raw = r#"{
      "format":"workbook","schemaVersion":1,"id":"upcoming-demo","revision":1,
      "title":"短期再学習デモ","description":"1分後の再出題を確認するための教材です。","language":"ja","tags":["デモ"],
      "notes":[{"id":"n","title":"短いNote","knowledgeCard":"カード","questions":[
        {"id":"q","revision":1,"type":"true_false","prompt":"この問題は1分後に再出題されます。","answer":true,"explanation":"Againを選ぶとLearning 1分ステップに入ります。"}
      ]}]
    }"#;
    let book = service.import_workbook_json(raw)?;
    service.start_session(Mode::New, Some(&book))?;
    let view = service.active_view()?.expect("active");
    let view = service.begin_question(&view.session_id, view.revision)?;
    let pending = service.evaluate_answer(
        &view.session_id,
        Some(&Answer::Bool(false)),
        false,
        None,
        false,
        view.revision,
    )?;
    let committed = ensure_committed(service, pending, Rating::Again)?;
    service.finish(&committed.session_id, committed.revision)?;
    Ok(())
}

fn finish_new_session(service: &WorkbookService, book_id: &str) -> anyhow::Result<()> {
    service.start_session(Mode::New, Some(book_id))?;
    let mut view = service.active_view()?.expect("active session");
    for _ in 0..2000 {
        match view.phase {
            Phase::Intro => {
                view = service.begin_question(&view.session_id, view.revision)?;
            }
            Phase::Question => {
                let prepared = prepare_answer(service, &view)?;
                let (answer, self_correct) = correct_answer(prepared.question.as_ref().unwrap());
                let pending = service.evaluate_answer(
                    &prepared.session_id,
                    answer.as_ref(),
                    false,
                    self_correct,
                    false,
                    prepared.revision,
                )?;
                view = ensure_committed(service, pending, Rating::Good)?;
            }
            Phase::Feedback => {
                view = service.next(&view.session_id, view.revision)?;
            }
            Phase::WorkbookResult => {
                view = service.continue_session(&view.session_id, view.revision)?;
            }
            Phase::OverallResult => return Ok(()),
        }
    }
    anyhow::bail!("new session did not finish")
}

fn start_review(service: &WorkbookService) -> anyhow::Result<StudyView> {
    service.start_session(Mode::Review, None)?;
    Ok(service.active_view()?.expect("active review"))
}

fn wait_for_question(service: &WorkbookService, mut view: StudyView) -> anyhow::Result<StudyView> {
    while view.phase == Phase::Intro {
        view = service.begin_question(&view.session_id, view.revision)?;
    }
    if view.phase != Phase::Question {
        anyhow::bail!("expected question, got {:?}", view.phase);
    }
    Ok(view)
}

fn ensure_committed(
    service: &WorkbookService,
    view: StudyView,
    rating: Rating,
) -> anyhow::Result<StudyView> {
    if view
        .feedback
        .as_ref()
        .is_some_and(|feedback| feedback.committed)
    {
        Ok(view)
    } else {
        Ok(service.commit_rating(
            &view.session_id,
            &uuid::Uuid::new_v4().to_string(),
            rating,
            view.revision,
        )?)
    }
}

fn prepare_answer(service: &WorkbookService, view: &StudyView) -> anyhow::Result<StudyView> {
    if view.revealed {
        return Ok(view.clone());
    }
    if view
        .question
        .as_ref()
        .is_some_and(|question| question.kind == QuestionType::Flashcard)
    {
        return Ok(service.save_draft(
            &view.session_id,
            None,
            view.hint_visible,
            true,
            view.revision,
        )?);
    }
    Ok(view.clone())
}

fn correct_answer(question: &Question) -> (Option<Answer>, Option<bool>) {
    match question.kind {
        QuestionType::SingleChoice => (
            question
                .single_choice_answer()
                .map(|value| Answer::Text(value.to_string())),
            None,
        ),
        QuestionType::TrueFalse => (
            question
                .answer
                .as_ref()
                .and_then(|value| value.as_bool())
                .map(Answer::Bool),
            None,
        ),
        QuestionType::MultipleChoice => (
            Some(Answer::Choices(question.multiple_choice_answers())),
            None,
        ),
        QuestionType::ShortText => (
            question
                .short_text_answers()
                .first()
                .cloned()
                .map(Answer::Text),
            None,
        ),
        QuestionType::Cloze => {
            let blanks: BTreeMap<String, String> = question
                .cloze_blanks()
                .into_iter()
                .filter_map(|(id, values)| values.first().cloned().map(|value| (id, value)))
                .collect();
            (Some(Answer::Blanks(blanks)), None)
        }
        QuestionType::Flashcard => (Some(Answer::Bool(true)), Some(true)),
        QuestionType::Numeric => {
            let spec = question.numeric_spec();
            let answer = spec.and_then(|spec| {
                let value = spec.value.value().ok()?;
                let text = match spec.unit {
                    Some(unit) => format!("{value}{unit}"),
                    None => format!("{value}"),
                };
                Some(Answer::Text(text))
            });
            (answer, None)
        }
        QuestionType::Ordering => {
            let order = question
                .ordering_spec()
                .and_then(|spec| spec.orders.first().cloned());
            (order.map(|order| Answer::Ordering { order }), None)
        }
        QuestionType::Matching => {
            let matches = question.matching_spec().map(|spec| {
                spec.pairs
                    .into_iter()
                    .map(|pair| (pair.left, pair.right))
                    .collect()
            });
            (matches.map(|matches| Answer::Matching { matches }), None)
        }
        QuestionType::Hotspot => {
            let point = question
                .hotspot
                .as_ref()
                .and_then(|hotspot| hotspot.regions.first())
                .map(|region| match &region.shape {
                    workbook_domain::HotspotShape::Circle { cx, cy, .. } => (*cx, *cy),
                    workbook_domain::HotspotShape::Rect { x, y, w, h } => {
                        (x + w / 2.0, y + h / 2.0)
                    }
                    workbook_domain::HotspotShape::Polygon { points } => {
                        let count = points.len().max(1) as f64;
                        (
                            points.iter().map(|point| point[0]).sum::<f64>() / count,
                            points.iter().map(|point| point[1]).sum::<f64>() / count,
                        )
                    }
                });
            (point.map(|(x, y)| Answer::Hotspot { x, y }), None)
        }
        QuestionType::Labeling => {
            let labels = question.labeling_spec().map(|spec| spec.pairs);
            (labels.map(|labels| Answer::Labels { labels }), None)
        }
    }
}
