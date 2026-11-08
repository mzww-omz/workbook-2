use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_workbook-lint"))
        .args(args)
        .output()
        .unwrap()
}

fn write_workbook(dir: &Path, name: &str, explanation: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            r#"{{
              "format":"workbook","schemaVersion":2,"id":"w1","revision":1,
              "title":"CLIテスト","description":"","language":"ja","tags":[],
              "notes":[{{"id":"n1","title":"N","knowledgeCard":"カード","questions":[
                {{"id":"q1","revision":1,"type":"true_false","prompt":"空は青い","answer":true,
                  "explanation":"{explanation}"}}]}}]
            }}"#
        ),
    )
    .unwrap();
    path
}

#[test]
fn human_output_and_exit_code_follow_the_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_workbook(
        dir.path(),
        "workbook.json",
        "昼間の空は青く見えます。理由は光の散乱です。",
    );
    let output = run(&[path.to_str().unwrap()]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("0 error, 0 warning"), "{stdout}");
    assert!(stdout.contains("インポート可能"), "{stdout}");
}

#[test]
fn directory_input_and_json_output_are_stable() {
    let dir = tempfile::tempdir().unwrap();
    write_workbook(
        dir.path(),
        "workbook.json",
        "昼間の空は青く見えます。理由は光の散乱です。",
    );
    let output = run(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["ok"], true);
    assert_eq!(value["summary"]["errors"], 0);
    assert!(value["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn strict_mode_fails_on_warnings_and_reports_them_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_workbook(dir.path(), "workbook.json", "短い");
    let normal = run(&[path.to_str().unwrap(), "--format=json"]);
    assert!(normal.status.success());
    let value: Value = serde_json::from_slice(&normal.stdout).unwrap();
    assert!(value["summary"]["warnings"].as_u64().unwrap() > 0);
    let strict = run(&[path.to_str().unwrap(), "--strict"]);
    assert_eq!(strict.status.code(), Some(1));
}

#[test]
fn missing_asset_is_a_ci_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workbook.json");
    std::fs::write(
        &path,
        r#"{
          "format":"workbook","schemaVersion":2,"id":"w1","revision":1,
          "title":"欠落","description":"","language":"ja","tags":[],
          "notes":[{"id":"n1","title":"N","knowledgeCard":"カード","questions":[
            {"id":"q1","revision":1,"type":"true_false","prompt":[
              {"type":"image","asset":"missing","alt":"欠落した画像"}],
              "answer":true,"explanation":"画像はありません。"}]}]
        }"#,
    )
    .unwrap();
    let output = run(&[path.to_str().unwrap(), "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert!(
        value["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "WB-ASSET-001")
    );
}

#[test]
fn usage_errors_exit_with_two() {
    let output = run(&[]);
    assert_eq!(output.status.code(), Some(2));
    let output = run(&["--format", "yaml", "somewhere"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn missing_path_is_an_io_error_not_a_workbook_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("does-not-exist.json");
    let output = run(&[missing.to_str().unwrap(), "--format", "json"]);
    assert_eq!(output.status.code(), Some(2));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["ok"], false);
    assert_eq!(value["diagnostics"][0]["code"], "WB-IO-001");
    assert_eq!(value["summary"]["errors"], 1);
}

#[test]
fn directory_without_a_workbook_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(&[dir.path().to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("読み込めません"), "{stdout}");
}

#[cfg(unix)]
#[test]
fn unreadable_file_is_an_io_error() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = write_workbook(
        dir.path(),
        "workbook.json",
        "昼間の空は青く見えます。理由は光の散乱です。",
    );
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o000);
    std::fs::set_permissions(&path, permissions).unwrap();
    // Root can read the file regardless of mode; there is nothing to verify
    // in that environment.
    let unreadable = std::fs::read(&path).is_err();
    let output = if unreadable {
        Some(run(&[path.to_str().unwrap(), "--format", "json"]))
    } else {
        None
    };
    // Restore before asserting so the tempdir can always be cleaned up.
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(&path, permissions).unwrap();
    let Some(output) = output else {
        return;
    };
    assert_eq!(output.status.code(), Some(2));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["diagnostics"][0]["code"], "WB-IO-001");
}

#[test]
fn malformed_json_and_invalid_workbooks_exit_with_one() {
    let dir = tempfile::tempdir().unwrap();

    let malformed = dir.path().join("malformed.json");
    std::fs::write(&malformed, "{not json").unwrap();
    let output = run(&[malformed.to_str().unwrap(), "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["diagnostics"][0]["code"], "WB-JSON-001");

    let invalid = dir.path().join("invalid.json");
    std::fs::write(
        &invalid,
        r#"{"format":"workbook","schemaVersion":2,"id":"w1","revision":1,
            "title":"x","description":"","language":"ja","tags":[],"notes":[]}"#,
    )
    .unwrap();
    let output = run(&[invalid.to_str().unwrap(), "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert!(value["summary"]["errors"].as_u64().unwrap() > 0);
}

#[test]
fn warning_only_workbooks_succeed_unless_strict() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_workbook(dir.path(), "workbook.json", "短い");
    let normal = run(&[path.to_str().unwrap()]);
    assert_eq!(normal.status.code(), Some(0));
    let strict = run(&[path.to_str().unwrap(), "--strict"]);
    assert_eq!(strict.status.code(), Some(1));
}

#[test]
fn json_output_contract_stays_stable_for_every_exit_class() {
    let dir = tempfile::tempdir().unwrap();
    let valid = write_workbook(
        dir.path(),
        "workbook.json",
        "昼間の空は青く見えます。理由は光の散乱です。",
    );
    let missing = dir.path().join("missing.json");

    for (path, expected_code, expected_errors) in
        [(valid.as_path(), 0_u8, 0_u64), (missing.as_path(), 2, 1)]
    {
        let output = run(&[path.to_str().unwrap(), "--format", "json"]);
        assert_eq!(
            output.status.code().map(|code| code as u8),
            Some(expected_code),
            "{}",
            path.display()
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["version"], 1);
        assert!(value["path"].as_str().is_some());
        assert_eq!(value["summary"]["errors"], expected_errors);
        assert!(value["diagnostics"].is_array());
        // Internal report kind must never leak into the JSON contract.
        assert!(value.get("kind").is_none());
    }
}
