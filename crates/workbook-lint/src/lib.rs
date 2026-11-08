//! Static workbook validation and authoring-quality diagnostics.
//!
//! The linter deliberately reuses `workbook-domain` for structural and
//! semantic validation. It adds authoring checks (readability, asset usage,
//! numeric unit consistency, hotspot hit size) and a stable diagnostic format
//! that does not require the GPUI application or a database.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use workbook_domain::{
    Content, ContentBlock, HotspotShape, ImageSource, MAX_JSON_BYTES, Question, QuestionType,
    ValidationError, Workbook, known_unit, parse_workbook_value, validate_relative_asset_path,
};
use workbook_import::load_workbook_file;

pub const DIAGNOSTIC_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

impl Diagnostic {
    fn new(
        code: &'static str,
        severity: Severity,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity,
            message: message.into(),
            path: path.into(),
            suggestion: None,
        }
    }

    fn suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReportKind {
    /// The workbook content was parsed and checked, even if it is invalid.
    #[default]
    Workbook,
    /// The CLI could not read the input path at all.
    Io,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LintReport {
    /// Version of the diagnostic JSON contract, not the workbook schema.
    pub version: u32,
    pub path: String,
    pub ok: bool,
    pub summary: Summary,
    pub diagnostics: Vec<Diagnostic>,
    /// Internal result class. Not serialized, so the JSON contract above does
    /// not change. It lets the CLI distinguish "invalid workbook" (exit 1)
    /// from "could not read the input" (exit 2).
    #[serde(skip)]
    pub kind: ReportKind,
}

impl LintReport {
    fn finish(mut self) -> Self {
        self.summary = Summary {
            errors: self
                .diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .count(),
            warnings: self
                .diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Warning)
                .count(),
            infos: self
                .diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Info)
                .count(),
        };
        self.ok = self.summary.errors == 0;
        self
    }

    /// CI-friendly exit status.
    ///
    /// * `0` — no errors (and no warnings in `--strict` mode)
    /// * `1` — workbook validation errors, or warnings in `--strict` mode
    /// * `2` — the input path could not be read at all
    pub fn exit_code(&self, strict: bool) -> i32 {
        if self.kind == ReportKind::Io {
            return 2;
        }
        if self.summary.errors > 0 || (strict && self.summary.warnings > 0) {
            1
        } else {
            0
        }
    }

    pub fn is_io_error(&self) -> bool {
        self.kind == ReportKind::Io
    }
}

/// Resolves a CLI path to a workbook JSON file. A directory must contain
/// `workbook.json` or exactly one top-level `.json` file.
pub fn resolve_workbook_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(format!("パスが見つかりません: {}", path.display()));
    }
    let default = path.join("workbook.json");
    if default.is_file() {
        return Ok(default);
    }
    let mut candidates = Vec::new();
    let entries = fs::read_dir(path)
        .map_err(|error| format!("ディレクトリを読めません {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("ディレクトリを読めません: {error}"))?;
        let candidate = entry.path();
        if candidate.is_file() && candidate.extension().is_some_and(|ext| ext == "json") {
            candidates.push(candidate);
        }
    }
    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => Err(format!(
            "Workbook JSON が見つかりません: {}（workbook.json を置くか .json を1つ指定してください）",
            path.display()
        )),
        _ => Err(format!(
            "Workbook JSON が複数あります: {}（workbook.json を置くかファイルを直接指定してください）",
            path.display()
        )),
    }
}

/// Report for a path that could not be read. These exit with status 2, not 1:
/// they are CLI/IO failures, not workbook validation errors.
fn io_report(path: impl Into<String>, message: impl Into<String>) -> LintReport {
    LintReport {
        version: DIAGNOSTIC_FORMAT_VERSION,
        path: path.into(),
        ok: false,
        summary: Summary::default(),
        diagnostics: vec![Diagnostic::new("WB-IO-001", Severity::Error, "$", message)],
        kind: ReportKind::Io,
    }
    .finish()
}

pub fn lint_path(path: &Path) -> LintReport {
    let workbook_path = match resolve_workbook_path(path) {
        Ok(path) => path,
        Err(message) => return io_report(path.display().to_string(), message),
    };
    let display_path = workbook_path.display().to_string();
    let metadata = match fs::metadata(&workbook_path) {
        Ok(metadata) => metadata,
        Err(error) => return io_report(display_path, format!("ファイルを読めません: {error}")),
    };
    if metadata.len() > MAX_JSON_BYTES as u64 {
        return LintReport {
            version: DIAGNOSTIC_FORMAT_VERSION,
            path: display_path,
            ok: false,
            summary: Summary::default(),
            diagnostics: vec![Diagnostic::new(
                "WB-JSON-002",
                Severity::Error,
                "$",
                format!(
                    "JSONは{}MB以下にしてください",
                    MAX_JSON_BYTES / (1024 * 1024)
                ),
            )],
            kind: ReportKind::Workbook,
        }
        .finish();
    }
    let raw = match fs::read_to_string(&workbook_path) {
        Ok(raw) => raw,
        Err(error) => return io_report(display_path, format!("ファイルを読めません: {error}")),
    };
    lint_raw(&raw, &display_path, Some(&workbook_path))
}

/// Lints already-loaded JSON. `source_path` is used for relative assets and is
/// optional so tests and tools can lint in-memory authoring output.
pub fn lint_raw(raw: &str, display_path: &str, source_path: Option<&Path>) -> LintReport {
    let mut diagnostics = Vec::new();
    let workbook = match parse_workbook_value(raw) {
        Ok(workbook) => workbook,
        Err(error) => {
            diagnostics.push(from_validation_error(&error));
            return finish(display_path, diagnostics);
        }
    };

    let structurally_valid = match workbook.validate() {
        Ok(()) => true,
        Err(error) => {
            diagnostics.push(from_validation_error(&error));
            false
        }
    };

    // Asset files are resolved relative to the workbook JSON. Only run file
    // loading after structural validation so an unrelated domain error is not
    // mislabelled as a missing image.
    if structurally_valid {
        let referenced = workbook.referenced_asset_ids();
        for asset in &workbook.assets {
            if !referenced.contains(&asset.id) {
                diagnostics.push(
                    Diagnostic::new(
                        "WB-ASSET-003",
                        Severity::Warning,
                        format!("assets[{}]", asset.id),
                        format!("画像アセット `{}` はどこからも参照されていません", asset.id),
                    )
                    .suggestion(
                        "不要なら削除し、使用するなら Content か Hotspot/Labeling から参照してください",
                    ),
                );
            }
            if asset.alt.trim().chars().count() < 4 {
                diagnostics.push(Diagnostic::new(
                    "WB-ASSET-004",
                    Severity::Warning,
                    format!("assets[{}].alt", asset.id),
                    "画像の代替テキストが短すぎます",
                ));
            }
        }
        if let Some(path) = source_path {
            match load_workbook_file(path) {
                Ok((_, assets)) => {
                    let stored: BTreeSet<String> =
                        assets.iter().map(|asset| asset.id.clone()).collect();
                    for asset in &workbook.assets {
                        if !stored.contains(&asset.id) {
                            diagnostics.push(Diagnostic::new(
                                "WB-ASSET-001",
                                Severity::Error,
                                format!("assets[{}]", asset.id),
                                "画像アセットを読み込めませんでした",
                            ));
                        }
                    }
                }
                Err(error) => diagnostics.push(Diagnostic::new(
                    "WB-ASSET-002",
                    Severity::Error,
                    "assets",
                    format!("画像アセットを検証できません: {error}"),
                )),
            }
        }
    }

    linter_checks(&workbook, &mut diagnostics);
    finish(display_path, diagnostics)
}

fn finish(path: &str, diagnostics: Vec<Diagnostic>) -> LintReport {
    LintReport {
        version: DIAGNOSTIC_FORMAT_VERSION,
        path: path.to_string(),
        ok: false,
        summary: Summary::default(),
        diagnostics,
        kind: ReportKind::Workbook,
    }
    .finish()
}

fn from_validation_error(error: &ValidationError) -> Diagnostic {
    let path =
        if error.path.is_empty() || error.path == "?" || error.message.starts_with("JSON形式") {
            "$".to_string()
        } else {
            error.path.clone()
        };
    let message = error.message.clone();
    let code = if message.starts_with("JSON形式") {
        "WB-JSON-001"
    } else if path == "schemaVersion" {
        "WB-SCHEMA-001"
    } else if message.contains("schemaVersion 2") {
        "WB-SCHEMA-002"
    } else if message.contains("画像") || message.contains("asset") || path.contains("asset") {
        "WB-ASSET-001"
    } else if path.contains("hotspot") {
        "WB-HOT-001"
    } else if path.contains("matching") || path.contains("labeling") {
        "WB-FORMAT-001"
    } else if path.contains(".answer") {
        "WB-ANSWER-001"
    } else {
        "WB-VALIDATION-001"
    };
    Diagnostic::new(code, Severity::Error, path, message)
}

fn linter_checks(workbook: &Workbook, diagnostics: &mut Vec<Diagnostic>) {
    let asset_ids: BTreeSet<String> = workbook
        .assets
        .iter()
        .map(|asset| asset.id.clone())
        .collect();
    for (note_index, note) in workbook.notes.iter().enumerate() {
        let note_path = format!("notes[{note_index}]");
        check_style(&format!("{note_path}.title"), &note.title, diagnostics);
        check_content(
            &note.knowledge_card,
            &format!("{note_path}.knowledgeCard"),
            workbook.schema_version,
            &asset_ids,
            diagnostics,
        );
        for (question_index, question) in note.questions.iter().enumerate() {
            let path = format!("{note_path}.questions[{question_index}]");
            check_question(
                workbook,
                question,
                &path,
                workbook.schema_version,
                &asset_ids,
                diagnostics,
            );
        }
    }
    for (index, tag) in workbook.tags.iter().enumerate() {
        check_style(&format!("tags[{index}]"), tag, diagnostics);
    }
    check_style("title", &workbook.title, diagnostics);
}

fn check_question(
    workbook: &Workbook,
    question: &Question,
    path: &str,
    schema_version: u32,
    asset_ids: &BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    check_style(&format!("{path}.id"), &question.id, diagnostics);
    if let Some(prompt) = &question.prompt {
        check_content(
            prompt,
            &format!("{path}.prompt"),
            schema_version,
            asset_ids,
            diagnostics,
        );
    }
    if let Some(hint) = &question.hint {
        check_content(
            hint,
            &format!("{path}.hint"),
            schema_version,
            asset_ids,
            diagnostics,
        );
    }
    check_content(
        &question.explanation,
        &format!("{path}.explanation"),
        schema_version,
        asset_ids,
        diagnostics,
    );
    if let Some(front) = &question.front {
        check_content(
            front,
            &format!("{path}.front"),
            schema_version,
            asset_ids,
            diagnostics,
        );
    }
    if let Some(back) = &question.back {
        check_content(
            back,
            &format!("{path}.back"),
            schema_version,
            asset_ids,
            diagnostics,
        );
    }
    if let Some(options) = &question.options {
        for (index, option) in options.iter().enumerate() {
            check_style(
                &format!("{path}.options[{index}].id"),
                &option.id,
                diagnostics,
            );
            check_content(
                &option.content,
                &format!("{path}.options[{index}].content"),
                schema_version,
                asset_ids,
                diagnostics,
            );
        }
        check_duplicate_options(question, path, diagnostics);
        check_guessable_answer(question, path, diagnostics);
    }
    if let Some(matching) = &question.matching {
        for (side, items) in [("left", &matching.left), ("right", &matching.right)] {
            for (index, item) in items.iter().enumerate() {
                check_style(
                    &format!("{path}.matching.{side}[{index}].id"),
                    &item.id,
                    diagnostics,
                );
                check_content(
                    &item.content,
                    &format!("{path}.matching.{side}[{index}].content"),
                    schema_version,
                    asset_ids,
                    diagnostics,
                );
            }
        }
    }
    if let Some(labeling) = &question.labeling {
        for (index, label) in labeling.labels.iter().enumerate() {
            check_style(
                &format!("{path}.labeling.labels[{index}].id"),
                &label.id,
                diagnostics,
            );
            check_content(
                &label.content,
                &format!("{path}.labeling.labels[{index}].content"),
                schema_version,
                asset_ids,
                diagnostics,
            );
        }
        for (index, target) in labeling.targets.iter().enumerate() {
            check_style(
                &format!("{path}.labeling.targets[{index}].id"),
                &target.id,
                diagnostics,
            );
        }
    }
    if question.explanation.plain_text().chars().count() < 15
        && question.kind != QuestionType::Flashcard
    {
        diagnostics.push(
            Diagnostic::new(
                "WB-QUALITY-003",
                Severity::Warning,
                format!("{path}.explanation"),
                "解説が短く、学習の手がかりが不足している可能性があります",
            )
            .suggestion("間違えやすい点や考え方を1文以上補ってください"),
        );
    }
    match question.kind {
        QuestionType::Numeric => check_numeric(question, path, diagnostics),
        QuestionType::Hotspot => check_hotspot(question, path, diagnostics),
        QuestionType::SingleChoice | QuestionType::MultipleChoice => {
            let _ = workbook;
        }
        _ => {}
    }
}

fn check_style(path: &str, value: &str, diagnostics: &mut Vec<Diagnostic>) {
    if value != value.trim() {
        diagnostics.push(
            Diagnostic::new(
                "WB-STYLE-001",
                Severity::Warning,
                path,
                "前後に不要な空白があります",
            )
            .suggestion("前後の空白を削除してください"),
        );
    }
}

fn check_content(
    content: &Content,
    path: &str,
    schema_version: u32,
    asset_ids: &BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if content.plain_text().chars().count() > 20_000 {
        diagnostics.push(Diagnostic::new(
            "WB-CONTENT-006",
            Severity::Warning,
            path,
            "1つのContentが非常に長く、学習画面で読みにくい可能性があります",
        ));
    }
    let Content::Blocks(blocks) = content else {
        return;
    };
    for (index, block) in blocks.iter().enumerate() {
        let block_path = format!("{path}[{index}]");
        match block {
            ContentBlock::Markdown { text } => {
                if text.trim().is_empty() {
                    diagnostics.push(Diagnostic::new(
                        "WB-CONTENT-001",
                        Severity::Error,
                        &block_path,
                        "空のMarkdownブロックがあります",
                    ));
                }
            }
            ContentBlock::Code { language, .. } => {
                if language
                    .as_ref()
                    .is_some_and(|value| value.trim().is_empty())
                {
                    diagnostics.push(Diagnostic::new(
                        "WB-CONTENT-008",
                        Severity::Info,
                        format!("{block_path}.language"),
                        "言語名が空です",
                    ));
                }
            }
            ContentBlock::Math { text } => check_math(text, &block_path, diagnostics),
            ContentBlock::Image(image) => {
                if image.alt.trim().chars().count() < 4 {
                    diagnostics.push(
                        Diagnostic::new(
                            "WB-CONTENT-004",
                            Severity::Warning,
                            format!("{block_path}.alt"),
                            "画像の代替テキストが短すぎます",
                        )
                        .suggestion("何が描かれているか分かる説明にしてください"),
                    );
                }
                if let ImageSource::Asset(id) = &image.source {
                    if schema_version < 2 {
                        diagnostics.push(Diagnostic::new(
                            "WB-SCHEMA-002",
                            Severity::Error,
                            format!("{block_path}.asset"),
                            "教材同梱画像は schemaVersion 2 でのみ使用できます",
                        ));
                    } else if !asset_ids.contains(id) {
                        diagnostics.push(Diagnostic::new(
                            "WB-ASSET-001",
                            Severity::Error,
                            format!("{block_path}.asset"),
                            format!("assets に存在しない画像IDです: {id}"),
                        ));
                    }
                }
            }
        }
    }
}

fn check_math(text: &str, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let dollars = text.chars().filter(|ch| *ch == '$').count();
    if !dollars.is_multiple_of(2) {
        diagnostics.push(Diagnostic::new(
            "WB-MATH-001",
            Severity::Warning,
            path,
            "数式の `$` 区切りが閉じられていない可能性があります",
        ));
    }
    let mut depth = 0i32;
    for ch in text.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            break;
        }
    }
    if depth != 0 {
        diagnostics.push(Diagnostic::new(
            "WB-MATH-002",
            Severity::Warning,
            path,
            "数式の波かっこの対応が取れていない可能性があります",
        ));
    }
    let left = text.matches("\\left").count();
    let right = text.matches("\\right").count();
    if left != right {
        diagnostics.push(Diagnostic::new(
            "WB-MATH-003",
            Severity::Warning,
            path,
            "`\\left` と `\\right` の数が一致していません",
        ));
    }
}

fn check_duplicate_options(question: &Question, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let Some(options) = &question.options else {
        return;
    };
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (index, option) in options.iter().enumerate() {
        let text = option.content.plain_text().trim().to_string();
        if text.is_empty() {
            continue;
        }
        if let Some(first) = seen.get(&text) {
            diagnostics.push(
                Diagnostic::new(
                    "WB-QUALITY-001",
                    Severity::Warning,
                    format!("{path}.options[{index}].content"),
                    format!("選択肢の内容が options[{first}] と重複しています"),
                )
                .suggestion("同じ内容の選択肢を削除するか、区別できる内容にしてください"),
            );
        } else {
            seen.insert(text, index);
        }
    }
}

fn check_guessable_answer(question: &Question, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let Some(options) = &question.options else {
        return;
    };
    if options.len() < 3 {
        return;
    }
    let correct: BTreeSet<String> = match question.kind {
        QuestionType::SingleChoice => question
            .single_choice_answer()
            .map(|id| BTreeSet::from([id.to_string()]))
            .unwrap_or_default(),
        QuestionType::MultipleChoice => question.multiple_choice_answers().into_iter().collect(),
        _ => return,
    };
    if correct.is_empty() {
        return;
    }
    let correct_len = options
        .iter()
        .filter(|option| correct.contains(&option.id))
        .map(|option| option.content.plain_text().chars().count())
        .max()
        .unwrap_or(0);
    let other_max = options
        .iter()
        .filter(|option| !correct.contains(&option.id))
        .map(|option| option.content.plain_text().chars().count())
        .max()
        .unwrap_or(0);
    if correct_len >= 20 && correct_len > other_max.saturating_mul(3).max(20) {
        diagnostics.push(
            Diagnostic::new(
                "WB-QUALITY-002",
                Severity::Warning,
                format!("{path}.options"),
                "正解だけが極端に長く、内容から正解を推測しやすい可能性があります",
            )
            .suggestion("選択肢の長さと情報量をそろえてください"),
        );
    }
}

fn check_numeric(question: &Question, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let Some(spec) = question.numeric_spec() else {
        return;
    };
    let Some(expected_unit) = spec.unit.as_deref() else {
        return;
    };
    let Some((expected_dimension, expected_factor)) = known_unit(expected_unit) else {
        return;
    };
    for (unit, factor) in &spec.accepted_units {
        let Some((dimension, accepted_factor)) = known_unit(unit) else {
            // Workbook-specific units are allowed; do not reject them just
            // because the built-in table does not know the symbol.
            continue;
        };
        if dimension != expected_dimension {
            diagnostics.push(
                Diagnostic::new(
                    "WB-NUM-006",
                    Severity::Error,
                    format!("{path}.answer.acceptedUnits.{unit}"),
                    format!(
                        "`{unit}` は `{expected_unit}` と次元が異なります（{} と {}）",
                        dimension, expected_dimension
                    ),
                )
                .suggestion("正解と同じ次元の単位だけを acceptedUnits に指定してください"),
            );
            continue;
        }
        let table_factor = accepted_factor / expected_factor;
        let relative = (table_factor - factor).abs() / table_factor.abs().max(f64::MIN_POSITIVE);
        if relative > 1e-6 {
            diagnostics.push(
                Diagnostic::new(
                    "WB-NUM-007",
                    Severity::Warning,
                    format!("{path}.answer.acceptedUnits.{unit}"),
                    format!("倍率 {factor} は単位表の換算値 {table_factor} と一致しません"),
                )
                .suggestion("acceptedUnits は「その単位の1単位が正解単位で何になるか」の倍率です"),
            );
        }
    }
    if spec.tolerance.is_some_and(|value| value > 1e6) {
        diagnostics.push(Diagnostic::new(
            "WB-NUM-008",
            Severity::Warning,
            format!("{path}.answer.tolerance"),
            "許容誤差が非常に大きく、異なる値も正解になる可能性があります",
        ));
    }
}

fn check_hotspot(question: &Question, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let Some(hotspot) = &question.hotspot else {
        return;
    };
    for (index, region) in hotspot.regions.iter().enumerate() {
        let region_path = format!("{path}.hotspot.regions[{index}]");
        let shape = &region.shape;
        if let HotspotShape::Polygon { points } = shape
            && polygon_self_intersects(points)
        {
            diagnostics.push(
                Diagnostic::new(
                    "WB-HOT-004",
                    Severity::Error,
                    format!("{region_path}.shape.points"),
                    "多角形の辺が自己交差しています",
                )
                .suggestion("頂点の順序を整え、単純な多角形にしてください"),
            );
        }
        let area = shape_area(shape);
        if area < 0.002 {
            diagnostics.push(
                Diagnostic::new(
                    "WB-HOT-002",
                    Severity::Warning,
                    format!("{region_path}.shape"),
                    format!(
                        "領域が非常に小さく（面積 {area:.4}）、画面上で選びにくい可能性があります"
                    ),
                )
                .suggestion("領域を広げるか、別の画像で示してください"),
            );
        }
        let (min_x, min_y, max_x, max_y) = shape_bounds(shape);
        if max_x - min_x < 0.02 || max_y - min_y < 0.02 {
            diagnostics.push(Diagnostic::new(
                "WB-HOT-003",
                Severity::Warning,
                format!("{region_path}.shape"),
                "領域が極端に細く、クリックしにくい可能性があります",
            ));
        }
        if shape.representative_point().is_none() {
            diagnostics.push(Diagnostic::new(
                "WB-HOT-005",
                Severity::Warning,
                format!("{region_path}.shape"),
                "キーボード操作用の代表点を求められません",
            ));
        }
    }
    for first in 0..hotspot.regions.len() {
        for second in (first + 1)..hotspot.regions.len() {
            if shapes_equal(
                &hotspot.regions[first].shape,
                &hotspot.regions[second].shape,
            ) {
                diagnostics.push(
                    Diagnostic::new(
                        "WB-HOT-006",
                        Severity::Warning,
                        format!("{path}.hotspot.regions[{second}].shape"),
                        format!(
                            "領域 {} と {} の図形が完全に重複しています",
                            hotspot.regions[first].id, hotspot.regions[second].id
                        ),
                    )
                    .suggestion("採点上どちらかを消すか、重ならない図形にしてください"),
                );
            }
        }
    }
}

fn shape_area(shape: &HotspotShape) -> f64 {
    match shape {
        HotspotShape::Circle { r, .. } => std::f64::consts::PI * r * r,
        HotspotShape::Rect { w, h, .. } => w * h,
        HotspotShape::Polygon { points } => polygon_area(points).abs(),
    }
}

fn shape_bounds(shape: &HotspotShape) -> (f64, f64, f64, f64) {
    match shape {
        HotspotShape::Circle { cx, cy, r } => (cx - r, cy - r, cx + r, cy + r),
        HotspotShape::Rect { x, y, w, h } => (*x, *y, x + w, y + h),
        HotspotShape::Polygon { points } => {
            let mut min_x = f64::INFINITY;
            let mut min_y = f64::INFINITY;
            let mut max_x = f64::NEG_INFINITY;
            let mut max_y = f64::NEG_INFINITY;
            for point in points {
                min_x = min_x.min(point[0]);
                min_y = min_y.min(point[1]);
                max_x = max_x.max(point[0]);
                max_y = max_y.max(point[1]);
            }
            (min_x, min_y, max_x, max_y)
        }
    }
}

fn polygon_area(points: &[[f64; 2]]) -> f64 {
    let mut area = 0.0;
    let mut j = points.len() - 1;
    for (i, point) in points.iter().enumerate() {
        area += (points[j][0] * point[1]) - (point[0] * points[j][1]);
        j = i;
    }
    area / 2.0
}

fn shapes_equal(first: &HotspotShape, second: &HotspotShape) -> bool {
    const EPSILON: f64 = 1e-9;
    match (first, second) {
        (
            HotspotShape::Circle {
                cx: acx,
                cy: acy,
                r: ar,
            },
            HotspotShape::Circle {
                cx: bcx,
                cy: bcy,
                r: br,
            },
        ) => {
            (acx - bcx).abs() < EPSILON && (acy - bcy).abs() < EPSILON && (ar - br).abs() < EPSILON
        }
        (
            HotspotShape::Rect {
                x: ax,
                y: ay,
                w: aw,
                h: ah,
            },
            HotspotShape::Rect {
                x: bx,
                y: by,
                w: bw,
                h: bh,
            },
        ) => {
            (ax - bx).abs() < EPSILON
                && (ay - by).abs() < EPSILON
                && (aw - bw).abs() < EPSILON
                && (ah - bh).abs() < EPSILON
        }
        (HotspotShape::Polygon { points: a }, HotspotShape::Polygon { points: b }) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(a, b)| (a[0] - b[0]).abs() < EPSILON && (a[1] - b[1]).abs() < EPSILON)
        }
        _ => false,
    }
}

fn polygon_self_intersects(points: &[[f64; 2]]) -> bool {
    let len = points.len();
    if len < 4 {
        return false;
    }
    for first in 0..len {
        let first_next = (first + 1) % len;
        for second in (first + 1)..len {
            let second_next = (second + 1) % len;
            // Adjacent edges share an endpoint and are allowed to touch.
            if first == second
                || first_next == second
                || second_next == first
                || (first == 0 && second_next == 0)
            {
                continue;
            }
            if segments_intersect(
                points[first],
                points[first_next],
                points[second],
                points[second_next],
            ) {
                return true;
            }
        }
    }
    false
}

fn segments_intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let orientation = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let on_segment = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        q[0] >= p[0].min(r[0]) - 1e-12
            && q[0] <= p[0].max(r[0]) + 1e-12
            && q[1] >= p[1].min(r[1]) - 1e-12
            && q[1] <= p[1].max(r[1]) + 1e-12
    };
    let o1 = orientation(a, b, c);
    let o2 = orientation(a, b, d);
    let o3 = orientation(c, d, a);
    let o4 = orientation(c, d, b);
    const EPSILON: f64 = 1e-12;
    if o1.abs() <= EPSILON && on_segment(a, c, b) {
        return true;
    }
    if o2.abs() <= EPSILON && on_segment(a, d, b) {
        return true;
    }
    if o3.abs() <= EPSILON && on_segment(c, a, d) {
        return true;
    }
    if o4.abs() <= EPSILON && on_segment(c, b, d) {
        return true;
    }
    (o1 > 0.0) != (o2 > 0.0) && (o3 > 0.0) != (o4 > 0.0)
}

/// Returns the safe, structural fix candidates currently understood by the
/// linter. No semantic rewrite (answers, units, coordinates, explanations) is
/// proposed. This is the stable API future `--fix` implementations build on.
pub fn fix_candidates(report: &LintReport) -> Vec<&Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "WB-STYLE-001")
        .collect()
}

/// Asset files are validated with the importer's path and MIME rules. This
/// helper is public so a caller can preflight a directory before linting.
pub fn validate_asset_path(value: &str) -> Result<(), String> {
    validate_relative_asset_path(value, "asset").map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_workbook(question: &str) -> String {
        workbook_with_assets(question, "[]")
    }

    fn workbook_with_assets(question: &str, assets: &str) -> String {
        format!(
            r#"{{
              "format":"workbook","schemaVersion":2,"id":"w1","revision":1,
              "title":"テスト","description":"","language":"ja","tags":[],
              "assets":{assets},
              "notes":[{{"id":"n1","title":"N","knowledgeCard":"カード","questions":[{question}]}}]
            }}"#
        )
    }

    fn lint(raw: &str) -> LintReport {
        lint_raw(raw, "workbook.json", None)
    }

    #[test]
    fn valid_workbook_has_no_errors() {
        let raw = base_workbook(
            r#"{"id":"q1","revision":1,"type":"true_false","prompt":"空は青い","answer":true,
                "explanation":"昼間の空は青く見えます。"}"#,
        );
        let report = lint(&raw);
        assert_eq!(report.summary.errors, 0, "{:?}", report.diagnostics);
        assert!(report.ok);
        assert_eq!(report.exit_code(false), 0);
    }

    #[test]
    fn invalid_json_is_reported_with_a_stable_code() {
        let report = lint("{not json");
        assert_eq!(report.diagnostics[0].code, "WB-JSON-001");
        assert_eq!(report.exit_code(false), 1);
        assert_eq!(report.kind, ReportKind::Workbook);
    }

    #[test]
    fn unreadable_inputs_exit_with_two() {
        let missing = std::path::Path::new("/definitely/not/a/workbook/path.json");
        let report = lint_path(missing);
        assert_eq!(report.diagnostics[0].code, "WB-IO-001");
        assert_eq!(report.kind, ReportKind::Io);
        assert!(report.is_io_error());
        assert_eq!(report.exit_code(false), 2);
        assert_eq!(report.exit_code(true), 2);

        let empty = tempfile::tempdir().unwrap();
        let report = lint_path(empty.path());
        assert_eq!(report.diagnostics[0].code, "WB-IO-001");
        assert_eq!(report.exit_code(false), 2);
    }

    #[test]
    fn unused_asset_is_a_warning() {
        let raw = workbook_with_assets(
            r#"{"id":"q1","revision":1,"type":"true_false","prompt":"空は青い","answer":true,
                "explanation":"昼間の空は青く見えます。"}"#,
            r#"[{"id":"unused","alt":"使われない画像","mediaType":"image/png","data":"AA=="}]"#,
        );
        let report = lint(&raw);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "WB-ASSET-003" && d.severity == Severity::Warning),
            "{:?}",
            report.diagnostics
        );
    }

    #[test]
    fn missing_asset_reference_is_an_error() {
        let raw = base_workbook(
            r#"{"id":"q1","revision":1,"type":"true_false","prompt":[
                {"type":"image","asset":"missing","alt":"細胞の図"}],"answer":true,
                "explanation":"説明です。"}"#,
        );
        let report = lint(&raw);
        assert!(report.summary.errors > 0);
        assert!(report.diagnostics.iter().any(|d| d.code == "WB-ASSET-001"));
    }

    #[test]
    fn numeric_dimension_conflict_is_an_error_but_custom_units_are_allowed() {
        let raw = base_workbook(
            r#"{"id":"q1","revision":1,"type":"numeric","prompt":"長さは？","explanation":"説明です。",
                "answer":{"value":3,"unit":"cm","acceptedUnits":{"kg":100}}}"#,
        );
        let report = lint(&raw);
        assert!(report.diagnostics.iter().any(|d| d.code == "WB-NUM-006"));

        let custom = base_workbook(
            r#"{"id":"q1","revision":1,"type":"numeric","prompt":"長さは？","explanation":"説明です。",
                "answer":{"value":3,"unit":"cm","acceptedUnits":{"unit-x":2}}}"#,
        );
        assert!(
            !lint(&custom)
                .diagnostics
                .iter()
                .any(|d| d.code == "WB-NUM-006")
        );
    }

    #[test]
    fn tiny_hotspot_region_is_only_a_warning() {
        let raw = workbook_with_assets(
            r#"{"id":"q1","revision":1,"type":"hotspot","prompt":"選ぶ","explanation":"説明です。",
                "hotspot":{"asset":"img","regions":[{"id":"r","alt":"点","shape":{"type":"circle","cx":0.5,"cy":0.5,"r":0.001}}]},
                "answer":{"regionIds":["r"]}}"#,
            r#"[{"id":"img","alt":"地図の画像","mediaType":"image/png","data":"AA=="}]"#,
        );
        let report = lint(&raw);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "WB-HOT-002" && d.severity == Severity::Warning)
        );
    }

    #[test]
    fn strict_mode_fails_on_warnings() {
        let raw = base_workbook(
            r#"{"id":"q1","revision":1,"type":"true_false","prompt":"空は青い","answer":true,
                "explanation":"短い"}"#,
        );
        let report = lint(&raw);
        assert_eq!(report.summary.errors, 0);
        assert!(report.summary.warnings > 0);
        assert_eq!(report.exit_code(false), 0);
        assert_eq!(report.exit_code(true), 1);
    }

    #[test]
    fn self_intersecting_polygon_is_reported() {
        let raw = workbook_with_assets(
            r#"{"id":"q1","revision":1,"type":"hotspot","prompt":"選ぶ","explanation":"説明です。",
                "hotspot":{"asset":"img","regions":[{"id":"r","alt":"交差","shape":{"type":"polygon",
                  "points":[[0.1,0.1],[0.8,0.8],[0.8,0.1],[0.1,0.8]]}}]},
                "answer":{"regionIds":["r"]}}"#,
            r#"[{"id":"img","alt":"地図の画像","mediaType":"image/png","data":"AA=="}]"#,
        );
        let report = lint(&raw);
        assert!(report.diagnostics.iter().any(|d| d.code == "WB-HOT-004"));
    }
}
