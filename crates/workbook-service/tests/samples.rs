use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use workbook_domain::{Mode, Phase};
use workbook_scheduler::FixedClock;
use workbook_service::WorkbookService;

fn examples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.agents/skills/workbook-authoring/assets/examples")
}

fn service() -> (tempfile::TempDir, WorkbookService) {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    ));
    let service = WorkbookService::open(dir.path(), clock).unwrap();
    (dir, service)
}

#[test]
fn every_skill_sample_imports_and_keeps_its_questions() {
    let (_dir, service) = service();
    let mut imported = 0;
    let mut entries: Vec<_> = std::fs::read_dir(examples_root())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path().join("workbook.json");
        assert!(path.is_file(), "missing {}", path.display());
        let local_id = service.import_workbook_file(&path).unwrap();
        let workbook = service.workbook(&local_id).unwrap();
        assert!(
            workbook.question_count() > 0,
            "{} has no questions",
            path.display()
        );
        imported += 1;
    }
    assert_eq!(imported, 6, "basic + five subjects");
}

#[test]
fn science_sample_serves_a_bundled_content_image() {
    let (_dir, service) = service();
    let path = examples_root().join("science/workbook.json");
    let workbook_id = service.import_workbook_file(&path).unwrap();
    service
        .start_session(Mode::New, Some(&workbook_id))
        .unwrap();
    let view = service.active_view().unwrap().unwrap();
    assert!(
        view.phase == Phase::Intro || view.phase == Phase::Question,
        "unexpected phase {:?}",
        view.phase
    );
    assert!(
        view.assets.iter().any(|asset| asset.id == "series-circuit"),
        "content image asset must be served before the first answer"
    );
    let bytes = &view
        .assets
        .iter()
        .find(|asset| asset.id == "series-circuit")
        .unwrap()
        .bytes;
    assert!(bytes.starts_with(b"<svg"));
}
