//! FSRS-7 adapter, Anki-style short-term steps, Learn Ahead, and queue
//! selection. This crate knows nothing about GPUI or SQLite.

pub mod clock;
pub mod planner;
pub mod queue;
pub mod step_policy;

pub use clock::{Clock, FixedClock, SystemClock};
pub use planner::{
    ExamPlan, FieldAllocation, PlanCard, PlanCoverage, PlanFeasibility, PlanInput, PlanQuestion,
    PlanReason, PlanWarning, PlannedQuestion, estimated_seconds, exam_deadline, plan_exam,
};
pub use queue::{QueueChoice, QueueInput, QueueKind, QueueQuestion, select_next};
pub use step_policy::{SchedulerSettings, StepPolicy, next_learning_state};

pub const ALGORITHM: &str = "FSRS-7";
pub const POLICY_VERSION: &str = "workbook2-step-v1";
pub const MAXIMUM_INTERVAL_DAYS: f64 = 36_500.0;
pub const GRADUATING_INTERVAL_DAYS: f64 = 1.0;

use chrono::{DateTime, Duration, Utc};
use fsrs::{FSRS, ItemState, MemoryState};
use thiserror::Error;
use workbook_domain::{CardState, Rating, StoredCard, StudyCalendar};

#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("FSRS計算に失敗しました: {0}")]
    Fsrs(#[from] fsrs::FSRSError),
    #[error("目標定着率が不正です")]
    Retention,
}

#[derive(Clone, Debug)]
pub struct Scheduler {
    fsrs: FSRS,
    settings: SchedulerSettings,
}

impl Scheduler {
    pub fn new(settings: SchedulerSettings) -> Result<Self, SchedulerError> {
        if !(0.70..=0.97).contains(&settings.desired_retention) {
            return Err(SchedulerError::Retention);
        }
        Ok(Self {
            fsrs: FSRS::new(&[])?,
            settings,
        })
    }

    pub fn settings(&self) -> &SchedulerSettings {
        &self.settings
    }

    pub fn with_retention(&self, desired_retention: f32) -> Result<Self, SchedulerError> {
        let mut settings = self.settings.clone();
        settings.desired_retention = desired_retention;
        Self::new(settings)
    }

    /// Estimated probability of recalling a saved card at `at`, assuming no
    /// further reviews. The forgetting curve stays inside `fsrs-rs`; the
    /// planner only consumes the number.
    pub fn retrievability_at(
        &self,
        card: &StoredCard,
        now: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> f32 {
        let anchor = card.last_review_at.unwrap_or(now);
        let days = ((at - anchor).num_milliseconds() as f64 / 86_400_000.0).max(0.0) as f32;
        self.fsrs.current_retrievability(
            MemoryState {
                stability: card.stability,
                difficulty: card.difficulty,
                stability_fast: card.stability_fast,
            },
            days,
        )
    }

    /// Applies one confirmed answer. `before` is the saved state before the
    /// answer, or `None` for a question that has never been reviewed.
    pub fn review(
        &self,
        before: Option<&StoredCard>,
        rating: Rating,
        now: DateTime<Utc>,
    ) -> Result<StoredCard, SchedulerError> {
        let elapsed_days = before
            .and_then(|card| card.last_review_at)
            .map(|last| ((now - last).num_milliseconds() as f64 / 86_400_000.0).max(0.0) as f32)
            .unwrap_or(0.0);
        let current_memory = before.map(|card| MemoryState {
            stability: card.stability,
            difficulty: card.difficulty,
            stability_fast: card.stability_fast,
        });
        let states = self.fsrs.next_states_with_elapsed_days(
            current_memory,
            self.settings.desired_retention,
            elapsed_days,
        )?;
        let item = pick_state(&states, rating);
        let model_interval = item.interval as f64;

        let input_state = before.map(|card| card.state).unwrap_or(CardState::New);
        let learning_step = before.map(|card| card.learning_step).unwrap_or(0);
        let policy = StepPolicy::new(self.settings.clone());
        let decision = policy.decide(
            input_state,
            learning_step,
            rating,
            model_interval,
            grading::graduating_interval(),
        );

        let due_at = now + duration_from_days(decision.scheduled_days);
        let previous_lapses = before.map(|card| card.lapses).unwrap_or(0);
        let lapses = previous_lapses
            + u32::from(input_state == CardState::Review && rating == Rating::Again);
        let reps = before.map(|card| card.reps).unwrap_or(0) + 1;

        Ok(StoredCard {
            stability: item.memory.stability,
            stability_fast: item.memory.stability_fast,
            difficulty: item.memory.difficulty,
            state: decision.state,
            due_at,
            last_review_at: Some(now),
            reps,
            lapses,
            scheduled_days: decision.scheduled_days,
            learning_step: decision.learning_step,
            algorithm: ALGORITHM.to_string(),
            policy_version: POLICY_VERSION.to_string(),
            revision: before.map(|card| card.revision + 1).unwrap_or(1),
        })
    }
}

fn pick_state(states: &fsrs::NextStates, rating: Rating) -> &ItemState {
    match rating {
        Rating::Again => &states.again,
        Rating::Hard => &states.hard,
        Rating::Good => &states.good,
        Rating::Easy => &states.easy,
    }
}

pub fn duration_from_days(days: f64) -> Duration {
    Duration::milliseconds((days * 86_400_000.0).round() as i64)
}

mod grading {
    pub fn graduating_interval() -> f64 {
        super::GRADUATING_INTERVAL_DAYS
    }
}

/// The queue selector needs only the 4 a.m. calendar, not the FSRS model.
pub fn learn_ahead_allowed(
    calendar: &StudyCalendar,
    now: DateTime<Utc>,
    due_at: DateTime<Utc>,
) -> bool {
    calendar.learn_ahead_allowed(now, due_at)
}

#[cfg(test)]
mod golden {
    use super::*;
    use serde::Deserialize;
    use std::fs;

    #[derive(Deserialize)]
    struct Fixture {
        settings: FixtureSettings,
        scenarios: Vec<Scenario>,
    }

    #[derive(Deserialize)]
    struct FixtureSettings {
        #[serde(rename = "desiredRetention")]
        desired_retention: f32,
        #[serde(rename = "learningSteps")]
        learning_steps: Vec<String>,
        #[serde(rename = "relearningSteps")]
        relearning_steps: Vec<String>,
    }

    #[derive(Deserialize)]
    struct Scenario {
        name: String,
        steps: Vec<Step>,
    }

    #[derive(Deserialize)]
    struct Step {
        at: String,
        rating: u8,
        card: FixtureCard,
    }

    #[derive(Deserialize)]
    struct FixtureCard {
        stability: f32,
        stability_fast: f32,
        difficulty: f32,
        state: u8,
        due_at: String,
        last_review_at: Option<String>,
        scheduled_days: f64,
        learning_step: u32,
        reps: u32,
        lapses: u32,
    }

    fn minutes(steps: &[String]) -> Vec<u32> {
        steps
            .iter()
            .map(|step| step.trim_end_matches('m').parse::<u32>().unwrap())
            .collect()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 2e-4_f32.max(b.abs() * 2e-4)
    }

    fn close_f64(a: f64, b: f64) -> bool {
        (a - b).abs() <= 2e-4_f64.max(b.abs() * 2e-4)
    }

    #[test]
    fn matches_ts_fsrs_6_0_0_fsrs7_golden_fixtures() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/legacy-fsrs7-golden.json"
        );
        let raw = fs::read_to_string(path).expect("golden fixture must exist");
        let fixture: Fixture = serde_json::from_str(&raw).unwrap();
        let settings = SchedulerSettings {
            desired_retention: fixture.settings.desired_retention,
            learning_steps_minutes: minutes(&fixture.settings.learning_steps),
            relearning_steps_minutes: minutes(&fixture.settings.relearning_steps),
            graduating_interval_days: GRADUATING_INTERVAL_DAYS,
            maximum_interval_days: MAXIMUM_INTERVAL_DAYS,
        };
        let scheduler = Scheduler::new(settings).unwrap();

        for scenario in fixture.scenarios {
            let mut previous: Option<StoredCard> = None;
            for (index, step) in scenario.steps.iter().enumerate().skip(1) {
                let at = DateTime::parse_from_rfc3339(&step.at)
                    .unwrap()
                    .with_timezone(&Utc);
                let rating = Rating::from_value(step.rating).unwrap();
                let actual = scheduler.review(previous.as_ref(), rating, at).unwrap();
                let expected = &step.card;
                let where_ = format!("{} step {index}", scenario.name);
                assert!(
                    close(actual.stability, expected.stability),
                    "{where_}: stability {} != {}",
                    actual.stability,
                    expected.stability
                );
                assert!(
                    close(actual.stability_fast, expected.stability_fast),
                    "{where_}: stability_fast {} != {}",
                    actual.stability_fast,
                    expected.stability_fast
                );
                assert!(
                    close(actual.difficulty, expected.difficulty),
                    "{where_}: difficulty {} != {}",
                    actual.difficulty,
                    expected.difficulty
                );
                assert_eq!(actual.state.value(), expected.state, "{where_}: state");
                assert_eq!(
                    actual.learning_step, expected.learning_step,
                    "{where_}: learning_step"
                );
                assert_eq!(actual.reps, expected.reps, "{where_}: reps");
                assert_eq!(actual.lapses, expected.lapses, "{where_}: lapses");
                assert!(
                    close_f64(actual.scheduled_days, expected.scheduled_days),
                    "{where_}: scheduled_days {} != {}",
                    actual.scheduled_days,
                    expected.scheduled_days
                );
                let expected_due = DateTime::parse_from_rfc3339(&expected.due_at)
                    .unwrap()
                    .with_timezone(&Utc);
                let difference = (actual.due_at - expected_due).num_milliseconds().abs();
                // fsrs-rs computes intervals in f32 while ts-fsrs uses f64 JS
                // arithmetic. The difference is relative and grows with long
                // intervals; assert a 0.1% band (plus 5s) and record the
                // observed mismatch in docs/COMPATIBILITY.md rather than
                // pretending the floating-point implementations are bit-equal.
                let tolerance =
                    5_000_i64 + (expected.scheduled_days * 86_400_000.0 * 0.001).round() as i64;
                assert!(
                    difference <= tolerance,
                    "{where_}: due_at {} != {} ({difference}ms > {tolerance}ms)",
                    actual.due_at,
                    expected_due
                );
                let expected_last = expected.last_review_at.as_ref().map(|value| {
                    DateTime::parse_from_rfc3339(value)
                        .unwrap()
                        .with_timezone(&Utc)
                });
                assert_eq!(
                    actual.last_review_at, expected_last,
                    "{where_}: last_review_at"
                );
                previous = Some(actual);
            }
        }
    }
}
