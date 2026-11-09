use chrono::{DateTime, Utc};
use workbook_domain::{CardState, Mode, SessionItemStatus, StudyCalendar};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueQuestion {
    pub item_id: String,
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
    pub ordinal: u32,
    pub status: SessionItemStatus,
    pub state: Option<CardState>,
    pub due_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueKind {
    Initial,
    ShortTerm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueChoice {
    pub item_id: String,
    pub kind: QueueKind,
}

pub struct QueueInput<'a> {
    pub mode: Mode,
    pub workbook_id: &'a str,
    pub questions: &'a [QueueQuestion],
    pub now: DateTime<Utc>,
    pub calendar: &'a StudyCalendar,
}

/// Selects the next question from the *current* workbook. The selector is
/// recomputed after every answer; it never relies on a fixed queue snapshot.
///
/// Priority:
/// 1. Learning/Relearning cards whose due time has arrived.
/// 2. The next pending initial question in the selected order.
/// 3. When and only when both are empty, a short-term card due within 20
///    minutes on the same learning day (Learn Ahead).
pub fn select_next(input: QueueInput<'_>) -> Option<QueueChoice> {
    let mut questions: Vec<&QueueQuestion> = input
        .questions
        .iter()
        .filter(|question| question.workbook_id == input.workbook_id)
        .collect();
    questions.sort_by(|a, b| {
        a.due_at
            .cmp(&b.due_at)
            .then(a.ordinal.cmp(&b.ordinal))
            .then(a.question_id.cmp(&b.question_id))
            .then(a.item_id.cmp(&b.item_id))
    });

    if matches!(input.mode, Mode::Practice | Mode::Mock) {
        return questions
            .into_iter()
            .filter(|question| question.status == SessionItemStatus::Pending)
            .min_by(|a, b| a.ordinal.cmp(&b.ordinal))
            .map(|question| QueueChoice {
                item_id: question.item_id.clone(),
                kind: QueueKind::Initial,
            });
    }

    let short_term_due = questions
        .iter()
        .filter(|question| {
            question.status != SessionItemStatus::Skipped
                && question.state.is_some_and(CardState::is_short_term)
                && question.due_at.is_some_and(|due| due <= input.now)
        })
        .min_by(|a, b| {
            a.due_at
                .cmp(&b.due_at)
                .then(a.ordinal.cmp(&b.ordinal))
                .then(a.question_id.cmp(&b.question_id))
        });
    if let Some(question) = short_term_due {
        return Some(QueueChoice {
            item_id: question.item_id.clone(),
            kind: QueueKind::ShortTerm,
        });
    }

    let next_initial = questions
        .iter()
        .filter(|question| question.status == SessionItemStatus::Pending)
        .min_by(|a, b| a.ordinal.cmp(&b.ordinal));
    if let Some(question) = next_initial {
        return Some(QueueChoice {
            item_id: question.item_id.clone(),
            kind: QueueKind::Initial,
        });
    }

    let learn_ahead = questions
        .iter()
        .filter(|question| {
            question.status != SessionItemStatus::Skipped
                && question.state.is_some_and(CardState::is_short_term)
                && question
                    .due_at
                    .is_some_and(|due| input.calendar.learn_ahead_allowed(input.now, due))
        })
        .min_by(|a, b| {
            a.due_at
                .cmp(&b.due_at)
                .then(a.ordinal.cmp(&b.ordinal))
                .then(a.question_id.cmp(&b.question_id))
        });
    learn_ahead.map(|question| QueueChoice {
        item_id: question.item_id.clone(),
        kind: QueueKind::ShortTerm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use workbook_domain::SessionItemStatus;

    fn calendar() -> StudyCalendar {
        StudyCalendar::new("Asia/Tokyo").unwrap()
    }

    fn utc(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 2, 2, h, m, 0).unwrap()
    }

    fn question(
        id: &str,
        ordinal: u32,
        state: Option<CardState>,
        due_at: Option<DateTime<Utc>>,
    ) -> QueueQuestion {
        QueueQuestion {
            item_id: id.to_string(),
            workbook_id: "w".to_string(),
            note_id: format!("n{id}"),
            question_id: format!("q{id}"),
            ordinal,
            status: SessionItemStatus::Answered,
            state,
            due_at,
        }
    }

    fn select(now: DateTime<Utc>, mode: Mode, questions: &[QueueQuestion]) -> Option<QueueChoice> {
        let calendar = calendar();
        select_next(QueueInput {
            mode,
            workbook_id: "w",
            questions,
            now,
            calendar: &calendar,
        })
    }

    #[test]
    fn due_short_term_wins_over_pending_initial() {
        let now = utc(3, 0);
        let questions = vec![
            question(
                "a",
                0,
                Some(CardState::Learning),
                Some(now - Duration::minutes(1)),
            ),
            question("b", 1, None, None),
        ];
        let mut pending = questions.clone();
        pending[1].status = SessionItemStatus::Pending;
        let choice = select(now, Mode::New, &pending).unwrap();
        assert_eq!(choice.item_id, "a");
        assert_eq!(choice.kind, QueueKind::ShortTerm);
    }

    #[test]
    fn pending_initial_blocks_learn_ahead_until_it_is_empty() {
        let now = utc(3, 0);
        let mut questions = vec![
            question(
                "a",
                0,
                Some(CardState::Learning),
                Some(now + Duration::minutes(10)),
            ),
            question("b", 1, None, None),
        ];
        questions[1].status = SessionItemStatus::Pending;
        let choice = select(now, Mode::New, &questions).unwrap();
        assert_eq!(choice.item_id, "b");
        assert_eq!(choice.kind, QueueKind::Initial);
    }

    #[test]
    fn learn_ahead_boundary_is_exactly_twenty_minutes() {
        let now = utc(3, 0);
        for (offset, expected) in [
            (Duration::minutes(19) + Duration::seconds(59), true),
            (Duration::minutes(20), true),
            (Duration::minutes(20) + Duration::seconds(1), false),
        ] {
            let questions = vec![question(
                "a",
                0,
                Some(CardState::Learning),
                Some(now + offset),
            )];
            assert_eq!(
                select(now, Mode::New, &questions).is_some(),
                expected,
                "offset {offset:?}"
            );
        }
    }

    #[test]
    fn learn_ahead_does_not_cross_the_four_am_boundary() {
        // 03:55 JST, due 04:05 JST. Both are UTC 18:55/19:05 on the prior date.
        let now = Utc.with_ymd_and_hms(2026, 2, 1, 18, 55, 0).unwrap();
        let due = Utc.with_ymd_and_hms(2026, 2, 1, 19, 5, 0).unwrap();
        let questions = vec![question("a", 0, Some(CardState::Relearning), Some(due))];
        assert!(select(now, Mode::Review, &questions).is_none());
    }

    #[test]
    fn practice_never_selects_short_term_repeats() {
        let now = utc(3, 0);
        let questions = vec![question(
            "a",
            0,
            Some(CardState::Learning),
            Some(now + Duration::minutes(1)),
        )];
        assert!(select(now, Mode::Practice, &questions).is_none());
    }

    #[test]
    fn oldest_due_short_term_is_selected_first() {
        let now = utc(3, 0);
        let questions = vec![
            question(
                "b",
                1,
                Some(CardState::Learning),
                Some(now - Duration::minutes(2)),
            ),
            question(
                "a",
                0,
                Some(CardState::Relearning),
                Some(now - Duration::minutes(5)),
            ),
        ];
        assert_eq!(select(now, Mode::New, &questions).unwrap().item_id, "a");
    }

    #[test]
    fn one_question_session_can_be_reissued_immediately() {
        let now = utc(3, 0);
        let questions = vec![question(
            "only",
            0,
            Some(CardState::Learning),
            Some(now + Duration::minutes(1)),
        )];
        let choice = select(now, Mode::New, &questions).unwrap();
        assert_eq!(choice.item_id, "only");
        assert_eq!(choice.kind, QueueKind::ShortTerm);
    }
}
