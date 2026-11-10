//! Pure Workbook domain types and validation.
//!
//! This crate intentionally has no dependency on GPUI, SQLite, or any
//! scheduling library. Content, questions, grading, and the 4 a.m. study
//! calendar live here so they can be tested without a desktop session.

pub mod answer;
pub mod calendar;
pub mod content;
pub mod exam;
pub mod grading;
pub mod question_format;
pub mod session;
pub mod settings;
pub mod topic;
pub mod workbook;

pub use answer::{Answer, answer_is_blank, automatic_rating, grade_answer, normalize_answer};
pub use calendar::{StudyCalendar, learning_day, next_learning_day_start};
pub use content::{Content, ContentBlock, ImageBlock, ImageFit, ImageSource};
pub use exam::{
    Exam, ExamCatalogQuestion, ExamDraft, ExamScope, ExamScopeGroup, ExamStudyMode, MockOptions,
    MockOrder, PracticeOptions, PracticeStrategy, QuestionRef, resolve_scope,
};

/// The general name for the same composable selection rules used by exams and
/// goals. Exam names remain for backward compatibility.
pub type StudyScope = ExamScope;
pub use grading::{
    AssistState, GradeDiagnostic, GradeResult, GradeStatus, NearMissReason, NearMissRecord,
    grade_answer_detailed,
};
pub use question_format::{
    AssetContent, AssetRef, HotspotAnswerSpec, HotspotRegion, HotspotShape, HotspotSpec,
    LabelOption, LabelTarget, LabelingAnswerSpec, LabelingSpec, MAX_ASSET_BYTES, MAX_ASSETS,
    MAX_TOTAL_ASSET_BYTES, MatchItem, MatchPair, MatchingAnswerSpec, MatchingSpec,
    NumericAnswerSpec, NumericLiteral, OrderingAnswerSpec, inspect_image, known_unit,
    normalize_unit, parse_numeric_answer, validate_relative_asset_path,
};
pub use session::{
    CardState, Mode, Phase, Rating, RatingSource, SessionItemStatus, SessionStatus, StoredCard,
};
pub use settings::{Settings, ThemeSetting};
pub use topic::{
    MAX_TOPIC_DEPTH, MAX_TOPIC_DESCRIPTION_CHARS, MAX_TOPIC_NAME_CHARS, Topic, expand_topic_ids,
    normalize_topic_name, parse_topic_path, topic_path, validate_topic_name,
};
pub use workbook::{
    ChoiceOption, MAX_JSON_BYTES, Note, Question, QuestionType, ValidationError, Workbook,
    parse_workbook, parse_workbook_value,
};
