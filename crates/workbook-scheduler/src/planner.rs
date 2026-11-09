//! Deadline-aware exam planning.
//!
//! This module is deliberately independent of SQLite and GPUI. It receives a
//! flat list of questions with their FSRS summary, attempt statistics, and a
//! duration estimate, then decides what to study today. FSRS still owns the
//! memory model: the planner only ranks and interleaves candidates.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, NaiveDate, Utc};
use workbook_domain::StudyCalendar;

/// Per-question FSRS values the planner needs. The forgetting curve itself is
/// not reimplemented here; the service passes the recall probability already
/// computed by `fsrs-rs`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanCard {
    pub due_at: DateTime<Utc>,
    pub last_review_at: Option<DateTime<Utc>>,
    pub lapses: u32,
    pub difficulty: f32,
    pub reps: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanQuestion {
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
    /// Question format (for interleaving variety and fallback estimates).
    pub kind: String,
    pub card: Option<PlanCard>,
    pub attempts: u32,
    pub incorrect: u32,
    /// Average measured answer time, or a format-based default.
    pub estimated_seconds: f64,
    /// Estimated probability of recall on the exam deadline assuming no further
    /// reviews. `None` for unstudied questions or exams without a date.
    pub exam_recall: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanReason {
    /// Never studied; needed to cover the scope.
    Coverage,
    /// FSRS says the card is due now.
    DueReview,
    /// Incorrect answers, lapses, or high difficulty.
    Weakness,
    /// Not due yet, but the estimated recall on the exam day is low.
    ExamRecall,
    /// Learned and healthy; included only to fill the remaining time.
    Maintenance,
}

impl PlanReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Coverage => "未学習",
            Self::DueReview => "復習期限",
            Self::Weakness => "苦手",
            Self::ExamRecall => "試験前の確認",
            Self::Maintenance => "維持",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedQuestion {
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
    pub kind: String,
    pub reason: PlanReason,
    pub estimated_seconds: f64,
    pub priority: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldAllocation {
    pub workbook_id: String,
    pub questions: usize,
    pub new_questions: usize,
    pub estimated_minutes: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanCoverage {
    pub total: usize,
    pub studied: usize,
    pub unstudied: usize,
    pub due: usize,
    pub weak: usize,
    pub selected: usize,
    pub selected_new: usize,
    pub selected_due: usize,
    pub selected_weak: usize,
    pub selected_early: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanFeasibility {
    /// No target date: nothing to be behind on.
    NoDeadline,
    /// One pass through unstudied and currently weak questions fits in the
    /// remaining daily budgets.
    Insufficient,
    Sufficient,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanWarning {
    NoQuestions,
    NoTime,
    Expired,
    InsufficientTime {
        required_minutes: u32,
        available_minutes: u32,
    },
    DailyNewLimit {
        remaining: usize,
    },
    TimeTooShort,
}

impl PlanWarning {
    pub fn message(&self) -> String {
        match self {
            Self::NoQuestions => "この試験の範囲に問題がありません".to_string(),
            Self::NoTime => "今日の学習可能時間が残っていません".to_string(),
            Self::Expired => "試験日を過ぎています。通常の復習を続けてください".to_string(),
            Self::InsufficientTime {
                required_minutes,
                available_minutes,
            } => format!(
                "範囲全体を1回ずつ学習するには約{required_minutes}分必要ですが、期限までの学習可能時間は約{available_minutes}分です。未学習を優先し、時間内で実行できる計画にしています"
            ),
            Self::DailyNewLimit { remaining } => {
                format!("新規問題の残り枠（{remaining}問）に合わせて、未学習は一部だけです")
            }
            Self::TimeTooShort => {
                "学習時間が1問分に満たないため、今日の計画を作れません".to_string()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExamPlan {
    /// Calendar days from today's learning day to the target date. `None` when
    /// the exam has no date. Negative only through `expired` semantics.
    pub days_remaining: Option<i64>,
    pub expired: bool,
    pub daily_minutes: u32,
    /// Seconds already spent on this exam today (excluding mock exams).
    pub studied_seconds_today: u32,
    pub planned_seconds: u32,
    pub coverage: PlanCoverage,
    pub feasibility: PlanFeasibility,
    pub required_minutes: u32,
    pub available_minutes: u32,
    pub selected: Vec<PlannedQuestion>,
    pub allocation: Vec<FieldAllocation>,
    pub warnings: Vec<PlanWarning>,
}

#[derive(Clone, Debug)]
pub struct PlanInput<'a> {
    pub now: DateTime<Utc>,
    pub calendar: &'a StudyCalendar,
    pub target_date: Option<NaiveDate>,
    pub daily_minutes: u32,
    /// Remaining new-question allowance for the current learning day.
    pub new_limit_remaining: usize,
    /// Time already spent on this exam during the current learning day.
    pub studied_seconds_today: u32,
    pub questions: &'a [PlanQuestion],
}

/// The instant that separates the exam day from the following learning day.
/// Using the 4 a.m. study-day boundary keeps `target_date` consistent with the
/// rest of the scheduler, including DST transitions.
pub fn exam_deadline(calendar: &StudyCalendar, target_date: NaiveDate) -> DateTime<Utc> {
    calendar.day_start(target_date + Duration::days(1))
}

fn default_estimate(kind: &str) -> f64 {
    match kind {
        "flashcard" | "true_false" | "single_choice" => 20.0,
        "multiple_choice" => 30.0,
        "cloze" | "short_text" | "numeric" => 45.0,
        "ordering" | "matching" => 60.0,
        "hotspot" | "labeling" => 45.0,
        _ => 30.0,
    }
}

pub fn estimated_seconds(kind: &str, attempts: u32, elapsed_ms: i64) -> f64 {
    if attempts > 0 && elapsed_ms > 0 {
        let average = elapsed_ms as f64 / f64::from(attempts) / 1_000.0;
        return average.clamp(5.0, 300.0);
    }
    default_estimate(kind)
}

#[derive(Clone, Debug)]
struct Candidate {
    question: PlanQuestion,
    new: bool,
    due: bool,
    weak: bool,
    exam_recall: Option<f32>,
    priority: f64,
}

fn weakness(card: Option<&PlanCard>, attempts: u32, incorrect: u32) -> f64 {
    let error_rate = if attempts == 0 {
        0.0
    } else {
        f64::from(incorrect) / f64::from(attempts)
    };
    let lapses = card
        .map(|card| f64::from(card.lapses.min(3)) / 3.0)
        .unwrap_or(0.0);
    let difficulty = card
        .map(|card| ((f64::from(card.difficulty) - 5.0) / 5.0).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    (error_rate * 0.6 + lapses * 0.2 + difficulty * 0.2).clamp(0.0, 1.0)
}

/// Plans one learning day. The function is deterministic for the same input and
/// never guarantees that the exam scope will be covered; it only produces the
/// best executable set within the stated time and new-question limits.
pub fn plan_exam(input: PlanInput<'_>) -> ExamPlan {
    let daily_minutes = input.daily_minutes.min(1_440);
    let today = input.calendar.learning_day(input.now);
    let days_remaining = input.target_date.map(|target| (target - today).num_days());
    let deadline = input
        .target_date
        .map(|target| exam_deadline(input.calendar, target));
    let expired = days_remaining.is_some_and(|days| days < 0);
    let study_days = days_remaining
        .map(|days| (days.max(0) + 1) as u32)
        .unwrap_or(1);

    let mut seen = BTreeSet::new();
    let mut questions = Vec::new();
    for question in input.questions {
        if seen.insert((question.workbook_id.clone(), question.question_id.clone())) {
            questions.push(question.clone());
        }
    }

    let total = questions.len();
    let mut studied = 0usize;
    let mut due = 0usize;
    let mut weak = 0usize;
    let mut unstudied_seconds = 0.0f64;
    let mut attention_seconds = 0.0f64;
    for question in &questions {
        let estimated = if question.estimated_seconds.is_finite() {
            question.estimated_seconds.clamp(5.0, 300.0)
        } else {
            estimated_seconds(&question.kind, question.attempts, 0)
        };
        match question.card {
            None => {
                unstudied_seconds += estimated;
            }
            Some(card) => {
                studied += 1;
                let is_due = card.due_at <= input.now;
                let is_weak = card.lapses > 0 || card.difficulty >= 7.0 || question.incorrect > 0;
                if is_due {
                    due += 1;
                }
                if is_weak {
                    weak += 1;
                }
                if is_due || is_weak || question.exam_recall.is_some_and(|recall| recall < 0.8) {
                    attention_seconds += estimated;
                }
            }
        }
    }
    let unstudied = total.saturating_sub(studied);

    let spent_seconds = input.studied_seconds_today;
    let required_minutes = ((unstudied_seconds + attention_seconds) / 60.0).ceil() as u32;
    let available_minutes = daily_minutes
        .saturating_mul(study_days)
        .saturating_sub(spent_seconds / 60);
    let feasibility = if input.target_date.is_none() || expired {
        PlanFeasibility::NoDeadline
    } else if required_minutes > available_minutes {
        PlanFeasibility::Insufficient
    } else {
        PlanFeasibility::Sufficient
    };

    let budget = (daily_minutes * 60).saturating_sub(spent_seconds) as f64;

    let coverage_gap = if total == 0 {
        0.0
    } else {
        unstudied as f64 / total as f64
    };
    let pressure = if expired {
        0.15
    } else {
        days_remaining
            .map(|days| 1.0 / (days.max(0) as f64 + 1.0))
            .unwrap_or(0.15)
    };
    let coverage_weight = 0.65 + 1.35 * coverage_gap * (0.6 + pressure);
    let consolidation_weight = 0.6 + 0.9 * (1.0 - coverage_gap) + 0.7 * pressure;

    let mut field_need: BTreeMap<String, f64> = BTreeMap::new();
    let mut candidates = Vec::new();
    for question in questions {
        let estimated = if question.estimated_seconds.is_finite() {
            question.estimated_seconds.clamp(5.0, 300.0)
        } else {
            estimated_seconds(&question.kind, question.attempts, 0)
        };
        let mut question = question;
        question.estimated_seconds = estimated;
        let new = question.card.is_none();
        let card = question.card.as_ref();
        let is_due = card.is_some_and(|card| card.due_at <= input.now);
        let is_weak = card.is_some_and(|card| {
            card.lapses > 0 || card.difficulty >= 7.0 || question.incorrect > 0
        });
        let recall_gap = question
            .exam_recall
            .map(|recall| (1.0 - recall).clamp(0.0, 1.0) as f64)
            .unwrap_or(0.0);
        let overdue_days = card
            .filter(|card| card.due_at <= input.now)
            .map(|card| {
                ((input.now - card.due_at).num_seconds() as f64 / 86_400.0).clamp(0.0, 30.0)
            })
            .unwrap_or(0.0);
        let overdue = 1.0 + overdue_days.sqrt() / 2.0;
        let recency = card
            .and_then(|card| card.last_review_at)
            .map(|last| ((input.now - last).num_days().max(0) as f64 / 30.0).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        let weak_score = weakness(card, question.attempts, question.incorrect);
        let exam_recall = question.exam_recall;

        let priority = if new {
            coverage_weight * (0.8 + pressure) + 0.1
        } else {
            consolidation_weight
                * (0.15
                    + if is_due { overdue } else { 0.0 }
                    + weak_score * 1.1
                    + recall_gap * 1.5
                    + recency * 0.25)
        };
        // A field with many unstudied or weak questions deserves more of the
        // budget, but never at the cost of a field being ignored entirely.
        let need = if new {
            2.0
        } else if is_due {
            1.5
        } else if is_weak {
            1.0
        } else {
            0.2
        };
        *field_need
            .entry(question.workbook_id.clone())
            .or_insert(0.0) += need;

        candidates.push(Candidate {
            question,
            new,
            due: is_due,
            weak: is_weak,
            exam_recall,
            priority,
        });
    }
    let max_field_need = field_need.values().copied().fold(0.0, f64::max);
    let mut field_order: Vec<(String, f64)> = field_need
        .iter()
        .map(|(field, need)| (field.clone(), *need))
        .collect();
    field_order.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    // Short sessions concentrate on the fields with the greatest need. As the
    // budget grows, more fields become active. Fields with due cards are always
    // active so a subject is never ignored solely because another one ranks
    // slightly higher today.
    let max_fields = if daily_minutes == 0 {
        1
    } else {
        ((f64::from(daily_minutes) / 20.0).ceil() as usize).clamp(1, field_order.len().max(1))
    };
    let mut active_fields: BTreeSet<String> = field_order
        .iter()
        .take(max_fields)
        .map(|(field, _)| field.clone())
        .collect();
    for candidate in &candidates {
        if candidate.due {
            active_fields.insert(candidate.question.workbook_id.clone());
        }
    }

    let mut warnings = Vec::new();
    if total == 0 {
        warnings.push(PlanWarning::NoQuestions);
    }
    if expired {
        warnings.push(PlanWarning::Expired);
    }
    if budget <= 0.0 {
        warnings.push(PlanWarning::NoTime);
    }
    if let PlanFeasibility::Insufficient = feasibility {
        warnings.push(PlanWarning::InsufficientTime {
            required_minutes,
            available_minutes,
        });
    }

    let mut selected: Vec<Candidate> = Vec::new();
    let mut used_seconds = 0.0f64;
    let mut new_count = 0usize;
    let mut field_counts: BTreeMap<String, usize> = BTreeMap::new();
    let new_cap = input.new_limit_remaining.min(unstudied);

    if budget <= 0.0 || total == 0 {
        if budget > 0.0 && total > 0 {
            warnings.push(PlanWarning::TimeTooShort);
        }
    } else {
        while !candidates.is_empty() {
            let mut best: Option<(usize, f64)> = None;
            for (index, candidate) in candidates.iter().enumerate() {
                if candidate.new && new_count >= new_cap {
                    continue;
                }
                if !active_fields.contains(&candidate.question.workbook_id) {
                    continue;
                }
                // A card scheduled after the exam is not worth an early review
                // unless it is weak or the model predicts poor recall on the
                // exam day. Otherwise the planner would add useless workload.
                let after_exam = deadline.is_some_and(|deadline| {
                    candidate
                        .question
                        .card
                        .is_some_and(|card| card.due_at > deadline)
                });
                if after_exam
                    && !candidate.due
                    && !candidate.weak
                    && !candidate.exam_recall.is_some_and(|recall| recall < 0.8)
                {
                    continue;
                }
                if !selected.is_empty()
                    && used_seconds + candidate.question.estimated_seconds > budget
                {
                    continue;
                }
                if selected.is_empty() && candidate.question.estimated_seconds > budget {
                    continue;
                }
                let field_count = *field_counts
                    .get(&candidate.question.workbook_id)
                    .unwrap_or(&0) as f64;
                let fairness = 1.0 / (1.0 + 0.45 * field_count);
                let untouched = if field_count == 0.0
                    && field_need
                        .get(&candidate.question.workbook_id)
                        .copied()
                        .unwrap_or(0.0)
                        > 0.0
                {
                    let need = field_need[&candidate.question.workbook_id];
                    0.25 * (need / max_field_need.max(1.0))
                } else {
                    0.0
                };
                let effective = candidate.priority * fairness + untouched;
                if best.is_none_or(|(_, value)| effective > value) {
                    best = Some((index, effective));
                }
            }
            let Some((index, _)) = best else {
                // The active fields are exhausted or blocked. Activate the
                // next most-needed field rather than leaving time unused.
                let next = field_order
                    .iter()
                    .find(|(field, _)| !active_fields.contains(field))
                    .map(|(field, _)| field.clone());
                if let Some(next) = next {
                    active_fields.insert(next);
                    continue;
                }
                if selected.is_empty() {
                    warnings.push(PlanWarning::TimeTooShort);
                }
                break;
            };
            let candidate = candidates.remove(index);
            used_seconds += candidate.question.estimated_seconds;
            if candidate.new {
                new_count += 1;
            }
            *field_counts
                .entry(candidate.question.workbook_id.clone())
                .or_insert(0) += 1;
            selected.push(candidate);
        }
    }

    if new_count >= new_cap && unstudied > new_count {
        warnings.push(PlanWarning::DailyNewLimit {
            remaining: input.new_limit_remaining,
        });
    }
    warnings.dedup();

    // Keep the planner's output stable and independent of caller ordering.
    selected.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.question.workbook_id.cmp(&b.question.workbook_id))
            .then(a.question.note_id.cmp(&b.question.note_id))
            .then(a.question.question_id.cmp(&b.question.question_id))
    });

    let new_items: Vec<Candidate> = selected.iter().filter(|item| item.new).cloned().collect();
    let learned_items: Vec<Candidate> = selected.iter().filter(|item| !item.new).cloned().collect();
    let learned_first = consolidation_weight > coverage_weight && !learned_items.is_empty();
    let mut ordered = if learned_first {
        interleave_learned(learned_items.clone())
    } else {
        coverage_blocks(new_items.clone())
    };
    let rest = if learned_first {
        coverage_blocks(new_items)
    } else {
        interleave_learned(learned_items)
    };
    ordered.extend(rest);

    let mut allocation: BTreeMap<String, FieldAllocation> = BTreeMap::new();
    let mut selected_new = 0usize;
    let mut selected_due = 0usize;
    let mut selected_weak = 0usize;
    let mut selected_early = 0usize;
    let mut planned = Vec::new();
    for item in &ordered {
        let reason = if item.new {
            selected_new += 1;
            PlanReason::Coverage
        } else if item.due {
            selected_due += 1;
            PlanReason::DueReview
        } else if item.weak {
            selected_weak += 1;
            PlanReason::Weakness
        } else if item.exam_recall.is_some_and(|recall| recall < 0.8) {
            selected_early += 1;
            PlanReason::ExamRecall
        } else {
            PlanReason::Maintenance
        };
        let field = allocation
            .entry(item.question.workbook_id.clone())
            .or_insert_with(|| FieldAllocation {
                workbook_id: item.question.workbook_id.clone(),
                questions: 0,
                new_questions: 0,
                estimated_minutes: 0.0,
            });
        field.questions += 1;
        if item.new {
            field.new_questions += 1;
        }
        field.estimated_minutes += item.question.estimated_seconds / 60.0;
        planned.push(PlannedQuestion {
            workbook_id: item.question.workbook_id.clone(),
            note_id: item.question.note_id.clone(),
            question_id: item.question.question_id.clone(),
            kind: item.question.kind.clone(),
            reason,
            estimated_seconds: item.question.estimated_seconds,
            priority: item.priority,
        });
    }
    let mut allocation: Vec<FieldAllocation> = allocation.into_values().collect();
    allocation.sort_by(|a, b| {
        b.estimated_minutes
            .partial_cmp(&a.estimated_minutes)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.workbook_id.cmp(&b.workbook_id))
    });

    let coverage = PlanCoverage {
        total,
        studied,
        unstudied,
        due,
        weak,
        selected: planned.len(),
        selected_new,
        selected_due,
        selected_weak,
        selected_early,
    };
    ExamPlan {
        days_remaining,
        expired,
        daily_minutes,
        studied_seconds_today: spent_seconds,
        planned_seconds: used_seconds.round() as u32,
        coverage,
        feasibility,
        required_minutes,
        available_minutes,
        selected: planned,
        allocation,
        warnings,
    }
}

fn unit_key(question: &PlanQuestion) -> (String, String) {
    let note = if question.note_id.trim().is_empty() {
        format!("#{}", question.question_id)
    } else {
        question.note_id.clone()
    };
    (question.workbook_id.clone(), note)
}

/// New content is studied unit by unit, ordered by the unit's best priority.
fn coverage_blocks(items: Vec<Candidate>) -> Vec<Candidate> {
    let mut units: BTreeMap<(String, String), Vec<Candidate>> = BTreeMap::new();
    for item in items {
        units
            .entry(unit_key(&item.question))
            .or_default()
            .push(item);
    }
    let mut blocks: Vec<Vec<Candidate>> = units.into_values().collect();
    for block in &mut blocks {
        block.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.question.question_id.cmp(&b.question.question_id))
        });
    }
    blocks.sort_by(|a, b| {
        let a_best = a.first().map(|item| item.priority).unwrap_or(0.0);
        let b_best = b.first().map(|item| item.priority).unwrap_or(0.0);
        b_best
            .partial_cmp(&a_best)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a[0].question.workbook_id.cmp(&b[0].question.workbook_id))
            .then(a[0].question.note_id.cmp(&b[0].question.note_id))
    });
    blocks.into_iter().flatten().collect()
}

/// Learned content is interleaved: adjacent items avoid both the same note and
/// the same question format whenever an alternative exists.
fn interleave_learned(mut items: Vec<Candidate>) -> Vec<Candidate> {
    items.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.question.workbook_id.cmp(&b.question.workbook_id))
            .then(a.question.note_id.cmp(&b.question.note_id))
            .then(a.question.question_id.cmp(&b.question.question_id))
    });
    let mut output = Vec::new();
    let mut last_unit: Option<(String, String)> = None;
    let mut last_kind: Option<String> = None;
    while !items.is_empty() {
        let strict = items.iter().position(|item| {
            Some(&unit_key(&item.question)) != last_unit.as_ref()
                && Some(&item.question.kind) != last_kind.as_ref()
        });
        let different_unit = items
            .iter()
            .position(|item| Some(&unit_key(&item.question)) != last_unit.as_ref());
        let index = strict.or(different_unit).unwrap_or(0);
        let item = items.remove(index);
        last_unit = Some(unit_key(&item.question));
        last_kind = Some(item.question.kind.clone());
        output.push(item);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn calendar() -> StudyCalendar {
        StudyCalendar::new("UTC").unwrap()
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap()
    }

    fn date(days: i64) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + Duration::days(days)
    }

    fn card(due_days: i64, lapses: u32, difficulty: f32) -> PlanCard {
        PlanCard {
            due_at: now() + Duration::days(due_days),
            last_review_at: Some(now() - Duration::days(2)),
            lapses,
            difficulty,
            reps: 2,
        }
    }

    fn question(
        workbook: &str,
        note: &str,
        id: &str,
        kind: &str,
        card: Option<PlanCard>,
    ) -> PlanQuestion {
        PlanQuestion {
            workbook_id: workbook.to_string(),
            note_id: note.to_string(),
            question_id: id.to_string(),
            kind: kind.to_string(),
            card,
            attempts: 0,
            incorrect: 0,
            estimated_seconds: 20.0,
            exam_recall: None,
        }
    }

    fn plan(target: Option<NaiveDate>, minutes: u32, questions: Vec<PlanQuestion>) -> ExamPlan {
        let calendar = calendar();
        plan_exam(PlanInput {
            now: now(),
            calendar: &calendar,
            target_date: target,
            daily_minutes: minutes,
            new_limit_remaining: 100,
            studied_seconds_today: 0,
            questions: &questions,
        })
    }

    #[test]
    fn duration_falls_back_by_question_format_and_uses_history_when_present() {
        assert!(estimated_seconds("flashcard", 0, 0) < estimated_seconds("short_text", 0, 0));
        assert_eq!(estimated_seconds("matching", 0, 0), 60.0);
        assert!((estimated_seconds("single_choice", 2, 60_000) - 30.0).abs() < 1e-9);
        assert_eq!(estimated_seconds("single_choice", 1, 1), 5.0);
    }

    #[test]
    fn a_short_deadline_selects_coverage_first() {
        let questions = (0..6)
            .map(|index| question("A", "n1", &format!("q{index}"), "true_false", None))
            .collect();
        let plan = plan(Some(date(0)), 1, questions);
        assert!(plan.coverage.selected_new > 0);
        assert_eq!(plan.feasibility, PlanFeasibility::Insufficient);
        assert!(
            plan.warnings
                .iter()
                .any(|warning| matches!(warning, PlanWarning::InsufficientTime { .. }))
        );
    }

    #[test]
    fn a_long_deadline_mixes_due_review_with_new_content() {
        let questions = vec![
            question("A", "n1", "q1", "true_false", Some(card(-1, 0, 3.0))),
            question("A", "n1", "q2", "true_false", None),
            question("B", "n2", "q3", "true_false", Some(card(-2, 1, 8.0))),
        ];
        let plan = plan(Some(date(90)), 20, questions);
        assert!(plan.coverage.selected_due >= 1);
        assert!(plan.coverage.selected_new >= 1);
        assert_eq!(plan.feasibility, PlanFeasibility::Sufficient);
    }

    #[test]
    fn unstudied_content_is_grouped_by_unit() {
        let questions = vec![
            question("A", "n1", "q1", "single_choice", None),
            question("B", "n2", "q2", "single_choice", None),
            question("A", "n1", "q3", "single_choice", None),
            question("B", "n2", "q4", "single_choice", None),
        ];
        let plan = plan(Some(date(0)), 10, questions);
        let units: Vec<_> = plan
            .selected
            .iter()
            .map(|item| (item.workbook_id.as_str(), item.note_id.as_str()))
            .collect();
        let mut seen = BTreeSet::new();
        let mut last: Option<(&str, &str)> = None;
        for unit in units {
            if last != Some(unit) {
                assert!(seen.insert(unit));
                last = Some(unit);
            }
        }
    }

    #[test]
    fn learned_items_avoid_repeating_the_same_unit_or_format() {
        let questions = vec![
            question("A", "n1", "q1", "true_false", Some(card(-1, 1, 8.0))),
            question("A", "n1", "q2", "true_false", Some(card(-1, 1, 8.0))),
            question("B", "n2", "q3", "short_text", Some(card(-1, 1, 8.0))),
            question("C", "n3", "q4", "matching", Some(card(-1, 1, 8.0))),
        ];
        let plan = plan(Some(date(0)), 60, questions);
        let keys: Vec<_> = plan
            .selected
            .iter()
            .map(|item| (item.workbook_id.as_str(), item.kind.as_str()))
            .collect();
        assert!(keys.windows(2).all(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn the_daily_time_budget_is_a_target_not_a_reason_to_overflow() {
        let questions: Vec<_> = (0..100)
            .map(|index| question("A", "n1", &format!("q{index}"), "short_text", None))
            .collect();
        let plan = plan(Some(date(30)), 10, questions);
        assert!(plan.coverage.selected > 0);
        assert!(plan.planned_seconds <= 600);
        assert!(plan.coverage.selected < 100);
    }

    #[test]
    fn time_spent_today_reduces_the_remaining_plan() {
        let questions: Vec<_> = (0..100)
            .map(|index| question("A", "n1", &format!("q{index}"), "short_text", None))
            .collect();
        let calendar = calendar();
        let full = plan_exam(PlanInput {
            now: now(),
            calendar: &calendar,
            target_date: Some(date(30)),
            daily_minutes: 10,
            new_limit_remaining: 100,
            studied_seconds_today: 0,
            questions: &questions,
        });
        let partial = plan_exam(PlanInput {
            now: now(),
            calendar: &calendar,
            target_date: Some(date(30)),
            daily_minutes: 10,
            new_limit_remaining: 100,
            studied_seconds_today: 480,
            questions: &questions,
        });
        assert!(partial.coverage.selected < full.coverage.selected);
        assert!(partial.planned_seconds <= 120);
    }

    #[test]
    fn no_new_limit_never_selects_unstudied_questions() {
        let questions = vec![
            question("A", "n1", "q1", "true_false", None),
            question("A", "n1", "q2", "true_false", Some(card(-1, 0, 3.0))),
        ];
        let calendar = calendar();
        let plan = plan_exam(PlanInput {
            now: now(),
            calendar: &calendar,
            target_date: Some(date(7)),
            daily_minutes: 30,
            new_limit_remaining: 0,
            studied_seconds_today: 0,
            questions: &questions,
        });
        assert_eq!(plan.coverage.selected_new, 0);
        assert!(
            plan.warnings
                .iter()
                .any(|warning| matches!(warning, PlanWarning::DailyNewLimit { .. }))
        );
    }

    #[test]
    fn questions_due_after_the_exam_are_early_review_candidates_only_when_recall_is_low() {
        let mut low_recall = question("A", "n1", "q1", "true_false", Some(card(30, 0, 4.0)));
        low_recall.exam_recall = Some(0.4);
        let mut high_recall = question("A", "n1", "q2", "true_false", Some(card(30, 0, 4.0)));
        high_recall.exam_recall = Some(0.98);
        high_recall.estimated_seconds = 50.0;
        let plan = plan(Some(date(5)), 1, vec![low_recall, high_recall]);
        assert_eq!(plan.selected.len(), 1);
        assert_eq!(plan.selected[0].question_id, "q1");
        assert_eq!(plan.selected[0].reason, PlanReason::ExamRecall);
    }

    #[test]
    fn healthy_cards_scheduled_after_the_exam_are_not_early_reviewed() {
        let mut healthy = question("A", "n1", "q1", "true_false", Some(card(30, 0, 4.0)));
        healthy.exam_recall = Some(0.95);
        let plan = plan(Some(date(5)), 60, vec![healthy]);
        assert!(plan.selected.is_empty());
    }

    #[test]
    fn expired_deadlines_are_reported_without_inventing_urgency() {
        let questions = vec![question("A", "n1", "q1", "true_false", None)];
        let plan = plan(Some(date(-2)), 10, questions);
        assert!(plan.expired);
        assert!(plan.warnings.contains(&PlanWarning::Expired));
        assert_eq!(plan.selected.len(), 1);
    }

    #[test]
    fn no_deadline_still_produces_a_useful_daily_plan() {
        let questions = vec![
            question("A", "n1", "q1", "true_false", None),
            question("A", "n1", "q2", "true_false", Some(card(-1, 0, 3.0))),
        ];
        let plan = plan(None, 10, questions);
        assert_eq!(plan.feasibility, PlanFeasibility::NoDeadline);
        assert!(!plan.selected.is_empty());
    }

    #[test]
    fn zero_minutes_is_a_valid_rest_day() {
        let questions = vec![question("A", "n1", "q1", "true_false", None)];
        let plan = plan(Some(date(3)), 0, questions);
        assert!(plan.selected.is_empty());
        assert!(plan.warnings.contains(&PlanWarning::NoTime));
    }

    #[test]
    fn missed_days_do_not_create_a_multi_day_backlog() {
        let questions: Vec<_> = (0..100)
            .map(|index| question("A", "n1", &format!("q{index}"), "short_text", None))
            .collect();
        let near = plan(Some(date(2)), 10, questions.clone());
        let far = plan(Some(date(20)), 10, questions);
        // Both plans are bounded by today's time budget; skipping days does not
        // queue every missed question onto the next plan.
        assert!(near.coverage.selected <= 30);
        assert!(far.coverage.selected <= 30);
        assert!(near.planned_seconds <= 600);
        assert!(far.planned_seconds <= 600);
    }

    #[test]
    fn short_sessions_concentrate_on_fewer_fields() {
        let questions: Vec<_> = (0..9)
            .flat_map(|field| {
                (0..10).map(move |item| {
                    question(
                        &format!("W{field}"),
                        &format!("n{field}"),
                        &format!("q{field}-{item}"),
                        "true_false",
                        None,
                    )
                })
            })
            .collect();
        let short = plan(Some(date(30)), 10, questions.clone());
        let long = plan(Some(date(30)), 120, questions);
        assert!(short.allocation.len() < 9);
        assert!(long.allocation.len() >= short.allocation.len());
        assert!(short.allocation.len() <= 3);
    }

    #[test]
    fn duplicate_question_references_are_selected_once() {
        let q = question("A", "n1", "q1", "true_false", None);
        let plan = plan(Some(date(3)), 10, vec![q.clone(), q]);
        assert_eq!(plan.coverage.selected, 1);
        assert_eq!(plan.coverage.total, 1);
    }

    #[test]
    fn arbitrary_field_counts_are_allocated_without_a_fixed_subject_rotation() {
        let questions: Vec<_> = (0..9)
            .map(|index| {
                question(
                    &format!("W{index}"),
                    &format!("n{index}"),
                    &format!("q{index}"),
                    "true_false",
                    None,
                )
            })
            .collect();
        let plan = plan(Some(date(2)), 10, questions);
        assert!(plan.allocation.len() >= 2);
        assert!(plan.allocation.len() <= 9);
        assert!(plan.coverage.selected >= plan.allocation.len());
    }

    #[test]
    fn more_time_plans_more_questions() {
        let questions: Vec<_> = (0..9)
            .map(|index| {
                question(
                    &format!("W{index}"),
                    &format!("n{index}"),
                    &format!("q{index}"),
                    "true_false",
                    None,
                )
            })
            .collect();
        let short = plan(Some(date(30)), 10, questions.clone());
        let long = plan(Some(date(30)), 120, questions);
        assert!(long.coverage.selected >= short.coverage.selected);
        assert!(long.allocation.len() >= short.allocation.len());
    }

    #[test]
    fn a_realistic_multi_day_scenario_is_self_consistent() {
        let calendar = calendar();
        let mut questions = Vec::new();
        for field in 0..5 {
            for note in 0..4 {
                for item in 0..5 {
                    let state = if (field + note + item) % 3 == 0 {
                        None
                    } else {
                        Some(card(-(item as i64), u32::from(item == 0), 8.0))
                    };
                    questions.push(question(
                        &format!("W{field}"),
                        &format!("n{note}"),
                        &format!("q{note}-{item}"),
                        if item % 2 == 0 {
                            "true_false"
                        } else {
                            "short_text"
                        },
                        state,
                    ));
                }
            }
        }
        let plan = plan_exam(PlanInput {
            now: now(),
            calendar: &calendar,
            target_date: Some(date(17)),
            daily_minutes: 60,
            new_limit_remaining: 10,
            studied_seconds_today: 0,
            questions: &questions,
        });
        assert!(plan.coverage.total == 100);
        assert!(plan.coverage.selected > 0);
        assert!(plan.planned_seconds <= 3_600);
        assert_eq!(plan.coverage.selected_new, 10);
        assert!(
            plan.coverage.selected_new
                + plan.coverage.selected_due
                + plan.coverage.selected_weak
                + plan.coverage.selected_early
                <= plan.coverage.selected
        );
    }
}
