use workbook_domain::{CardState, Rating};

use crate::{GRADUATING_INTERVAL_DAYS, MAXIMUM_INTERVAL_DAYS};

#[derive(Clone, Debug, PartialEq)]
pub struct SchedulerSettings {
    pub desired_retention: f32,
    /// Anki-style learning steps in minutes. The legacy app uses 1, 10.
    pub learning_steps_minutes: Vec<u32>,
    /// Relearning steps in minutes. The legacy app uses 10.
    pub relearning_steps_minutes: Vec<u32>,
    pub graduating_interval_days: f64,
    pub maximum_interval_days: f64,
}

impl Default for SchedulerSettings {
    fn default() -> Self {
        Self {
            desired_retention: 0.9,
            learning_steps_minutes: vec![1, 10],
            relearning_steps_minutes: vec![10],
            graduating_interval_days: GRADUATING_INTERVAL_DAYS,
            maximum_interval_days: MAXIMUM_INTERVAL_DAYS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepDecision {
    pub state: CardState,
    pub scheduled_days: f64,
    pub learning_step: u32,
}

#[derive(Clone, Debug)]
pub struct StepPolicy {
    settings: SchedulerSettings,
}

impl StepPolicy {
    pub fn new(settings: SchedulerSettings) -> Self {
        Self { settings }
    }

    /// Matches ts-fsrs 6.0.0 `DefaultScheduler` with
    /// `allowModelOverride: false` and the legacy step lists. The FSRS memory
    /// state itself is always advanced by the model; this policy only decides
    /// the interval, card state, and learning step.
    pub fn decide(
        &self,
        input_state: CardState,
        current_step: u32,
        rating: Rating,
        model_interval_days: f64,
        graduating_interval_days: f64,
    ) -> StepDecision {
        let steps = if matches!(input_state, CardState::Review | CardState::Relearning) {
            &self.settings.relearning_steps_minutes
        } else {
            &self.settings.learning_steps_minutes
        };
        let step = step_interval(steps, input_state, current_step, rating);
        let has_steps = !steps.is_empty();
        let scheduled_days = match step {
            Some((days, _)) => days,
            None if has_steps => model_interval_days.max(graduating_interval_days),
            None => model_interval_days,
        }
        .clamp(0.0, self.settings.maximum_interval_days);

        let state = if scheduled_days < graduating_interval_days {
            next_learning_state(input_state)
        } else {
            CardState::Review
        };
        let learning_step = if scheduled_days < graduating_interval_days {
            step.map(|(_, next)| next).unwrap_or(0)
        } else {
            0
        };
        StepDecision {
            state,
            scheduled_days,
            learning_step,
        }
    }
}

/// Returns `(days until due, next learning step index)`.
fn step_interval(
    steps: &[u32],
    state: CardState,
    current_step: u32,
    rating: Rating,
) -> Option<(f64, u32)> {
    if steps.is_empty() || current_step as usize >= steps.len() {
        return None;
    }
    let current = current_step as usize;
    let first = f64::from(steps[0]);
    let second = steps.get(1).map(|value| f64::from(*value));
    let days = |minutes: f64| minutes / 1440.0;

    if state == CardState::Review {
        return (rating == Rating::Again).then_some((days(first), 0));
    }

    match rating {
        Rating::Again => Some((days(first), 0)),
        Rating::Hard => {
            let minutes = match second {
                Some(second) => (first + second) / 2.0,
                None => first * 1.5,
            };
            Some((days(minutes), current_step))
        }
        Rating::Good => steps
            .get(current + 1)
            .map(|minutes| (days(f64::from(*minutes)), current_step + 1)),
        Rating::Easy => None,
    }
}

pub fn next_learning_state(state: CardState) -> CardState {
    match state {
        CardState::New => CardState::Learning,
        CardState::Review => CardState::Relearning,
        CardState::Learning => CardState::Learning,
        CardState::Relearning => CardState::Relearning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> StepPolicy {
        StepPolicy::new(SchedulerSettings::default())
    }

    #[test]
    fn new_good_enters_the_ten_minute_step_and_graduation_takes_one_day() {
        let policy = policy();
        let first = policy.decide(CardState::New, 0, Rating::Good, 7.0 / 1440.0, 1.0);
        assert_eq!(first.state, CardState::Learning);
        assert_eq!(first.learning_step, 1);
        assert!((first.scheduled_days - 10.0 / 1440.0).abs() < 1e-12);

        let second = policy.decide(CardState::Learning, 1, Rating::Good, 0.005, 1.0);
        assert_eq!(second.state, CardState::Review);
        assert_eq!(second.learning_step, 0);
        assert_eq!(second.scheduled_days, 1.0);
    }

    #[test]
    fn new_again_is_one_minute_and_hard_is_five_and_a_half() {
        let policy = policy();
        let again = policy.decide(CardState::New, 0, Rating::Again, 0.0, 1.0);
        assert_eq!(again.state, CardState::Learning);
        assert!((again.scheduled_days - 1.0 / 1440.0).abs() < 1e-12);
        assert_eq!(again.learning_step, 0);

        let hard = policy.decide(CardState::New, 0, Rating::Hard, 0.0, 1.0);
        assert_eq!(hard.state, CardState::Learning);
        assert!((hard.scheduled_days - 5.5 / 1440.0).abs() < 1e-12);
        assert_eq!(hard.learning_step, 0);
    }

    #[test]
    fn new_easy_skips_learning_and_enters_review() {
        let decision = policy().decide(CardState::New, 0, Rating::Easy, 53.0, 1.0);
        assert_eq!(decision.state, CardState::Review);
        assert_eq!(decision.scheduled_days, 53.0);
        assert_eq!(decision.learning_step, 0);
    }

    #[test]
    fn review_again_enters_relearning_for_ten_minutes() {
        let decision = policy().decide(CardState::Review, 0, Rating::Again, 13.0, 1.0);
        assert_eq!(decision.state, CardState::Relearning);
        assert!((decision.scheduled_days - 10.0 / 1440.0).abs() < 1e-12);
        assert_eq!(decision.learning_step, 0);
    }

    #[test]
    fn relearning_good_graduates_but_hard_stays() {
        let policy = policy();
        let good = policy.decide(CardState::Relearning, 0, Rating::Good, 0.002, 1.0);
        assert_eq!(good.state, CardState::Review);
        assert_eq!(good.scheduled_days, 1.0);

        let hard = policy.decide(CardState::Relearning, 0, Rating::Hard, 0.002, 1.0);
        assert_eq!(hard.state, CardState::Relearning);
        assert!((hard.scheduled_days - 15.0 / 1440.0).abs() < 1e-12);
    }

    #[test]
    fn review_hard_and_good_stay_in_review_with_model_intervals() {
        let policy = policy();
        let hard = policy.decide(CardState::Review, 0, Rating::Hard, 12.0, 1.0);
        assert_eq!(hard.state, CardState::Review);
        assert_eq!(hard.scheduled_days, 12.0);
        let good = policy.decide(CardState::Review, 0, Rating::Good, 20.0, 1.0);
        assert_eq!(good.state, CardState::Review);
        assert_eq!(good.scheduled_days, 20.0);
    }

    #[test]
    fn maximum_interval_is_clamped() {
        let decision = policy().decide(CardState::Review, 0, Rating::Good, 50_000.0, 1.0);
        assert_eq!(decision.scheduled_days, 36_500.0);
    }
}
