//! Development-only demo data seeder used for screenshots and manual QA.
//!
//! Usage:
//!   cargo run -p workbook-app --bin seed_demo -- /tmp/workbook2-demo today
//!   cargo run -p workbook-app --bin seed_demo -- /tmp/workbook2-demo feedback
//!
//! Profiles: today, library, history, settings, study, feedback, result,
//! v2-numeric, v2-ordering, v2-matching, v2-hotspot, v2-labeling, v2-near-miss,
//! v2-math.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use chrono::{Duration, Utc};
use workbook_domain::{Answer, Mode, Phase, Question, QuestionType, Rating};
use workbook_scheduler::FixedClock;
use workbook_service::{StudyView, WorkbookService};

/// Marker file that proves a directory belongs to this development seeder.
/// A directory without this marker is never modified.
const SEED_MARKER: &str = ".workbook2-demo-seed";
const SEED_MARKER_CONTENT: &[u8] = b"workbook2-demo-seed-v1\n";

/// Only these Seeder-owned entries are removed when re-seeding. Anything else
/// in the directory (including user files) is left untouched.
const SEED_MANAGED_FILES: &[&str] = &[
    "workbook.sqlite",
    "workbook.sqlite-wal",
    "workbook.sqlite-shm",
];
const SEED_MANAGED_DIRS: &[&str] = &["backups"];

/// Absolute locations that must never be used as a seed directory, even if a
/// marker file happens to exist there.
const PROTECTED_ROOTS: &[&str] = &[
    "/Users",
    "/Applications",
    "/System",
    "/Library",
    "/tmp",
    "/private",
    "/private/tmp",
    "/var",
    "/private/var",
    "/etc",
    "/usr",
    "/bin",
    "/sbin",
    "/opt",
    "/dev",
    "/Volumes",
];

#[derive(Clone, Debug)]
struct SeedPaths {
    cwd: PathBuf,
    home: Option<PathBuf>,
}

impl SeedPaths {
    fn current() -> Self {
        let cwd = std::env::current_dir()
            .ok()
            .and_then(|path| path.canonicalize().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .and_then(|path| path.canonicalize().ok());
        Self { cwd, home }
    }
}

fn validate_profile(profile: &str) -> anyhow::Result<()> {
    let simple = matches!(
        profile,
        "today" | "library" | "history" | "settings" | "study" | "feedback" | "result" | "upcoming"
    );
    let expanded = profile.strip_prefix("v2-").is_some_and(|target| {
        matches!(
            target,
            "numeric" | "near-miss" | "ordering" | "matching" | "hotspot" | "labeling" | "math"
        )
    });
    if !simple && !expanded {
        anyhow::bail!("unknown profile: {profile}");
    }
    Ok(())
}

/// Prepares a Seeder-only directory without ever deleting an arbitrary path.
///
/// * A new directory is created and marked.
/// * An existing directory is accepted only when it contains the exact Seeder
///   marker. Then only the SQLite database and `backups/` are removed.
/// * Relative paths, `..`, symlinks, the project root/home, their ancestors,
///   and system directories are rejected before anything is written.
fn prepare_seed_directory(directory: &Path) -> anyhow::Result<()> {
    prepare_seed_directory_with(directory, &SeedPaths::current())
}

fn prepare_seed_directory_with(directory: &Path, paths: &SeedPaths) -> anyhow::Result<()> {
    validate_seed_path(directory, paths)?;
    if directory.exists() {
        verify_seed_marker(directory)?;
        clear_seed_data(directory)?;
    } else {
        std::fs::create_dir_all(directory)?;
        // Re-check after creation: an ancestor may have been a symlink.
        let canonical = directory.canonicalize()?;
        validate_seed_path(&canonical, paths)?;
        write_seed_marker(directory)?;
    }
    Ok(())
}

fn validate_seed_path(directory: &Path, paths: &SeedPaths) -> anyhow::Result<()> {
    if directory.as_os_str().is_empty() {
        anyhow::bail!("保存先を指定してください");
    }
    if !directory.is_absolute() {
        anyhow::bail!(
            "保存先は絶対パスで指定してください（例: /tmp/workbook2-demo）: {}",
            directory.display()
        );
    }
    if directory
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        anyhow::bail!(
            "保存先に . や .. を含めることはできません: {}",
            directory.display()
        );
    }
    if let Ok(metadata) = std::fs::symlink_metadata(directory) {
        if metadata.file_type().is_symlink() {
            anyhow::bail!(
                "保存先をシンボリックリンクにすることはできません: {}",
                directory.display()
            );
        }
        if !metadata.is_dir() {
            anyhow::bail!(
                "保存先がディレクトリではありません: {}",
                directory.display()
            );
        }
    }

    let normalized = normalize_existing_prefix(directory)?;
    reject_sensitive_path(&normalized, paths)?;
    reject_sensitive_path(directory, paths)?;
    Ok(())
}

/// Canonicalizes the longest existing prefix and appends the missing
/// components. This resolves macOS aliases such as `/tmp -> /private/tmp`
/// without requiring the final directory to exist yet.
fn normalize_existing_prefix(path: &Path) -> anyhow::Result<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut suffix = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(|name| name.to_os_string()) else {
            anyhow::bail!("保存先を解決できません: {}", path.display());
        };
        suffix.push(name);
        if !existing.pop() {
            anyhow::bail!("保存先を解決できません: {}", path.display());
        }
    }
    let mut normalized = existing.canonicalize()?;
    for component in suffix.into_iter().rev() {
        normalized.push(component);
    }
    Ok(normalized)
}

fn reject_sensitive_path(path: &Path, paths: &SeedPaths) -> anyhow::Result<()> {
    if path.parent().is_none() {
        anyhow::bail!("ルートディレクトリは保存先にできません");
    }
    if path == paths.cwd || paths.cwd.starts_with(path) {
        anyhow::bail!(
            "プロジェクトルートやその親ディレクトリは保存先にできません: {}",
            path.display()
        );
    }
    if let Some(home) = paths.home.as_ref()
        && (path == home || home.starts_with(path))
    {
        anyhow::bail!(
            "ホームディレクトリやその親は保存先にできません: {}",
            path.display()
        );
    }
    if PROTECTED_ROOTS
        .iter()
        .any(|protected| path == Path::new(protected))
    {
        anyhow::bail!(
            "システムディレクトリは保存先にできません: {}",
            path.display()
        );
    }
    Ok(())
}

fn verify_seed_marker(directory: &Path) -> anyhow::Result<()> {
    let marker = directory.join(SEED_MARKER);
    let metadata = std::fs::symlink_metadata(&marker).map_err(|_| {
        anyhow::anyhow!(
            "既存ディレクトリはSeeder管理下ではありません（マーカー {} がありません）。データを変更せず終了しました: {}",
            SEED_MARKER,
            directory.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!(
            "Seederマーカーが安全なファイルではありません: {}",
            marker.display()
        );
    }
    let content = std::fs::read(&marker)?;
    if content != SEED_MARKER_CONTENT {
        anyhow::bail!(
            "Seederマーカーの内容が一致しません。データを変更せず終了しました: {}",
            marker.display()
        );
    }
    Ok(())
}

fn write_seed_marker(directory: &Path) -> anyhow::Result<()> {
    let marker = directory.join(SEED_MARKER);
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(mut file) => {
            file.write_all(SEED_MARKER_CONTENT)?;
            file.sync_all()?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another invocation claimed the directory. Accept it only when it
            // wrote the same marker.
            let content = std::fs::read(&marker)?;
            if content == SEED_MARKER_CONTENT {
                Ok(())
            } else {
                anyhow::bail!("Seederマーカーの内容が一致しません: {}", marker.display())
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn clear_seed_data(directory: &Path) -> anyhow::Result<()> {
    // Validate every managed entry before removing anything. A symlink or an
    // unexpected file type must abort without a partial reset.
    let mut files = Vec::new();
    for name in SEED_MANAGED_FILES {
        let path = directory.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || metadata.is_dir() {
                    anyhow::bail!(
                        "Seeder管理ファイルが通常ファイルではありません: {}",
                        path.display()
                    );
                }
                files.push(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let mut dirs = Vec::new();
    for name in SEED_MANAGED_DIRS {
        let path = directory.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    anyhow::bail!(
                        "Seeder管理ディレクトリが通常ディレクトリではありません: {}",
                        path.display()
                    );
                }
                dirs.push(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    for path in files {
        std::fs::remove_file(&path)?;
    }
    for path in dirs {
        std::fs::remove_dir_all(&path)?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let directory = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "/tmp/workbook2-demo".to_string()),
    );
    let profile = args.next().unwrap_or_else(|| "today".to_string());
    validate_profile(&profile)?;
    prepare_seed_directory(&directory)?;
    let start = if profile == "upcoming" {
        Utc::now()
    } else {
        Utc::now() - Duration::days(60)
    };
    let clock = Arc::new(FixedClock::new(start));
    let service = WorkbookService::open(&directory, clock.clone())?;
    if profile == "upcoming" {
        seed_upcoming(&service)?;
        println!("seeded profile={} at {}", profile, directory.display());
        return Ok(());
    }
    if profile == "v2-math" {
        seed_math(&service)?;
        println!("seeded profile={} at {}", profile, directory.display());
        return Ok(());
    }
    if let Some(target) = profile.strip_prefix("v2-") {
        seed_expanded(&service, target)?;
        println!("seeded profile={} at {}", profile, directory.display());
        return Ok(());
    }
    let book_id = service.import_workbook_json(workbook_service::SAMPLE_WORKBOOK_JSON)?;

    finish_new_session(&service, &book_id)?;
    clock.advance(Duration::days(45));

    match profile.as_str() {
        "study" => {
            let view = start_review(&service)?;
            wait_for_question(&service, view)?;
        }
        "feedback" => {
            let view = start_review(&service)?;
            let view = wait_for_question(&service, view)?;
            let view = prepare_answer(&service, &view)?;
            let (answer, self_correct) = correct_answer(view.question.as_ref().unwrap());
            let pending = service.evaluate_answer(
                &view.session_id,
                answer.as_ref(),
                false,
                self_correct,
                false,
                view.revision,
            )?;
            assert_eq!(pending.phase, Phase::Feedback);
        }
        "result" => {
            let mut view = start_review(&service)?;
            loop {
                match view.phase {
                    Phase::WorkbookResult | Phase::OverallResult => break,
                    Phase::Intro => {
                        view = service.begin_question(&view.session_id, view.revision)?;
                    }
                    Phase::Question => {
                        let prepared = prepare_answer(&service, &view)?;
                        let (answer, self_correct) =
                            correct_answer(prepared.question.as_ref().unwrap());
                        let pending = service.evaluate_answer(
                            &prepared.session_id,
                            answer.as_ref(),
                            false,
                            self_correct,
                            false,
                            prepared.revision,
                        )?;
                        view = ensure_committed(&service, pending, Rating::Good)?;
                    }
                    Phase::Feedback => {
                        view = service.next(&view.session_id, view.revision)?;
                    }
                }
            }
        }
        "today" | "library" | "history" | "settings" => {}
        other => anyhow::bail!("unknown profile: {other}"),
    }

    println!("seeded profile={} at {}", profile, directory.display());
    Ok(())
}

/// Math, inline math, image fit, and compact option rendering for manual QA
/// and the `docs/screenshots/math.png` capture.
fn seed_math(service: &WorkbookService) -> anyhow::Result<()> {
    let book = service.import_workbook_json(MATH_DEMO_JSON)?;
    service.start_session(Mode::Practice, Some(&book))?;
    let view = service.active_view()?.expect("active practice session");
    wait_for_question(service, view)?;
    Ok(())
}

const MATH_DEMO_JSON: &str = r#"{
  "format":"workbook","schemaVersion":2,"id":"workbook-math-demo","revision":1,
  "title":"数式と画像の表示確認","description":"数式の組版、インライン数式、contain と cover。",
  "language":"ja","tags":["確認"],
  "assets":[
    {"id":"wide-diagram","alt":"横長の見本画像","mediaType":"image/svg+xml",
     "data":"PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI4MDAiIGhlaWdodD0iMzAwIiB2aWV3Qm94PSIwIDAgODAwIDMwMCI+PHJlY3Qgd2lkdGg9IjgwMCIgaGVpZ2h0PSIzMDAiIGZpbGw9IiNlOGVkZjUiLz48cmVjdCB4PSIwIiB5PSIwIiB3aWR0aD0iNDAwIiBoZWlnaHQ9IjMwMCIgZmlsbD0iIzlkYjRkNiIvPjxjaXJjbGUgY3g9IjYwMCIgY3k9IjE1MCIgcj0iOTAiIGZpbGw9IiNjOTZiNWEiLz48cmVjdCB4PSI4MCIgeT0iMTEwIiB3aWR0aD0iMjQwIiBoZWlnaHQ9IjgwIiBmaWxsPSIjNGY3YTUyIi8+PC9zdmc+"}
  ],
  "notes":[{"id":"n-math","title":"数式の表示","knowledgeCard":[
    {"type":"math","text":"\\frac{1}{2}"},
    {"type":"math","text":"x^2 + 2x + 1"},
    {"type":"math","text":"\\sqrt{3}"}
  ],"questions":[
    {"id":"q-math","revision":1,"type":"single_choice",
     "prompt":[
       {"type":"math","text":"\\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}"},
       {"type":"math","text":"a_1 + a_2 + \\sum_{i=1}^{n} i"},
       {"type":"math","text":"\\sin\\theta + |x| + \\text{速さ}"},
       {"type":"markdown","text":"インライン数式: 速さは $v=\\frac{d}{t}$ です。"},
       {"type":"image","asset":"wide-diagram","alt":"横長画像（contain）","fit":"contain"},
       {"type":"image","asset":"wide-diagram","alt":"横長画像（cover）","fit":"cover"}
     ],
     "options":[
       {"id":"a","content":[{"type":"math","text":"\\frac{1}{2}"}]},
       {"id":"b","content":[{"type":"math","text":"x^2"}]},
       {"id":"c","content":[{"type":"math","text":"\\sqrt{3}"}]}
     ],
     "answer":{"optionId":"b"},
     "explanation":[
       {"type":"math","text":"\\int_0^1 x\\,dx = \\frac{1}{2}"}
     ]}
  ]}]
}"#;

fn seed_expanded(service: &WorkbookService, target: &str) -> anyhow::Result<()> {
    let book = service.import_workbook_json(workbook_service::SAMPLE_WORKBOOK_V2_JSON)?;
    service.start_session(Mode::Practice, Some(&book))?;
    let mut view = service.active_view()?.expect("active practice session");
    let wanted = match target {
        "numeric" | "near-miss" => QuestionType::Numeric,
        "ordering" => QuestionType::Ordering,
        "matching" => QuestionType::Matching,
        "hotspot" => QuestionType::Hotspot,
        "labeling" => QuestionType::Labeling,
        other => anyhow::bail!("unknown expanded profile: {other}"),
    };
    for _ in 0..100 {
        match view.phase {
            Phase::Intro => {
                view = service.begin_question(&view.session_id, view.revision)?;
            }
            Phase::Question => {
                let kind = view.question.as_ref().map(|question| question.kind);
                if kind == Some(wanted) {
                    if target == "near-miss" {
                        let pending = service.evaluate_answer(
                            &view.session_id,
                            Some(&Answer::Text("3A".to_string())),
                            false,
                            None,
                            false,
                            view.revision,
                        )?;
                        assert_eq!(pending.phase, Phase::Feedback);
                    }
                    return Ok(());
                }
                view = service.skip(&view.session_id, view.revision)?;
            }
            _ => anyhow::bail!("expanded target not found"),
        }
    }
    anyhow::bail!("expanded target not found")
}

fn seed_upcoming(service: &WorkbookService) -> anyhow::Result<()> {
    let raw = r#"{
      "format":"workbook","schemaVersion":1,"id":"upcoming-demo","revision":1,
      "title":"短期再学習デモ","description":"1分後の再出題を確認するための教材です。","language":"ja","tags":["デモ"],
      "notes":[{"id":"n","title":"短いNote","knowledgeCard":"カード","questions":[
        {"id":"q","revision":1,"type":"true_false","prompt":"この問題は1分後に再出題されます。","answer":true,"explanation":"Againを選ぶとLearning 1分ステップに入ります。"}
      ]}]
    }"#;
    let book = service.import_workbook_json(raw)?;
    service.start_session(Mode::New, Some(&book))?;
    let view = service.active_view()?.expect("active");
    let view = service.begin_question(&view.session_id, view.revision)?;
    let pending = service.evaluate_answer(
        &view.session_id,
        Some(&Answer::Bool(false)),
        false,
        None,
        false,
        view.revision,
    )?;
    let committed = ensure_committed(service, pending, Rating::Again)?;
    service.finish(&committed.session_id, committed.revision)?;
    Ok(())
}

fn finish_new_session(service: &WorkbookService, book_id: &str) -> anyhow::Result<()> {
    service.start_session(Mode::New, Some(book_id))?;
    let mut view = service.active_view()?.expect("active session");
    for _ in 0..2000 {
        match view.phase {
            Phase::Intro => {
                view = service.begin_question(&view.session_id, view.revision)?;
            }
            Phase::Question => {
                let prepared = prepare_answer(service, &view)?;
                let (answer, self_correct) = correct_answer(prepared.question.as_ref().unwrap());
                let pending = service.evaluate_answer(
                    &prepared.session_id,
                    answer.as_ref(),
                    false,
                    self_correct,
                    false,
                    prepared.revision,
                )?;
                view = ensure_committed(service, pending, Rating::Good)?;
            }
            Phase::Feedback => {
                view = service.next(&view.session_id, view.revision)?;
            }
            Phase::WorkbookResult => {
                view = service.continue_session(&view.session_id, view.revision)?;
            }
            Phase::OverallResult => return Ok(()),
        }
    }
    anyhow::bail!("new session did not finish")
}

fn start_review(service: &WorkbookService) -> anyhow::Result<StudyView> {
    service.start_session(Mode::Review, None)?;
    Ok(service.active_view()?.expect("active review"))
}

fn wait_for_question(service: &WorkbookService, mut view: StudyView) -> anyhow::Result<StudyView> {
    while view.phase == Phase::Intro {
        view = service.begin_question(&view.session_id, view.revision)?;
    }
    if view.phase != Phase::Question {
        anyhow::bail!("expected question, got {:?}", view.phase);
    }
    Ok(view)
}

fn ensure_committed(
    service: &WorkbookService,
    view: StudyView,
    rating: Rating,
) -> anyhow::Result<StudyView> {
    if view
        .feedback
        .as_ref()
        .is_some_and(|feedback| feedback.committed)
    {
        Ok(view)
    } else {
        Ok(service.commit_rating(
            &view.session_id,
            &uuid::Uuid::new_v4().to_string(),
            rating,
            view.revision,
        )?)
    }
}

fn prepare_answer(service: &WorkbookService, view: &StudyView) -> anyhow::Result<StudyView> {
    if view.revealed {
        return Ok(view.clone());
    }
    if view
        .question
        .as_ref()
        .is_some_and(|question| question.kind == QuestionType::Flashcard)
    {
        return Ok(service.save_draft(
            &view.session_id,
            None,
            view.hint_visible,
            true,
            view.revision,
        )?);
    }
    Ok(view.clone())
}

fn correct_answer(question: &Question) -> (Option<Answer>, Option<bool>) {
    match question.kind {
        QuestionType::SingleChoice => (
            question
                .single_choice_answer()
                .map(|value| Answer::Text(value.to_string())),
            None,
        ),
        QuestionType::TrueFalse => (
            question
                .answer
                .as_ref()
                .and_then(|value| value.as_bool())
                .map(Answer::Bool),
            None,
        ),
        QuestionType::MultipleChoice => (
            Some(Answer::Choices(question.multiple_choice_answers())),
            None,
        ),
        QuestionType::ShortText => (
            question
                .short_text_answers()
                .first()
                .cloned()
                .map(Answer::Text),
            None,
        ),
        QuestionType::Cloze => {
            let blanks: BTreeMap<String, String> = question
                .cloze_blanks()
                .into_iter()
                .filter_map(|(id, values)| values.first().cloned().map(|value| (id, value)))
                .collect();
            (Some(Answer::Blanks(blanks)), None)
        }
        QuestionType::Flashcard => (Some(Answer::Bool(true)), Some(true)),
        QuestionType::Numeric => {
            let spec = question.numeric_spec();
            let answer = spec.and_then(|spec| {
                let value = spec.value.value().ok()?;
                let text = match spec.unit {
                    Some(unit) => format!("{value}{unit}"),
                    None => format!("{value}"),
                };
                Some(Answer::Text(text))
            });
            (answer, None)
        }
        QuestionType::Ordering => {
            let order = question
                .ordering_spec()
                .and_then(|spec| spec.orders.first().cloned());
            (order.map(|order| Answer::Ordering { order }), None)
        }
        QuestionType::Matching => {
            let matches = question.matching_spec().map(|spec| {
                spec.pairs
                    .into_iter()
                    .map(|pair| (pair.left, pair.right))
                    .collect()
            });
            (matches.map(|matches| Answer::Matching { matches }), None)
        }
        QuestionType::Hotspot => {
            let point = question
                .hotspot
                .as_ref()
                .and_then(|hotspot| hotspot.regions.first())
                .map(|region| match &region.shape {
                    workbook_domain::HotspotShape::Circle { cx, cy, .. } => (*cx, *cy),
                    workbook_domain::HotspotShape::Rect { x, y, w, h } => {
                        (x + w / 2.0, y + h / 2.0)
                    }
                    workbook_domain::HotspotShape::Polygon { points } => {
                        let count = points.len().max(1) as f64;
                        (
                            points.iter().map(|point| point[0]).sum::<f64>() / count,
                            points.iter().map(|point| point[1]).sum::<f64>() / count,
                        )
                    }
                });
            (point.map(|(x, y)| Answer::Hotspot { x, y }), None)
        }
        QuestionType::Labeling => {
            let labels = question.labeling_spec().map(|spec| spec.pairs);
            (labels.map(|labels| Answer::Labels { labels }), None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fake_paths(base: &Path) -> SeedPaths {
        SeedPaths {
            cwd: base.join("project"),
            home: Some(base.join("home")),
        }
    }

    #[test]
    fn a_new_directory_is_created_and_marked() {
        let root = tempdir().unwrap();
        let paths = fake_paths(root.path());
        let seed = root.path().join("demo");
        prepare_seed_directory_with(&seed, &paths).unwrap();
        assert!(seed.is_dir());
        assert_eq!(
            std::fs::read(seed.join(SEED_MARKER)).unwrap(),
            SEED_MARKER_CONTENT
        );
    }

    #[test]
    fn an_owned_directory_is_reseeded_without_deleting_user_files() {
        let root = tempdir().unwrap();
        let paths = fake_paths(root.path());
        let seed = root.path().join("demo");
        prepare_seed_directory_with(&seed, &paths).unwrap();

        std::fs::write(seed.join("workbook.sqlite"), b"old database").unwrap();
        std::fs::write(seed.join("notes.txt"), b"user data").unwrap();
        std::fs::create_dir(seed.join("backups")).unwrap();
        std::fs::write(seed.join("backups").join("daily.sqlite"), b"backup").unwrap();

        prepare_seed_directory_with(&seed, &paths).unwrap();

        assert!(!seed.join("workbook.sqlite").exists());
        assert!(!seed.join("workbook.sqlite-wal").exists());
        assert!(!seed.join("backups").exists());
        assert_eq!(std::fs::read(seed.join("notes.txt")).unwrap(), b"user data");
        assert!(seed.join(SEED_MARKER).is_file());
    }

    #[test]
    fn a_directory_without_the_marker_is_never_modified() {
        let root = tempdir().unwrap();
        let paths = fake_paths(root.path());
        let seed = root.path().join("user-data");
        std::fs::create_dir(&seed).unwrap();
        std::fs::write(seed.join("important.txt"), b"keep me").unwrap();

        let error = prepare_seed_directory_with(&seed, &paths).unwrap_err();
        assert!(error.to_string().contains("Seeder管理下ではありません"));
        assert_eq!(
            std::fs::read(seed.join("important.txt")).unwrap(),
            b"keep me"
        );
    }

    #[test]
    fn an_empty_unmarked_directory_is_not_claimed() {
        let root = tempdir().unwrap();
        let paths = fake_paths(root.path());
        let seed = root.path().join("empty");
        std::fs::create_dir(&seed).unwrap();
        assert!(prepare_seed_directory_with(&seed, &paths).is_err());
        assert!(seed.is_dir());
        assert!(!seed.join(SEED_MARKER).exists());
    }

    #[test]
    fn the_project_root_and_its_ancestors_are_rejected() {
        let root = tempdir().unwrap();
        let project = root.path().join("project");
        let nested = project.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(project.join("Cargo.toml"), b"keep").unwrap();
        let paths = SeedPaths {
            cwd: nested,
            home: Some(root.path().join("home")),
        };

        assert!(prepare_seed_directory_with(&project, &paths).is_err());
        assert!(prepare_seed_directory_with(root.path(), &paths).is_err());
        assert_eq!(std::fs::read(project.join("Cargo.toml")).unwrap(), b"keep");
    }

    #[test]
    fn relative_and_unsafe_paths_are_rejected() {
        let root = tempdir().unwrap();
        let paths = fake_paths(root.path());
        for path in [
            PathBuf::from("relative-demo"),
            PathBuf::from(""),
            PathBuf::from("/"),
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp/../etc"),
        ] {
            assert!(
                prepare_seed_directory_with(&path, &paths).is_err(),
                "should reject {}",
                path.display()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_seed_directory_is_rejected_without_touching_the_target() {
        let root = tempdir().unwrap();
        let paths = fake_paths(root.path());
        let real = root.path().join("real");
        prepare_seed_directory_with(&real, &paths).unwrap();
        std::fs::write(real.join("notes.txt"), b"keep").unwrap();

        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let error = prepare_seed_directory_with(&link, &paths).unwrap_err();
        assert!(error.to_string().contains("シンボリックリンク"));
        assert_eq!(std::fs::read(real.join("notes.txt")).unwrap(), b"keep");
        assert!(real.join(SEED_MARKER).is_file());
    }

    #[test]
    fn a_tampered_marker_blocks_reseeding() {
        let root = tempdir().unwrap();
        let paths = fake_paths(root.path());
        let seed = root.path().join("demo");
        prepare_seed_directory_with(&seed, &paths).unwrap();
        std::fs::write(seed.join("workbook.sqlite"), b"old").unwrap();
        std::fs::write(seed.join(SEED_MARKER), b"not ours").unwrap();

        assert!(prepare_seed_directory_with(&seed, &paths).is_err());
        assert_eq!(std::fs::read(seed.join("workbook.sqlite")).unwrap(), b"old");
    }

    #[cfg(unix)]
    #[test]
    fn a_managed_directory_symlink_aborts_before_removing_the_database() {
        let root = tempdir().unwrap();
        let paths = fake_paths(root.path());
        let seed = root.path().join("demo");
        prepare_seed_directory_with(&seed, &paths).unwrap();
        std::fs::write(seed.join("workbook.sqlite"), b"old").unwrap();
        let target = root.path().join("backup-target");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, seed.join("backups")).unwrap();

        assert!(prepare_seed_directory_with(&seed, &paths).is_err());
        assert_eq!(std::fs::read(seed.join("workbook.sqlite")).unwrap(), b"old");
        assert!(target.is_dir());
    }

    #[test]
    fn a_managed_file_symlink_blocks_reseeding() {
        let root = tempdir().unwrap();
        let paths = fake_paths(root.path());
        let seed = root.path().join("demo");
        prepare_seed_directory_with(&seed, &paths).unwrap();
        let target = root.path().join("target.sqlite");
        std::fs::write(&target, b"keep").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, seed.join("workbook.sqlite")).unwrap();

        #[cfg(unix)]
        {
            assert!(prepare_seed_directory_with(&seed, &paths).is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        }
    }
}
