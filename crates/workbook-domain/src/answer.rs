use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::grading::{GradeStatus, grade_answer_detailed};
use crate::session::Rating;
use crate::workbook::Question;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Answer {
    Text(String),
    Bool(bool),
    Choices(Vec<String>),
    Blanks(BTreeMap<String, String>),
    /// Ordered option IDs. Never display text: the IDs can be replayed even
    /// after an author edits labels.
    Ordering {
        order: Vec<String>,
    },
    /// Left item ID → right item ID.
    Matching {
        matches: BTreeMap<String, String>,
    },
    /// A point in normalized image coordinates (0..1, origin top-left).
    Hotspot {
        x: f64,
        y: f64,
    },
    /// Target/slot ID → label ID.
    Labels {
        labels: BTreeMap<String, String>,
    },
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
        Some(Answer::Ordering { order }) => order.is_empty(),
        Some(Answer::Matching { matches }) => matches.is_empty(),
        Some(Answer::Labels { labels }) => labels.is_empty(),
        Some(Answer::Hotspot { .. }) => false,
        Some(Answer::Bool(_)) => false,
    }
}

/// Grades an answer. `None` means the question has no automatic correctness
/// (flashcards), matching the legacy Workbook behaviour.
///
/// Near misses are reported as `Some(false)`: they are not final answers and
/// the service layer stops before committing FSRS state. Use
/// [`grade_answer_detailed`] when the near-miss reason is needed.
pub fn grade_answer(question: &Question, answer: Option<&Answer>) -> Option<bool> {
    grade_answer_detailed(question, answer).map(|result| result.status == GradeStatus::Correct)
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
        assert!(answer_is_blank(Some(&Answer::Blanks(BTreeMap::new()))));
        assert!(answer_is_blank(Some(&Answer::Ordering {
            order: Vec::new()
        })));
        assert!(answer_is_blank(Some(&Answer::Matching {
            matches: BTreeMap::new()
        })));
        assert!(answer_is_blank(Some(&Answer::Labels {
            labels: BTreeMap::new()
        })));
        assert!(!answer_is_blank(Some(&Answer::Hotspot { x: 0.0, y: 0.0 })));
        assert!(!answer_is_blank(Some(&Answer::Bool(false))));
        assert!(!answer_is_blank(Some(&Answer::Text("x".to_string()))));
    }

    #[test]
    fn new_answer_shapes_round_trip_without_colliding() {
        let answers = [
            Answer::Ordering {
                order: vec!["a".into(), "b".into()],
            },
            Answer::Matching {
                matches: BTreeMap::from([("l1".into(), "r1".into())]),
            },
            Answer::Hotspot { x: 0.25, y: 0.75 },
            Answer::Labels {
                labels: BTreeMap::from([("t1".into(), "l1".into())]),
            },
            Answer::Blanks(BTreeMap::from([("x".into(), "y".into())])),
        ];
        for answer in answers {
            let json = serde_json::to_string(&answer).unwrap();
            let parsed: Answer = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, answer);
        }
    }
}
