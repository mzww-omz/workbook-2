use std::path::PathBuf;

use workbook_lint::{LintReport, Severity, lint_path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Text,
    Json,
}

fn main() {
    std::process::exit(run(std::env::args().skip(1).collect()));
}

fn run(args: Vec<String>) -> i32 {
    let mut path: Option<PathBuf> = None;
    let mut format = Format::Text;
    let mut strict = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => {
                print_help();
                return 0;
            }
            "--strict" => strict = true,
            "--format" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("--format には text または json を指定してください");
                    return 2;
                };
                format = match parse_format(value) {
                    Some(format) => format,
                    None => {
                        eprintln!("不明な --format です: {value}");
                        return 2;
                    }
                };
            }
            value if value.starts_with("--format=") => {
                let Some(value) = value.strip_prefix("--format=") else {
                    unreachable!();
                };
                format = match parse_format(value) {
                    Some(format) => format,
                    None => {
                        eprintln!("不明な --format です: {value}");
                        return 2;
                    }
                };
            }
            value if value.starts_with('-') => {
                eprintln!("不明なオプションです: {value}");
                return 2;
            }
            value => {
                if path.is_some() {
                    eprintln!("Workbook のパスは1つだけ指定してください");
                    return 2;
                }
                path = Some(PathBuf::from(value));
            }
        }
        index += 1;
    }
    let Some(path) = path else {
        eprintln!("Workbook のファイルまたはディレクトリを指定してください");
        print_help();
        return 2;
    };

    let report = lint_path(&path);
    match format {
        Format::Text => print_text(&report),
        Format::Json => match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("JSON出力に失敗しました: {error}");
                return 2;
            }
        },
    }
    report.exit_code(strict)
}

fn parse_format(value: &str) -> Option<Format> {
    match value {
        "text" => Some(Format::Text),
        "json" => Some(Format::Json),
        _ => None,
    }
}

fn print_help() {
    println!(
        "workbook-lint — Workbook 2 教材の静的検証\n\n\
         使い方:\n  workbook-lint <workbook.json | ディレクトリ> [--format text|json] [--strict]\n\n\
         ディレクトリには workbook.json を置くか、.json を1つだけ含めてください。\n\
         --strict は Warning も失敗として扱います（CI向け）。\n\
         終了コード: 0=成功, 1=教材検証エラー, 2=引数/IO/JSON出力エラー"
    );
}

fn print_text(report: &LintReport) {
    println!("{}", report.path);
    for diagnostic in &report.diagnostics {
        let label = match diagnostic.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
            Severity::Info => "INFO",
        };
        println!("{} {}", label, diagnostic.code);
        println!("  {}", diagnostic.path);
        println!("  {}", diagnostic.message);
        if let Some(suggestion) = &diagnostic.suggestion {
            println!("  提案: {suggestion}");
        }
        println!();
    }
    println!(
        "結果: {} error, {} warning, {} info",
        report.summary.errors, report.summary.warnings, report.summary.infos
    );
    if report.is_io_error() {
        println!("NG: 入力ファイルまたはディレクトリを読み込めません");
    } else if report.ok {
        println!("OK: インポート可能です");
    } else {
        println!("NG: Error を修正してください");
    }
}
