//! Exam scope and persistence-neutral exam metadata.
//!
//! An exam never copies questions. It stores composable selection rules that
//! are resolved against the current workbook library whenever a session starts
//! or statistics are shown. This keeps question identity and learning history
//! stable across imports, reclassifications, and scope edits.

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::workbook::ValidationError;

/// A question identified by its workbook-local primary key. Workbook IDs are
/// the local SQLite IDs, not the authored JSON IDs, so two imported copies of
/// the same JSON remain independent.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionRef {
    pub workbook_id: String,
    pub question_id: String,
}

impl QuestionRef {
    pub fn new(workbook_id: impl Into<String>, question_id: impl Into<String>) -> Self {
        Self {
            workbook_id: workbook_id.into(),
            question_id: question_id.into(),
        }
    }

    fn is_valid(&self) -> bool {
        !self.workbook_id.trim().is_empty() && !self.question_id.trim().is_empty()
    }
}

/// One inclusion group. Conditions inside a group are ANDed:
/// `(workbook in workbook_ids) AND (workbook tag in tags)`.
///
/// Multiple values inside a condition are ORed, so
/// `workbook_ids = [A], tags = [X, Y]` means `A AND (X OR Y)`.
/// An empty list means "no restriction for this condition", but a group that
/// is entirely empty is invalid: an unrestricted group would accidentally
/// match the whole library.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExamScopeGroup {
    #[serde(default)]
    pub workbook_ids: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Topic IDs. Selecting a topic also includes its descendants; the service
    /// expands the hierarchy before resolution.
    #[serde(default)]
    pub topics: Vec<String>,
}

impl ExamScopeGroup {
    pub fn is_empty(&self) -> bool {
        self.workbook_ids.is_empty() && self.tags.is_empty() && self.topics.is_empty()
    }
}

/// Composable exam selection rules.
///
/// Resolution order is deliberately simple and explicit:
/// 1. A question matches when any inclusion group matches OR it is listed in
///    `include_questions`.
/// 2. Exclusions are applied afterwards by question, workbook, or workbook
///    tag.
/// 3. Results are deduplicated by `(workbook_id, question_id)` and sorted
///    deterministically.
///
/// An empty scope resolves to no questions. It never means "everything".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExamScope {
    #[serde(default)]
    pub groups: Vec<ExamScopeGroup>,
    #[serde(default)]
    pub include_questions: Vec<QuestionRef>,
    #[serde(default)]
    pub exclude_questions: Vec<QuestionRef>,
    #[serde(default)]
    pub exclude_workbooks: Vec<String>,
    #[serde(default)]
    pub exclude_tags: Vec<String>,
    #[serde(default)]
    pub exclude_topics: Vec<String>,
}

impl ExamScope {
    pub fn has_inclusion_rules(&self) -> bool {
        !self.groups.is_empty() || !self.include_questions.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        !self.has_inclusion_rules()
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        for (index, group) in self.groups.iter().enumerate() {
            if group.is_empty() {
                return Err(ValidationError::new(
                    format!("scope.groups[{index}]"),
                    "条件のないグループは作成できません",
                ));
            }
            for (i, workbook_id) in group.workbook_ids.iter().enumerate() {
                if workbook_id.trim().is_empty() {
                    return Err(ValidationError::new(
                        format!("scope.groups[{index}].workbookIds[{i}]"),
                        "Workbook IDが空です",
                    ));
                }
            }
            for (i, tag) in group.tags.iter().enumerate() {
                if tag.trim().is_empty() {
                    return Err(ValidationError::new(
                        format!("scope.groups[{index}].tags[{i}]"),
                        "タグが空です",
                    ));
                }
            }
            for (i, topic) in group.topics.iter().enumerate() {
                if topic.trim().is_empty() {
                    return Err(ValidationError::new(
                        format!("scope.groups[{index}].topics[{i}]"),
                        "Topic IDが空です",
                    ));
                }
            }
        }
        for (field, refs) in [
            ("scope.includeQuestions", &self.include_questions),
            ("scope.excludeQuestions", &self.exclude_questions),
        ] {
            for (index, reference) in refs.iter().enumerate() {
                if !reference.is_valid() {
                    return Err(ValidationError::new(
                        format!("{field}[{index}]"),
                        "問題参照が不正です",
                    ));
                }
            }
        }
        for (index, workbook_id) in self.exclude_workbooks.iter().enumerate() {
            if workbook_id.trim().is_empty() {
                return Err(ValidationError::new(
                    format!("scope.excludeWorkbooks[{index}]"),
                    "除外Workbook IDが空です",
                ));
            }
        }
        for (index, tag) in self.exclude_tags.iter().enumerate() {
            if tag.trim().is_empty() {
                return Err(ValidationError::new(
                    format!("scope.excludeTags[{index}]"),
                    "除外タグが空です",
                ));
            }
        }
        for (index, topic) in self.exclude_topics.iter().enumerate() {
            if topic.trim().is_empty() {
                return Err(ValidationError::new(
                    format!("scope.excludeTopics[{index}]"),
                    "除外Topic IDが空です",
                ));
            }
        }
        Ok(())
    }

    /// A saveable exam must select something, not merely exclude things.
    pub fn validate_for_save(&self) -> Result<(), ValidationError> {
        self.validate()?;
        if !self.has_inclusion_rules() {
            return Err(ValidationError::new(
                "scope",
                "Workbook・タグ・問題のいずれかを含める条件を指定してください",
            ));
        }
        Ok(())
    }
}

/// A question as seen by the scope resolver. The catalog is rebuilt from the
/// current library for every resolution; it is never persisted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExamCatalogQuestion {
    pub workbook_id: String,
    pub workbook_title: String,
    pub note_id: String,
    pub question_id: String,
    pub question_prompt: String,
    /// Question format used for a first-pass time estimate when no attempt
    /// history exists.
    pub question_kind: String,
    pub workbook_tags: Vec<String>,
    /// Stable topic IDs assigned to this question. Empty when none.
    pub topic_ids: Vec<String>,
}

impl ExamCatalogQuestion {
    pub fn reference(&self) -> QuestionRef {
        QuestionRef::new(self.workbook_id.clone(), self.question_id.clone())
    }

    fn has_tag(&self, tag: &str) -> bool {
        self.workbook_tags.iter().any(|value| value == tag)
    }

    fn has_topic(&self, topic_id: &str) -> bool {
        self.topic_ids.iter().any(|value| value == topic_id)
    }
}

/// Resolves a scope against a current catalog. Missing workbook/question
/// references are ignored; empty scopes resolve to an empty list.
pub fn resolve_scope(
    scope: &ExamScope,
    catalog: &[ExamCatalogQuestion],
) -> Vec<ExamCatalogQuestion> {
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for question in catalog {
        if !is_included(scope, question) || is_excluded(scope, question) {
            continue;
        }
        let key = (question.workbook_id.clone(), question.question_id.clone());
        if seen.insert(key) {
            output.push(question.clone());
        }
    }
    output.sort_by(|a, b| {
        a.workbook_id
            .cmp(&b.workbook_id)
            .then(a.note_id.cmp(&b.note_id))
            .then(a.question_id.cmp(&b.question_id))
    });
    output
}

fn is_included(scope: &ExamScope, question: &ExamCatalogQuestion) -> bool {
    if scope
        .groups
        .iter()
        .any(|group| group_matches(group, question))
    {
        return true;
    }
    scope
        .include_questions
        .iter()
        .any(|reference| reference_matches(reference, question))
}

fn is_excluded(scope: &ExamScope, question: &ExamCatalogQuestion) -> bool {
    scope
        .exclude_questions
        .iter()
        .any(|reference| reference_matches(reference, question))
        || scope
            .exclude_workbooks
            .iter()
            .any(|workbook_id| workbook_id == &question.workbook_id)
        || scope.exclude_tags.iter().any(|tag| question.has_tag(tag))
        || scope
            .exclude_topics
            .iter()
            .any(|topic| question.has_topic(topic))
}

fn group_matches(group: &ExamScopeGroup, question: &ExamCatalogQuestion) -> bool {
    if group.is_empty() {
        return false;
    }
    let workbook_matches = group.workbook_ids.is_empty()
        || group
            .workbook_ids
            .iter()
            .any(|workbook_id| workbook_id == &question.workbook_id);
    let tag_matches = group.tags.is_empty() || group.tags.iter().any(|tag| question.has_tag(tag));
    let topic_matches =
        group.topics.is_empty() || group.topics.iter().any(|topic| question.has_topic(topic));
    workbook_matches && tag_matches && topic_matches
}

fn reference_matches(reference: &QuestionRef, question: &ExamCatalogQuestion) -> bool {
    reference.workbook_id == question.workbook_id && reference.question_id == question.question_id
}

/// The three study modes an exam exposes. Review and Practice reuse the
/// ordinary session engine; Mock is a fixed-selection assessment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExamStudyMode {
    Review,
    Practice,
    Mock,
}

impl ExamStudyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Practice => "practice",
            Self::Mock => "mock",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "review" => Some(Self::Review),
            "practice" => Some(Self::Practice),
            "mock" => Some(Self::Mock),
            _ => None,
        }
    }
}

/// Practice selection strategies. The definitions are intentionally narrow so
/// the UI can explain them precisely.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PracticeStrategy {
    /// Every question currently in scope.
    All,
    /// Every question in scope in a shuffled order.
    Random,
    /// Questions with no recorded FSRS state.
    Unstudied,
    /// Questions with at least one recorded incorrect attempt.
    PreviouslyIncorrect,
    /// Questions with a lapse, difficulty >= 7, or at least one incorrect
    /// attempt. This is a transparent heuristic, not knowledge tracing.
    Weak,
}

impl PracticeStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Random => "random",
            Self::Unstudied => "unstudied",
            Self::PreviouslyIncorrect => "previously_incorrect",
            Self::Weak => "weak",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "すべて",
            Self::Random => "ランダム",
            Self::Unstudied => "未学習",
            Self::PreviouslyIncorrect => "以前に誤答",
            Self::Weak => "苦手",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MockOrder {
    Fixed,
    Random,
}

/// Options for a fixed-selection mock session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MockOptions {
    #[serde(default)]
    pub question_limit: Option<u32>,
    #[serde(default)]
    pub time_limit_seconds: Option<u32>,
    #[serde(default = "default_mock_order")]
    pub order: MockOrder,
}

fn default_mock_order() -> MockOrder {
    MockOrder::Fixed
}

impl Default for MockOptions {
    fn default() -> Self {
        Self {
            question_limit: None,
            time_limit_seconds: None,
            order: MockOrder::Fixed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PracticeOptions {
    pub strategy: PracticeStrategy,
    #[serde(default)]
    pub question_limit: Option<u32>,
}

impl Default for PracticeOptions {
    fn default() -> Self {
        Self {
            strategy: PracticeStrategy::All,
            question_limit: None,
        }
    }
}

/// A persisted exam. Every optional field really is optional: a personal
/// project can be an exam with only a title and scope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Exam {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_date: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_score: Option<f64>,
    /// The learner's intended study time per day. `0` is a valid rest day.
    #[serde(default = "default_daily_minutes")]
    pub daily_minutes: u32,
    pub scope: ExamScope,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<DateTime<Utc>>,
}

pub fn default_daily_minutes() -> u32 {
    30
}

impl Exam {
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        let title = self.title.trim();
        if title.is_empty() {
            return Err(ValidationError::new("title", "試験名を入力してください"));
        }
        if title.chars().count() > 200 {
            return Err(ValidationError::new(
                "title",
                "試験名は200文字以内にしてください",
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

/// Input accepted by the service when creating or updating an exam. Optional
/// values are normalized (blank strings become `None`) by the service.
#[derive(Clone, Debug, PartialEq)]
pub struct ExamDraft {
    pub title: String,
    pub description: Option<String>,
    pub target_date: Option<NaiveDate>,
    pub target_score: Option<f64>,
    pub daily_minutes: u32,
    pub scope: ExamScope,
}

impl ExamDraft {
    pub fn into_exam(self, now: DateTime<Utc>) -> Exam {
        let id = uuid::Uuid::new_v4().to_string();
        Exam {
            id,
            title: self.title.trim().to_string(),
            description: self.description,
            target_date: self.target_date,
            target_score: self.target_score,
            daily_minutes: self.daily_minutes,
            scope: self.scope,
            created_at: now,
            updated_at: now,
            archived_at: None,
        }
    }

    pub fn apply_to(self, exam: &mut Exam, now: DateTime<Utc>) {
        exam.title = self.title.trim().to_string();
        exam.description = self.description;
        exam.target_date = self.target_date;
        exam.target_score = self.target_score;
        exam.daily_minutes = self.daily_minutes;
        exam.scope = self.scope;
        exam.updated_at = now;
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        let exam = self.clone().into_exam(Utc::now());
        exam.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(workbook: &str, note: &str, id: &str, tags: &[&str]) -> ExamCatalogQuestion {
        ExamCatalogQuestion {
            workbook_id: workbook.to_string(),
            workbook_title: workbook.to_string(),
            note_id: note.to_string(),
            question_id: id.to_string(),
            question_prompt: format!("prompt {id}"),
            question_kind: "true_false".to_string(),
            workbook_tags: tags.iter().map(|tag| tag.to_string()).collect(),
            topic_ids: Vec::new(),
        }
    }

    fn catalog() -> Vec<ExamCatalogQuestion> {
        vec![
            question("A", "n1", "q1", &["electricity"]),
            question("A", "n1", "q2", &["electricity"]),
            question("A", "n2", "q3", &["chemistry"]),
            question("A", "n2", "q4", &["advanced"]),
            question("B", "n3", "q5", &["electricity"]),
            question("C", "n4", "q6", &["chemistry", "advanced"]),
        ]
    }

    fn ids(scope: &ExamScope) -> Vec<String> {
        resolve_scope(scope, &catalog())
            .into_iter()
            .map(|question| question.question_id)
            .collect()
    }

    #[test]
    fn workbook_inclusion_selects_the_whole_workbook() {
        let scope = ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: vec!["A".into()],
                tags: vec![],
                topics: Vec::new(),
            }],
            ..Default::default()
        };
        assert_eq!(ids(&scope), ["q1", "q2", "q3", "q4"]);
    }

    #[test]
    fn tag_inclusion_matches_workbook_tags() {
        let scope = ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: vec![],
                tags: vec!["electricity".into()],
                topics: Vec::new(),
            }],
            ..Default::default()
        };
        assert_eq!(ids(&scope), ["q1", "q2", "q5"]);
    }

    #[test]
    fn workbook_and_tag_are_anded_not_ored() {
        // A AND (electricity OR chemistry) must not accidentally include every
        // question in A, nor every electricity question in B.
        let scope = ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: vec!["A".into()],
                tags: vec!["electricity".into(), "chemistry".into()],
                topics: Vec::new(),
            }],
            ..Default::default()
        };
        assert_eq!(ids(&scope), ["q1", "q2", "q3"]);
    }

    #[test]
    fn multiple_groups_are_ored() {
        let scope = ExamScope {
            groups: vec![
                ExamScopeGroup {
                    workbook_ids: vec!["A".into()],
                    tags: vec!["chemistry".into()],
                    topics: Vec::new(),
                },
                ExamScopeGroup {
                    workbook_ids: vec!["B".into()],
                    tags: vec![],
                    topics: Vec::new(),
                },
            ],
            ..Default::default()
        };
        assert_eq!(ids(&scope), ["q3", "q5"]);
    }

    #[test]
    fn direct_question_inclusion_works_without_groups() {
        let scope = ExamScope {
            include_questions: vec![QuestionRef::new("A", "q4"), QuestionRef::new("C", "q6")],
            ..Default::default()
        };
        assert_eq!(ids(&scope), ["q4", "q6"]);
    }

    #[test]
    fn exclusions_are_applied_after_inclusion() {
        let scope = ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: vec!["A".into()],
                tags: vec![],
                topics: Vec::new(),
            }],
            include_questions: vec![QuestionRef::new("B", "q5")],
            exclude_questions: vec![QuestionRef::new("A", "q2")],
            exclude_workbooks: vec!["B".into()],
            exclude_tags: vec!["advanced".into()],
            exclude_topics: Vec::new(),
        };
        assert_eq!(ids(&scope), ["q1", "q3"]);
    }

    #[test]
    fn duplicate_questions_are_removed_by_stable_id() {
        let scope = ExamScope {
            groups: vec![
                ExamScopeGroup {
                    workbook_ids: vec!["A".into()],
                    tags: vec![],
                    topics: Vec::new(),
                },
                ExamScopeGroup {
                    workbook_ids: vec![],
                    tags: vec!["electricity".into()],
                    topics: Vec::new(),
                },
            ],
            include_questions: vec![QuestionRef::new("A", "q1")],
            ..Default::default()
        };
        assert_eq!(ids(&scope), ["q1", "q2", "q3", "q4", "q5"]);
    }

    #[test]
    fn an_empty_scope_matches_nothing_and_never_everything() {
        assert!(ids(&ExamScope::default()).is_empty());
        let exclusion_only = ExamScope {
            exclude_tags: vec!["advanced".into()],
            exclude_topics: Vec::new(),
            ..Default::default()
        };
        assert!(ids(&exclusion_only).is_empty());
    }

    #[test]
    fn missing_references_are_ignored_gracefully() {
        let scope = ExamScope {
            include_questions: vec![
                QuestionRef::new("missing", "q"),
                QuestionRef::new("A", "q1"),
            ],
            ..Default::default()
        };
        assert_eq!(ids(&scope), ["q1"]);
    }

    #[test]
    fn resolution_reflects_reclassification_without_changing_identity() {
        let scope = ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: vec![],
                tags: vec!["electricity".into()],
                topics: Vec::new(),
            }],
            ..Default::default()
        };
        assert_eq!(ids(&scope), ["q1", "q2", "q5"]);

        let mut changed = catalog();
        changed[2].workbook_tags.push("electricity".into());
        changed[4].workbook_tags.clear();
        let resolved: Vec<String> = resolve_scope(&scope, &changed)
            .into_iter()
            .map(|question| question.question_id)
            .collect();
        assert_eq!(resolved, ["q1", "q2", "q3"]);
    }

    #[test]
    fn empty_groups_are_ignored_by_the_resolver_and_rejected_by_validation() {
        let scope = ExamScope {
            groups: vec![ExamScopeGroup::default()],
            ..Default::default()
        };
        assert!(ids(&scope).is_empty());
        assert!(scope.validate().is_err());
    }

    #[test]
    fn scope_validation_rejects_malformed_references() {
        let scope = ExamScope {
            include_questions: vec![QuestionRef::new("", "q")],
            ..Default::default()
        };
        assert!(scope.validate().is_err());
        let scope = ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: vec![String::new()],
                tags: vec![],
                topics: Vec::new(),
            }],
            ..Default::default()
        };
        assert!(scope.validate().is_err());
    }

    #[test]
    fn exam_requires_a_title_and_at_least_one_inclusion_rule() {
        let mut exam = ExamDraft {
            title: " ".into(),
            description: None,
            target_date: None,
            target_score: None,
            daily_minutes: 30,
            scope: ExamScope::default(),
        }
        .into_exam(Utc::now());
        assert!(exam.validate().is_err());
        exam.title = "Final".into();
        assert!(exam.validate().is_err());
        exam.scope.groups.push(ExamScopeGroup {
            workbook_ids: vec!["A".into()],
            tags: vec![],
            topics: Vec::new(),
        });
        assert!(exam.validate().is_ok());
    }

    #[test]
    fn daily_minutes_are_bounded_and_optional_for_old_data() {
        let mut draft = ExamDraft {
            title: "Plan".into(),
            description: None,
            target_date: None,
            target_score: None,
            daily_minutes: 30,
            scope: ExamScope {
                include_questions: vec![QuestionRef::new("A", "q1")],
                ..Default::default()
            },
        };
        assert!(draft.clone().validate().is_ok());
        draft.daily_minutes = 1_441;
        assert!(draft.validate().is_err());
        let exam: Exam = serde_json::from_str(
            r#"{"id":"e","title":"t","targetDate":null,"targetScore":null,"scope":{"groups":[{"workbookIds":["A"],"tags":[]}],"includeQuestions":[],"excludeQuestions":[],"excludeWorkbooks":[],"excludeTags":[]},"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(exam.daily_minutes, default_daily_minutes());
    }

    #[test]
    fn topic_groups_are_anded_with_workbooks_and_exclusions_win() {
        let mut catalog = catalog();
        catalog[0].topic_ids = vec!["linear".into()];
        catalog[1].topic_ids = vec!["linear".into(), "algebra".into()];
        catalog[4].topic_ids = vec!["linear".into()];
        let scope = ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: vec!["A".into()],
                tags: Vec::new(),
                topics: vec!["algebra".into()],
            }],
            exclude_topics: vec!["linear".into()],
            ..Default::default()
        };
        let resolved = resolve_scope(&scope, &catalog);
        // q2 is the only A question in algebra, but it is also excluded by the
        // linear exclusion. Exclusions apply after inclusion.
        assert!(resolved.is_empty());
        let scope = ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: vec!["A".into()],
                tags: Vec::new(),
                topics: vec!["algebra".into()],
            }],
            ..Default::default()
        };
        let resolved: Vec<String> = resolve_scope(&scope, &catalog)
            .into_iter()
            .map(|question| question.question_id)
            .collect();
        assert_eq!(resolved, ["q2"]);
    }

    #[test]
    fn questions_without_topics_are_not_matched_by_a_topic_group() {
        let scope = ExamScope {
            groups: vec![ExamScopeGroup {
                workbook_ids: Vec::new(),
                tags: Vec::new(),
                topics: vec!["linear".into()],
            }],
            ..Default::default()
        };
        assert!(ids(&scope).is_empty());
    }

    #[test]
    fn optional_exam_fields_really_are_optional() {
        let exam = ExamDraft {
            title: "Personal project".into(),
            description: None,
            target_date: None,
            target_score: None,
            daily_minutes: 30,
            scope: ExamScope {
                include_questions: vec![QuestionRef::new("A", "q1")],
                ..Default::default()
            },
        }
        .into_exam(Utc::now());
        assert!(exam.target_date.is_none());
        assert!(exam.target_score.is_none());
        assert!(!exam.is_archived());
        assert!(exam.validate().is_ok());
    }
}
