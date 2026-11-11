-- Workbook 2 schema v5, copied verbatim from commit 70608ab
-- (schema.rs at that commit). Used only by migration tests.

CREATE TABLE workbooks(
  local_id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL,
  source_revision INTEGER NOT NULL,
  title TEXT NOT NULL,
  content_json TEXT NOT NULL,
  imported_at TEXT NOT NULL,
  last_used_at TEXT
);

CREATE TABLE notes(
  workbook_id TEXT NOT NULL REFERENCES workbooks(local_id) ON DELETE CASCADE,
  note_id TEXT NOT NULL,
  introduced INTEGER NOT NULL DEFAULT 0,
  introduced_at TEXT,
  PRIMARY KEY(workbook_id, note_id)
);

CREATE TABLE questions(
  workbook_id TEXT NOT NULL,
  question_id TEXT NOT NULL,
  note_id TEXT NOT NULL,
  buried_until TEXT,
  PRIMARY KEY(workbook_id, question_id),
  FOREIGN KEY(workbook_id, note_id) REFERENCES notes(workbook_id, note_id) ON DELETE CASCADE
);
CREATE INDEX questions_note ON questions(workbook_id, note_id);

CREATE TABLE assets(
  workbook_id TEXT NOT NULL REFERENCES workbooks(local_id) ON DELETE CASCADE,
  asset_id TEXT NOT NULL,
  media_type TEXT NOT NULL,
  alt TEXT NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  bytes BLOB NOT NULL,
  PRIMARY KEY(workbook_id, asset_id)
);

CREATE TABLE exams(
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  description TEXT,
  target_date TEXT,
  target_score REAL,
  daily_minutes INTEGER NOT NULL DEFAULT 30,
  scope_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  archived_at TEXT
);
CREATE INDEX exams_archive ON exams(archived_at, target_date);

CREATE TABLE mock_answers(
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  item_id TEXT NOT NULL UNIQUE REFERENCES session_items(id) ON DELETE CASCADE,
  workbook_id TEXT NOT NULL,
  note_id TEXT NOT NULL,
  question_id TEXT NOT NULL,
  answered_at TEXT NOT NULL,
  correct INTEGER,
  answer_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX mock_answers_session ON mock_answers(session_id, answered_at);

CREATE TABLE mock_results(
  session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
  exam_id TEXT NOT NULL REFERENCES exams(id) ON DELETE CASCADE,
  submitted_at TEXT NOT NULL,
  total INTEGER NOT NULL,
  correct INTEGER NOT NULL,
  incorrect INTEGER NOT NULL,
  unanswered INTEGER NOT NULL,
  ungraded INTEGER NOT NULL,
  duration_ms INTEGER NOT NULL
);
CREATE INDEX mock_results_exam ON mock_results(exam_id, submitted_at DESC);

CREATE TABLE review_states(
  workbook_id TEXT NOT NULL,
  question_id TEXT NOT NULL,
  state INTEGER NOT NULL,
  stability REAL NOT NULL,
  stability_fast REAL NOT NULL,
  difficulty REAL NOT NULL,
  due_at TEXT NOT NULL,
  last_review_at TEXT,
  reps INTEGER NOT NULL,
  lapses INTEGER NOT NULL,
  scheduled_days REAL NOT NULL,
  learning_step INTEGER NOT NULL,
  algorithm TEXT NOT NULL,
  policy_version TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(workbook_id, question_id),
  FOREIGN KEY(workbook_id, question_id) REFERENCES questions(workbook_id, question_id) ON DELETE CASCADE
);
CREATE INDEX review_states_due ON review_states(due_at, workbook_id);

CREATE TABLE sessions(
  id TEXT PRIMARY KEY,
  mode TEXT NOT NULL,
  status TEXT NOT NULL,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  current_workbook_id TEXT,
  current_item_id TEXT,
  phase TEXT NOT NULL,
  draft_json TEXT,
  hint INTEGER NOT NULL DEFAULT 0,
  revealed INTEGER NOT NULL DEFAULT 0,
  pending_correct INTEGER,
  pending_unknown INTEGER NOT NULL DEFAULT 0,
  pending_at TEXT,
  question_started_at TEXT,
  question_elapsed_ms INTEGER NOT NULL DEFAULT 0,
  cursor_kind TEXT NOT NULL DEFAULT 'initial',
  assist_json TEXT,
  revision INTEGER NOT NULL DEFAULT 0,
  exam_id TEXT REFERENCES exams(id) ON DELETE SET NULL,
  time_limit_seconds INTEGER
);
CREATE UNIQUE INDEX one_active_session ON sessions(status) WHERE status = 'active';

CREATE TABLE session_workbooks(
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  workbook_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  result_ack INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(session_id, workbook_id)
);

CREATE TABLE session_items(
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL,
  workbook_id TEXT NOT NULL,
  note_id TEXT NOT NULL,
  question_id TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  UNIQUE(session_id, ordinal)
);
CREATE INDEX session_items_queue ON session_items(session_id, workbook_id, status, ordinal);

CREATE TABLE attempts(
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  workbook_id TEXT NOT NULL,
  note_id TEXT NOT NULL,
  question_id TEXT NOT NULL,
  answered_at TEXT NOT NULL,
  correct INTEGER,
  unknown INTEGER NOT NULL,
  hint INTEGER NOT NULL,
  elapsed_ms INTEGER NOT NULL,
  rating INTEGER NOT NULL,
  rating_source TEXT NOT NULL DEFAULT 'manual',
  answer_json TEXT NOT NULL,
  before_state_json TEXT,
  after_state_json TEXT,
  state_revision INTEGER NOT NULL,
  short_term INTEGER NOT NULL,
  assist_json TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX attempts_session ON attempts(session_id, answered_at);
CREATE INDEX attempts_question ON attempts(workbook_id, question_id, answered_at);

CREATE TABLE settings(
  id INTEGER PRIMARY KEY CHECK(id = 1),
  data TEXT NOT NULL
);

CREATE TABLE meta(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
