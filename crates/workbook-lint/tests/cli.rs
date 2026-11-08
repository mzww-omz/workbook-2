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
