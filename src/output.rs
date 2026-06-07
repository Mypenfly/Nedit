//! 终端输出格式化 (Output)
//!
//! 提供彩色终端输出和格式化功能。
//!
//! ## 实现逻辑
//!
//! 1. 检测终端能力（`is_terminal`），管道/重定向时自动关闭颜色
//! 2. 新增行前加绿色 `+`，删除行前加红色 `-`
//! 3. ContentBlock 输出带行号前缀
//! 4. Phase 6: 错误彩色输出、上下文展示、分隔符
//!
//! ## 对应文档
//!
//! 详见 INSTRUCTION.md 第 5.2 节 "输出格式"

use crate::model::LineNumber;
use colored::Colorize;
use std::io::IsTerminal;

/// 上下文中展示的最大行数（修改区域上下各最多 7 行）
pub const CONTEXT_MAX_LINES: usize = 7;

/// 输出行的差异状态
#[derive(Debug, PartialEq)]
pub enum DiffLineKind {
    /// 新增的行
    Added,
    /// 删除的行
    Deleted,
    /// 未变更的行（上下文）
    Unchanged,
    /// 分隔符行（"~~~~~~~~"）
    Separator,
}

/// 一条带差异标记的输出行
#[derive(Debug, PartialEq)]
pub struct DiffLine {
    /// 差异状态
    pub kind: DiffLineKind,
    /// 行号（可选的）
    pub line_number: Option<LineNumber>,
    /// 内容文本
    pub content: String,
}

impl DiffLine {
    /// 创建一个分隔符行
    pub fn separator() -> Self {
        DiffLine {
            kind: DiffLineKind::Separator,
            line_number: None,
            content: String::new(),
        }
    }

    /// 创建一个不变的上下文行
    pub fn unchanged(line_number: LineNumber, content: String) -> Self {
        DiffLine {
            kind: DiffLineKind::Unchanged,
            line_number: Some(line_number),
            content,
        }
    }

    /// 创建一个新增行
    pub fn added(line_number: LineNumber, content: String) -> Self {
        DiffLine {
            kind: DiffLineKind::Added,
            line_number: Some(line_number),
            content,
        }
    }

    /// 创建一个删除行
    pub fn deleted(line_number: LineNumber, content: String) -> Self {
        DiffLine {
            kind: DiffLineKind::Deleted,
            line_number: Some(line_number),
            content,
        }
    }
}

/// 终端输出格式化器
///
/// 负责将差异行列表格式化为彩色终端输出。
pub struct OutputFormatter {
    /// 是否启用彩色输出
    use_color: bool,
}

impl OutputFormatter {
    /// 创建新的输出格式化器实例
    ///
    /// 自动检测当前输出是否为终端，决定是否启用彩色。
    pub fn new() -> Self {
        let use_color = std::io::stdout().is_terminal();
        OutputFormatter { use_color }
    }
}

impl Default for OutputFormatter {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputFormatter {
    /// 创建强制启用/禁用颜色的格式化器
    #[allow(dead_code)]
    pub fn with_color(use_color: bool) -> Self {
        OutputFormatter { use_color }
    }

    /// 格式化差异行列表为字符串输出
    ///
    /// 每行格式为: `[前缀] [行号]: [内容]`
    /// 新增行绿色 `+`，删除行红色 `-`，未变更行灰色无前缀。
    /// 分隔符行显示为灰色的 "  ~~~~~~~~"。
    pub fn format_diff_lines(&self, lines: &[DiffLine]) -> String {
        let mut output = String::new();

        for line in lines {
            match line.kind {
                DiffLineKind::Added => {
                    let prefix = if self.use_color {
                        "+".green().to_string()
                    } else {
                        "+".to_string()
                    };
                    let content = if self.use_color {
                        line.content.green().to_string()
                    } else {
                        line.content.clone()
                    };
                    if let Some(line_num) = line.line_number {
                        output.push_str(&format!("{} L{}: {}\n", prefix, line_num, content));
                    } else {
                        output.push_str(&format!("{} {}\n", prefix, content));
                    }
                }
                DiffLineKind::Deleted => {
                    let prefix = if self.use_color {
                        "-".red().to_string()
                    } else {
                        "-".to_string()
                    };
                    let content = if self.use_color {
                        line.content.red().to_string()
                    } else {
                        line.content.clone()
                    };
                    if let Some(line_num) = line.line_number {
                        output.push_str(&format!("{} L{}: {}\n", prefix, line_num, content));
                    } else {
                        output.push_str(&format!("{} {}\n", prefix, content));
                    }
                }
                DiffLineKind::Unchanged => {
                    let content = if self.use_color {
                        line.content.dimmed().to_string()
                    } else {
                        line.content.clone()
                    };
                    if let Some(line_num) = line.line_number {
                        output.push_str(&format!("  L{}: {}\n", line_num, content));
                    } else {
                        output.push_str(&format!("  {}\n", content));
                    }
                }
                DiffLineKind::Separator => {
                    let sep = if self.use_color {
                        "  ~~~~~~~~".dimmed().to_string()
                    } else {
                        "  ~~~~~~~~".to_string()
                    };
                    output.push_str(&format!("{}\n", sep));
                }
            }
        }

        output
    }

    /// 格式化 ContentBlock 为带行号的纯文本输出（无颜色、无差异标记）
    #[allow(dead_code)]
    pub fn format_block(&self, block: &crate::model::ContentBlock) -> String {
        let lines: Vec<DiffLine> = block
            .lines
            .iter()
            .map(|line| DiffLine {
                kind: DiffLineKind::Unchanged,
                line_number: Some(line.line_num),
                content: line.content.clone(),
            })
            .collect();
        self.format_diff_lines(&lines)
    }
}

/// 格式化带颜色的错误输出
///
/// 错误标题 "Error:" 使用红色加粗，错误描述使用黄色。
/// "Hint:" 提示使用绿色加粗，提示内容使用白色/默认色。
pub fn format_error_colored(title: &str, detail: &str, hints: &[&str]) -> String {
    let use_color = std::io::stdout().is_terminal();
    format_error_impl(title, detail, hints, use_color)
}

/// 格式化错误输出（可指定是否使用颜色）
pub fn format_error_with_color(
    title: &str,
    detail: &str,
    hints: &[&str],
    use_color: bool,
) -> String {
    format_error_impl(title, detail, hints, use_color)
}

fn format_error_impl(title: &str, detail: &str, hints: &[&str], use_color: bool) -> String {
    let mut output = String::new();

    if use_color {
        output.push_str(&format!("{} {}\n", "Error:".red().bold(), title.yellow()));
    } else {
        output.push_str(&format!("Error: {}\n", title));
    }

    if !detail.is_empty() {
        for line in detail.lines() {
            if use_color {
                output.push_str(&format!("  {}\n", line.dimmed()));
            } else {
                output.push_str(&format!("  {}\n", line));
            }
        }
    }

    for hint in hints {
        if use_color {
            output.push_str(&format!("  {} {}\n", "Hint:".green().bold(), hint.white()));
        } else {
            output.push_str(&format!("  Hint: {}\n", hint));
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LineNumber;

    #[test]
    fn test_output_formatter_no_color_creates_correct_prefixes() {
        let formatter = OutputFormatter::with_color(false);
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Added,
                line_number: Some(LineNumber::new(3)),
                content: "let x = 1;".to_string(),
            },
            DiffLine {
                kind: DiffLineKind::Deleted,
                line_number: Some(LineNumber::new(4)),
                content: "old_code();".to_string(),
            },
            DiffLine {
                kind: DiffLineKind::Unchanged,
                line_number: Some(LineNumber::new(5)),
                content: "fn main() {".to_string(),
            },
        ];
        let output = formatter.format_diff_lines(&lines);
        assert!(output.contains("+ L3: let x = 1;"));
        assert!(output.contains("- L4: old_code();"));
        assert!(output.contains("  L5: fn main() {"));
    }

    #[test]
    fn test_output_formatter_no_line_number() {
        let formatter = OutputFormatter::with_color(false);
        let lines = vec![
            DiffLine {
                kind: DiffLineKind::Added,
                line_number: None,
                content: "new line".to_string(),
            },
            DiffLine {
                kind: DiffLineKind::Deleted,
                line_number: None,
                content: "deleted line".to_string(),
            },
        ];
        let output = formatter.format_diff_lines(&lines);
        assert!(output.contains("+ new line"));
        assert!(output.contains("- deleted line"));
    }

    #[test]
    fn test_output_formatter_format_block() {
        use crate::model::{ContentBlock, Line, MatchInfo};
        let block = ContentBlock {
            start_line: LineNumber::new(10),
            end_line: LineNumber::new(11),
            first_line_index: std::collections::HashMap::new(),
            match_info: MatchInfo::Location {
                matched_line_count: 1,
            },
            lines: vec![
                Line {
                    line_num: LineNumber::new(10),
                    taps: 0,
                    diff_taps: 0,
                    content: "fn foo() {".to_string(),
                    stripped_content: crate::model::stripped_content("fn foo() {"),
                },
                Line {
                    line_num: LineNumber::new(11),
                    taps: 4,
                    diff_taps: 4,
                    content: "    bar();".to_string(),
                    stripped_content: crate::model::stripped_content("    bar();"),
                },
            ],
        };
        let formatter = OutputFormatter::with_color(false);
        let output = formatter.format_block(&block);
        assert!(output.contains("fn foo() {"));
        assert!(output.contains("bar();"));
    }

    #[test]
    fn test_output_formatter_empty_lines() {
        let formatter = OutputFormatter::with_color(false);
        let output = formatter.format_diff_lines(&[]);
        assert_eq!(output, "");
    }

    #[test]
    fn test_separator_formatting_no_color() {
        let formatter = OutputFormatter::with_color(false);
        let lines = vec![DiffLine::separator()];
        let output = formatter.format_diff_lines(&lines);
        assert!(output.contains("~~~~~~~~"));
    }

    #[test]
    fn test_unchanged_lines_with_separator() {
        let formatter = OutputFormatter::with_color(false);
        let lines = vec![
            DiffLine::unchanged(LineNumber::new(10), "fn foo() {".to_string()),
            DiffLine::added(LineNumber::new(11), "    let x = 1;".to_string()),
            DiffLine::unchanged(LineNumber::new(12), "}".to_string()),
            DiffLine::separator(),
            DiffLine::deleted(LineNumber::new(20), "    old();".to_string()),
        ];
        let output = formatter.format_diff_lines(&lines);
        assert!(output.contains("  L10: fn foo() {"));
        assert!(output.contains("+ L11:     let x = 1;"));
        assert!(output.contains("  L12: }"));
        assert!(output.contains("~~~~~~~~"));
        assert!(output.contains("- L20:     old();"));
    }

    #[test]
    fn test_diff_line_constructors() {
        let sep = DiffLine::separator();
        assert_eq!(sep.kind, DiffLineKind::Separator);

        let uc = DiffLine::unchanged(LineNumber::new(5), "ctx".to_string());
        assert_eq!(uc.kind, DiffLineKind::Unchanged);
        assert_eq!(uc.line_number, Some(LineNumber::new(5)));

        let ad = DiffLine::added(LineNumber::new(3), "new".to_string());
        assert_eq!(ad.kind, DiffLineKind::Added);

        let dl = DiffLine::deleted(LineNumber::new(7), "del".to_string());
        assert_eq!(dl.kind, DiffLineKind::Deleted);
    }

    #[test]
    fn test_format_error_no_color() {
        let output = format_error_with_color(
            "Location 匹配失败",
            "  fn main() {\n      let x = 1;\n  ...",
            &["请检查定位内容，或使用行号定位: //!@Location:@行号"],
            false,
        );
        assert!(output.contains("Error: Location 匹配失败"));
        assert!(output.contains("fn main()"));
        assert!(output.contains("Hint: 请检查定位内容"));
    }

    #[test]
    fn test_format_error_with_color() {
        let output = format_error_with_color("Location 匹配失败", "", &["提示内容"], true);
        assert!(output.contains("Error:"));
        assert!(output.contains("Hint:"));
        assert!(output.contains("提示内容"));
    }

    #[test]
    fn test_format_error_empty_hints() {
        let output = format_error_with_color("简单错误", "", &[], false);
        assert!(output.contains("Error: 简单错误"));
        assert!(!output.contains("Hint:"));
    }
}
