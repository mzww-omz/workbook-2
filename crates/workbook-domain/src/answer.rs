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

/// True when the learner submitted nothing at all. This is distinct from a
/// wrong answer: a blank response must not be auto-rated as `Again`.
pub fn answer_is_blank(answer: Option<&Answer>) -> bool {
    match answer {
        None => true,
        Some(Answer::Text(value)) => value.trim().is_empty(),
        Some(Answer::Choices(values)) => values.is_empty(),
        Some(Answer::Blanks(values)) => values.values().all(|value| value.trim().is_empty()),
        Some(Answer::Bool(_)) => false,
    }
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

/// Conservative automatic rating derived only from a graded answer, the hint
/// state, and an explicit "わからない" response.
///
/// `None` means the answer cannot be scored automatically (for example a
/// subjective flashcard) and the caller must keep the manual rating flow.
///
/// `Easy` is deliberately never returned: elapsed time or a first correct
/// answer is not trustworthy evidence of ease. The learner can still choose
/// `Easy` manually.
pub fn automatic_rating(correct: Option<bool>, hint: bool, unknown: bool) -> Option<Rating> {
    if unknown || correct == Some(false) {
        Some(Rating::Again)
    } else if correct == Some(true) {
        Some(if hint { Rating::Hard } else { Rating::Good })
    } else {
        None
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
    fn automatic_ratings_are_conservative_and_never_easy() {
        assert_eq!(
            automatic_rating(Some(true), false, false),
            Some(Rating::Good)
        );
        assert_eq!(
            automatic_rating(Some(true), true, false),
            Some(Rating::Hard)
        );
        assert_eq!(
            automatic_rating(Some(false), false, false),
            Some(Rating::Again)
        );
        assert_eq!(
            automatic_rating(Some(false), true, false),
            Some(Rating::Again)
        );
        assert_eq!(automatic_rating(None, false, true), Some(Rating::Again));
        assert_eq!(automatic_rating(None, false, false), None);
        assert_ne!(
            automatic_rating(Some(true), false, false),
            Some(Rating::Easy)
        );
    }

    #[test]
    fn blank_answers_are_distinct_from_wrong_answers() {
        assert!(answer_is_blank(None));
        assert!(answer_is_blank(Some(&Answer::Text("   ".to_string()))));
        assert!(answer_is_blank(Some(&Answer::Choices(Vec::new()))));
        assert!(!answer_is_blank(Some(&Answer::Bool(false))));
        assert!(!answer_is_blank(Some(&Answer::Text("x".to_string()))));
    }
}
