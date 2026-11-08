use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    New,
    Review,
    Practice,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Review => "review",
            Self::Practice => "practice",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "new" => Some(Self::New),
            "review" => Some(Self::Review),
            "practice" => Some(Self::Practice),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Intro,
    Question,
    Feedback,
    WorkbookResult,
    OverallResult,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intro => "intro",
            Self::Question => "question",
            Self::Feedback => "feedback",
            Self::WorkbookResult => "workbookResult",
            Self::OverallResult => "overallResult",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "intro" => Some(Self::Intro),
            "question" => Some(Self::Question),
            "feedback" => Some(Self::Feedback),
            "workbookResult" => Some(Self::WorkbookResult),
            "overallResult" => Some(Self::OverallResult),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionItemStatus {
    Pending,
    Answered,
    Skipped,
}

impl SessionItemStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Answered => "answered",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "answered" => Some(Self::Answered),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum Rating {
    Again = 1,
    Hard = 2,
    Good = 3,
    Easy = 4,
}

impl Rating {
    pub fn value(self) -> u8 {
        self as u8
    }

    pub fn from_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Again),
            2 => Some(Self::Hard),
            3 => Some(Self::Good),
            4 => Some(Self::Easy),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Again => "Again",
            Self::Hard => "Hard",
            Self::Good => "Good",
            Self::Easy => "Easy",
        }
    }

    pub fn japanese_label(self) -> &'static str {
        match self {
            Self::Again => "もう一度",
            Self::Hard => "難しい",
            Self::Good => "できた",
            Self::Easy => "簡単",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum CardState {
    New = 0,
    Learning = 1,
    Review = 2,
    Relearning = 3,
}

impl CardState {
    pub fn value(self) -> u8 {
        self as u8
    }

    pub fn from_value(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::New),
            1 => Some(Self::Learning),
            2 => Some(Self::Review),
            3 => Some(Self::Relearning),
            _ => None,
        }
    }

    pub fn is_short_term(self) -> bool {
        matches!(self, Self::Learning | Self::Relearning)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredCard {
    pub stability: f32,
    pub stability_fast: f32,
    pub difficulty: f32,
    pub state: CardState,
    pub due_at: DateTime<Utc>,
    pub last_review_at: Option<DateTime<Utc>>,
    pub reps: u32,
    pub lapses: u32,
    pub scheduled_days: f64,
    pub learning_step: u32,
    pub algorithm: String,
    pub policy_version: String,
    pub revision: u64,
}

impl StoredCard {
    pub fn is_new(&self) -> bool {
        self.state == CardState::New
    }

    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.due_at <= now
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentQuestionRef {
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
}
