use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::content::{Content, ContentError, validate_https_url};

pub const MAX_JSON_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{path}: {message}")]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

impl ValidationError {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl From<ContentError> for ValidationError {
    fn from(value: ContentError) -> Self {
        Self::new(value.path, value.message)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionType {
    SingleChoice,
    TrueFalse,
    MultipleChoice,
    ShortText,
    Cloze,
    Flashcard,
}

impl QuestionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleChoice => "single_choice",
            Self::TrueFalse => "true_false",
            Self::MultipleChoice => "multiple_choice",
            Self::ShortText => "short_text",
            Self::Cloze => "cloze",
            Self::Flashcard => "flashcard",
        }
    }

    pub fn is_choice(self) -> bool {
        matches!(self, Self::SingleChoice | Self::MultipleChoice)
    }
}

impl std::str::FromStr for QuestionType {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "single_choice" => Ok(Self::SingleChoice),
            "true_false" => Ok(Self::TrueFalse),
            "multiple_choice" => Ok(Self::MultipleChoice),
            "short_text" => Ok(Self::ShortText),
            "cloze" => Ok(Self::Cloze),
            "flashcard" => Ok(Self::Flashcard),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceOption {
    pub id: String,
    pub content: Content,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub revision: i64,
    /// JSON uses `type`; the rest of this struct intentionally mirrors the v1
    /// format so round-tripping a workbook preserves all content.
    #[serde(rename = "type")]
    pub kind: QuestionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<Content>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<Content>,
    pub explanation: Content,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<ChoiceOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front: Option<Content>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<Content>,
}

impl Question {
    pub fn prompt(&self) -> Option<&Content> {
        self.prompt.as_ref()
    }

    pub fn prompt_text(&self) -> String {
        self.prompt
            .as_ref()
            .map(Content::plain_text)
            .unwrap_or_default()
    }

    pub fn single_choice_answer(&self) -> Option<&str> {
        self.answer.as_ref()?.get("optionId")?.as_str()
    }

    pub fn multiple_choice_answers(&self) -> Vec<String> {
        self.answer
            .as_ref()
            .and_then(|value| value.get("optionIds"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn short_text_answers(&self) -> Vec<String> {
        self.answer
            .as_ref()
            .and_then(|value| value.get("accepted"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn cloze_blanks(&self) -> BTreeMap<String, Vec<String>> {
        let mut output = BTreeMap::new();
        let Some(blanks) = self
            .answer
            .as_ref()
            .and_then(|value| value.get("blanks"))
            .and_then(Value::as_object)
        else {
            return output;
        };
        for (id, values) in blanks {
            let values = values
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            output.insert(id.clone(), values);
        }
        output
    }

    pub fn cloze_blank_ids(&self) -> Vec<String> {
        let Some(Content::Text(prompt)) = &self.prompt else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        let mut rest = prompt.as_str();
        while let Some(start) = rest.find("{{blank:") {
            let after = &rest[start + "{{blank:".len()..];
            let Some(end) = after.find("}}") else {
                break;
            };
            let id = &after[..end];
            if !id.is_empty() && !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_string());
            }
            rest = &after[end + 2..];
        }
        ids
    }

    pub fn answer_summary(&self) -> String {
        match self.kind {
            QuestionType::SingleChoice => self
                .options
                .as_ref()
                .and_then(|options| {
                    let id = self.single_choice_answer()?;
                    options
                        .iter()
                        .find(|option| option.id == id)
                        .map(|option| option.content.plain_text())
                })
                .unwrap_or_else(|| "（正解なし）".to_string()),
            QuestionType::TrueFalse => match self.answer.as_ref().and_then(Value::as_bool) {
                Some(true) => "○ 正しい".to_string(),
                Some(false) => "× まちがい".to_string(),
                None => "（正解なし）".to_string(),
            },
            QuestionType::MultipleChoice => {
                let ids = self.multiple_choice_answers();
                self.options
                    .as_ref()
                    .map(|options| {
                        options
                            .iter()
                            .filter(|option| ids.contains(&option.id))
                            .map(|option| option.content.plain_text())
                            .collect::<Vec<_>>()
                            .join(" / ")
                    })
                    .unwrap_or_default()
            }
            QuestionType::ShortText => self.short_text_answers().join(" / "),
            QuestionType::Cloze => self
                .cloze_blanks()
                .into_iter()
                .map(|(id, values)| format!("{id}: {}", values.join(" / ")))
                .collect::<Vec<_>>()
                .join("\n"),
            QuestionType::Flashcard => self
                .back
                .as_ref()
                .map(Content::plain_text)
                .unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    pub id: String,
    pub title: String,
    #[serde(rename = "knowledgeCard")]
    pub knowledge_card: Content,
    pub questions: Vec<Question>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workbook {
    pub format: String,
    pub schema_version: u32,
    pub id: String,
    pub revision: i64,
    pub title: String,
    pub description: String,
    pub language: String,
    pub tags: Vec<String>,
    pub notes: Vec<Note>,
}

impl Workbook {
    pub fn all_questions(&self) -> impl Iterator<Item = (&Note, &Question)> {
        self.notes
            .iter()
            .flat_map(|note| note.questions.iter().map(move |question| (note, question)))
    }

    pub fn question_count(&self) -> usize {
        self.notes.iter().map(|note| note.questions.len()).sum()
    }

    pub fn find_question(&self, note_id: &str, question_id: &str) -> Option<(&Note, &Question)> {
        let note = self.notes.iter().find(|note| note.id == note_id)?;
        let question = note
            .questions
            .iter()
            .find(|question| question.id == question_id)?;
        Some((note, question))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("Workbook serialization cannot fail")
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.format != "workbook" {
            return Err(ValidationError::new(
                "format",
                "workbook を指定してください",
            ));
        }
        if self.schema_version != 1 {
            return Err(ValidationError::new(
                "schemaVersion",
                "schemaVersion 1 のみ対応しています",
            ));
        }
        validate_id(&self.id, "id")?;
        if self.revision <= 0 {
            return Err(ValidationError::new(
                "revision",
                "1以上の整数にしてください",
            ));
        }
        validate_required_text(&self.title, "title")?;
        if self.description.chars().count() > 100_000 {
            return Err(ValidationError::new("description", "説明が長すぎます"));
        }
        validate_required_text(&self.language, "language")?;
        if self.tags.len() > 100 {
            return Err(ValidationError::new(
                "tags",
                "タグは100個以下にしてください",
            ));
        }
        for (i, tag) in self.tags.iter().enumerate() {
            validate_required_text(tag, &format!("tags[{i}]"))?;
        }
        if self.notes.is_empty() {
            return Err(ValidationError::new("notes", "Noteを1つ以上含めてください"));
        }
        if self.notes.len() > 5000 {
            return Err(ValidationError::new(
                "notes",
                "Noteは5000個以下にしてください",
            ));
        }

        let mut note_ids = BTreeSet::new();
        let mut question_ids = BTreeSet::new();
        for (ni, note) in self.notes.iter().enumerate() {
            let path = format!("notes[{ni}]");
            validate_id(&note.id, &format!("{path}.id"))?;
            if !note_ids.insert(note.id.as_str()) {
                return Err(ValidationError::new(
                    format!("{path}.id"),
                    "Note IDが重複しています",
                ));
            }
            validate_required_text(&note.title, &format!("{path}.title"))?;
            note.knowledge_card
                .validate(&format!("{path}.knowledgeCard"))?;
            if note.questions.is_empty() {
                return Err(ValidationError::new(
                    format!("{path}.questions"),
                    "Questionを1つ以上含めてください",
                ));
            }
            if note.questions.len() > 2000 {
                return Err(ValidationError::new(
                    format!("{path}.questions"),
                    "Questionは2000個以下にしてください",
                ));
            }
            for (qi, question) in note.questions.iter().enumerate() {
                let qpath = format!("{path}.questions[{qi}]");
                question.validate(&qpath)?;
                if !question_ids.insert(question.id.as_str()) {
                    return Err(ValidationError::new(
                        format!("{qpath}.id"),
                        "Question IDが教材全体で重複しています",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Question {
    fn validate(&self, path: &str) -> Result<(), ValidationError> {
        validate_id(&self.id, &format!("{path}.id"))?;
        if self.revision <= 0 {
            return Err(ValidationError::new(
                format!("{path}.revision"),
                "1以上の整数にしてください",
            ));
        }
        if let Some(hint) = &self.hint {
            hint.validate(&format!("{path}.hint"))?;
        }
        self.explanation.validate(&format!("{path}.explanation"))?;
        match self.kind {
            QuestionType::SingleChoice | QuestionType::MultipleChoice => {
                let prompt = self.prompt.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.prompt"), "問題文が必要です")
                })?;
                prompt.validate(&format!("{path}.prompt"))?;
                ensure_absent(self.front.as_ref(), &format!("{path}.front"))?;
                ensure_absent(self.back.as_ref(), &format!("{path}.back"))?;
                let options = self.options.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.options"), "選択肢が必要です")
                })?;
                if !(2..=6).contains(&options.len()) {
                    return Err(ValidationError::new(
                        format!("{path}.options"),
                        "選択肢は2〜6個にしてください",
                    ));
                }
                let mut ids = BTreeSet::new();
                for (oi, option) in options.iter().enumerate() {
                    validate_id(&option.id, &format!("{path}.options[{oi}].id"))?;
                    if !ids.insert(option.id.as_str()) {
                        return Err(ValidationError::new(
                            format!("{path}.options[{oi}].id"),
                            "選択肢IDが重複しています",
                        ));
                    }
                    option
                        .content
                        .validate(&format!("{path}.options[{oi}].content"))?;
                }
                if self.kind == QuestionType::SingleChoice {
                    let answer = self.answer.as_ref().ok_or_else(|| {
                        ValidationError::new(format!("{path}.answer"), "正解が必要です")
                    })?;
                    let object = answer_object(answer, &format!("{path}.answer"))?;
                    ensure_keys(object, &["optionId"], &format!("{path}.answer"))?;
                    let option_id =
                        object
                            .get("optionId")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                ValidationError::new(
                                    format!("{path}.answer.optionId"),
                                    "文字列を指定してください",
                                )
                            })?;
                    if !ids.contains(option_id) {
                        return Err(ValidationError::new(
                            format!("{path}.answer.optionId"),
                            "正解IDが存在しません",
                        ));
                    }
                } else {
                    let answer = self.answer.as_ref().ok_or_else(|| {
                        ValidationError::new(format!("{path}.answer"), "正解が必要です")
                    })?;
                    let object = answer_object(answer, &format!("{path}.answer"))?;
                    ensure_keys(object, &["optionIds"], &format!("{path}.answer"))?;
                    let values = object
                        .get("optionIds")
                        .and_then(Value::as_array)
                        .ok_or_else(|| {
                            ValidationError::new(
                                format!("{path}.answer.optionIds"),
                                "配列を指定してください",
                            )
                        })?;
                    if values.is_empty() || values.len() > 6 {
                        return Err(ValidationError::new(
                            format!("{path}.answer.optionIds"),
                            "1〜6個の正解を指定してください",
                        ));
                    }
                    let mut seen = BTreeSet::new();
                    for (i, value) in values.iter().enumerate() {
                        let id = value.as_str().ok_or_else(|| {
                            ValidationError::new(
                                format!("{path}.answer.optionIds[{i}]"),
                                "文字列を指定してください",
                            )
                        })?;
                        if !seen.insert(id) {
                            return Err(ValidationError::new(
                                format!("{path}.answer.optionIds"),
                                "正解IDが重複しています",
                            ));
                        }
                        if !ids.contains(id) {
                            return Err(ValidationError::new(
                                format!("{path}.answer.optionIds[{i}]"),
                                "正解IDが存在しません",
                            ));
                        }
                    }
                }
            }
            QuestionType::TrueFalse => {
                let prompt = self.prompt.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.prompt"), "問題文が必要です")
                })?;
                prompt.validate(&format!("{path}.prompt"))?;
                ensure_absent(self.front.as_ref(), &format!("{path}.front"))?;
                ensure_absent(self.back.as_ref(), &format!("{path}.back"))?;
                ensure_absent(self.options.as_ref(), &format!("{path}.options"))?;
                if !self.answer.as_ref().is_some_and(Value::is_boolean) {
                    return Err(ValidationError::new(
                        format!("{path}.answer"),
                        "true または false を指定してください",
                    ));
                }
            }
            QuestionType::ShortText => {
                let prompt = self.prompt.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.prompt"), "問題文が必要です")
                })?;
                prompt.validate(&format!("{path}.prompt"))?;
                ensure_absent(self.front.as_ref(), &format!("{path}.front"))?;
                ensure_absent(self.back.as_ref(), &format!("{path}.back"))?;
                ensure_absent(self.options.as_ref(), &format!("{path}.options"))?;
                let answer = self.answer.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.answer"), "正解が必要です")
                })?;
                let object = answer_object(answer, &format!("{path}.answer"))?;
                ensure_keys(object, &["accepted"], &format!("{path}.answer"))?;
                validate_accepted(
                    object.get("accepted"),
                    &format!("{path}.answer.accepted"),
                    1,
                )?;
            }
            QuestionType::Cloze => {
                let prompt = self.prompt.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.prompt"), "問題文が必要です")
                })?;
                let Content::Text(prompt_text) = prompt else {
                    return Err(ValidationError::new(
                        format!("{path}.prompt"),
                        "穴埋め問題のpromptは文字列にしてください",
                    ));
                };
                validate_required_text(prompt_text, &format!("{path}.prompt"))?;
                ensure_absent(self.front.as_ref(), &format!("{path}.front"))?;
                ensure_absent(self.back.as_ref(), &format!("{path}.back"))?;
                ensure_absent(self.options.as_ref(), &format!("{path}.options"))?;
                let blank_ids = self.cloze_blank_ids();
                if blank_ids.is_empty() {
                    return Err(ValidationError::new(
                        format!("{path}.prompt"),
                        "{{blank:id}} を1つ以上含めてください",
                    ));
                }
                if has_malformed_cloze(prompt_text) {
                    return Err(ValidationError::new(
                        format!("{path}.prompt"),
                        "空欄記法が壊れています",
                    ));
                }
                let answer = self.answer.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.answer"), "正解が必要です")
                })?;
                let object = answer_object(answer, &format!("{path}.answer"))?;
                ensure_keys(object, &["blanks"], &format!("{path}.answer"))?;
                let blanks = object
                    .get("blanks")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        ValidationError::new(
                            format!("{path}.answer.blanks"),
                            "オブジェクトが必要です",
                        )
                    })?;
                if blanks.len() != blank_ids.len() {
                    return Err(ValidationError::new(
                        format!("{path}.answer.blanks"),
                        "空欄IDと正答候補が対応していません",
                    ));
                }
                for blank in &blank_ids {
                    let accepted = blanks.get(blank).ok_or_else(|| {
                        ValidationError::new(
                            format!("{path}.answer.blanks"),
                            format!("空欄IDと正答候補が対応していません: {blank}"),
                        )
                    })?;
                    validate_accepted(Some(accepted), &format!("{path}.answer.blanks.{blank}"), 1)?;
                }
            }
            QuestionType::Flashcard => {
                ensure_absent(self.prompt.as_ref(), &format!("{path}.prompt"))?;
                ensure_absent(self.options.as_ref(), &format!("{path}.options"))?;
                ensure_absent(self.answer.as_ref(), &format!("{path}.answer"))?;
                let front = self.front.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.front"), "表面が必要です")
                })?;
                let back = self.back.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.back"), "裏面が必要です")
                })?;
                front.validate(&format!("{path}.front"))?;
                back.validate(&format!("{path}.back"))?;
            }
        }
        Ok(())
    }
}

fn validate_accepted(value: Option<&Value>, path: &str, min: usize) -> Result<(), ValidationError> {
    let values = value
        .and_then(Value::as_array)
        .ok_or_else(|| ValidationError::new(path, "配列を指定してください"))?;
    if values.len() < min || values.len() > 100 {
        return Err(ValidationError::new(
            path,
            "1〜100個の候補を指定してください",
        ));
    }
    for (i, value) in values.iter().enumerate() {
        let text = value.as_str().ok_or_else(|| {
            ValidationError::new(format!("{path}[{i}]"), "文字列を指定してください")
        })?;
        validate_required_text(text, &format!("{path}[{i}]"))?;
        if text.chars().count() > 100_000 {
            return Err(ValidationError::new(
                format!("{path}[{i}]"),
                "候補が長すぎます",
            ));
        }
    }
    Ok(())
}

fn validate_id(value: &str, path: &str) -> Result<(), ValidationError> {
    validate_required_text(value, path)?;
    if value.chars().count() > 160 {
        return Err(ValidationError::new(path, "IDは160文字以下にしてください"));
    }
    Ok(())
}

fn validate_required_text(value: &str, path: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        return Err(ValidationError::new(path, "空にできません"));
    }
    if value.chars().count() > 100_000 {
        return Err(ValidationError::new(path, "長すぎます"));
    }
    Ok(())
}

fn answer_object<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a Map<String, Value>, ValidationError> {
    value
        .as_object()
        .ok_or_else(|| ValidationError::new(path, "オブジェクトを指定してください"))
}

fn ensure_keys(
    object: &Map<String, Value>,
    expected: &[&str],
    path: &str,
) -> Result<(), ValidationError> {
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(ValidationError::new(
            path,
            format!("{} だけを指定してください", expected.join(", ")),
        ));
    }
    Ok(())
}

fn ensure_absent<T>(value: Option<T>, path: &str) -> Result<(), ValidationError> {
    if value.is_some() {
        return Err(ValidationError::new(path, "この問題形式では使用できません"));
    }
    Ok(())
}

fn has_malformed_cloze(prompt: &str) -> bool {
    let mut rest = prompt;
    while let Some(start) = rest.find("{{blank:") {
        let after = &rest[start + "{{blank:".len()..];
        let Some(end) = after.find("}}") else {
            return true;
        };
        if after[..end].is_empty() {
            return true;
        }
        rest = &after[end + 2..];
    }
    rest.contains("{{blank:")
}

pub fn parse_workbook(raw: &str) -> Result<Workbook, ValidationError> {
    if raw.len() > MAX_JSON_BYTES {
        return Err(ValidationError::new(
            "$",
            format!(
                "JSONは{}MB以下にしてください",
                MAX_JSON_BYTES / (1024 * 1024)
            ),
        ));
    }
    let deserializer = &mut serde_json::Deserializer::from_str(raw);
    let workbook: Workbook = serde_path_to_error::deserialize(deserializer).map_err(|error| {
        let path = error.path().to_string();
        let path = if path.is_empty() {
            "$".to_string()
        } else {
            path
        };
        ValidationError::new(path, format!("JSON形式: {}", error.inner()))
    })?;
    workbook.validate()?;
    Ok(workbook)
}

/// Validates an HTTPS image URL for callers that store image blocks outside a
/// full workbook (for example legacy migration previews).
pub fn validate_image_url(url: &str, path: &str) -> Result<(), ValidationError> {
    validate_https_url(url, path).map_err(ValidationError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"{
      "format":"workbook","schemaVersion":1,"id":"w1","revision":1,
      "title":"T","description":"","language":"ja","tags":[],
      "notes":[{"id":"n1","title":"N","knowledgeCard":"card","questions":[
        {"id":"q1","revision":1,"type":"true_false","prompt":"p","answer":true,"explanation":"e"}
      ]}]
    }"#;

    #[test]
    fn valid_minimal_workbook_parses() {
        let workbook = parse_workbook(MINIMAL).unwrap();
        assert_eq!(workbook.title, "T");
        assert_eq!(workbook.question_count(), 1);
    }

    #[test]
    fn duplicate_question_ids_are_rejected_across_notes() {
        let raw = MINIMAL.replace(
            "\"notes\":[{\"id\":\"n1\",\"title\":\"N\",\"knowledgeCard\":\"card\",\"questions\":[\n        {\"id\":\"q1\",\"revision\":1,\"type\":\"true_false\",\"prompt\":\"p\",\"answer\":true,\"explanation\":\"e\"}\n      ]}",
            "\"notes\":[{\"id\":\"n1\",\"title\":\"N\",\"knowledgeCard\":\"card\",\"questions\":[\n        {\"id\":\"q1\",\"revision\":1,\"type\":\"true_false\",\"prompt\":\"p\",\"answer\":true,\"explanation\":\"e\"},\n        {\"id\":\"q1\",\"revision\":1,\"type\":\"true_false\",\"prompt\":\"p\",\"answer\":true,\"explanation\":\"e\"}\n      ]}",
        );
        let error = parse_workbook(&raw).unwrap_err();
        assert!(error.message.contains("重複"));
    }

    #[test]
    fn cloze_requires_matching_blank_ids() {
        let raw = r#"{
          "format":"workbook","schemaVersion":1,"id":"w1","revision":1,
          "title":"T","description":"","language":"ja","tags":[],
          "notes":[{"id":"n1","title":"N","knowledgeCard":"card","questions":[
            {"id":"q1","revision":1,"type":"cloze","prompt":"a {{blank:x}} b","answer":{"blanks":{"y":["v"]}},"explanation":"e"}
          ]}]
        }"#;
        let error = parse_workbook(raw).unwrap_err();
        assert!(error.message.contains("対応"));
    }

    #[test]
    fn unknown_fields_are_rejected_with_a_path() {
        let raw = MINIMAL.replace("\"title\":\"T\"", "\"title\":\"T\",\"extra\":1");
        let error = parse_workbook(&raw).unwrap_err();
        assert!(error.path.contains("extra") || error.message.contains("unknown"));
    }
}
