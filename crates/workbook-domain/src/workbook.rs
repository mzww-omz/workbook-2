use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::content::{Content, ContentError, validate_https_url};
use crate::question_format::{
    AssetRef, HotspotAnswerSpec, HotspotSpec, LabelingAnswerSpec, LabelingSpec, MatchingAnswerSpec,
    MatchingSpec, NumericAnswerSpec, OrderingAnswerSpec, inspect_image,
};

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
    Numeric,
    Ordering,
    Matching,
    Hotspot,
    Labeling,
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
            Self::Numeric => "numeric",
            Self::Ordering => "ordering",
            Self::Matching => "matching",
            Self::Hotspot => "hotspot",
            Self::Labeling => "labeling",
        }
    }

    pub fn is_choice(self) -> bool {
        matches!(self, Self::SingleChoice | Self::MultipleChoice)
    }

    /// Formats added in workbook schema version 2.
    pub fn requires_schema_v2(self) -> bool {
        matches!(
            self,
            Self::Numeric | Self::Ordering | Self::Matching | Self::Hotspot | Self::Labeling
        )
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
            "numeric" => Ok(Self::Numeric),
            "ordering" => Ok(Self::Ordering),
            "matching" => Ok(Self::Matching),
            "hotspot" => Ok(Self::Hotspot),
            "labeling" => Ok(Self::Labeling),
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
    /// Display order for `ordering` questions. When absent the authored
    /// `options` order is shown.
    #[serde(
        rename = "initialOrder",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub initial_order: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matching: Option<MatchingSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotspot: Option<HotspotSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labeling: Option<LabelingSpec>,
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

    pub fn numeric_spec(&self) -> Option<NumericAnswerSpec> {
        serde_json::from_value(self.answer.clone()?).ok()
    }

    pub fn ordering_spec(&self) -> Option<OrderingAnswerSpec> {
        serde_json::from_value(self.answer.clone()?).ok()
    }

    pub fn matching_spec(&self) -> Option<MatchingAnswerSpec> {
        serde_json::from_value(self.answer.clone()?).ok()
    }

    pub fn hotspot_spec(&self) -> Option<HotspotAnswerSpec> {
        serde_json::from_value(self.answer.clone()?).ok()
    }

    pub fn labeling_spec(&self) -> Option<LabelingAnswerSpec> {
        serde_json::from_value(self.answer.clone()?).ok()
    }

    /// Item IDs in the order they should first appear on screen.
    pub fn display_order(&self) -> Vec<String> {
        if let Some(initial) = &self.initial_order {
            return initial.clone();
        }
        self.options
            .as_ref()
            .map(|options| options.iter().map(|option| option.id.clone()).collect())
            .unwrap_or_default()
    }

    /// Every bundled image referenced by this question, including rich
    /// content (prompt, options, hint, explanation) and the Hotspot/Labeling
    /// image. The service uses this to serve all images the current view needs.
    pub fn referenced_asset_ids(&self) -> BTreeSet<String> {
        let mut ids = BTreeSet::new();
        for content in [
            self.prompt.as_ref(),
            self.hint.as_ref(),
            Some(&self.explanation),
            self.front.as_ref(),
            self.back.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            content.referenced_asset_ids(&mut ids);
        }
        if let Some(options) = &self.options {
            for option in options {
                option.content.referenced_asset_ids(&mut ids);
            }
        }
        if let Some(matching) = &self.matching {
            for item in matching.left.iter().chain(&matching.right) {
                item.content.referenced_asset_ids(&mut ids);
            }
        }
        if let Some(labeling) = &self.labeling {
            ids.insert(labeling.asset.clone());
            for label in &labeling.labels {
                label.content.referenced_asset_ids(&mut ids);
            }
        }
        if let Some(hotspot) = &self.hotspot {
            ids.insert(hotspot.asset.clone());
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
            QuestionType::Numeric => match self.numeric_spec() {
                Some(spec) => {
                    let value = spec
                        .value
                        .value()
                        .map(format_number)
                        .unwrap_or_else(|_| "?".to_string());
                    let mut text = match spec.unit {
                        Some(unit) => format!("{value} {unit}"),
                        None => value,
                    };
                    if let Some(sig) = spec.significant_figures {
                        text.push_str(&format!("（有効数字{sig}桁）"));
                    }
                    text
                }
                None => "（正解なし）".to_string(),
            },
            QuestionType::Ordering => match self.ordering_spec() {
                Some(spec) => spec
                    .orders
                    .first()
                    .map(|order| {
                        order
                            .iter()
                            .enumerate()
                            .map(|(index, id)| {
                                let label = self
                                    .options
                                    .as_ref()
                                    .and_then(|options| {
                                        options
                                            .iter()
                                            .find(|option| &option.id == id)
                                            .map(|option| option.content.plain_text())
                                    })
                                    .unwrap_or_else(|| id.clone());
                                format!("{}. {label}", index + 1)
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default(),
                None => "（正解なし）".to_string(),
            },
            QuestionType::Matching => match (self.matching_spec(), self.matching.as_ref()) {
                (Some(spec), Some(matching)) => spec
                    .pairs
                    .iter()
                    .map(|pair| {
                        let left = matching
                            .left
                            .iter()
                            .find(|item| item.id == pair.left)
                            .map(|item| item.content.plain_text())
                            .unwrap_or_else(|| pair.left.clone());
                        let right = matching
                            .right
                            .iter()
                            .find(|item| item.id == pair.right)
                            .map(|item| item.content.plain_text())
                            .unwrap_or_else(|| pair.right.clone());
                        format!("{left} → {right}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => "（正解なし）".to_string(),
            },
            QuestionType::Hotspot => match (self.hotspot_spec(), self.hotspot.as_ref()) {
                (Some(spec), Some(hotspot)) => spec
                    .region_ids
                    .iter()
                    .map(|id| {
                        hotspot
                            .regions
                            .iter()
                            .find(|region| &region.id == id)
                            .map(|region| region.alt.clone())
                            .unwrap_or_else(|| id.clone())
                    })
                    .collect::<Vec<_>>()
                    .join(" / "),
                _ => "（正解なし）".to_string(),
            },
            QuestionType::Labeling => match (self.labeling_spec(), self.labeling.as_ref()) {
                (Some(spec), Some(labeling)) => {
                    let mut parts = Vec::new();
                    for (target, label) in &spec.pairs {
                        let target_text = labeling
                            .targets
                            .iter()
                            .find(|item| &item.id == target)
                            .map(|item| item.alt.clone())
                            .unwrap_or_else(|| target.clone());
                        let label_text = labeling
                            .labels
                            .iter()
                            .find(|item| &item.id == label)
                            .map(|item| item.content.plain_text())
                            .unwrap_or_else(|| label.clone());
                        parts.push(format!("{target_text} → {label_text}"));
                    }
                    parts.join("\n")
                }
                _ => "（正解なし）".to_string(),
            },
        }
    }
}

fn format_number(value: f64) -> String {
    if (value - value.round()).abs() < 1e-9 {
        format!("{}", value.round() as i64)
    } else {
        let text = format!("{value}");
        text
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AssetRef>,
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

    /// All bundled asset IDs referenced anywhere in the workbook. Used by the
    /// linter to report unused assets without duplicating the content walk.
    pub fn referenced_asset_ids(&self) -> BTreeSet<String> {
        let mut ids = BTreeSet::new();
        for note in &self.notes {
            note.knowledge_card.referenced_asset_ids(&mut ids);
            for question in &note.questions {
                ids.extend(question.referenced_asset_ids());
            }
        }
        ids
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
        if !(1..=2).contains(&self.schema_version) {
            return Err(ValidationError::new(
                "schemaVersion",
                "schemaVersion 1 または 2 に対応しています",
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
        if self.schema_version == 1 && !self.assets.is_empty() {
            return Err(ValidationError::new(
                "assets",
                "schemaVersion 1 では画像アセットを使用できません",
            ));
        }
        if self.assets.len() > crate::question_format::MAX_ASSETS {
            return Err(ValidationError::new(
                "assets",
                format!(
                    "画像アセットは{}個以下にしてください",
                    crate::question_format::MAX_ASSETS
                ),
            ));
        }
        let mut asset_ids = BTreeSet::new();
        for (index, asset) in self.assets.iter().enumerate() {
            let path = format!("assets[{index}]");
            asset.validate(&path)?;
            if !asset_ids.insert(asset.id.clone()) {
                return Err(ValidationError::new(
                    format!("{path}.id"),
                    "画像アセットIDが重複しています",
                ));
            }
            if let Some(data) = asset.data.as_ref()
                && data.len() > crate::question_format::MAX_ASSET_BYTES * 2
            {
                return Err(ValidationError::new(
                    format!("{path}.data"),
                    "埋め込み画像が大きすぎます",
                ));
            }
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
            note.knowledge_card.validate_with_assets(
                &format!("{path}.knowledgeCard"),
                self.schema_version,
                &asset_ids,
            )?;
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
                question.validate(&qpath, self.schema_version, &asset_ids)?;
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

    /// Decodes embedded images. External file references are resolved by the
    /// import layer from the workbook's directory.
    pub fn embedded_assets(&self) -> Result<Vec<crate::AssetContent>, ValidationError> {
        let mut output = Vec::new();
        let mut total_bytes = 0usize;
        for asset in &self.assets {
            let Some(bytes) = asset.embedded_bytes()? else {
                return Err(ValidationError::new(
                    format!("assets.{}", asset.id),
                    "file 参照の画像は教材ファイルからインポートしてください",
                ));
            };
            total_bytes += bytes.len();
            if total_bytes > crate::MAX_TOTAL_ASSET_BYTES {
                return Err(ValidationError::new(
                    "assets",
                    "画像アセットの合計サイズが上限を超えています",
                ));
            }
            let (width, height) =
                inspect_image(&bytes, &asset.media_type, &format!("assets.{}", asset.id))?;
            output.push(crate::AssetContent {
                id: asset.id.clone(),
                alt: asset.alt.clone(),
                media_type: asset.media_type.clone(),
                width,
                height,
                bytes,
            });
        }
        Ok(output)
    }
}

impl Question {
    fn validate(
        &self,
        path: &str,
        schema_version: u32,
        asset_ids: &BTreeSet<String>,
    ) -> Result<(), ValidationError> {
        validate_id(&self.id, &format!("{path}.id"))?;
        if self.revision <= 0 {
            return Err(ValidationError::new(
                format!("{path}.revision"),
                "1以上の整数にしてください",
            ));
        }
        if schema_version == 1 && self.kind.requires_schema_v2() {
            return Err(ValidationError::new(
                format!("{path}.type"),
                "この問題形式は schemaVersion 2 でのみ使用できます",
            ));
        }
        if schema_version < 2 && !asset_ids.is_empty() {
            return Err(ValidationError::new(
                path.to_string(),
                "画像アセットは schemaVersion 2 でのみ使用できます",
            ));
        }
        if self.kind != QuestionType::Ordering {
            ensure_absent(self.initial_order.as_ref(), &format!("{path}.initialOrder"))?;
        }
        if self.kind != QuestionType::Matching {
            ensure_absent(self.matching.as_ref(), &format!("{path}.matching"))?;
        }
        if self.kind != QuestionType::Hotspot {
            ensure_absent(self.hotspot.as_ref(), &format!("{path}.hotspot"))?;
        }
        if self.kind != QuestionType::Labeling {
            ensure_absent(self.labeling.as_ref(), &format!("{path}.labeling"))?;
        }
        if let Some(hint) = &self.hint {
            hint.validate_with_assets(&format!("{path}.hint"), schema_version, asset_ids)?;
        }
        self.explanation.validate_with_assets(
            &format!("{path}.explanation"),
            schema_version,
            asset_ids,
        )?;
        match self.kind {
            QuestionType::SingleChoice | QuestionType::MultipleChoice => {
                let prompt = self.prompt.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.prompt"), "問題文が必要です")
                })?;
                prompt.validate_with_assets(
                    &format!("{path}.prompt"),
                    schema_version,
                    asset_ids,
                )?;
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
                    option.content.validate_with_assets(
                        &format!("{path}.options[{oi}].content"),
                        schema_version,
                        asset_ids,
                    )?;
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
                prompt.validate_with_assets(
                    &format!("{path}.prompt"),
                    schema_version,
                    asset_ids,
                )?;
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
                prompt.validate_with_assets(
                    &format!("{path}.prompt"),
                    schema_version,
                    asset_ids,
                )?;
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
                front.validate_with_assets(&format!("{path}.front"), schema_version, asset_ids)?;
                back.validate_with_assets(&format!("{path}.back"), schema_version, asset_ids)?;
            }
            QuestionType::Numeric => {
                require_prompt(self, path, schema_version, asset_ids)?;
                ensure_absent(self.options.as_ref(), &format!("{path}.options"))?;
                ensure_absent(self.front.as_ref(), &format!("{path}.front"))?;
                ensure_absent(self.back.as_ref(), &format!("{path}.back"))?;
                let spec: NumericAnswerSpec = parse_answer_spec(self, path)?;
                spec.validate(&format!("{path}.answer"))?;
            }
            QuestionType::Ordering => {
                require_prompt(self, path, schema_version, asset_ids)?;
                ensure_absent(self.front.as_ref(), &format!("{path}.front"))?;
                ensure_absent(self.back.as_ref(), &format!("{path}.back"))?;
                let options = self.options.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.options"), "並べ替える項目が必要です")
                })?;
                if !(2..=crate::question_format::MAX_ORDERING_ITEMS).contains(&options.len()) {
                    return Err(ValidationError::new(
                        format!("{path}.options"),
                        format!(
                            "項目は2〜{}個にしてください",
                            crate::question_format::MAX_ORDERING_ITEMS
                        ),
                    ));
                }
                let mut ids = BTreeSet::new();
                for (index, option) in options.iter().enumerate() {
                    validate_id(&option.id, &format!("{path}.options[{index}].id"))?;
                    if !ids.insert(option.id.clone()) {
                        return Err(ValidationError::new(
                            format!("{path}.options[{index}].id"),
                            "選択肢IDが重複しています",
                        ));
                    }
                    option.content.validate_with_assets(
                        &format!("{path}.options[{index}].content"),
                        schema_version,
                        asset_ids,
                    )?;
                }
                if let Some(initial) = self.initial_order.as_ref()
                    && (initial.len() != ids.len()
                        || initial.iter().collect::<BTreeSet<_>>().len() != initial.len()
                        || initial.iter().any(|id| !ids.contains(id)))
                {
                    return Err(ValidationError::new(
                        format!("{path}.initialOrder"),
                        "すべての項目IDを1回ずつ指定してください",
                    ));
                }
                let spec: OrderingAnswerSpec = parse_answer_spec(self, path)?;
                spec.validate(&format!("{path}.answer"), &ids)?;
            }
            QuestionType::Matching => {
                require_prompt(self, path, schema_version, asset_ids)?;
                ensure_absent(self.options.as_ref(), &format!("{path}.options"))?;
                ensure_absent(self.front.as_ref(), &format!("{path}.front"))?;
                ensure_absent(self.back.as_ref(), &format!("{path}.back"))?;
                let matching = self.matching.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.matching"), "対応付けの項目が必要です")
                })?;
                if !(2..=12).contains(&matching.left.len())
                    || !(2..=12).contains(&matching.right.len())
                    || matching.left.len() != matching.right.len()
                {
                    return Err(ValidationError::new(
                        format!("{path}.matching"),
                        "左右それぞれ2〜12個の同数の項目にしてください",
                    ));
                }
                let mut ids = BTreeSet::new();
                for (side, items) in [("left", &matching.left), ("right", &matching.right)] {
                    for (index, item) in items.iter().enumerate() {
                        validate_id(&item.id, &format!("{path}.matching.{side}[{index}].id"))?;
                        if !ids.insert(item.id.clone()) {
                            return Err(ValidationError::new(
                                format!("{path}.matching.{side}[{index}].id"),
                                "項目IDが重複しています",
                            ));
                        }
                        item.content.validate_with_assets(
                            &format!("{path}.matching.{side}[{index}].content"),
                            schema_version,
                            asset_ids,
                        )?;
                    }
                }
                let left: BTreeSet<String> =
                    matching.left.iter().map(|item| item.id.clone()).collect();
                let right: BTreeSet<String> =
                    matching.right.iter().map(|item| item.id.clone()).collect();
                let spec: MatchingAnswerSpec = parse_answer_spec(self, path)?;
                spec.validate(&format!("{path}.answer"), &left, &right)?;
            }
            QuestionType::Hotspot => {
                require_prompt(self, path, schema_version, asset_ids)?;
                ensure_absent(self.options.as_ref(), &format!("{path}.options"))?;
                ensure_absent(self.front.as_ref(), &format!("{path}.front"))?;
                ensure_absent(self.back.as_ref(), &format!("{path}.back"))?;
                let hotspot = self.hotspot.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.hotspot"), "領域データが必要です")
                })?;
                if !asset_ids.contains(&hotspot.asset) {
                    return Err(ValidationError::new(
                        format!("{path}.hotspot.asset"),
                        "assets に存在する画像IDを指定してください",
                    ));
                }
                if !(1..=100).contains(&hotspot.regions.len()) {
                    return Err(ValidationError::new(
                        format!("{path}.hotspot.regions"),
                        "領域は1〜100個にしてください",
                    ));
                }
                let mut ids = BTreeSet::new();
                for (index, region) in hotspot.regions.iter().enumerate() {
                    let region_path = format!("{path}.hotspot.regions[{index}]");
                    validate_id(&region.id, &format!("{region_path}.id"))?;
                    if !ids.insert(region.id.clone()) {
                        return Err(ValidationError::new(
                            format!("{region_path}.id"),
                            "領域IDが重複しています",
                        ));
                    }
                    if region.alt.trim().is_empty() || region.alt.chars().count() > 2_000 {
                        return Err(ValidationError::new(
                            format!("{region_path}.alt"),
                            "領域名を1〜2000文字で指定してください",
                        ));
                    }
                    region.shape.validate(&format!("{region_path}.shape"))?;
                }
                let spec: HotspotAnswerSpec = parse_answer_spec(self, path)?;
                spec.validate(&format!("{path}.answer"), &ids)?;
            }
            QuestionType::Labeling => {
                require_prompt(self, path, schema_version, asset_ids)?;
                ensure_absent(self.options.as_ref(), &format!("{path}.options"))?;
                ensure_absent(self.front.as_ref(), &format!("{path}.front"))?;
                ensure_absent(self.back.as_ref(), &format!("{path}.back"))?;
                let labeling = self.labeling.as_ref().ok_or_else(|| {
                    ValidationError::new(format!("{path}.labeling"), "ラベルデータが必要です")
                })?;
                if !asset_ids.contains(&labeling.asset) {
                    return Err(ValidationError::new(
                        format!("{path}.labeling.asset"),
                        "assets に存在する画像IDを指定してください",
                    ));
                }
                if !(1..=50).contains(&labeling.targets.len())
                    || !(1..=50).contains(&labeling.labels.len())
                    || labeling.targets.len() != labeling.labels.len()
                {
                    return Err(ValidationError::new(
                        format!("{path}.labeling"),
                        "配置先とラベルを1〜50個の同数にしてください",
                    ));
                }
                let mut target_ids = BTreeSet::new();
                for (index, target) in labeling.targets.iter().enumerate() {
                    let target_path = format!("{path}.labeling.targets[{index}]");
                    validate_id(&target.id, &format!("{target_path}.id"))?;
                    if !target_ids.insert(target.id.clone()) {
                        return Err(ValidationError::new(
                            format!("{target_path}.id"),
                            "配置先IDが重複しています",
                        ));
                    }
                    if !target.x.is_finite()
                        || !target.y.is_finite()
                        || !(0.0..=1.0).contains(&target.x)
                        || !(0.0..=1.0).contains(&target.y)
                    {
                        return Err(ValidationError::new(
                            &target_path,
                            "0〜1の正規化座標で指定してください",
                        ));
                    }
                    if target.alt.trim().is_empty() || target.alt.chars().count() > 2_000 {
                        return Err(ValidationError::new(
                            format!("{target_path}.alt"),
                            "配置先の名前を1〜2000文字で指定してください",
                        ));
                    }
                }
                let mut label_ids = BTreeSet::new();
                for (index, label) in labeling.labels.iter().enumerate() {
                    let label_path = format!("{path}.labeling.labels[{index}]");
                    validate_id(&label.id, &format!("{label_path}.id"))?;
                    if !label_ids.insert(label.id.clone()) {
                        return Err(ValidationError::new(
                            format!("{label_path}.id"),
                            "ラベルIDが重複しています",
                        ));
                    }
                    label.content.validate_with_assets(
                        &format!("{label_path}.content"),
                        schema_version,
                        asset_ids,
                    )?;
                }
                let spec: LabelingAnswerSpec = parse_answer_spec(self, path)?;
                spec.validate(&format!("{path}.answer"), &target_ids, &label_ids)?;
            }
        }
        Ok(())
    }
}

fn require_prompt(
    question: &Question,
    path: &str,
    schema_version: u32,
    asset_ids: &BTreeSet<String>,
) -> Result<(), ValidationError> {
    let prompt = question
        .prompt
        .as_ref()
        .ok_or_else(|| ValidationError::new(format!("{path}.prompt"), "問題文が必要です"))?;
    prompt.validate_with_assets(&format!("{path}.prompt"), schema_version, asset_ids)?;
    Ok(())
}

fn parse_answer_spec<T: serde::de::DeserializeOwned>(
    question: &Question,
    path: &str,
) -> Result<T, ValidationError> {
    let answer = question
        .answer
        .as_ref()
        .ok_or_else(|| ValidationError::new(format!("{path}.answer"), "正解が必要です"))?;
    serde_json::from_value(answer.clone()).map_err(|error| {
        ValidationError::new(
            format!("{path}.answer"),
            format!("回答仕様が不正です: {error}"),
        )
    })
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

/// Deserializes a workbook without running semantic validation. The linter
/// uses this to keep examining quality problems after the first validation
/// error; importers must use [`parse_workbook`].
pub fn parse_workbook_value(raw: &str) -> Result<Workbook, ValidationError> {
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
    serde_path_to_error::deserialize(deserializer).map_err(|error| {
        let path = error.path().to_string();
        let path = if path.is_empty() {
            "$".to_string()
        } else {
            path
        };
        ValidationError::new(path, format!("JSON形式: {}", error.inner()))
    })
}

pub fn parse_workbook(raw: &str) -> Result<Workbook, ValidationError> {
    let workbook = parse_workbook_value(raw)?;
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

    fn v2_raw(question: &str) -> String {
        format!(
            r#"{{"format":"workbook","schemaVersion":2,"id":"w1","revision":1,
            "title":"T","description":"","language":"ja","tags":[],
            "assets":[{{"id":"img","alt":"図","mediaType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAADklEQVR4nGP4DwYMEAoAU7oL9ZisIGcAAAAASUVORK5CYII="}}],
            "notes":[{{"id":"n1","title":"N","knowledgeCard":"card","questions":[{question}]}}]}}"#
        )
    }

    #[test]
    fn schema_v1_rejects_new_question_types() {
        let raw = MINIMAL.replace(
            r#""type":"true_false","prompt":"p","answer":true"#,
            r#""type":"numeric","prompt":"p","answer":{"value":3,"unit":"A"}"#,
        );
        let error = parse_workbook(&raw).unwrap_err();
        assert!(error.message.contains("schemaVersion 2"));
    }

    #[test]
    fn schema_v2_accepts_each_new_question_type() {
        let numeric = r#"{"id":"q","revision":1,"type":"numeric","prompt":"p","explanation":"e","answer":{"value":"3/2","unit":"m","significantFigures":2}}"#;
        let ordering = r#"{"id":"q","revision":1,"type":"ordering","prompt":"p","explanation":"e","options":[{"id":"a","content":"A"},{"id":"b","content":"B"}],"initialOrder":["b","a"],"answer":{"orders":[["a","b"]]}}"#;
        let matching = r#"{"id":"q","revision":1,"type":"matching","prompt":"p","explanation":"e","matching":{"left":[{"id":"l1","content":"A"},{"id":"l2","content":"B"}],"right":[{"id":"r1","content":"1"},{"id":"r2","content":"2"}]},"answer":{"pairs":[{"left":"l1","right":"r1"},{"left":"l2","right":"r2"}]}}"#;
        let hotspot = r#"{"id":"q","revision":1,"type":"hotspot","prompt":"p","explanation":"e","hotspot":{"asset":"img","regions":[{"id":"r","alt":"領域","shape":{"type":"circle","cx":0.5,"cy":0.5,"r":0.2}}]},"answer":{"regionIds":["r"]}}"#;
        let labeling = r#"{"id":"q","revision":1,"type":"labeling","prompt":"p","explanation":"e","labeling":{"asset":"img","targets":[{"id":"t","x":0.5,"y":0.5,"alt":"配置先"}],"labels":[{"id":"l","content":"ラベル"}]},"answer":{"pairs":{"t":"l"}}}"#;
        for question in [numeric, ordering, matching, hotspot, labeling] {
            parse_workbook(&v2_raw(question)).unwrap();
        }
    }

    #[test]
    fn schema_v2_rejects_duplicate_option_and_region_ids() {
        let ordering = r#"{"id":"q","revision":1,"type":"ordering","prompt":"p","explanation":"e","options":[{"id":"a","content":"A"},{"id":"a","content":"B"}],"answer":{"orders":[["a","a"]]}}"#;
        assert!(
            parse_workbook(&v2_raw(ordering))
                .unwrap_err()
                .message
                .contains("重複")
        );
        let hotspot = r#"{"id":"q","revision":1,"type":"hotspot","prompt":"p","explanation":"e","hotspot":{"asset":"img","regions":[{"id":"r","alt":"A","shape":{"type":"rect","x":0.0,"y":0.0,"w":0.5,"h":0.5}},{"id":"r","alt":"B","shape":{"type":"circle","cx":0.5,"cy":0.5,"r":0.1}}]},"answer":{"regionIds":["r"]}}"#;
        assert!(
            parse_workbook(&v2_raw(hotspot))
                .unwrap_err()
                .message
                .contains("重複")
        );
    }

    #[test]
    fn schema_v2_rejects_out_of_range_hotspot_coordinates() {
        let hotspot = r#"{"id":"q","revision":1,"type":"hotspot","prompt":"p","explanation":"e","hotspot":{"asset":"img","regions":[{"id":"r","alt":"A","shape":{"type":"rect","x":1.2,"y":0.0,"w":0.5,"h":0.5}}]},"answer":{"regionIds":["r"]}}"#;
        let error = parse_workbook(&v2_raw(hotspot)).unwrap_err();
        assert!(error.message.contains("正規化"));
    }

    #[test]
    fn schema_v2_rejects_matching_assignments_that_are_not_bijections() {
        let matching = r#"{"id":"q","revision":1,"type":"matching","prompt":"p","explanation":"e","matching":{"left":[{"id":"l1","content":"A"},{"id":"l2","content":"B"}],"right":[{"id":"r1","content":"1"},{"id":"r2","content":"2"}]},"answer":{"pairs":[{"left":"l1","right":"r1"},{"left":"l2","right":"r1"}]}}"#;
        let error = parse_workbook(&v2_raw(matching)).unwrap_err();
        assert!(error.message.contains("重複"));
    }

    #[test]
    fn repeated_asset_ids_are_rejected() {
        let raw = v2_raw(
            r#"{"id":"q","revision":1,"type":"hotspot","prompt":"p","explanation":"e","hotspot":{"asset":"img","regions":[{"id":"r","alt":"A","shape":{"type":"circle","cx":0.5,"cy":0.5,"r":0.1}}]},"answer":{"regionIds":["r"]}}"#,
        )
        .replace(
            r#""assets":[{"id":"img","alt":"図","mediaType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAADklEQVR4nGP4DwYMEAoAU7oL9ZisIGcAAAAASUVORK5CYII="}]"#,
            r#""assets":[{"id":"img","alt":"図","mediaType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAADklEQVR4nGP4DwYMEAoAU7oL9ZisIGcAAAAASUVORK5CYII="},{"id":"img","alt":"図2","mediaType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAADklEQVR4nGP4DwYMEAoAU7oL9ZisIGcAAAAASUVORK5CYII="}]"#,
        );
        let error = parse_workbook(&raw).unwrap_err();
        assert!(error.message.contains("重複"));
    }

    #[test]
    fn unsafe_asset_paths_and_media_type_mismatches_are_rejected() {
        for path in ["../secret.png", "/tmp/a.png", "images/../../a.png", "a.svg"] {
            let raw = v2_raw(
                r#"{"id":"q","revision":1,"type":"hotspot","prompt":"p","explanation":"e","hotspot":{"asset":"img","regions":[{"id":"r","alt":"A","shape":{"type":"circle","cx":0.5,"cy":0.5,"r":0.1}}]},"answer":{"regionIds":["r"]}}"#,
            )
            .replace(
                r#"{"id":"img","alt":"図","mediaType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAADklEQVR4nGP4DwYMEAoAU7oL9ZisIGcAAAAASUVORK5CYII="}"#,
                &format!(r#"{{"id":"img","alt":"図","mediaType":"image/png","file":"{path}"}}"#),
            );
            assert!(parse_workbook(&raw).is_err(), "{path} should be rejected");
        }
    }

    #[test]
    fn safe_svg_assets_are_accepted_at_parse_time() {
        let raw = v2_raw(
            r#"{"id":"q","revision":1,"type":"hotspot","prompt":"p","explanation":"e","hotspot":{"asset":"img","regions":[{"id":"r","alt":"A","shape":{"type":"circle","cx":0.5,"cy":0.5,"r":0.1}}]},"answer":{"regionIds":["r"]}}"#,
        )
        .replace(
            r#"{"id":"img","alt":"図","mediaType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAADklEQVR4nGP4DwYMEAoAU7oL9ZisIGcAAAAASUVORK5CYII="}"#,
            r#"{"id":"img","alt":"図","mediaType":"image/svg+xml","file":"map.svg"}"#,
        );
        let workbook = parse_workbook(&raw).unwrap();
        assert_eq!(workbook.assets[0].media_type, "image/svg+xml");
    }

    #[test]
    fn embedded_assets_must_be_valid_images() {
        let raw = v2_raw(
            r#"{"id":"q","revision":1,"type":"hotspot","prompt":"p","explanation":"e","hotspot":{"asset":"img","regions":[{"id":"r","alt":"A","shape":{"type":"circle","cx":0.5,"cy":0.5,"r":0.1}}]},"answer":{"regionIds":["r"]}}"#,
        )
        .replace(
            "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAADklEQVR4nGP4DwYMEAoAU7oL9ZisIGcAAAAASUVORK5CYII=",
            "AA==",
        );
        let workbook = parse_workbook(&raw).unwrap();
        assert!(workbook.embedded_assets().is_err());
    }
}
