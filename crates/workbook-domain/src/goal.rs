//! Goals are general study targets. A school exam, a certification, a language
//! test, or a personal project with no deadline all use the same model.
//!
//! An existing `Exam` is adapted into a goal instead of being migrated or
//! duplicated. Exam rows, mock results, and sessions remain untouched; the goal
//! list simply surfaces them alongside generic goals. This keeps old data
//! intact while giving the planner one shape to reason about.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::exam::{Exam, ExamScope, default_daily_minutes};
use crate::workbook::ValidationError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalSource {
    /// A row in the `goals` table.
    Goal,
    /// An existing exam shown as a goal. Mock exams remain exam-specific.
    Exam,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Goal {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_date: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_score: Option<f64>,
    #[serde(default = "default_daily_minutes")]
    pub daily_minutes: u32,
    pub scope: ExamScope,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<DateTime<Utc>>,
    /// `Some` only for the read-only adaptation of an existing exam.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exam_id: Option<String>,
}

impl Goal {
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    pub fn source(&self) -> GoalSource {
        if self.exam_id.is_some() {
            GoalSource::Exam
        } else {
            GoalSource::Goal
        }
    }

    pub fn from_exam(exam: &Exam) -> Self {
        Self {
            id: exam.id.clone(),
            title: exam.title.clone(),
            description: exam.description.clone(),
            target_date: exam.target_date,
            target_score: exam.target_score,
            daily_minutes: exam.daily_minutes,
            scope: exam.scope.clone(),
            created_at: exam.created_at,
            updated_at: exam.updated_at,
            archived_at: exam.archived_at,
            exam_id: Some(exam.id.clone()),
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        let title = self.title.trim();
        if title.is_empty() {
            return Err(ValidationError::new("title", "目標名を入力してください"));
        }
        if title.chars().count() > 200 {
            return Err(ValidationError::new(
                "title",
                "目標名は200文字以内にしてください",
            ));
        }
        if let Some(description) = self.description.as_deref()
            && description.chars().count() > 2_000
        {
            return Err(ValidationError::new(
                "description",
                "説明は2000文字以内にしてください",
            ));
        }
        if let Some(score) = self.target_score
            && (!score.is_finite() || score < 0.0)
        {
            return Err(ValidationError::new(
                "targetScore",
                "目標スコアは0以上の数値で指定してください",
            ));
        }
        if self.daily_minutes > 1_440 {
            return Err(ValidationError::new(
                "dailyMinutes",
                "1日の学習時間は0〜1440分で指定してください",
            ));
        }
        self.scope.validate_for_save()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GoalDraft {
    pub title: String,
    pub description: Option<String>,
    pub target_date: Option<NaiveDate>,
    pub target_score: Option<f64>,
    pub daily_minutes: u32,
    pub scope: ExamScope,
}

impl GoalDraft {
    pub fn into_goal(self, now: DateTime<Utc>) -> Goal {
        Goal {
            id: uuid::Uuid::new_v4().to_string(),
            title: self.title.trim().to_string(),
            description: self.description,
            target_date: self.target_date,
            target_score: self.target_score,
            daily_minutes: self.daily_minutes,
            scope: self.scope,
            created_at: now,
            updated_at: now,
            archived_at: None,
            exam_id: None,
        }
    }

    pub fn apply_to(self, goal: &mut Goal, now: DateTime<Utc>) {
        goal.title = self.title.trim().to_string();
        goal.description = self.description;
        goal.target_date = self.target_date;
        goal.target_score = self.target_score;
        goal.daily_minutes = self.daily_minutes;
        goal.scope = self.scope;
        goal.updated_at = now;
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        self.clone().into_goal(Utc::now()).validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exam::{ExamScope, QuestionRef};

    fn draft() -> GoalDraft {
        GoalDraft {
            title: "個人プロジェクト".to_string(),
            description: None,
            target_date: None,
            target_score: None,
            daily_minutes: 30,
            scope: ExamScope {
                include_questions: vec![QuestionRef::new("A", "q1")],
                ..Default::default()
            },
        }
    }

    #[test]
    fn a_goal_without_a_deadline_is_valid() {
        let goal = draft().into_goal(Utc::now());
        assert!(goal.target_date.is_none());
        assert!(goal.validate().is_ok());
        assert_eq!(goal.source(), GoalSource::Goal);
    }

    #[test]
    fn exams_are_adapted_without_becoming_a_second_record() {
        let exam = crate::ExamDraft {
            title: "資格試験".to_string(),
            description: None,
            target_date: None,
            target_score: Some(80.0),
            daily_minutes: 45,
            scope: ExamScope {
                include_questions: vec![QuestionRef::new("A", "q1")],
                ..Default::default()
            },
        }
        .into_exam(Utc::now());
        let goal = Goal::from_exam(&exam);
        assert_eq!(goal.id, exam.id);
        assert_eq!(goal.exam_id.as_deref(), Some(exam.id.as_str()));
        assert_eq!(goal.source(), GoalSource::Exam);
        assert_eq!(goal.daily_minutes, 45);
        assert!(goal.validate().is_ok());
    }

    #[test]
    fn goals_require_a_title_and_an_inclusion_rule() {
        let mut goal = draft().into_goal(Utc::now());
        goal.title = " ".to_string();
        assert!(goal.validate().is_err());
        goal.title = "目標".to_string();
        goal.scope = ExamScope::default();
        assert!(goal.validate().is_err());
    }

    #[test]
    fn daily_minutes_and_scores_are_bounded() {
        let mut goal = draft().into_goal(Utc::now());
        goal.daily_minutes = 1_441;
        assert!(goal.validate().is_err());
        goal.daily_minutes = 0;
        assert!(goal.validate().is_ok());
        goal.target_score = Some(-1.0);
        assert!(goal.validate().is_err());
    }
}
