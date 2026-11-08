//! Structured grading and assisted grading ("near miss").
//!
//! The old `grade_answer` API returns `Option<bool>` and is kept for
//! compatibility. `grade_answer_detailed` adds the states the Study screen
//! needs: correct, near miss, incorrect, and manual. A near miss is never a
//! final answer and never reaches FSRS.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::answer::{Answer, normalize_answer};
use crate::question_format::{
    HotspotAnswerSpec, LabelingAnswerSpec, MatchingAnswerSpec, NumericAnswerSpec,
    OrderingAnswerSpec, known_unit, normalize_unit, parse_numeric_answer,
};
use crate::workbook::{Question, QuestionType};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradeStatus {
    Correct,
    NearMiss,
    Incorrect,
    Manual,
}

impl GradeStatus {
    pub fn is_correct(self) -> bool {
        self == GradeStatus::Correct
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NearMissReason {
    SignificantFigures,
    MissingUnit,
    WrongUnit,
    EquivalentExpression,
    IncompleteAnswer,
    MinorTypo,
    FormatError,
}

impl NearMissReason {
    pub fn message(self) -> &'static str {
        match self {
            NearMissReason::SignificantFigures => {
                "おしい！ 数値は合っています。有効数字の桁数を確認してみましょう。"
            }
            NearMissReason::MissingUnit => {
                "おしい！ 単位が足りないようです。問題文で指定された単位を確認してみましょう。"
            }
            NearMissReason::WrongUnit => {
                "おしい！ 数値は近いようです。単位が問題に合っているか確認してみましょう。"
            }
            NearMissReason::EquivalentExpression => {
                "値は同じですが、問題で指定された単位や表し方に合わせてみましょう。"
            }
            NearMissReason::IncompleteAnswer => {
                "おしい！ まだ埋まっていない項目があります。すべて埋めてから回答しましょう。"
            }
            NearMissReason::MinorTypo => {
                "おしい！ もう少しで合っています。綴りや表記を確認してみましょう。"
            }
            NearMissReason::FormatError => {
                "回答の形式を確認してみましょう。数値や単位の書き方を見直してください。"
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GradeDiagnostic {
    /// Number of parts (pairs, positions, items) that matched.
    pub matched: usize,
    /// Total number of parts the question asks for.
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GradeResult {
    pub status: GradeStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<NearMissReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<GradeDiagnostic>,
}

impl GradeResult {
    pub fn correct() -> Self {
        Self {
            status: GradeStatus::Correct,
            reasons: Vec::new(),
            diagnostic: None,
        }
    }

    pub fn incorrect() -> Self {
        Self {
            status: GradeStatus::Incorrect,
            reasons: Vec::new(),
            diagnostic: None,
        }
    }

    pub fn manual() -> Self {
        Self {
            status: GradeStatus::Manual,
            reasons: Vec::new(),
            diagnostic: None,
        }
    }

    pub fn near_miss(reason: NearMissReason) -> Self {
        Self {
            status: GradeStatus::NearMiss,
            reasons: vec![reason],
            diagnostic: None,
        }
    }

    pub fn near_miss_with(reason: NearMissReason, diagnostic: GradeDiagnostic) -> Self {
        Self {
            status: GradeStatus::NearMiss,
            reasons: vec![reason],
            diagnostic: Some(diagnostic),
        }
    }

    pub fn with_diagnostic(mut self, matched: usize, total: usize) -> Self {
        self.diagnostic = Some(GradeDiagnostic { matched, total });
        self
    }

    pub fn messages(&self) -> Vec<&'static str> {
        self.reasons.iter().map(|reason| reason.message()).collect()
    }
}

/// A near miss that the learner chose to retry. Kept with the attempt so the
/// assistance is reproducible in history and backup/restore.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NearMissRecord {
    pub answer: Option<Answer>,
    pub result: GradeResult,
    pub answered_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssistState {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<NearMissRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<GradeResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_grade: Option<GradeResult>,
}

impl AssistState {
    pub fn assisted(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn retry_count(&self) -> usize {
        self.history.len()
    }

    pub fn from_json(raw: Option<&str>) -> Self {
        raw.and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default()
    }
}

/// Detailed grading. `None` means the question cannot be scored automatically.
pub fn grade_answer_detailed(question: &Question, answer: Option<&Answer>) -> Option<GradeResult> {
    match question.kind {
        QuestionType::SingleChoice => {
            let expected = question.single_choice_answer()?;
            Some(match answer {
                Some(Answer::Text(value)) if value == expected => GradeResult::correct(),
                _ => GradeResult::incorrect(),
            })
        }
        QuestionType::TrueFalse => {
            let expected = question.answer.as_ref()?.as_bool()?;
            Some(match answer {
                Some(Answer::Bool(value)) if *value == expected => GradeResult::correct(),
                _ => GradeResult::incorrect(),
            })
        }
        QuestionType::MultipleChoice => {
            let expected = question.multiple_choice_answers();
            Some(match answer {
                Some(Answer::Choices(values)) => {
                    let mut values = values.clone();
                    values.sort();
                    values.dedup();
                    let mut expected = expected;
                    expected.sort();
                    if values == expected {
                        GradeResult::correct()
                    } else {
                        let matched = values.iter().filter(|id| expected.contains(id)).count();
                        GradeResult::incorrect().with_diagnostic(matched, expected.len())
                    }
                }
                _ => GradeResult::incorrect(),
            })
        }
        QuestionType::ShortText => {
            let accepted = question.short_text_answers();
            Some(match answer {
                Some(Answer::Text(value)) => {
                    let value = normalize_answer(value);
                    if accepted
                        .iter()
                        .any(|candidate| normalize_answer(candidate) == value)
                    {
                        GradeResult::correct()
                    } else {
                        GradeResult::incorrect()
                    }
                }
                _ => GradeResult::incorrect(),
            })
        }
        QuestionType::Cloze => {
            let expected = question.cloze_blanks();
            Some(match answer {
                Some(Answer::Blanks(values)) => {
                    let mut matched = 0;
                    for (id, accepted) in &expected {
                        let is_match = values.get(id).is_some_and(|value| {
                            let value = normalize_answer(value);
                            accepted
                                .iter()
                                .any(|candidate| normalize_answer(candidate) == value)
                        });
                        if is_match {
                            matched += 1;
                        }
                    }
                    if matched == expected.len() {
                        GradeResult::correct()
                    } else {
                        GradeResult::incorrect().with_diagnostic(matched, expected.len())
                    }
                }
                _ => GradeResult::incorrect(),
            })
        }
        QuestionType::Flashcard => None,
        QuestionType::Numeric => Some(grade_numeric(question, answer)),
        QuestionType::Ordering => Some(grade_ordering(question, answer)),
        QuestionType::Matching => Some(grade_matching(question, answer)),
        QuestionType::Hotspot => Some(grade_hotspot(question, answer)),
        QuestionType::Labeling => Some(grade_labeling(question, answer)),
    }
}

fn numeric_spec(question: &Question) -> Option<NumericAnswerSpec> {
    serde_json::from_value(question.answer.clone()?).ok()
}

fn ordering_spec(question: &Question) -> Option<OrderingAnswerSpec> {
    serde_json::from_value(question.answer.clone()?).ok()
}

fn matching_spec(question: &Question) -> Option<MatchingAnswerSpec> {
    serde_json::from_value(question.answer.clone()?).ok()
}

fn hotspot_spec(question: &Question) -> Option<HotspotAnswerSpec> {
    serde_json::from_value(question.answer.clone()?).ok()
}

fn labeling_spec(question: &Question) -> Option<LabelingAnswerSpec> {
    serde_json::from_value(question.answer.clone()?).ok()
}

fn grade_numeric(question: &Question, answer: Option<&Answer>) -> GradeResult {
    let Some(spec) = numeric_spec(question) else {
        return GradeResult::incorrect();
    };
    let Ok(expected) = spec.value.value() else {
        return GradeResult::incorrect();
    };
    let Some(Answer::Text(raw)) = answer else {
        return GradeResult::incorrect();
    };
    let parsed = match parse_numeric_answer(raw) {
        Ok(parsed) => parsed,
        Err(_) => return GradeResult::near_miss(NearMissReason::FormatError),
    };
    let expected_unit = spec.unit.as_ref().map(|unit| normalize_unit(unit));
    let input_unit = parsed.unit.as_ref().map(|unit| normalize_unit(unit));
    let accepted_units: BTreeMap<String, f64> = spec
        .accepted_units
        .iter()
        .map(|(unit, factor)| (normalize_unit(unit), *factor))
        .collect();
    let raw_tolerance = spec.tolerance.unwrap_or_else(|| {
        let scale = expected.abs().max(1.0);
        scale * 1e-9
    });

    // `known_unit` gives a factor into the shared base unit of a dimension
    // (m, kg, s, A, ...). `acceptedUnits` is authored relative to the expected
    // unit, so an accepted value is first converted into the expected unit and
    // then into the base unit. When the expected unit is unknown there is no
    // shared base, so the comparison stays in expected-unit space.
    let expected_known = expected_unit.as_deref().and_then(known_unit);
    let base_factor = expected_known.map(|(_, factor)| factor);
    let space_factor = base_factor.unwrap_or(1.0);
    let mut expected_compare = expected * space_factor;
    let mut tolerance = raw_tolerance * space_factor;
    let mut converted = parsed.value * space_factor;
    let mut unit_issue: Option<NearMissReason> = None;

    match expected_unit.as_deref() {
        Some(expected_unit) => match input_unit.as_deref() {
            None => {
                if spec.require_unit {
                    unit_issue = Some(NearMissReason::MissingUnit);
                }
            }
            Some(unit) if unit == expected_unit => {}
            Some(unit) => {
                if let Some(factor) = accepted_units.get(unit).copied() {
                    // The factor maps the accepted unit onto the expected unit.
                    converted = parsed.value * factor * space_factor;
                } else if let Some((expected_dimension, _)) = expected_known
                    && let Some((input_dimension, input_factor)) = known_unit(unit)
                    && expected_dimension == input_dimension
                {
                    converted = parsed.value * input_factor;
                    unit_issue = Some(NearMissReason::EquivalentExpression);
                } else {
                    // Different dimensions or an unknown unit: the number can
                    // still be "near" when it matches the expected number, but
                    // the units are not physically comparable.
                    expected_compare = expected;
                    tolerance = raw_tolerance;
                    converted = parsed.value;
                    unit_issue = Some(NearMissReason::WrongUnit);
                }
            }
        },
        None => {
            if let Some(unit) = input_unit.as_deref() {
                if let Some(factor) = accepted_units.get(unit).copied() {
                    converted = parsed.value * factor;
                } else {
                    expected_compare = expected;
                    tolerance = raw_tolerance;
                    converted = parsed.value;
                    unit_issue = Some(NearMissReason::WrongUnit);
                }
            }
        }
    }

    // The explicit tolerance is in expected-unit terms. Normalizing it with the
    // same factor as the values keeps e.g. a 0.01 cm tolerance meaningful when
    // the learner answers in metres. A small relative epsilon absorbs binary
    // floating-point noise in the conversion itself.
    let epsilon = expected_compare.abs().max(1.0) * 1e-12;
    let numeric_matches = (converted - expected_compare).abs() <= tolerance + epsilon;
    if !numeric_matches {
        return GradeResult::incorrect();
    }

    if let Some(reason) = unit_issue {
        // A missing/wrong unit can still be "near" when the number itself is
        // right; that is exactly the assisted-grading case. `requireUnit` only
        // decides whether a missing unit is acceptable, not whether a unit of
        // the wrong dimension is equivalent.
        return GradeResult::near_miss(reason);
    }

    if let Some(required) = spec.significant_figures
        && parsed.significant_figures != required
    {
        return GradeResult::near_miss_with(
            NearMissReason::SignificantFigures,
            GradeDiagnostic {
                matched: 1,
                total: 1,
            },
        );
    }

    let _ = parsed.fraction;
    GradeResult::correct()
}

fn grade_ordering(question: &Question, answer: Option<&Answer>) -> GradeResult {
    let Some(spec) = ordering_spec(question) else {
        return GradeResult::incorrect();
    };
    let option_ids: BTreeSet<String> = question
        .options
        .as_ref()
        .map(|options| options.iter().map(|option| option.id.clone()).collect())
        .unwrap_or_default();
    let Some(Answer::Ordering { order }) = answer else {
        return GradeResult::incorrect();
    };
    if order.iter().any(|id| !option_ids.contains(id))
        || order.iter().collect::<BTreeSet<_>>().len() != order.len()
    {
        return GradeResult::near_miss(NearMissReason::FormatError);
    }
    if order.len() < option_ids.len() {
        return GradeResult::near_miss_with(
            NearMissReason::IncompleteAnswer,
            GradeDiagnostic {
                matched: order.len(),
                total: option_ids.len(),
            },
        );
    }
    let mut best = 0;
    for accepted in &spec.orders {
        if order == accepted {
            return GradeResult::correct();
        }
        let matched = order
            .iter()
            .zip(accepted.iter())
            .filter(|(given, expected)| given == expected)
            .count();
        best = best.max(matched);
    }
    GradeResult::incorrect().with_diagnostic(best, option_ids.len())
}

fn grade_matching(question: &Question, answer: Option<&Answer>) -> GradeResult {
    let Some(spec) = matching_spec(question) else {
        return GradeResult::incorrect();
    };
    let left: BTreeSet<String> = question
        .matching
        .as_ref()
        .map(|matching| matching.left.iter().map(|item| item.id.clone()).collect())
        .unwrap_or_default();
    let right: BTreeSet<String> = question
        .matching
        .as_ref()
        .map(|matching| matching.right.iter().map(|item| item.id.clone()).collect())
        .unwrap_or_default();
    let Some(Answer::Matching { matches }) = answer else {
        return GradeResult::incorrect();
    };
    if matches.keys().any(|id| !left.contains(id))
        || matches.values().any(|id| !right.contains(id))
        || matches.values().collect::<BTreeSet<_>>().len() != matches.len()
    {
        return GradeResult::near_miss(NearMissReason::FormatError);
    }
    if matches.len() < left.len() {
        return GradeResult::near_miss_with(
            NearMissReason::IncompleteAnswer,
            GradeDiagnostic {
                matched: matches.len(),
                total: left.len(),
            },
        );
    }
    let mut best = 0;
    for accepted in spec.accepted() {
        let expected: BTreeMap<&str, &str> = accepted
            .iter()
            .map(|pair| (pair.left.as_str(), pair.right.as_str()))
            .collect();
        if expected.len() == matches.len()
            && matches
                .iter()
                .all(|(left, right)| expected.get(left.as_str()) == Some(&right.as_str()))
        {
            return GradeResult::correct();
        }
        let matched = matches
            .iter()
            .filter(|(left, right)| expected.get(left.as_str()) == Some(&right.as_str()))
            .count();
        best = best.max(matched);
    }
    GradeResult::incorrect().with_diagnostic(best, left.len())
}

fn grade_hotspot(question: &Question, answer: Option<&Answer>) -> GradeResult {
    let Some(spec) = hotspot_spec(question) else {
        return GradeResult::incorrect();
    };
    let Some(hotspot) = question.hotspot.as_ref() else {
        return GradeResult::incorrect();
    };
    let Some(Answer::Hotspot { x, y }) = answer else {
        return GradeResult::incorrect();
    };
    if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y) {
        return GradeResult::near_miss(NearMissReason::FormatError);
    }
    let accepted: BTreeSet<&str> = spec.region_ids.iter().map(String::as_str).collect();
    // Regions may overlap. Grading only looks at the point, not at array order,
    // so a click inside any accepted region is correct even when an earlier
    // non-accepted region also contains it. A point that only lands in
    // non-accepted regions (or in no region) stays incorrect.
    if hotspot
        .regions
        .iter()
        .any(|region| region.shape.contains(*x, *y) && accepted.contains(region.id.as_str()))
    {
        GradeResult::correct()
    } else {
        GradeResult::incorrect()
    }
}

fn grade_labeling(question: &Question, answer: Option<&Answer>) -> GradeResult {
    let Some(spec) = labeling_spec(question) else {
        return GradeResult::incorrect();
    };
    let Some(labeling) = question.labeling.as_ref() else {
        return GradeResult::incorrect();
    };
    let targets: BTreeSet<String> = labeling
        .targets
        .iter()
        .map(|target| target.id.clone())
        .collect();
    let labels: BTreeSet<String> = labeling
        .labels
        .iter()
        .map(|label| label.id.clone())
        .collect();
    let Some(Answer::Labels { labels: given }) = answer else {
        return GradeResult::incorrect();
    };
    if given.keys().any(|id| !targets.contains(id))
        || given.values().any(|id| !labels.contains(id))
        || given.values().collect::<BTreeSet<_>>().len() != given.len()
    {
        return GradeResult::near_miss(NearMissReason::FormatError);
    }
    if given.len() < targets.len() {
        return GradeResult::near_miss_with(
            NearMissReason::IncompleteAnswer,
            GradeDiagnostic {
                matched: given.len(),
                total: targets.len(),
            },
        );
    }
    let mut best = 0;
    for accepted in spec.accepted() {
        if accepted == given {
            return GradeResult::correct();
        }
        let matched = given
            .iter()
            .filter(|(target, label)| accepted.get(*target) == Some(*label))
            .count();
        best = best.max(matched);
    }
    GradeResult::incorrect().with_diagnostic(best, targets.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbook::parse_workbook;

    fn workbook_with(question_json: &str) -> crate::Workbook {
        let raw = format!(
            r#"{{"format":"workbook","schemaVersion":2,"id":"w1","revision":1,
            "title":"T","description":"","language":"ja","tags":[],
            "assets":[
              {{"id":"map","alt":"地図","mediaType":"image/png","data":"AA=="}},
              {{"id":"cell","alt":"細胞","mediaType":"image/png","data":"AA=="}}
            ],
            "notes":[{{"id":"n1","title":"N","knowledgeCard":"card","questions":[{question_json}]}}]}}"#
        );
        parse_workbook(&raw).unwrap()
    }

    fn question(raw: &str) -> crate::Question {
        workbook_with(raw).notes[0].questions[0].clone()
    }

    fn numeric(raw: &str, answer: &str) -> GradeResult {
        let q = question(raw);
        grade_answer_detailed(&q, Some(&Answer::Text(answer.to_string()))).unwrap()
    }

    #[test]
    fn numeric_accepts_integers_decimals_and_fractions() {
        let q = r#"{"id":"q","revision":1,"type":"numeric","prompt":"値は？","explanation":"e",
            "answer":{"value":1.5,"unit":"m"}}"#;
        assert_eq!(numeric(q, "3/2m").status, GradeStatus::Correct);
        assert_eq!(numeric(q, "1.50 m").status, GradeStatus::Correct);
        assert_eq!(numeric(q, "-1.5m").status, GradeStatus::Incorrect);
    }

    #[test]
    fn numeric_requires_the_configured_significant_figures() {
        let q = r#"{"id":"q","revision":1,"type":"numeric","prompt":"有効数字2桁で","explanation":"e",
            "answer":{"value":3,"unit":"m","significantFigures":2}}"#;
        let result = numeric(q, "3m");
        assert_eq!(result.status, GradeStatus::NearMiss);
        assert_eq!(result.reasons, vec![NearMissReason::SignificantFigures]);
        assert_eq!(numeric(q, "3.0m").status, GradeStatus::Correct);
    }

    #[test]
    fn numeric_converts_both_units_to_a_common_base() {
        let q = r#"{"id":"q","revision":1,"type":"numeric","prompt":"長さ","explanation":"e",
            "answer":{"value":3,"unit":"cm","requireUnit":true}}"#;
        assert_eq!(numeric(q, "3cm").status, GradeStatus::Correct);
        assert_eq!(numeric(q, "3m").status, GradeStatus::Incorrect);

        let equivalent = numeric(q, "0.03m");
        assert_eq!(equivalent.status, GradeStatus::NearMiss);
        assert_eq!(
            equivalent.reasons,
            vec![NearMissReason::EquivalentExpression]
        );
        let millimetres = numeric(q, "30mm");
        assert_eq!(millimetres.status, GradeStatus::NearMiss);
        assert_eq!(
            millimetres.reasons,
            vec![NearMissReason::EquivalentExpression]
        );

        let mass = numeric(q, "3kg");
        assert_ne!(mass.status, GradeStatus::Correct);
        assert_eq!(mass.reasons, vec![NearMissReason::WrongUnit]);
    }

    #[test]
    fn numeric_conversion_works_in_the_reverse_direction() {
        let q = r#"{"id":"q","revision":1,"type":"numeric","prompt":"長さ","explanation":"e",
            "answer":{"value":0.03,"unit":"m","requireUnit":true}}"#;
        assert_eq!(numeric(q, "0.03m").status, GradeStatus::Correct);
        let centimetres = numeric(q, "3cm");
        assert_eq!(centimetres.status, GradeStatus::NearMiss);
        assert_eq!(
            centimetres.reasons,
            vec![NearMissReason::EquivalentExpression]
        );
        let kilometres = numeric(q, "0.00003km");
        assert_eq!(kilometres.status, GradeStatus::NearMiss);
        assert_eq!(
            kilometres.reasons,
            vec![NearMissReason::EquivalentExpression]
        );

        let current = r#"{"id":"q","revision":1,"type":"numeric","prompt":"電流","explanation":"e",
            "answer":{"value":3000,"unit":"mA","acceptedUnits":{"A":1000},"requireUnit":true}}"#;
        assert_eq!(numeric(current, "3A").status, GradeStatus::Correct);
        let without_opt_in = r#"{"id":"q","revision":1,"type":"numeric","prompt":"電流","explanation":"e",
            "answer":{"value":3000,"unit":"mA","requireUnit":true}}"#;
        let result = numeric(without_opt_in, "3A");
        assert_eq!(result.status, GradeStatus::NearMiss);
        assert_eq!(result.reasons, vec![NearMissReason::EquivalentExpression]);
    }

    #[test]
    fn numeric_tolerance_is_compared_in_the_base_unit() {
        let q = r#"{"id":"q","revision":1,"type":"numeric","prompt":"長さ","explanation":"e",
            "answer":{"value":3,"unit":"cm","tolerance":0.01,"requireUnit":true}}"#;
        let within = numeric(q, "0.0301m");
        assert_eq!(within.status, GradeStatus::NearMiss);
        assert_eq!(within.reasons, vec![NearMissReason::EquivalentExpression]);
        assert_eq!(numeric(q, "0.0302m").status, GradeStatus::Incorrect);
    }

    #[test]
    fn numeric_significant_figures_ignore_the_exponent() {
        let q = r#"{"id":"q","revision":1,"type":"numeric","prompt":"値","explanation":"e",
            "answer":{"value":300,"unit":"m","significantFigures":1,"requireUnit":true}}"#;
        assert_eq!(numeric(q, "3e2m").status, GradeStatus::Correct);
        let three_digits = numeric(q, "3.0e2m");
        assert_eq!(three_digits.status, GradeStatus::NearMiss);
        assert_eq!(
            three_digits.reasons,
            vec![NearMissReason::SignificantFigures]
        );
        assert_eq!(numeric(q, "4e2m").status, GradeStatus::Incorrect);
    }

    #[test]
    fn numeric_wrong_dimension_is_never_equivalent_without_require_unit() {
        let q = r#"{"id":"q","revision":1,"type":"numeric","prompt":"電流","explanation":"e",
            "answer":{"value":3,"unit":"A"}}"#;
        let result = numeric(q, "3V");
        assert_eq!(result.status, GradeStatus::NearMiss);
        assert_eq!(result.reasons, vec![NearMissReason::WrongUnit]);
        // Omitting the unit is fine when the workbook does not require one.
        assert_eq!(numeric(q, "3").status, GradeStatus::Correct);
    }

    #[test]
    fn numeric_converts_only_opted_in_units() {
        let q = r#"{"id":"q","revision":1,"type":"numeric","prompt":"電流","explanation":"e",
            "answer":{"value":3,"unit":"A","acceptedUnits":{"mA":0.001},"requireUnit":true}}"#;
        assert_eq!(numeric(q, "3000mA").status, GradeStatus::Correct);
        let q2 = r#"{"id":"q","revision":1,"type":"numeric","prompt":"電流","explanation":"e",
            "answer":{"value":3,"unit":"A","requireUnit":true}}"#;
        let result = numeric(q2, "3000mA");
        assert_eq!(result.status, GradeStatus::NearMiss);
        assert_eq!(result.reasons, vec![NearMissReason::EquivalentExpression]);
        let result = numeric(q2, "3");
        assert_eq!(result.reasons, vec![NearMissReason::MissingUnit]);
        let result = numeric(q2, "3V");
        assert_eq!(result.reasons, vec![NearMissReason::WrongUnit]);
    }

    #[test]
    fn numeric_rejects_malformed_expressions_and_division_by_zero() {
        let q = r#"{"id":"q","revision":1,"type":"numeric","prompt":"値","explanation":"e",
            "answer":{"value":3,"unit":"A"}}"#;
        let result = numeric(q, "3/0A");
        assert_eq!(result.status, GradeStatus::NearMiss);
        assert_eq!(result.reasons, vec![NearMissReason::FormatError]);
        assert_eq!(numeric(q, "abc").status, GradeStatus::NearMiss);
        assert_eq!(numeric(q, "--3A").status, GradeStatus::NearMiss);
        assert_eq!(numeric(q, "3..0A").status, GradeStatus::NearMiss);
    }

    #[test]
    fn numeric_handles_negative_values_exponents_and_tolerance_boundaries() {
        let q = r#"{"id":"q","revision":1,"type":"numeric","prompt":"値","explanation":"e",
            "answer":{"value":-0.05,"unit":"m","tolerance":0.001}}"#;
        assert_eq!(numeric(q, "-5e-2m").status, GradeStatus::Correct);
        assert_eq!(numeric(q, "-0.0509m").status, GradeStatus::Correct);
        assert_eq!(numeric(q, "-0.052m").status, GradeStatus::Incorrect);
        let fraction = r#"{"id":"q","revision":1,"type":"numeric","prompt":"値","explanation":"e",
            "answer":{"value":"1/3","unit":"m","tolerance":0.001}}"#;
        assert_eq!(numeric(fraction, "1/3m").status, GradeStatus::Correct);
        assert_eq!(numeric(fraction, "2/6m").status, GradeStatus::Correct);
        assert_eq!(numeric(fraction, "0.333m").status, GradeStatus::Correct);
    }

    #[test]
    fn numerical_grading_is_reproducible() {
        let q = r#"{"id":"q","revision":1,"type":"numeric","prompt":"値","explanation":"e",
            "answer":{"value":3,"unit":"A","significantFigures":2}}"#;
        let first = numeric(q, "3.0A");
        for _ in 0..10 {
            assert_eq!(numeric(q, "3.0A"), first);
        }
    }

    #[test]
    fn ordering_requires_an_accepted_full_order_and_reports_partial_matches() {
        let q = r#"{"id":"q","revision":1,"type":"ordering","prompt":"並べ替え","explanation":"e",
            "options":[{"id":"a","content":"A"},{"id":"b","content":"B"},{"id":"c","content":"C"}],
            "answer":{"orders":[["a","b","c"]]}}"#;
        let question = question(q);
        let correct = grade_answer_detailed(
            &question,
            Some(&Answer::Ordering {
                order: vec!["a".into(), "b".into(), "c".into()],
            }),
        )
        .unwrap();
        assert_eq!(correct.status, GradeStatus::Correct);
        let wrong = grade_answer_detailed(
            &question,
            Some(&Answer::Ordering {
                order: vec!["a".into(), "c".into(), "b".into()],
            }),
        )
        .unwrap();
        assert_eq!(wrong.status, GradeStatus::Incorrect);
        assert_eq!(wrong.diagnostic.unwrap().matched, 1);
        let partial = grade_answer_detailed(
            &question,
            Some(&Answer::Ordering {
                order: vec!["a".into(), "b".into()],
            }),
        )
        .unwrap();
        assert_eq!(partial.status, GradeStatus::NearMiss);
        assert_eq!(partial.reasons, vec![NearMissReason::IncompleteAnswer]);
    }

    #[test]
    fn ordering_accepts_multiple_authored_orders() {
        let q = r#"{"id":"q","revision":1,"type":"ordering","prompt":"並べ替え","explanation":"e",
            "options":[{"id":"a","content":"A"},{"id":"b","content":"B"}],
            "answer":{"orders":[["a","b"],["b","a"]]}}"#;
        for order in [vec!["a", "b"], vec!["b", "a"]] {
            let result = grade_answer_detailed(
                &question(q),
                Some(&Answer::Ordering {
                    order: order.into_iter().map(str::to_string).collect(),
                }),
            )
            .unwrap();
            assert_eq!(result.status, GradeStatus::Correct);
        }
    }

    #[test]
    fn matching_grades_pairs_and_reports_matched_count() {
        let q = r#"{"id":"q","revision":1,"type":"matching","prompt":"対応","explanation":"e",
            "matching":{"left":[{"id":"l1","content":"A"},{"id":"l2","content":"B"}],
                        "right":[{"id":"r1","content":"1"},{"id":"r2","content":"2"}]},
            "answer":{"pairs":[{"left":"l1","right":"r1"},{"left":"l2","right":"r2"}]}}"#;
        let question = question(q);
        let correct = grade_answer_detailed(
            &question,
            Some(&Answer::Matching {
                matches: BTreeMap::from([
                    ("l1".to_string(), "r1".to_string()),
                    ("l2".to_string(), "r2".to_string()),
                ]),
            }),
        )
        .unwrap();
        assert_eq!(correct.status, GradeStatus::Correct);
        let wrong = grade_answer_detailed(
            &question,
            Some(&Answer::Matching {
                matches: BTreeMap::from([
                    ("l1".to_string(), "r1".to_string()),
                    ("l2".to_string(), "r1".to_string()),
                ]),
            }),
        )
        .unwrap();
        assert_eq!(wrong.status, GradeStatus::NearMiss);
        assert_eq!(wrong.reasons, vec![NearMissReason::FormatError]);
    }

    #[test]
    fn hotspot_uses_normalized_shapes_and_accepts_any_listed_region() {
        let q = r#"{"id":"q","revision":1,"type":"hotspot","prompt":"地図","explanation":"e",
            "hotspot":{"asset":"map","regions":[
              {"id":"kanto","alt":"関東","shape":{"type":"rect","x":0.5,"y":0.4,"w":0.2,"h":0.2}},
              {"id":"kansai","alt":"関西","shape":{"type":"circle","cx":0.3,"cy":0.6,"r":0.1}}]},
            "answer":{"regionIds":["kanto"]}}"#;
        let question = question(q);
        let correct =
            grade_answer_detailed(&question, Some(&Answer::Hotspot { x: 0.55, y: 0.45 })).unwrap();
        assert_eq!(correct.status, GradeStatus::Correct);
        let wrong =
            grade_answer_detailed(&question, Some(&Answer::Hotspot { x: 0.3, y: 0.6 })).unwrap();
        assert_eq!(wrong.status, GradeStatus::Incorrect);
        let invalid =
            grade_answer_detailed(&question, Some(&Answer::Hotspot { x: 2.0, y: 0.5 })).unwrap();
        assert_eq!(invalid.status, GradeStatus::NearMiss);
    }

    #[test]
    fn labeling_requires_every_slot_and_reports_partial_matches() {
        let q = r#"{"id":"q","revision":1,"type":"labeling","prompt":"細胞","explanation":"e",
            "labeling":{"asset":"cell",
              "targets":[{"id":"t1","x":0.2,"y":0.3,"alt":"核"},{"id":"t2","x":0.6,"y":0.5,"alt":"膜"}],
              "labels":[{"id":"l1","content":"核"},{"id":"l2","content":"細胞膜"}]},
            "answer":{"pairs":{"t1":"l1","t2":"l2"}}}"#;
        let question = question(q);
        let correct = grade_answer_detailed(
            &question,
            Some(&Answer::Labels {
                labels: BTreeMap::from([
                    ("t1".to_string(), "l1".to_string()),
                    ("t2".to_string(), "l2".to_string()),
                ]),
            }),
        )
        .unwrap();
        assert_eq!(correct.status, GradeStatus::Correct);
        let partial = grade_answer_detailed(
            &question,
            Some(&Answer::Labels {
                labels: BTreeMap::from([("t1".to_string(), "l1".to_string())]),
            }),
        )
        .unwrap();
        assert_eq!(partial.status, GradeStatus::NearMiss);
        assert_eq!(partial.reasons, vec![NearMissReason::IncompleteAnswer]);
    }

    #[test]
    fn hotspot_supports_circle_rect_and_polygon_regions() {
        let q = r#"{"id":"q","revision":1,"type":"hotspot","prompt":"図","explanation":"e",
            "hotspot":{"asset":"map","regions":[
              {"id":"circle","alt":"円","shape":{"type":"circle","cx":0.2,"cy":0.2,"r":0.1}},
              {"id":"rect","alt":"矩形","shape":{"type":"rect","x":0.4,"y":0.1,"w":0.2,"h":0.2}},
              {"id":"poly","alt":"多角形","shape":{"type":"polygon","points":[[0.1,0.6],[0.4,0.6],[0.25,0.9]]}}]},
            "answer":{"regionIds":["circle","rect"]}}"#;
        let question = question(q);
        for (x, y, expected) in [
            (0.2, 0.2, GradeStatus::Correct),
            (0.5, 0.2, GradeStatus::Correct),
            (0.25, 0.7, GradeStatus::Incorrect),
            (0.9, 0.9, GradeStatus::Incorrect),
        ] {
            let result = grade_answer_detailed(&question, Some(&Answer::Hotspot { x, y })).unwrap();
            assert_eq!(result.status, expected, "{x},{y}");
        }
    }

    #[test]
    fn hotspot_overlapping_regions_are_order_independent() {
        let cases = [
            r#"[
              {"id":"wrong","alt":"誤","shape":{"type":"rect","x":0.2,"y":0.2,"w":0.5,"h":0.5}},
              {"id":"right","alt":"正","shape":{"type":"circle","cx":0.5,"cy":0.5,"r":0.1}}]"#,
            r#"[
              {"id":"right","alt":"正","shape":{"type":"circle","cx":0.5,"cy":0.5,"r":0.1}},
              {"id":"wrong","alt":"誤","shape":{"type":"rect","x":0.2,"y":0.2,"w":0.5,"h":0.5}}]"#,
        ];
        for regions in cases {
            let q = format!(
                r#"{{"id":"q","revision":1,"type":"hotspot","prompt":"図","explanation":"e",
                "hotspot":{{"asset":"map","regions":{regions}}},
                "answer":{{"regionIds":["right"]}}}}"#
            );
            let question = question(&q);
            let result =
                grade_answer_detailed(&question, Some(&Answer::Hotspot { x: 0.5, y: 0.5 }))
                    .unwrap();
            assert_eq!(result.status, GradeStatus::Correct, "{regions}");
        }
    }

    #[test]
    fn hotspot_clicking_only_non_accepted_overlapping_region_is_incorrect() {
        let q = r#"{"id":"q","revision":1,"type":"hotspot","prompt":"図","explanation":"e",
            "hotspot":{"asset":"map","regions":[
              {"id":"right","alt":"正","shape":{"type":"rect","x":0.2,"y":0.2,"w":0.2,"h":0.2}},
              {"id":"wrong","alt":"誤","shape":{"type":"circle","cx":0.7,"cy":0.7,"r":0.1}}]},
            "answer":{"regionIds":["right"]}}"#;
        let question = question(q);
        let result =
            grade_answer_detailed(&question, Some(&Answer::Hotspot { x: 0.7, y: 0.7 })).unwrap();
        assert_eq!(result.status, GradeStatus::Incorrect);
    }

    #[test]
    fn matching_accepts_an_alternative_full_assignment() {
        let q = r#"{"id":"q","revision":1,"type":"matching","prompt":"対応","explanation":"e",
            "matching":{"left":[{"id":"l1","content":"A"},{"id":"l2","content":"B"}],
                        "right":[{"id":"r1","content":"1"},{"id":"r2","content":"2"}]},
            "answer":{"pairs":[{"left":"l1","right":"r1"},{"left":"l2","right":"r2"}],
                      "alternatives":[[{"left":"l1","right":"r2"},{"left":"l2","right":"r1"}]]}}"#;
        let question = question(q);
        for matches in [
            BTreeMap::from([
                ("l1".to_string(), "r1".to_string()),
                ("l2".to_string(), "r2".to_string()),
            ]),
            BTreeMap::from([
                ("l1".to_string(), "r2".to_string()),
                ("l2".to_string(), "r1".to_string()),
            ]),
        ] {
            let result =
                grade_answer_detailed(&question, Some(&Answer::Matching { matches })).unwrap();
            assert_eq!(result.status, GradeStatus::Correct);
        }
    }

    #[test]
    fn duplicate_assignments_are_rejected_as_format_errors() {
        let q = r#"{"id":"q","revision":1,"type":"labeling","prompt":"配置","explanation":"e",
            "labeling":{"asset":"cell",
              "targets":[{"id":"t1","x":0.2,"y":0.3,"alt":"A"},{"id":"t2","x":0.6,"y":0.5,"alt":"B"}],
              "labels":[{"id":"l1","content":"X"},{"id":"l2","content":"Y"}]},
            "answer":{"pairs":{"t1":"l1","t2":"l2"}}}"#;
        let question = question(q);
        let duplicate = grade_answer_detailed(
            &question,
            Some(&Answer::Labels {
                labels: BTreeMap::from([
                    ("t1".to_string(), "l1".to_string()),
                    ("t2".to_string(), "l1".to_string()),
                ]),
            }),
        )
        .unwrap();
        assert_eq!(duplicate.status, GradeStatus::NearMiss);
        assert_eq!(duplicate.reasons, vec![NearMissReason::FormatError]);
    }
}
