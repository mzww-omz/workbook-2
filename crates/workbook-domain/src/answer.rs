use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::session::Rating;
use crate::workbook::{Question, QuestionType};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Answer {
    Text(String),
    Bool(bool),
    Choices(Vec<String>),
    Blanks(BTreeMap<String, String>),
}

pub fn normalize_answer(value: &str) -> String {
    value
        .nfkc()
        .collect::<String>()
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Grades an answer. `None` means the question has no automatic correctness
/// (flashcards), matching the legacy Workbook behaviour.
pub fn grade_answer(question: &Question, answer: Option<&Answer>) -> Option<bool> {
    match question.kind {
        QuestionType::SingleChoice => {
            let expected = question.single_choice_answer()?;
            match answer {
                Some(Answer::Text(value)) => Some(value == expected),
                _ => Some(false),
            }
        }
        QuestionType::TrueFalse => {
            let expected = question.answer.as_ref()?.as_bool()?;
            match answer {
                Some(Answer::Bool(value)) => Some(*value == expected),
                _ => Some(false),
            }
        }
        QuestionType::MultipleChoice => {
            let expected = question.multiple_choice_answers();
            match answer {
                Some(Answer::Choices(values)) => {
                    let mut values = values.clone();
                    values.sort();
                    values.dedup();
                    let mut expected = expected;
                    expected.sort();
                    Some(values == expected)
                }
                _ => Some(false),
            }
        }
        QuestionType::ShortText => {
            let accepted = question.short_text_answers();
            match answer {
                Some(Answer::Text(value)) => {
                    let value = normalize_answer(value);
                    Some(
                        accepted
                            .iter()
                            .any(|candidate| normalize_answer(candidate) == value),
                    )
                }
                _ => Some(false),
            }
        }
        QuestionType::Cloze => {
            let expected = question.cloze_blanks();
            match answer {
                Some(Answer::Blanks(values)) => Some(expected.iter().all(|(id, accepted)| {
                    values.get(id).is_some_and(|value| {
                        let value = normalize_answer(value);
                        accepted
                            .iter()
                            .any(|candidate| normalize_answer(candidate) == value)
                    })
                })),
                _ => Some(false),
            }
        }
        QuestionType::Flashcard => None,
    }
}

/// Conservative automatic rating used when the learner does not choose one.
/// A wrong answer is always Again; a hinted correct answer starts at Hard.
/// Easy is never assigned automatically.
pub fn automatic_rating(correct: Option<bool>, hint: bool, unknown: bool) -> Rating {
    if unknown || correct != Some(true) {
        Rating::Again
    } else if hint {
        Rating::Hard
    } else {
        Rating::Good
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_matches_the_legacy_rules() {
        assert_eq!(normalize_answer("  ＡＢＣ  "), "abc");
        assert_eq!(normalize_answer("a   b"), "a b");
        assert_eq!(normalize_answer("Promise"), "promise");
    }

    #[test]
    fn automatic_ratings_never_promote_to_easy() {
        assert_eq!(automatic_rating(Some(true), false, false), Rating::Good);
        assert_eq!(automatic_rating(Some(true), true, false), Rating::Hard);
        assert_eq!(automatic_rating(Some(false), false, false), Rating::Again);
        assert_eq!(automatic_rating(None, false, true), Rating::Again);
    }
}
