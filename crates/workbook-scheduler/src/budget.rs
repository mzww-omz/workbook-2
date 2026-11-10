//! Time-budgeted session selection.
//!
//! A time budget is a selection target, never a countdown that destroys work.
//! The service asks this module which candidates fit in the learner's stated
//! time; the session then runs the ordinary queue and lets the final answer
//! finish even when the estimate was optimistic.
//!
//! Priority is explicit and matches Today/Exam semantics:
//! 1. due short-term Learning/Relearning cards
//! 2. due Review cards
//! 3. new content (already capped by the daily new limit)
//! 4. Learn Ahead cards (within 20 minutes and the same learning day)
//!
//! FSRS due dates are read, never rewritten. A question that did not fit in
//! the budget is simply not added to this session; it remains due.

use chrono::{DateTime, Utc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BudgetCategory {
    ShortTermDue,
    DueReview,
    New,
    LearnAhead,
}

impl BudgetCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::ShortTermDue => "短期再学習",
            Self::DueReview => "復習",
            Self::New => "新規",
            Self::LearnAhead => "先取り",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BudgetQuestion {
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
    pub kind: String,
    pub category: BudgetCategory,
    pub due_at: Option<DateTime<Utc>>,
    /// Stable position in the input. Used only to keep the selection
    /// deterministic when priorities and due times tie.
    pub ordinal: u32,
    pub estimated_seconds: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BudgetSelection {
    pub selected: Vec<BudgetQuestion>,
    pub estimated_seconds: u32,
    /// Candidates that were ready but did not fit. They stay due.
    pub deferred: usize,
    /// True when even the first candidate's estimate exceeded the budget. It
    /// is still selected: a session with zero questions would not let the
    /// learner study at all, and the estimate is intentionally approximate.
    pub overshoot: bool,
}

impl BudgetSelection {
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }
}

fn normalized_estimate(value: f64, kind: &str) -> f64 {
    if value.is_finite() && value > 0.0 {
        value.clamp(5.0, 300.0)
    } else {
        crate::planner::estimated_seconds(kind, 0, 0)
    }
}

/// Selects candidates until the budget is used. The caller supplies the
/// candidates; this function applies the priority order itself so a caller
/// cannot accidentally put Learn Ahead before due reviews.
pub fn select_within_budget(questions: &[BudgetQuestion], budget_seconds: u32) -> BudgetSelection {
    if budget_seconds == 0 {
        return BudgetSelection {
            selected: Vec::new(),
            estimated_seconds: 0,
            deferred: questions.len(),
            overshoot: false,
        };
    }
    let budget = f64::from(budget_seconds);
    let mut ordered: Vec<BudgetQuestion> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for question in questions {
        if !seen.insert((question.workbook_id.clone(), question.question_id.clone())) {
            continue;
        }
        let mut question = question.clone();
        question.estimated_seconds =
            normalized_estimate(question.estimated_seconds, &question.kind);
        ordered.push(question);
    }
    ordered.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then(a.due_at.cmp(&b.due_at))
            .then(a.ordinal.cmp(&b.ordinal))
            .then(a.workbook_id.cmp(&b.workbook_id))
            .then(a.note_id.cmp(&b.note_id))
            .then(a.question_id.cmp(&b.question_id))
    });

    let mut selected = Vec::new();
    let mut used = 0.0f64;
    let mut overshoot = false;
    for question in ordered.iter() {
        let fits = used + question.estimated_seconds <= budget;
        if selected.is_empty() {
            // Always make a 3-minute session usable even if the only
            // candidate is a long matching question. The UI shows the target
            // and never force-finishes mid-answer.
            overshoot = !fits;
            used += question.estimated_seconds;
            selected.push(question.clone());
            continue;
        }
        if !fits {
            break;
        }
        used += question.estimated_seconds;
        selected.push(question.clone());
    }
    BudgetSelection {
        deferred: questions.len().saturating_sub(selected.len()),
        estimated_seconds: used.round() as u32,
        selected,
        overshoot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 1, 9, 0, 0).unwrap()
    }

    fn question(
        id: &str,
        category: BudgetCategory,
        due_at: Option<DateTime<Utc>>,
        estimated_seconds: f64,
        ordinal: u32,
    ) -> BudgetQuestion {
        BudgetQuestion {
            workbook_id: "w".to_string(),
            note_id: format!("n{id}"),
            question_id: id.to_string(),
            kind: "single_choice".to_string(),
            category,
            due_at,
            ordinal,
            estimated_seconds,
        }
    }

    fn ids(selection: &BudgetSelection) -> Vec<&str> {
        selection
            .selected
            .iter()
            .map(|question| question.question_id.as_str())
            .collect()
    }

    #[test]
    fn three_minutes_selects_only_what_fits_and_keeps_the_rest_due() {
        let questions: Vec<_> = (0..20)
            .map(|index| {
                question(
                    &format!("q{index}"),
                    BudgetCategory::DueReview,
                    Some(now()),
                    20.0,
                    index,
                )
            })
            .collect();
        let selection = select_within_budget(&questions, 180);
        assert_eq!(selection.selected.len(), 9);
        assert_eq!(selection.estimated_seconds, 180);
        assert!(selection.deferred > 0);
        assert!(!selection.overshoot);
    }

    #[test]
    fn the_boundary_around_the_budget_is_deterministic() {
        let questions: Vec<_> = (0..10)
            .map(|index| {
                question(
                    &format!("q{index}"),
                    BudgetCategory::DueReview,
                    Some(now()),
                    20.0,
                    index,
                )
            })
            .collect();
        assert_eq!(select_within_budget(&questions, 179).selected.len(), 8);
        assert_eq!(select_within_budget(&questions, 180).selected.len(), 9);
        assert_eq!(select_within_budget(&questions, 181).selected.len(), 9);
        assert_eq!(select_within_budget(&questions, 200).selected.len(), 10);
    }

    #[test]
    fn a_budget_never_selects_nothing_when_a_question_exists() {
        let questions = vec![question(
            "long",
            BudgetCategory::DueReview,
            Some(now()),
            300.0,
            0,
        )];
        let selection = select_within_budget(&questions, 180);
        assert_eq!(ids(&selection), ["long"]);
        assert!(selection.overshoot);
        assert!(selection.estimated_seconds >= 180);
    }

    #[test]
    fn zero_seconds_selects_nothing_and_does_not_claim_completion() {
        let questions = vec![question(
            "q",
            BudgetCategory::DueReview,
            Some(now()),
            20.0,
            0,
        )];
        let selection = select_within_budget(&questions, 0);
        assert!(selection.is_empty());
        assert_eq!(selection.deferred, 1);
    }

    #[test]
    fn priority_is_short_term_then_review_then_new_then_learn_ahead() {
        let questions = vec![
            question(
                "ahead",
                BudgetCategory::LearnAhead,
                Some(now() + Duration::minutes(10)),
                20.0,
                0,
            ),
            question("new", BudgetCategory::New, None, 20.0, 1),
            question("review", BudgetCategory::DueReview, Some(now()), 20.0, 2),
            question(
                "short",
                BudgetCategory::ShortTermDue,
                Some(now() - Duration::minutes(1)),
                20.0,
                3,
            ),
        ];
        let selection = select_within_budget(&questions, 80);
        assert_eq!(ids(&selection), ["short", "review", "new", "ahead"]);
    }

    #[test]
    fn new_content_is_not_preferred_over_due_reviews() {
        let questions = vec![
            question("new", BudgetCategory::New, None, 20.0, 0),
            question(
                "review",
                BudgetCategory::DueReview,
                Some(now() - Duration::days(1)),
                20.0,
                1,
            ),
        ];
        let selection = select_within_budget(&questions, 20);
        assert_eq!(ids(&selection), ["review"]);
    }

    #[test]
    fn duplicate_references_are_selected_once() {
        let question = question("q", BudgetCategory::DueReview, Some(now()), 20.0, 0);
        let selection = select_within_budget(&[question.clone(), question], 600);
        assert_eq!(selection.selected.len(), 1);
    }

    #[test]
    fn non_finite_estimates_fall_back_to_the_format_estimate() {
        let mut question = question("q", BudgetCategory::DueReview, Some(now()), f64::NAN, 0);
        question.kind = "short_text".to_string();
        let selection = select_within_budget(&[question], 600);
        assert_eq!(selection.estimated_seconds, 45);
    }

    #[test]
    fn twenty_minutes_can_include_many_questions_without_overflowing() {
        let questions: Vec<_> = (0..100)
            .map(|index| {
                question(
                    &format!("q{index}"),
                    BudgetCategory::DueReview,
                    Some(now()),
                    30.0,
                    index,
                )
            })
            .collect();
        let selection = select_within_budget(&questions, 1_200);
        assert_eq!(selection.estimated_seconds, 1_200);
        assert_eq!(selection.selected.len(), 40);
    }
}
