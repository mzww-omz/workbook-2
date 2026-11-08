//! LaTeX math parsing and GPUI-native typesetting.
//!
//! Workbook math blocks used to be rendered as their raw LaTeX source in a
//! monospace font. This module parses a deliberately small, documented LaTeX
//! subset into a layout tree and renders that tree with GPUI flex/text
//! primitives: fractions stack, scripts are raised/lowered and scaled,
//! radicals get an overline, and large operators get limits above and below.
//!
//! The parser and the layout decisions are pure so they can be tested without
//! a display. Unsupported or malformed input never panics: [`render_math`]
//! falls back to the source text plus an explicit "not typeset" notice, and
//! every math element carries the source as its accessibility label.

use gpui_kit::component::Theme;
use gpui_kit::{
    AnyElement, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};

/// Script size relative to the current math size.
pub(crate) const SCRIPT_SCALE: f32 = 0.68;
/// How far scripts are raised/lowered, in ems of the current size.
pub(crate) const SCRIPT_SHIFT_EM: f32 = 0.34;
/// Extra vertical room a fraction needs for its rule, in ems.
pub(crate) const FRACTION_RULE_EM: f32 = 0.36;
/// Relative size of a `\sum`/`\prod` glyph at display size.
pub(crate) const BIG_OPERATOR_SCALE: f32 = 1.55;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SymbolKind {
    Identifier,
    Number,
    Operator,
    Delimiter,
    Punctuation,
    Upright,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpaceKind {
    Thin,
    Medium,
    Quad,
}

impl SpaceKind {
    fn ems(self) -> f32 {
        match self {
            Self::Thin => 0.16,
            Self::Medium => 0.28,
            Self::Quad => 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MathNode {
    Row(Vec<MathNode>),
    Symbol {
        text: String,
        kind: SymbolKind,
    },
    Text(String),
    Space(SpaceKind),
    LineBreak,
    Fraction {
        numerator: Box<MathNode>,
        denominator: Box<MathNode>,
    },
    Radical {
        index: Option<Box<MathNode>>,
        body: Box<MathNode>,
    },
    Script {
        base: Box<MathNode>,
        sup: Option<Box<MathNode>>,
        sub: Option<Box<MathNode>>,
    },
    Delimited {
        open: Option<String>,
        close: Option<String>,
        body: Box<MathNode>,
    },
    BigOperator {
        symbol: String,
    },
    Function {
        name: String,
        limits: bool,
    },
    Overline(Box<MathNode>),
    Underline(Box<MathNode>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MathError {
    pub message: &'static str,
    pub position: usize,
}

impl MathError {
    fn new(message: &'static str, position: usize) -> Self {
        Self { message, position }
    }
}

type MathResult<T> = Result<T, MathError>;

/// Parses the supported LaTeX subset into a layout tree.
///
/// Authors sometimes wrap a math block's text in `$...$` even though the block
/// is already math; strip one matching pair so it does not render literal
/// dollar signs.
pub(crate) fn parse_math(input: &str) -> MathResult<MathNode> {
    let input = strip_outer_math_delimiters(input);
    let mut parser = Parser::new(input);
    let (nodes, terminator) = parser.parse_sequence(Stop::Eof)?;
    if let Terminator::Brace(position) = terminator {
        return Err(MathError::new(
            "対応する `{` のない `}` があります",
            position,
        ));
    }
    Ok(MathNode::Row(nodes))
}

fn strip_outer_math_delimiters(input: &str) -> &str {
    let trimmed = input.trim();
    let width = if trimmed.len() >= 4 && trimmed.starts_with("$$") && trimmed.ends_with("$$") {
        2
    } else if trimmed.len() >= 2 && trimmed.starts_with('$') && trimmed.ends_with('$') {
        1
    } else {
        return input;
    };
    let inner = &trimmed[width..trimmed.len() - width];
    if inner.trim().is_empty() {
        input
    } else {
        inner
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Eof,
    Brace,
    Bracket,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Terminator {
    Done,
    Brace(usize),
    Bracket(usize),
    Right(Option<String>),
}

const MAX_DEPTH: usize = 64;

struct Parser {
    chars: Vec<char>,
    position: usize,
    depth: usize,
}

impl Parser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            position: 0,
            depth: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.position + offset).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let value = self.peek()?;
        self.position += 1;
        Some(value)
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.position += 1;
        }
    }

    fn parse_sequence(&mut self, stop: Stop) -> MathResult<(Vec<MathNode>, Terminator)> {
        let mut nodes = Vec::new();
        loop {
            self.skip_whitespace();
            let Some(character) = self.peek() else {
                return match stop {
                    Stop::Eof => Ok((nodes, Terminator::Done)),
                    Stop::Brace => Err(MathError::new(
                        "`{` に対応する `}` がありません",
                        self.position,
                    )),
                    Stop::Bracket => Err(MathError::new(
                        "`[` に対応する `]` がありません",
                        self.position,
                    )),
                    Stop::Right => Err(MathError::new(
                        "`\\left` に対応する `\\right` がありません",
                        self.position,
                    )),
                };
            };

            match character {
                '}' => {
                    let position = self.position;
                    if stop == Stop::Brace {
                        self.position += 1;
                        return Ok((nodes, Terminator::Brace(position)));
                    }
                    return Err(MathError::new(
                        "対応する `{` のない `}` があります",
                        position,
                    ));
                }
                ']' if stop == Stop::Bracket => {
                    let position = self.position;
                    self.position += 1;
                    return Ok((nodes, Terminator::Bracket(position)));
                }
                ']' => {
                    return Err(MathError::new(
                        "対応する `[` のない `]` があります",
                        self.position,
                    ));
                }
                '\\' if stop == Stop::Right && self.peek_word("right") => {
                    let close = self.parse_right()?;
                    return Ok((nodes, Terminator::Right(close)));
                }
                '&' => {
                    // Alignment points are not supported (no matrices); ignore
                    // them rather than showing a stray ampersand.
                    self.position += 1;
                }
                _ => {
                    let Some(atom) = self.parse_atom()? else {
                        continue;
                    };
                    let atom = self.parse_scripts(atom)?;
                    nodes.push(atom);
                }
            }
        }
    }

    fn peek_word(&self, word: &str) -> bool {
        if self.peek() != Some('\\') {
            return false;
        }
        for (offset, expected) in word.chars().enumerate() {
            if self.peek_at(offset + 1) != Some(expected) {
                return false;
            }
        }
        // Ensure the command name ends here (e.g. `\rightarrow` is not `\right`).
        !self
            .peek_at(word.len() + 1)
            .is_some_and(|character| character.is_ascii_alphabetic())
    }

    fn parse_right(&mut self) -> MathResult<Option<String>> {
        self.position += 1; // backslash
        self.position += "right".len();
        self.skip_whitespace();
        self.parse_delimiter()
    }

    fn parse_scripts(&mut self, mut atom: MathNode) -> MathResult<MathNode> {
        let mut sup: Option<MathNode> = None;
        let mut sub: Option<MathNode> = None;
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some('^') if sup.is_none() => {
                    self.position += 1;
                    sup = Some(self.parse_argument()?);
                }
                Some('_') if sub.is_none() => {
                    self.position += 1;
                    sub = Some(self.parse_argument()?);
                }
                Some('\'') => {
                    let mut primes = String::new();
                    while self.peek() == Some('\'') {
                        primes.push('′');
                        self.position += 1;
                    }
                    let prime = MathNode::Symbol {
                        text: primes,
                        kind: SymbolKind::Upright,
                    };
                    sup = Some(match sup {
                        Some(existing) => MathNode::Row(vec![existing, prime]),
                        None => prime,
                    });
                }
                Some('^') | Some('_') => {
                    return Err(MathError::new(
                        "上付き・下付きが重複しています",
                        self.position,
                    ));
                }
                _ => break,
            }
        }
        if sup.is_some() || sub.is_some() {
            atom = MathNode::Script {
                base: Box::new(atom),
                sup: sup.map(Box::new),
                sub: sub.map(Box::new),
            };
        }
        Ok(atom)
    }

    fn parse_argument(&mut self) -> MathResult<MathNode> {
        self.skip_whitespace();
        if self.peek() == Some('{') {
            return self.parse_group();
        }
        let Some(atom) = self.parse_atom()? else {
            return Err(MathError::new(
                "上付き・下付きの内容がありません",
                self.position,
            ));
        };
        Ok(atom)
    }

    fn parse_group(&mut self) -> MathResult<MathNode> {
        self.skip_whitespace();
        let start = self.position;
        if self.bump() != Some('{') {
            return Err(MathError::new("`{` が必要です", start));
        }
        if self.depth >= MAX_DEPTH {
            return Err(MathError::new("数式の入れ子が深すぎます", start));
        }
        self.depth += 1;
        let result = self.parse_sequence(Stop::Brace);
        self.depth -= 1;
        let (nodes, _) = result?;
        Ok(MathNode::Row(nodes))
    }

    fn parse_atom(&mut self) -> MathResult<Option<MathNode>> {
        let Some(character) = self.peek() else {
            return Ok(None);
        };
        match character {
            '{' => Ok(Some(self.parse_group()?)),
            '^' | '_' => Err(MathError::new(
                "上付き・下付きの対象がありません",
                self.position,
            )),
            '\\' => Ok(Some(self.parse_command()?)),
            '~' => {
                self.position += 1;
                Ok(Some(MathNode::Space(SpaceKind::Medium)))
            }
            _ => Ok(Some(self.parse_plain_atom())),
        }
    }

    fn parse_plain_atom(&mut self) -> MathNode {
        let character = self.bump().expect("caller checked a character");
        if character.is_ascii_digit() || character == '.' {
            let mut text = String::from(character);
            while let Some(next) = self.peek() {
                if next.is_ascii_digit() || (next == '.' && !text.contains('.')) {
                    text.push(next);
                    self.position += 1;
                } else {
                    break;
                }
            }
            return MathNode::Symbol {
                text,
                kind: SymbolKind::Number,
            };
        }
        if is_operator(character) {
            return MathNode::Symbol {
                text: character.to_string(),
                kind: SymbolKind::Operator,
            };
        }
        match character {
            '(' | ')' | '[' | ']' | '|' => MathNode::Symbol {
                text: character.to_string(),
                kind: SymbolKind::Delimiter,
            },
            ',' | ';' => MathNode::Symbol {
                text: character.to_string(),
                kind: SymbolKind::Punctuation,
            },
            _ if character.is_ascii_alphabetic() => MathNode::Symbol {
                text: character.to_string(),
                kind: SymbolKind::Identifier,
            },
            // Japanese and other non-ASCII words inside math are shown upright:
            // TeX would not italicise them, and `\text{}` is still the
            // documented way to write running text.
            _ => MathNode::Symbol {
                text: character.to_string(),
                kind: SymbolKind::Upright,
            },
        }
    }

    fn parse_command(&mut self) -> MathResult<MathNode> {
        let start = self.position;
        self.position += 1; // backslash
        let Some(first) = self.peek() else {
            return Err(MathError::new("`\\` の後にコマンドがありません", start));
        };
        let name = if first.is_ascii_alphabetic() {
            let mut name = String::new();
            while let Some(character) = self.peek() {
                if character.is_ascii_alphabetic() {
                    name.push(character);
                    self.position += 1;
                } else {
                    break;
                }
            }
            name
        } else {
            self.position += 1;
            first.to_string()
        };

        match name.as_str() {
            "frac" | "dfrac" | "tfrac" => {
                let numerator = self.parse_group()?;
                let denominator = self.parse_group()?;
                Ok(MathNode::Fraction {
                    numerator: Box::new(numerator),
                    denominator: Box::new(denominator),
                })
            }
            "sqrt" => {
                self.skip_whitespace();
                let index = if self.peek() == Some('[') {
                    self.position += 1;
                    let (nodes, _) = self.parse_sequence(Stop::Bracket)?;
                    Some(Box::new(MathNode::Row(nodes)))
                } else {
                    None
                };
                let body = self.parse_group()?;
                Ok(MathNode::Radical {
                    index,
                    body: Box::new(body),
                })
            }
            "text" | "textrm" | "textnormal" | "mathrm" | "operatorname" => {
                Ok(MathNode::Text(self.parse_raw_text()?))
            }
            "overline" => Ok(MathNode::Overline(Box::new(self.parse_group()?))),
            "underline" => Ok(MathNode::Underline(Box::new(self.parse_group()?))),
            "left" => {
                self.skip_whitespace();
                let open = self.parse_delimiter()?;
                if self.depth >= MAX_DEPTH {
                    return Err(MathError::new("数式の入れ子が深すぎます", start));
                }
                self.depth += 1;
                let result = self.parse_sequence(Stop::Right);
                self.depth -= 1;
                let (nodes, terminator) = result?;
                let close = match terminator {
                    Terminator::Right(close) => close,
                    _ => {
                        return Err(MathError::new(
                            "`\\left` に対応する `\\right` がありません",
                            start,
                        ));
                    }
                };
                Ok(MathNode::Delimited {
                    open,
                    close,
                    body: Box::new(MathNode::Row(nodes)),
                })
            }
            "right" => Err(MathError::new(
                "`\\right` に対応する `\\left` がありません",
                start,
            )),
            "sum" | "prod" | "coprod" | "int" | "iint" | "iiint" | "oint" | "bigcup" | "bigcap"
            | "bigoplus" | "bigotimes" => Ok(MathNode::BigOperator {
                symbol: big_operator(&name).to_string(),
            }),
            "lim" | "max" | "min" | "sup" | "inf" | "det" | "gcd" | "Pr" => {
                Ok(MathNode::Function {
                    name: name.clone(),
                    limits: true,
                })
            }
            "sin" | "cos" | "tan" | "cot" | "sec" | "csc" | "arcsin" | "arccos" | "arctan"
            | "sinh" | "cosh" | "tanh" | "log" | "ln" | "exp" | "deg" | "dim" | "ker" | "hom"
            | "arg" => Ok(MathNode::Function {
                name: name.clone(),
                limits: false,
            }),
            "pm" => Ok(operator_symbol("±")),
            "mp" => Ok(operator_symbol("∓")),
            "times" => Ok(operator_symbol("×")),
            "div" => Ok(operator_symbol("÷")),
            "cdot" => Ok(operator_symbol("·")),
            "ast" => Ok(operator_symbol("∗")),
            "star" => Ok(operator_symbol("⋆")),
            "circ" => Ok(operator_symbol("∘")),
            "bullet" => Ok(operator_symbol("•")),
            "le" | "leq" => Ok(operator_symbol("≤")),
            "ge" | "geq" => Ok(operator_symbol("≥")),
            "ne" | "neq" => Ok(operator_symbol("≠")),
            "approx" => Ok(operator_symbol("≈")),
            "equiv" => Ok(operator_symbol("≡")),
            "sim" => Ok(operator_symbol("∼")),
            "propto" => Ok(operator_symbol("∝")),
            "ll" => Ok(operator_symbol("≪")),
            "gg" => Ok(operator_symbol("≫")),
            "infty" => Ok(operator_symbol("∞")),
            "to" | "rightarrow" => Ok(operator_symbol("→")),
            "leftarrow" => Ok(operator_symbol("←")),
            "leftrightarrow" => Ok(operator_symbol("↔")),
            "Rightarrow" => Ok(operator_symbol("⇒")),
            "Leftarrow" => Ok(operator_symbol("⇐")),
            "Leftrightarrow" => Ok(operator_symbol("⇔")),
            "mapsto" => Ok(operator_symbol("↦")),
            "angle" => Ok(operator_symbol("∠")),
            "perp" => Ok(operator_symbol("⊥")),
            "parallel" => Ok(operator_symbol("∥")),
            "therefore" => Ok(operator_symbol("∴")),
            "because" => Ok(operator_symbol("∵")),
            "ldots" | "dots" => Ok(MathNode::Symbol {
                text: "…".to_string(),
                kind: SymbolKind::Upright,
            }),
            "cdots" => Ok(MathNode::Symbol {
                text: "⋯".to_string(),
                kind: SymbolKind::Upright,
            }),
            "degree" => Ok(MathNode::Symbol {
                text: "°".to_string(),
                kind: SymbolKind::Upright,
            }),
            "prime" => Ok(MathNode::Symbol {
                text: "′".to_string(),
                kind: SymbolKind::Upright,
            }),
            "quad" => Ok(MathNode::Space(SpaceKind::Quad)),
            "qquad" => Ok(MathNode::Row(vec![
                MathNode::Space(SpaceKind::Quad),
                MathNode::Space(SpaceKind::Quad),
            ])),
            "," | ":" => Ok(MathNode::Space(SpaceKind::Thin)),
            ";" | " " => Ok(MathNode::Space(SpaceKind::Medium)),
            "!" => Ok(MathNode::Space(SpaceKind::Thin)),
            "\\" => Ok(MathNode::LineBreak),
            "{" => Ok(delimiter_symbol("{")),
            "}" => Ok(delimiter_symbol("}")),
            "%" => Ok(MathNode::Symbol {
                text: "%".to_string(),
                kind: SymbolKind::Upright,
            }),
            "_" => Ok(MathNode::Symbol {
                text: "_".to_string(),
                kind: SymbolKind::Upright,
            }),
            "&" => Ok(MathNode::Symbol {
                text: "&".to_string(),
                kind: SymbolKind::Upright,
            }),
            "#" => Ok(MathNode::Symbol {
                text: "#".to_string(),
                kind: SymbolKind::Upright,
            }),
            "$" => Ok(MathNode::Symbol {
                text: "$".to_string(),
                kind: SymbolKind::Upright,
            }),
            "lvert" | "rvert" | "vert" => Ok(delimiter_symbol("|")),
            "lVert" | "rVert" | "Vert" => Ok(delimiter_symbol("‖")),
            "langle" => Ok(delimiter_symbol("⟨")),
            "rangle" => Ok(delimiter_symbol("⟩")),
            "lfloor" => Ok(delimiter_symbol("⌊")),
            "rfloor" => Ok(delimiter_symbol("⌋")),
            "lceil" => Ok(delimiter_symbol("⌈")),
            "rceil" => Ok(delimiter_symbol("⌉")),
            "backslash" => Ok(delimiter_symbol("\\")),
            "limits" | "nolimits" | "displaystyle" | "textstyle" => Ok(MathNode::Row(Vec::new())),
            greek => match greek_symbol(greek) {
                Some(symbol) => Ok(MathNode::Symbol {
                    text: symbol.to_string(),
                    kind: SymbolKind::Upright,
                }),
                None => Err(MathError::new(
                    "対応していないLaTeXコマンドです",
                    self.position,
                )),
            },
        }
    }

    fn parse_raw_text(&mut self) -> MathResult<String> {
        self.skip_whitespace();
        if self.bump() != Some('{') {
            return Err(MathError::new(
                "`\\text{}` の `{` がありません",
                self.position,
            ));
        }
        let mut text = String::new();
        let mut depth = 1usize;
        while let Some(character) = self.bump() {
            match character {
                '{' => {
                    depth += 1;
                    text.push('{');
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(text);
                    }
                    text.push('}');
                }
                '\\' => match self.bump() {
                    Some(escaped @ ('{' | '}' | '%' | '_' | '&' | '#' | '$' | '\\')) => {
                        text.push(escaped)
                    }
                    Some(other) => {
                        text.push('\\');
                        text.push(other);
                    }
                    None => text.push('\\'),
                },
                other => text.push(other),
            }
        }
        Err(MathError::new(
            "`\\text{}` の `}` がありません",
            self.position,
        ))
    }

    fn parse_delimiter(&mut self) -> MathResult<Option<String>> {
        let Some(character) = self.peek() else {
            return Err(MathError::new("区切り記号がありません", self.position));
        };
        if character == '\\' {
            self.position += 1;
            let Some(name) = self.bump() else {
                return Err(MathError::new("区切り記号がありません", self.position));
            };
            let text = match name {
                '{' => "{",
                '}' => "}",
                '|' => "|",
                other => match other {
                    _ if other.is_ascii_alphabetic() => {
                        let mut command = String::from(other);
                        while let Some(next) = self.peek() {
                            if next.is_ascii_alphabetic() {
                                command.push(next);
                                self.position += 1;
                            } else {
                                break;
                            }
                        }
                        match command.as_str() {
                            "langle" => "⟨",
                            "rangle" => "⟩",
                            "lvert" => "|",
                            "rvert" => "|",
                            "lVert" => "‖",
                            "rVert" => "‖",
                            "Vert" => "‖",
                            "lfloor" => "⌊",
                            "rfloor" => "⌋",
                            "lceil" => "⌈",
                            "rceil" => "⌉",
                            "backslash" => "\\",
                            _ => {
                                return Err(MathError::new(
                                    "対応していない区切り記号です",
                                    self.position,
                                ));
                            }
                        }
                    }
                    _ => {
                        return Err(MathError::new("区切り記号がありません", self.position));
                    }
                },
            };
            return Ok(Some(text.to_string()));
        }
        self.position += 1;
        match character {
            '.' => Ok(None),
            '(' | ')' | '[' | ']' | '|' | '/' | '<' | '>' => Ok(Some(character.to_string())),
            '{' | '}' => Ok(Some(character.to_string())),
            _ => Err(MathError::new(
                "対応していない区切り記号です",
                self.position,
            )),
        }
    }
}

fn operator_symbol(text: &str) -> MathNode {
    MathNode::Symbol {
        text: text.to_string(),
        kind: SymbolKind::Operator,
    }
}

fn delimiter_symbol(text: &str) -> MathNode {
    MathNode::Symbol {
        text: text.to_string(),
        kind: SymbolKind::Delimiter,
    }
}

fn is_operator(character: char) -> bool {
    matches!(
        character,
        '+' | '-'
            | '='
            | '<'
            | '>'
            | '±'
            | '×'
            | '÷'
            | '·'
            | '/'
            | '∗'
            | '≈'
            | '≠'
            | '≤'
            | '≥'
            | '→'
            | '←'
            | '∞'
            | '∑'
            | '∏'
            | '∫'
    )
}

fn big_operator(name: &str) -> &'static str {
    match name {
        "sum" => "∑",
        "prod" => "∏",
        "coprod" => "∐",
        "int" => "∫",
        "iint" => "∬",
        "iiint" => "∭",
        "oint" => "∮",
        "bigcup" => "⋃",
        "bigcap" => "⋂",
        "bigoplus" => "⨁",
        "bigotimes" => "⨂",
        _ => "∑",
    }
}

fn greek_symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" => "ε",
        "varepsilon" => "ϵ",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "omicron" => "ο",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" => "ρ",
        "varrho" => "ϱ",
        "sigma" => "σ",
        "varsigma" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" => "φ",
        "varphi" => "ϕ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        _ => return None,
    })
}

/// How tall an expression is in ems of its own font size. Used to scale
/// radicals and `\left`/`\right` delimiters around fractions.
pub(crate) fn tallness(node: &MathNode) -> f32 {
    match node {
        MathNode::Row(nodes) => nodes
            .iter()
            .map(tallness)
            .fold(1.0_f32, |current, next| current.max(next)),
        MathNode::Symbol { .. }
        | MathNode::Text(_)
        | MathNode::Space(_)
        | MathNode::Function { .. } => 1.0,
        MathNode::LineBreak => 1.0,
        MathNode::Fraction {
            numerator,
            denominator,
        } => tallness(numerator) + tallness(denominator) + FRACTION_RULE_EM,
        MathNode::Radical { body, .. } => tallness(body) + 0.18,
        MathNode::Script { base, sup, sub } => {
            let scripts = sup
                .as_deref()
                .map(tallness)
                .unwrap_or(0.0)
                .max(sub.as_deref().map(tallness).unwrap_or(0.0));
            tallness(base).max(scripts + SCRIPT_SHIFT_EM + 0.1)
        }
        MathNode::Delimited { body, .. } => tallness(body),
        MathNode::BigOperator { .. } => BIG_OPERATOR_SCALE,
        MathNode::Overline(body) | MathNode::Underline(body) => tallness(body) + 0.14,
    }
}

fn has_limits_style(node: &MathNode) -> bool {
    matches!(
        node,
        MathNode::BigOperator { .. } | MathNode::Function { limits: true, .. }
    )
}

/// Scales the radical glyph so a fraction or nested root is fully covered.
pub(crate) fn radical_scale(body: &MathNode) -> f32 {
    (tallness(body) * 0.9).clamp(1.0, 2.8)
}

/// Delimiters around tall content grow, like `\left(\frac{a}{b}\right)`.
pub(crate) fn delimiter_scale(body: &MathNode) -> f32 {
    (tallness(body) * 0.85).clamp(1.0, 2.4)
}

/// Display math is a block; options and table cells use compact math.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MathMode {
    Display,
    Inline,
}

/// Resolved visual style for a math tree.
#[derive(Clone)]
struct MathStyle {
    size: f32,
    color: Hsla,
    rule_color: Hsla,
    font_family: SharedString,
    mode: MathMode,
}

impl MathStyle {
    fn new(theme: &Theme, size: f32, mode: MathMode) -> Self {
        Self {
            size,
            color: theme.foreground,
            rule_color: theme.border,
            font_family: theme.font_family.clone(),
            mode,
        }
    }

    fn scaled(&self, factor: f32) -> Self {
        Self {
            size: (self.size * factor).max(6.0),
            ..self.clone()
        }
    }

    fn ems(&self, ems: f32) -> f32 {
        (self.size * ems).max(1.0)
    }
}

/// Renders a LaTeX math string with GPUI primitives.
///
/// Invalid input is never a crash: the raw source is shown in a readable
/// monospace fallback with an explanatory note, and the source is exposed as
/// the accessibility label.
pub(crate) fn render_math(text: &str, theme: &Theme, size: f32, mode: MathMode) -> AnyElement {
    match parse_math(text) {
        Ok(node) => {
            let style = MathStyle::new(theme, size, mode);
            div()
                .id(math_element_id(text))
                .role(gpui_kit::Role::Image)
                .flex()
                .flex_col()
                .items_start()
                .gap_1()
                .min_w(px(0.0))
                .max_w_full()
                .aria_label(text.to_string())
                .child(render_math_node(&node, &style))
                .into_any_element()
        }
        Err(error) => render_math_fallback(text, theme, size, mode, &error),
    }
}

/// Element ID for a math element. Math has no widget state, so the ID only
/// needs to keep GPUI's stateful accessibility node unique within a frame.
fn math_element_id(text: &str) -> gpui_kit::ElementId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let serial = NEXT.fetch_add(1, Ordering::Relaxed) as usize;
    gpui_kit::ElementId::named_usize(format!("math-{}", text.len()), serial)
}

fn render_math_fallback(
    text: &str,
    theme: &Theme,
    size: f32,
    _mode: MathMode,
    error: &MathError,
) -> AnyElement {
    div()
        .id(math_element_id(text))
        .role(gpui_kit::Role::Image)
        .flex()
        .flex_col()
        .gap_1()
        .min_w(px(0.0))
        .aria_label(text.to_string())
        .child(
            div()
                .font_family(theme.mono_font_family.clone())
                .text_size(px((size * 0.92).max(11.0)))
                .line_height(px(size * 1.3))
                .text_color(theme.foreground)
                .whitespace_nowrap()
                .child(text.to_string()),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme.muted_foreground)
                .child(format!(
                    "数式として組版できません（{}文字目: {}）",
                    error.position + 1,
                    error.message
                )),
        )
        .into_any_element()
}

fn render_math_node(node: &MathNode, style: &MathStyle) -> AnyElement {
    match node {
        MathNode::Row(nodes) => render_math_row(nodes, style),
        MathNode::Symbol { text, kind } => render_symbol(text, *kind, style),
        MathNode::Text(text) => render_text_run(text, style),
        MathNode::Space(kind) => div().w(px(style.ems(kind.ems()))).into_any_element(),
        MathNode::LineBreak => div().w_full().h(px(1.0)).into_any_element(),
        MathNode::Fraction {
            numerator,
            denominator,
        } => render_fraction(numerator, denominator, style),
        MathNode::Radical { index, body } => render_radical(index.as_deref(), body, style),
        MathNode::Script { base, sup, sub } => {
            render_script(base, sup.as_deref(), sub.as_deref(), style)
        }
        MathNode::Delimited { open, close, body } => {
            render_delimited(open.as_deref(), close.as_deref(), body, style)
        }
        MathNode::BigOperator { symbol } => div()
            .text_size(px(style.size * BIG_OPERATOR_SCALE))
            .line_height(px(style.size * BIG_OPERATOR_SCALE * 1.15))
            .text_color(style.color)
            .font_family(style.font_family.clone())
            .child(symbol.clone())
            .into_any_element(),
        MathNode::Function { name, .. } => render_text_run(name, style),
        MathNode::Overline(body) => div()
            .flex()
            .flex_col()
            .items_start()
            .border_t_1()
            .border_color(style.rule_color)
            .pt(px(style.ems(0.06)))
            .child(render_math_node(body, style))
            .into_any_element(),
        MathNode::Underline(body) => div()
            .flex()
            .flex_col()
            .items_start()
            .border_b_1()
            .border_color(style.rule_color)
            .pb(px(style.ems(0.06)))
            .child(render_math_node(body, style))
            .into_any_element(),
    }
}

fn render_math_row(nodes: &[MathNode], style: &MathStyle) -> AnyElement {
    let mut lines: Vec<Vec<&MathNode>> = vec![Vec::new()];
    for node in nodes {
        if *node == MathNode::LineBreak {
            lines.push(Vec::new());
        } else {
            lines.last_mut().expect("at least one line").push(node);
        }
    }
    let mut column = div().flex().flex_col().items_start().gap_1().min_w(px(0.0));
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_y(px(style.ems(0.12)))
            .min_w(px(0.0))
            .max_w_full();
        for (index, node) in line.iter().enumerate() {
            let rendered = render_math_node(node, style);
            if is_binary_operator(node, &line, index) {
                row = row.child(
                    div()
                        .mx(px(style.ems(0.2)))
                        .child(rendered)
                        .into_any_element(),
                );
            } else {
                row = row.child(rendered);
            }
        }
        column = column.child(row);
    }
    column.into_any_element()
}

/// Binary `+`/`-` get medium spacing, but a leading sign or a sign after an
/// operator or opening delimiter must stay attached to its operand. Without
/// this the numerator of `\frac{-b}{2a}` is indented and `x-2` looks like one
/// identifier.
fn is_binary_operator(node: &MathNode, line: &[&MathNode], index: usize) -> bool {
    let MathNode::Symbol { text, kind } = node else {
        return false;
    };
    if *kind != SymbolKind::Operator {
        return false;
    }
    if text != "+" && text != "-" {
        return true;
    }
    if index == 0 {
        return false;
    }
    ends_operand(line[index - 1])
}

fn ends_operand(node: &MathNode) -> bool {
    match node {
        MathNode::Symbol { text, kind } => match kind {
            SymbolKind::Identifier | SymbolKind::Number | SymbolKind::Upright => true,
            SymbolKind::Delimiter => {
                matches!(text.as_str(), ")" | "]" | "}" | "|" | "⟩" | "⌋" | "⌉")
            }
            SymbolKind::Operator | SymbolKind::Punctuation => false,
        },
        MathNode::Text(_)
        | MathNode::Fraction { .. }
        | MathNode::Radical { .. }
        | MathNode::Script { .. }
        | MathNode::Delimited { .. }
        | MathNode::BigOperator { .. }
        | MathNode::Function { .. }
        | MathNode::Overline(_)
        | MathNode::Underline(_) => true,
        MathNode::Row(nodes) => nodes.last().is_some_and(ends_operand),
        MathNode::Space(_) | MathNode::LineBreak => false,
    }
}

fn render_symbol(text: &str, kind: SymbolKind, style: &MathStyle) -> AnyElement {
    let mut element = div()
        .flex()
        .items_center()
        .text_size(px(style.size))
        .line_height(px(style.size * 1.22))
        .text_color(style.color)
        .font_family(style.font_family.clone())
        .whitespace_nowrap()
        .child(text.to_string());
    match kind {
        SymbolKind::Identifier => element = element.italic(),
        SymbolKind::Punctuation => {
            element = element.mr(px(style.ems(0.12)));
        }
        SymbolKind::Delimiter => {
            element = element.mx(px(style.ems(0.02)));
        }
        SymbolKind::Operator | SymbolKind::Number | SymbolKind::Upright => {}
    }
    element.into_any_element()
}

fn render_text_run(text: &str, style: &MathStyle) -> AnyElement {
    div()
        .text_size(px(style.size))
        .line_height(px(style.size * 1.22))
        .text_color(style.color)
        .font_family(style.font_family.clone())
        .whitespace_nowrap()
        .child(text.to_string())
        .into_any_element()
}

fn render_fraction(numerator: &MathNode, denominator: &MathNode, style: &MathStyle) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .mx(px(style.ems(0.1)))
        .min_w(px(0.0))
        .child(render_math_node(numerator, style))
        .child(
            div()
                .flex()
                .w_full()
                .min_w(px(style.ems(0.5)))
                .h(px(1.0))
                .my(px(style.ems(0.1)))
                .bg(style.color),
        )
        .child(render_math_node(denominator, style))
        .into_any_element()
}

fn render_radical(index: Option<&MathNode>, body: &MathNode, style: &MathStyle) -> AnyElement {
    let radical_style = style.scaled(radical_scale(body));
    let mut root = div()
        .flex()
        .items_end()
        .mx(px(style.ems(0.04)))
        .min_w(px(0.0));
    if let Some(index) = index {
        root = root.child(
            div()
                .relative()
                .top(px(-style.ems(0.3)))
                .mr(px(-style.ems(0.12)))
                .child(render_math_node(index, &style.scaled(SCRIPT_SCALE * 0.9))),
        );
    }
    root = root
        .child(
            div()
                .text_size(px(radical_style.size))
                .line_height(px(radical_style.size * 0.95))
                .text_color(style.color)
                .font_family(style.font_family.clone())
                .child("√"),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_start()
                .border_t_1()
                .border_color(style.rule_color)
                .pt(px(style.ems(0.05)))
                .min_w(px(0.0))
                .child(render_math_node(body, style)),
        );
    root.into_any_element()
}

fn render_script(
    base: &MathNode,
    sup: Option<&MathNode>,
    sub: Option<&MathNode>,
    style: &MathStyle,
) -> AnyElement {
    if style.mode == MathMode::Display && has_limits_style(base) {
        let script_style = style.scaled(SCRIPT_SCALE);
        let mut column = div()
            .flex()
            .flex_col()
            .items_center()
            .mx(px(style.ems(0.12)))
            .min_w(px(0.0));
        if let Some(sup) = sup {
            column = column.child(render_math_node(sup, &script_style));
        }
        column = column.child(render_math_node(base, style));
        if let Some(sub) = sub {
            column = column.child(render_math_node(sub, &script_style));
        }
        return column.into_any_element();
    }

    let script_style = style.scaled(SCRIPT_SCALE);
    let row = div()
        .flex()
        .items_center()
        .min_w(px(0.0))
        .child(render_math_node(base, style));
    let mut scripts = div().flex().flex_col().items_start().min_w(px(0.0));
    if let Some(sup) = sup {
        scripts = scripts.child(
            div()
                .relative()
                .top(px(-style.ems(SCRIPT_SHIFT_EM)))
                .child(render_math_node(sup, &script_style)),
        );
    }
    if let Some(sub) = sub {
        scripts = scripts.child(
            div()
                .relative()
                .top(px(style.ems(SCRIPT_SHIFT_EM * 0.35)))
                .child(render_math_node(sub, &script_style)),
        );
    }
    row.child(scripts).into_any_element()
}

fn render_delimited(
    open: Option<&str>,
    close: Option<&str>,
    body: &MathNode,
    style: &MathStyle,
) -> AnyElement {
    let delimiter_style = style.scaled(delimiter_scale(body));
    let mut row = div()
        .flex()
        .items_center()
        .mx(px(style.ems(0.04)))
        .min_w(px(0.0));
    if let Some(open) = open {
        row = row.child(render_delimiter(open, &delimiter_style));
    }
    row = row.child(render_math_node(body, style));
    if let Some(close) = close {
        row = row.child(render_delimiter(close, &delimiter_style));
    }
    row.into_any_element()
}

fn render_delimiter(text: &str, style: &MathStyle) -> AnyElement {
    div()
        .text_size(px(style.size))
        .line_height(px(style.size * 1.1))
        .text_color(style.color)
        .font_family(style.font_family.clone())
        .child(text.to_string())
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Inline math in Markdown / plain text
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InlineSegment {
    Text(String),
    Math(String),
}

/// Splits running text into plain runs and `$...$` / `\(...\)` math runs.
///
/// Fenced code blocks and inline code spans are left untouched so dollar signs
/// in code are not mistaken for math. Unmatched delimiters stay literal text.
pub(crate) fn split_inline_math(text: &str) -> Vec<InlineSegment> {
    if !text.contains('$') && !text.contains("\\(") {
        return vec![InlineSegment::Text(text.to_string())];
    }

    let mut segments = Vec::new();
    let mut plain = String::new();
    let mut fence: Option<(char, usize)> = None;

    for line in text.split_inclusive('\n') {
        if let Some((marker, count)) = fence {
            plain.push_str(line);
            if is_fence_line(line, marker, count) {
                fence = None;
            }
            continue;
        }
        if let Some((marker, count)) = fence_open(line) {
            plain.push_str(line);
            fence = Some((marker, count));
            continue;
        }
        scan_inline_line(line, &mut plain, &mut segments);
    }

    push_text(&mut segments, &mut plain);
    merge_text_segments(&mut segments);
    if segments.is_empty() {
        segments.push(InlineSegment::Text(String::new()));
    }
    segments
}

fn fence_open(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|value| *value == marker).count();
    (count >= 3).then_some((marker, count))
}

fn is_fence_line(line: &str, marker: char, count: usize) -> bool {
    let trimmed = line.trim_start_matches(' ');
    let run = trimmed.chars().take_while(|value| *value == marker).count();
    run >= count && trimmed.chars().skip(run).all(|value| value.is_whitespace())
}

fn merge_text_segments(segments: &mut Vec<InlineSegment>) {
    let mut merged: Vec<InlineSegment> = Vec::with_capacity(segments.len());
    for segment in segments.drain(..) {
        match (merged.last_mut(), segment) {
            (Some(InlineSegment::Text(previous)), InlineSegment::Text(next)) => {
                previous.push_str(&next);
            }
            (_, segment) => merged.push(segment),
        }
    }
    *segments = merged;
}

fn scan_inline_line(line: &str, plain: &mut String, segments: &mut Vec<InlineSegment>) {
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];

        if character == '`' {
            let mut end = index + 1;
            while end < chars.len() && chars[end] != '`' {
                end += 1;
            }
            if end < chars.len() && chars[end] == '`' {
                for character in &chars[index..=end] {
                    plain.push(*character);
                }
                index = end + 1;
                continue;
            }
        }

        if character == '\\' && chars.get(index + 1) == Some(&'$') {
            plain.push('$');
            index += 2;
            continue;
        }

        if character == '$'
            && chars.get(index + 1) == Some(&'$')
            && let Some(end) = find_sequence(&chars, index + 2, &['$', '$'])
        {
            let math: String = chars[index + 2..end].iter().collect();
            if !math.trim().is_empty() {
                push_text(segments, plain);
                segments.push(InlineSegment::Math(math));
                index = end + 2;
                continue;
            }
        }

        if character == '$'
            && let Some(end) = find_closing_dollar(&chars, index + 1)
        {
            let math: String = chars[index + 1..end].iter().collect();
            if !math.trim().is_empty() {
                push_text(segments, plain);
                segments.push(InlineSegment::Math(math));
                index = end + 1;
                continue;
            }
        }

        if character == '\\'
            && chars.get(index + 1) == Some(&'(')
            && let Some(end) = find_sequence(&chars, index + 2, &['\\', ')'])
        {
            let math: String = chars[index + 2..end].iter().collect();
            if !math.trim().is_empty() {
                push_text(segments, plain);
                segments.push(InlineSegment::Math(math));
                index = end + 2;
                continue;
            }
        }

        plain.push(character);
        index += 1;
    }
    push_text(segments, plain);
}

fn push_text(segments: &mut Vec<InlineSegment>, plain: &mut String) {
    if !plain.is_empty() {
        segments.push(InlineSegment::Text(std::mem::take(plain)));
    }
}

fn find_closing_dollar(chars: &[char], start: usize) -> Option<usize> {
    let mut index = start;
    while index < chars.len() {
        match chars[index] {
            '\\' if chars.get(index + 1) == Some(&'$') => {
                index += 2;
            }
            '$' => return Some(index),
            _ => index += 1,
        }
    }
    None
}

fn find_sequence(chars: &[char], start: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let mut index = start;
    while index + needle.len() <= chars.len() {
        if chars[index..index + needle.len()] == *needle {
            return Some(index);
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(text: &str, kind: SymbolKind) -> MathNode {
        MathNode::Symbol {
            text: text.to_string(),
            kind,
        }
    }

    fn row(nodes: Vec<MathNode>) -> MathNode {
        MathNode::Row(nodes)
    }

    fn parse(input: &str) -> MathNode {
        parse_math(input).unwrap_or_else(|error| panic!("{input}: {error:?}"))
    }

    fn contains<F: Fn(&MathNode) -> bool>(node: &MathNode, predicate: &F) -> bool {
        if predicate(node) {
            return true;
        }
        match node {
            MathNode::Row(nodes) => nodes.iter().any(|node| contains(node, predicate)),
            MathNode::Fraction {
                numerator,
                denominator,
            } => contains(numerator, predicate) || contains(denominator, predicate),
            MathNode::Radical { index, body } => {
                index
                    .as_deref()
                    .is_some_and(|node| contains(node, predicate))
                    || contains(body, predicate)
            }
            MathNode::Script { base, sup, sub } => {
                contains(base, predicate)
                    || sup.as_deref().is_some_and(|node| contains(node, predicate))
                    || sub.as_deref().is_some_and(|node| contains(node, predicate))
            }
            MathNode::Delimited { body, .. } => contains(body, predicate),
            MathNode::Overline(body) | MathNode::Underline(body) => contains(body, predicate),
            MathNode::Symbol { .. }
            | MathNode::Text(_)
            | MathNode::Space(_)
            | MathNode::LineBreak
            | MathNode::BigOperator { .. }
            | MathNode::Function { .. } => false,
        }
    }

    #[test]
    fn required_minimum_formulas_parse() {
        for input in [
            r"\frac{1}{2}",
            "x^2 + 2x + 1",
            r"\sqrt{3}",
            r"\frac{-b \pm \sqrt{b^2-4ac}}{2a}",
            "a_1 + a_2",
            r"\sum_{i=1}^{n} i",
        ] {
            let node = parse(input);
            assert!(matches!(node, MathNode::Row(_)), "{input}");
        }
    }

    #[test]
    fn parses_fraction_as_vertical_layout_nodes() {
        let node = parse(r"\frac{1}{2}");
        let MathNode::Row(nodes) = &node else {
            panic!("{node:?}");
        };
        let MathNode::Fraction {
            numerator,
            denominator,
        } = &nodes[0]
        else {
            panic!("{nodes:?}");
        };
        assert_eq!(**numerator, row(vec![sym("1", SymbolKind::Number)]));
        assert_eq!(**denominator, row(vec![sym("2", SymbolKind::Number)]));
        assert!((tallness(&node) - (1.0 + 1.0 + FRACTION_RULE_EM)).abs() < 1e-6);
    }

    #[test]
    fn parses_superscripts_and_subscripts_with_smaller_scripts() {
        let node = parse("x^2 + a_1");
        assert!(contains(&node, &|node| matches!(
            node,
            MathNode::Script {
                sup: Some(_),
                sub: None,
                ..
            }
        )));
        assert!(contains(&node, &|node| matches!(
            node,
            MathNode::Script {
                sup: None,
                sub: Some(_),
                ..
            }
        )));
        // Two glyphs sit on one row: the script must be scaled, not rendered
        // at the same size as the base.
        let script_scale = SCRIPT_SCALE;
        let script_shift = SCRIPT_SHIFT_EM;
        assert!(script_scale < 1.0);
        assert!(script_shift > 0.0);
        let scripted = parse("x^2");
        let MathNode::Row(nodes) = &scripted else {
            panic!();
        };
        assert!(matches!(nodes[0], MathNode::Script { .. }));
    }

    #[test]
    fn supports_both_scripts_on_one_base() {
        let node = parse("x_i^2");
        let MathNode::Row(nodes) = &node else {
            panic!();
        };
        let MathNode::Script { sup, sub, .. } = &nodes[0] else {
            panic!("{nodes:?}");
        };
        assert!(sup.is_some());
        assert!(sub.is_some());
    }

    #[test]
    fn parses_quadratic_formula_with_nested_fraction_and_root() {
        let node = parse(r"\frac{-b \pm \sqrt{b^2-4ac}}{2a}");
        let MathNode::Row(nodes) = &node else {
            panic!();
        };
        let MathNode::Fraction {
            numerator,
            denominator,
        } = &nodes[0]
        else {
            panic!("{nodes:?}");
        };
        assert!(contains(numerator, &|node| matches!(
            node,
            MathNode::Radical { .. }
        )));
        assert!(contains(numerator, &|node| matches!(
            node,
            MathNode::Symbol { text, .. } if text == "±"
        )));
        assert!(tallness(&node) > tallness(numerator.as_ref()));
        assert!(tallness(denominator.as_ref()) >= 1.0);
    }

    #[test]
    fn parses_nth_root_and_sum_limits() {
        let root = parse(r"\sqrt[3]{x}");
        let MathNode::Row(nodes) = &root else {
            panic!();
        };
        let MathNode::Radical { index, .. } = &nodes[0] else {
            panic!("{nodes:?}");
        };
        assert!(index.is_some());
        assert!(
            radical_scale(&nodes[0])
                >= radical_scale(match &nodes[0] {
                    MathNode::Radical { body, .. } => body,
                    _ => unreachable!(),
                })
        );

        let sum = parse(r"\sum_{i=1}^{n} i");
        assert!(contains(&sum, &|node| matches!(
            node,
            MathNode::Script { base, .. } if matches!(base.as_ref(), MathNode::BigOperator { .. })
        )));
        assert!(matches!(
            &parse(r"\sum"),
            MathNode::Row(nodes) if matches!(nodes[0], MathNode::BigOperator { .. })
        ));
    }

    #[test]
    fn parses_greek_functions_absolute_value_and_text() {
        let node = parse(r"\sin\theta + |x| + \text{速さ}");
        assert!(contains(&node, &|node| matches!(
            node,
            MathNode::Function { name, .. } if name == "sin"
        )));
        assert!(contains(&node, &|node| matches!(
            node,
            MathNode::Symbol { text, .. } if text == "θ"
        )));
        assert!(contains(&node, &|node| matches!(
            node,
            MathNode::Text(text) if text == "速さ"
        )));
        assert!(contains(&node, &|node| matches!(
            node,
            MathNode::Symbol { text, kind: SymbolKind::Delimiter } if text == "|"
        )));
    }

    #[test]
    fn scaled_delimiters_grow_with_fraction_body() {
        let small = parse(r"\left(x\right)");
        let tall = parse(r"\left(\frac{1}{2}\right)");
        let small_body = match &small {
            MathNode::Row(nodes) => match &nodes[0] {
                MathNode::Delimited { body, .. } => body.as_ref(),
                _ => panic!(),
            },
            _ => panic!(),
        };
        let tall_body = match &tall {
            MathNode::Row(nodes) => match &nodes[0] {
                MathNode::Delimited { body, .. } => body.as_ref(),
                _ => panic!(),
            },
            _ => panic!(),
        };
        assert!(delimiter_scale(tall_body) > delimiter_scale(small_body));
        assert!(delimiter_scale(tall_body) <= 2.4);
    }

    #[test]
    fn long_formulas_keep_every_node() {
        let input = "a_1 + a_2 + a_3 + a_4 + a_5 + a_6 + a_7 + a_8 + a_9 + a_{10}";
        let node = parse(input);
        let MathNode::Row(nodes) = &node else {
            panic!();
        };
        assert!(nodes.len() >= 10, "{nodes:?}");
        // Wrapping is a renderer concern; the parser must never drop tokens.
        assert!(contains(&node, &|node| matches!(
            node,
            MathNode::Script { sub: Some(_), .. }
        )));
    }

    #[test]
    fn unsupported_or_unbalanced_syntax_returns_an_error() {
        for input in [
            r"\frac{1}{2",
            r"x^",
            r"\sqrt[3{x}",
            r"\left(\frac{1}{2}",
            r"}",
            r"\unknowncommand",
        ] {
            assert!(parse_math(input).is_err(), "{input}");
        }
        // Empty input is valid and renders as an empty row.
        assert!(parse_math("").is_ok());

        let deep = format!("{}x{}", "{".repeat(200), "}".repeat(200));
        let error = parse_math(&deep).expect_err("deep nesting must be rejected safely");
        assert!(error.message.contains("深すぎ"));
    }

    #[test]
    fn parses_plain_workbook_math_such_as_slopes() {
        for input in ["y = 2x + 1", "I = V / R", "1/2", "-2"] {
            let node = parse(input);
            assert!(matches!(node, MathNode::Row(_)), "{input}");
        }
        // A block that already contains math delimiters is not shown with
        // literal dollars.
        for input in ["$x^2$", "$$x^2$$", "  $\\frac{1}{2}$  "] {
            let node = parse(input);
            assert!(
                !contains(&node, &|node| matches!(
                    node,
                    MathNode::Symbol { text, .. } if text == "$"
                )),
                "{input}"
            );
        }
    }

    #[test]
    fn unary_signs_stay_attached_and_binary_operators_get_space() {
        let node = parse("-b + x - 2");
        let MathNode::Row(nodes) = &node else {
            panic!();
        };
        let line: Vec<&MathNode> = nodes.iter().collect();
        assert!(!is_binary_operator(line[0], &line, 0));
        assert!(!is_binary_operator(line[1], &line, 1));
        assert!(is_binary_operator(line[2], &line, 2));
        assert!(is_binary_operator(line[4], &line, 4));

        let node = parse(r"\left(-b\right)");
        let MathNode::Row(nodes) = &node else {
            panic!();
        };
        let MathNode::Delimited { body, .. } = &nodes[0] else {
            panic!();
        };
        let MathNode::Row(inner) = body.as_ref() else {
            panic!();
        };
        let line: Vec<&MathNode> = inner.iter().collect();
        assert!(!is_binary_operator(line[0], &line, 0));
    }

    #[test]
    fn splits_inline_math_and_leaves_code_alone() {
        assert_eq!(
            split_inline_math("速さは $v=\\frac{d}{t}$ です"),
            vec![
                InlineSegment::Text("速さは ".to_string()),
                InlineSegment::Math("v=\\frac{d}{t}".to_string()),
                InlineSegment::Text(" です".to_string()),
            ]
        );
        assert_eq!(
            split_inline_math("`$not math$` and $x$"),
            vec![
                InlineSegment::Text("`$not math$` and ".to_string()),
                InlineSegment::Math("x".to_string()),
            ]
        );
        assert_eq!(
            split_inline_math("```\n$x$\n```\n$x$"),
            vec![
                InlineSegment::Text("```\n$x$\n```\n".to_string()),
                InlineSegment::Math("x".to_string()),
            ]
        );
        assert_eq!(
            split_inline_math(r"price \$5 and \(x^2\)"),
            vec![
                InlineSegment::Text("price $5 and ".to_string()),
                InlineSegment::Math("x^2".to_string()),
            ]
        );
        assert_eq!(
            split_inline_math("$$\\frac{1}{2}$$ の値"),
            vec![
                InlineSegment::Math("\\frac{1}{2}".to_string()),
                InlineSegment::Text(" の値".to_string()),
            ]
        );
        // Unmatched dollars stay literal.
        assert_eq!(
            split_inline_math("cost is $5"),
            vec![InlineSegment::Text("cost is $5".to_string())]
        );
    }
}
