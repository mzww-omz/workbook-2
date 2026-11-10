//! Topics are arbitrary study areas: a school subject, a unit, a skill, or a
//! personal project label. They are not tied to a school system.
//!
//! Identity is the stable `id`. A topic may have a parent, but a flat list of
//! topics is equally valid. Two topics with the same name under different
//! parents are distinct; only the same normalized name under the same parent
//! resolves to one topic. This prevents accidental duplicates without ever
//! merging unrelated study areas by name alone.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::workbook::ValidationError;

pub const MAX_TOPIC_NAME_CHARS: usize = 200;
pub const MAX_TOPIC_DESCRIPTION_CHARS: usize = 2_000;
pub const MAX_TOPIC_DEPTH: usize = 32;
pub const TOPIC_PATH_SEPARATOR: char = '/';

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Topic {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub description: String,
}

impl Topic {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.id.trim().is_empty() || self.id.chars().count() > 160 {
            return Err(ValidationError::new("id", "Topic IDが不正です"));
        }
        validate_topic_name(&self.name, "name")?;
        if self.description.chars().count() > MAX_TOPIC_DESCRIPTION_CHARS {
            return Err(ValidationError::new(
                "description",
                "説明は2000文字以内にしてください",
            ));
        }
        if self.parent_id.as_deref() == Some(self.id.as_str()) {
            return Err(ValidationError::new(
                "parentId",
                "自分自身を親Topicにはできません",
            ));
        }
        Ok(())
    }
}

pub fn validate_topic_name(name: &str, path: &str) -> Result<String, ValidationError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ValidationError::new(path, "Topic名を入力してください"));
    }
    if trimmed.chars().count() > MAX_TOPIC_NAME_CHARS {
        return Err(ValidationError::new(
            path,
            format!("Topic名は{MAX_TOPIC_NAME_CHARS}文字以内にしてください"),
        ));
    }
    if trimmed.contains(TOPIC_PATH_SEPARATOR) {
        return Err(ValidationError::new(
            path,
            "Topic名に / は使えません（階層は親Topicで指定します）",
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(ValidationError::new(path, "Topic名に制御文字は使えません"));
    }
    Ok(trimmed.to_string())
}

/// NFKC + case folding + whitespace collapse. Used only to detect accidental
/// duplicates inside one parent; it is never used as the identity itself.
pub fn normalize_topic_name(name: &str) -> String {
    name.nfkc()
        .flat_map(|character| character.to_lowercase())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parses an authored topic path such as `数学/一次関数`. Empty segments and
/// over-deep paths are rejected instead of being silently repaired.
pub fn parse_topic_path(path: &str) -> Result<Vec<String>, ValidationError> {
    let segments: Vec<&str> = path.split(TOPIC_PATH_SEPARATOR).collect();
    if segments.len() > MAX_TOPIC_DEPTH {
        return Err(ValidationError::new(
            "topics",
            format!("Topicの階層は{MAX_TOPIC_DEPTH}段までです"),
        ));
    }
    let mut output = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let name = validate_topic_name(segment, &format!("topics[{index}]"))?;
        output.push(name);
    }
    if output.is_empty() {
        return Err(ValidationError::new("topics", "Topic名を入力してください"));
    }
    Ok(output)
}

/// Human-readable path for display. Cycles or missing parents stop at the
/// last resolvable name instead of looping.
pub fn topic_path(topics: &[Topic], id: &str) -> Option<Vec<String>> {
    let by_id: BTreeMap<&str, &Topic> = topics
        .iter()
        .map(|topic| (topic.id.as_str(), topic))
        .collect();
    let mut current = by_id.get(id).copied()?;
    let mut names = vec![current.name.clone()];
    let mut seen = BTreeSet::new();
    seen.insert(current.id.as_str());
    while let Some(parent_id) = current.parent_id.as_deref() {
        if !seen.insert(parent_id) {
            break;
        }
        let Some(parent) = by_id.get(parent_id).copied() else {
            break;
        };
        names.push(parent.name.clone());
        current = parent;
    }
    names.reverse();
    Some(names)
}

/// Selecting a parent topic also selects its descendants. Unknown IDs remain
/// in the set so a saved scope keeps referring to a topic that is temporarily
/// absent (for example during a restore preview); resolution simply ignores
/// them until the topic exists again.
pub fn expand_topic_ids<'a>(
    topics: &[Topic],
    selected: impl IntoIterator<Item = &'a String>,
) -> BTreeSet<String> {
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for topic in topics {
        if let Some(parent) = topic.parent_id.as_deref() {
            children.entry(parent).or_default().push(topic.id.as_str());
        }
    }
    let mut output = BTreeSet::new();
    let mut queue: VecDeque<String> = selected.into_iter().cloned().collect();
    while let Some(id) = queue.pop_front() {
        if !output.insert(id.clone()) {
            continue;
        }
        if let Some(child_ids) = children.get(id.as_str()) {
            for child in child_ids {
                queue.push_back((*child).to_string());
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic(id: &str, name: &str, parent: Option<&str>) -> Topic {
        Topic {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: parent.map(str::to_string),
            description: String::new(),
        }
    }

    #[test]
    fn names_are_normalized_for_duplicate_detection_but_not_identity() {
        assert_eq!(normalize_topic_name("  一次 関数 "), "一次 関数");
        assert_eq!(normalize_topic_name("ＭＡＴＨ"), "math");
        assert_eq!(normalize_topic_name("Math"), "math");
    }

    #[test]
    fn a_topic_path_rejects_empty_segments_and_slashes_in_names() {
        assert_eq!(
            parse_topic_path("数学/一次関数").unwrap(),
            ["数学", "一次関数"]
        );
        assert!(parse_topic_path("数学//一次関数").is_err());
        assert!(parse_topic_path(" / ").is_err());
        assert!(parse_topic_path("").is_err());
    }

    #[test]
    fn the_same_name_under_different_parents_stays_distinct() {
        let topics = vec![
            topic("math", "数学", None),
            topic("science", "理科", None),
            topic("linear", "一次関数", Some("math")),
            topic("science-linear", "一次関数", Some("science")),
        ];
        assert_eq!(topic_path(&topics, "linear").unwrap(), ["数学", "一次関数"]);
        assert_eq!(
            topic_path(&topics, "science-linear").unwrap(),
            ["理科", "一次関数"]
        );
        let expanded = expand_topic_ids(&topics, [&"math".to_string()]);
        assert!(expanded.contains("math"));
        assert!(expanded.contains("linear"));
        assert!(!expanded.contains("science-linear"));
    }

    #[test]
    fn selecting_a_parent_includes_all_descendants() {
        let topics = vec![
            topic("a", "A", None),
            topic("b", "B", Some("a")),
            topic("c", "C", Some("b")),
            topic("d", "D", None),
        ];
        let expanded = expand_topic_ids(&topics, [&"a".to_string()]);
        assert_eq!(expanded.len(), 3);
        assert!(expanded.contains("c"));
        assert!(!expanded.contains("d"));
    }

    #[test]
    fn cycles_in_imported_data_do_not_loop_forever() {
        let topics = vec![topic("a", "A", Some("b")), topic("b", "B", Some("a"))];
        assert!(topic_path(&topics, "a").is_some());
    }

    #[test]
    fn topic_validation_rejects_self_parents_and_bad_names() {
        let mut topic = topic("a", "A", None);
        assert!(topic.validate().is_ok());
        topic.parent_id = Some("a".to_string());
        assert!(topic.validate().is_err());
        topic.parent_id = None;
        topic.name = "a/b".to_string();
        assert!(topic.validate().is_err());
        topic.name = " ".to_string();
        assert!(topic.validate().is_err());
    }
}
