//! Question revision policy.
//!
//! FSRS state belongs to a study target, not merely to a question ID. Editing
//! an explanation or moving a question to another topic keeps the same target.
//! Changing the prompt, the answer, the options, or the question format creates
//! a different thing to remember, so the old state must not be inherited.
//!
//! The classifier is deliberately conservative. A human can judge a typo fix
//! as harmless, but this code cannot reliably tell a typo from a changed
//! question. It only keeps state for fields that are provably not part of the
//! study target (explanation, hint, topic assignment, display-only metadata).
//! Everything else is treated as new study content.

use crate::workbook::Question;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestionChange {
    /// No study-relevant field changed.
    Unchanged,
    /// Explanation, hint, topic, or revision metadata changed only. FSRS state
    /// is preserved and the previous revision stays current.
    DisplayOnly,
    /// The question now asks something different. The old FSRS state is reset
    /// and the revision is incremented; attempts remain as history.
    StudyContent,
}

/// Fields that define what the learner is asked to remember.
#[derive(Debug, PartialEq)]
struct StudySignature<'a> {
    kind: crate::workbook::QuestionType,
    prompt: &'a Option<crate::content::Content>,
    front: &'a Option<crate::content::Content>,
    back: &'a Option<crate::content::Content>,
    options: &'a Option<Vec<crate::workbook::ChoiceOption>>,
    answer: &'a Option<serde_json::Value>,
    initial_order: &'a Option<Vec<String>>,
    matching: &'a Option<crate::question_format::MatchingSpec>,
    hotspot: &'a Option<crate::question_format::HotspotSpec>,
    labeling: &'a Option<crate::question_format::LabelingSpec>,
}

fn signature(question: &Question) -> StudySignature<'_> {
    StudySignature {
        kind: question.kind,
        prompt: &question.prompt,
        front: &question.front,
        back: &question.back,
        options: &question.options,
        answer: &question.answer,
        initial_order: &question.initial_order,
        matching: &question.matching,
        hotspot: &question.hotspot,
        labeling: &question.labeling,
    }
}

pub fn classify_question_change(previous: &Question, next: &Question) -> QuestionChange {
    let study_changed =
        signature(previous) != signature(next) || previous.revision != next.revision;
    if study_changed {
        return QuestionChange::StudyContent;
    }
    if previous.explanation != next.explanation
        || previous.hint != next.hint
        || previous.topics != next.topics
    {
        return QuestionChange::DisplayOnly;
    }
    QuestionChange::Unchanged
}

/// Normalizes the revision before saving. A semantic change always advances the
/// stored revision, even when the editor left the old number in place. A
/// display-only change keeps the stored revision so the identity of what was
/// studied stays stable.
pub fn resolved_revision(previous: &Question, next: &Question, change: QuestionChange) -> i64 {
    match change {
        QuestionChange::StudyContent => next.revision.max(previous.revision.saturating_add(1)),
        QuestionChange::Unchanged | QuestionChange::DisplayOnly => previous.revision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use crate::workbook::QuestionType;

    fn question() -> Question {
        serde_json::from_value(serde_json::json!({
            "id": "q1",
            "revision": 1,
            "type": "single_choice",
            "prompt": "1 + 1 は？",
            "explanation": "2 です",
            "hint": "数える",
            "topics": ["数学/計算"],
            "options": [
                {"id": "a", "content": "1"},
                {"id": "b", "content": "2"}
            ],
            "answer": {"optionId": "b"}
        }))
        .unwrap()
    }

    #[test]
    fn explanation_hint_and_topic_changes_keep_the_study_target() {
        let previous = question();
        let mut next = previous.clone();
        next.explanation = Content::Text("補足しました".to_string());
        assert_eq!(
            classify_question_change(&previous, &next),
            QuestionChange::DisplayOnly
        );
        next = previous.clone();
        next.hint = Some(Content::Text("別のヒント".to_string()));
        assert_eq!(
            classify_question_change(&previous, &next),
            QuestionChange::DisplayOnly
        );
        next = previous.clone();
        next.topics = vec!["数学/別単元".to_string()];
        assert_eq!(
            classify_question_change(&previous, &next),
            QuestionChange::DisplayOnly
        );
        assert_eq!(
            resolved_revision(&previous, &next, QuestionChange::DisplayOnly),
            1
        );
    }

    #[test]
    fn answer_prompt_format_option_and_flashcard_changes_are_new_content() {
        let previous = question();
        let mut next = previous.clone();
        next.answer = Some(serde_json::json!({"optionId": "a"}));
        assert_eq!(
            classify_question_change(&previous, &next),
            QuestionChange::StudyContent
        );

        let mut next = previous.clone();
        next.prompt = Some(Content::Text("2 + 2 は？".to_string()));
        assert_eq!(
            classify_question_change(&previous, &next),
            QuestionChange::StudyContent
        );

        let mut next = previous.clone();
        next.kind = QuestionType::TrueFalse;
        assert_eq!(
            classify_question_change(&previous, &next),
            QuestionChange::StudyContent
        );

        let mut next = previous.clone();
        next.options.as_mut().unwrap()[1].content = Content::Text("3".to_string());
        assert_eq!(
            classify_question_change(&previous, &next),
            QuestionChange::StudyContent
        );

        let flashcard: Question = serde_json::from_value(serde_json::json!({
            "id": "f1",
            "revision": 1,
            "type": "flashcard",
            "front": "表",
            "back": "裏",
            "explanation": "e"
        }))
        .unwrap();
        let mut changed = flashcard.clone();
        changed.back = Some(Content::Text("別の裏".to_string()));
        assert_eq!(
            classify_question_change(&flashcard, &changed),
            QuestionChange::StudyContent
        );
    }

    #[test]
    fn a_manual_revision_bump_is_always_a_new_study_target() {
        let previous = question();
        let mut next = previous.clone();
        next.revision = 2;
        assert_eq!(
            classify_question_change(&previous, &next),
            QuestionChange::StudyContent
        );
        assert_eq!(
            resolved_revision(&previous, &next, QuestionChange::StudyContent),
            2
        );
        next.revision = 1;
        assert_eq!(
            resolved_revision(&previous, &next, QuestionChange::StudyContent),
            2
        );
    }
}
