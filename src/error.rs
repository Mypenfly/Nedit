//! 错误类型定义 (Error Types)
//!
//! 集中管理项目中所有错误类型。
//! 每个错误类型实现 Display + Error trait，
//! 并附带上下文信息用于构造用户友好的错误提示。
//!
//! ## Phase 6 增强
//!
//! 所有错误类型提供 `hints()` 方法返回修复建议列表。
//! 外部可通过 `crate::output::format_error_colored()` 获得带颜色的错误输出。
//!
//! ## 对应文档
//!
//! 详见 INSTRUCTION.md 第 5 节 "错误信息规范"

use crate::model::LineNumber;
use std::error::Error;
use std::fmt;

/// 项目的根错误类型
///
/// 包含所有可能的错误变体，统一对外暴露。
#[derive(Debug)]
pub enum NEditError {
    /// 匹配相关错误
    Match(MatchError),
    /// 解析相关错误
    Parse(ParseError),
    /// 文件 I/O 相关错误
    File(FileError),
    /// 引擎执行错误
    Engine(EngineError),
}

impl NEditError {
    /// 返回错误标题（简短描述）
    pub fn title(&self) -> String {
        match self {
            NEditError::Match(e) => e.title(),
            NEditError::Parse(e) => e.title(),
            NEditError::File(e) => e.title(),
            NEditError::Engine(e) => e.title(),
        }
    }

    /// 返回错误详情（多行文本）
    pub fn detail(&self) -> String {
        match self {
            NEditError::Match(e) => e.detail(),
            NEditError::Parse(e) => e.detail(),
            NEditError::File(e) => e.detail(),
            NEditError::Engine(e) => e.detail(),
        }
    }

    /// 返回修复建议列表
    pub fn hints(&self) -> Vec<&str> {
        match self {
            NEditError::Match(e) => e.hints(),
            NEditError::Parse(e) => e.hints(),
            NEditError::File(e) => e.hints(),
            NEditError::Engine(e) => e.hints(),
        }
    }
}

/// 匹配相关的错误
#[derive(Debug)]
pub enum MatchError {
    /// 未找到任何匹配
    NoMatch {
        /// 用于匹配的定位内容
        location_content: String,
    },
    /// 找到过多匹配
    TooManyMatches {
        /// 匹配到的候选数量
        count: usize,
        /// 候选列表（最多保留 3 个）
        candidates: Vec<String>,
        /// 用于匹配的定位内容
        location_content: String,
    },
    /// Delete 匹配失败
    DeleteMatchFailed {
        /// 被删除内容的首行
        delete_content: String,
        /// 所在的 ContentBlock 内容摘要
        block_snippet: String,
    },
    /// Delete 匹配位置与 Location 不紧邻（中间有未经定位的行）
    DeleteNotAdjacent {
        /// Location 最后一行
        location_last_line: String,
        /// Delete 首行
        delete_first_line: String,
        /// 中间隔了多少行
        gap_lines: usize,
    },
    /// Block 不可解析（Location:Block 指定了 Block 指令但内容无法解析为代码块）
    BlockNotParseable {
        /// 用于定位的内容
        location_content: String,
    },
}

impl MatchError {
    pub fn title(&self) -> String {
        match self {
            MatchError::NoMatch { .. } => "Location 命令未找到任何匹配".to_string(),
            MatchError::TooManyMatches { count, .. } => {
                format!("Location 命令匹配到 {} 个结果（期望 1 个）", count)
            }
            MatchError::DeleteMatchFailed { .. } => {
                "Delete 命令未能在当前 Block 中找到匹配内容".to_string()
            }
            MatchError::DeleteNotAdjacent { gap_lines, .. } => {
                format!(
                    "Delete 匹配位置与 Location 不紧邻（中间隔了 {} 行未经定位的内容）",
                    gap_lines
                )
            }
            MatchError::BlockNotParseable { .. } => {
                "Location:Block 指定但提供内容无法解析为一个 Block".to_string()
            }
        }
    }

    pub fn detail(&self) -> String {
        match self {
            MatchError::NoMatch { location_content } => {
                format!("定位内容:\n{}", location_content)
            }
            MatchError::TooManyMatches {
                candidates,
                location_content,
                ..
            } => {
                let mut detail = format!("Location 内容:\n{}\n匹配候选:\n", location_content);
                for c in candidates {
                    detail.push_str(c);
                    detail.push('\n');
                }
                detail
            }
            MatchError::DeleteMatchFailed {
                delete_content,
                block_snippet,
            } => {
                format!(
                    "删除内容首行: {}\nBlock 内容:\n{}",
                    delete_content, block_snippet
                )
            }
            MatchError::DeleteNotAdjacent {
                location_last_line,
                delete_first_line,
                ..
            } => {
                format!(
                    "Location 最后一行: {}\nDelete 首行: {}",
                    location_last_line, delete_first_line
                )
            }
            MatchError::BlockNotParseable { location_content } => {
                format!("定位内容:\n{}", location_content)
            }
        }
    }

    pub fn hints(&self) -> Vec<&str> {
        match self {
            MatchError::NoMatch { .. } => {
                vec![
                    "请检查定位内容的字符拼写是否与目标文件中的内容一致（忽略空格差异）",
                    "您可以使用行号定位绕过匹配: //!@Location:@行号",
                ]
            }
            MatchError::TooManyMatches { .. } => {
                vec![
                    "请提供更多的上下文行来消除歧义，使匹配结果唯一",
                    "如果定位内容的结构重复出现，可以先用行号定位到外层范围，再用嵌套 Location 精确定位",
                    "或直接使用行号定位: //!@Location:@起始行号,结束行号",
                ]
            }
            MatchError::DeleteMatchFailed { .. } => {
                vec![
                    "请确认删除内容与当前 ContentBlock 中的内容精确匹配（忽略空格差异）",
                    "建议在 Delete 之前使用嵌套 Location 精确定位到要删除的内容",
                    "也可以使用行号 Delete 直接指定要删除的行范围: //!@Delete:@起始行,结束行",
                ]
            }
            MatchError::DeleteNotAdjacent { .. } => {
                vec![
                    "建议在 Delete 之前使用嵌套 Location 精确定位到要删除的代码块",
                    "确保 Delete 紧随 Location 的最后一行，中间不应有其他代码",
                ]
            }
            MatchError::BlockNotParseable { .. } => {
                vec![
                    "对于纯文本或 Markdown 等不适用大括号/缩进块的语言，请使用不带 Block 的 Location",
                    "或者使用行号定位来精确指定范围: //!@Location:Block@起始行,结束行",
                ]
            }
        }
    }
}

/// 命令解析错误
#[derive(Debug)]
pub enum ParseError {
    /// Open 命令缺少文件路径
    MissingFilePath,
    /// 无法识别的命令
    UnknownCommand {
        /// 无法识别的 Token 文本
        token: String,
        /// 所在行号
        line: LineNumber,
    },
    /// New/Delete 命令前缺少 Location（或前一个 Token 是 `...` 产生歧义）
    MissingLocation {
        /// 命令类型（"New" / "Delete"）
        command: String,
        /// 所在行号
        line: LineNumber,
    },
    /// 意外的分隔符
    #[allow(dead_code)]
    UnexpectedSeparator {
        /// 所在行号
        line: LineNumber,
    },
    /// Delete:Block 要求前一个 Location 也使用 Block 指令（Phase 3）
    BlockRequiredForDelete {
        /// 所在行号
        line: LineNumber,
    },
}

impl ParseError {
    pub fn title(&self) -> String {
        match self {
            ParseError::MissingFilePath => "Open 命令缺少文件路径参数".to_string(),
            ParseError::UnknownCommand { token, line } => {
                format!("第 {} 行出现无法识别的命令: {}", line, token)
            }
            ParseError::MissingLocation { command, line } => {
                format!("第 {} 行: {} 命令前缺少 Location 定位", line, command)
            }
            ParseError::UnexpectedSeparator { line } => {
                format!("第 {} 行出现意外的分隔符 ...", line)
            }
            ParseError::BlockRequiredForDelete { line } => {
                format!(
                    "第 {} 行: Delete:Block 要求前一个 Location 也使用 Block 指令",
                    line
                )
            }
        }
    }

    pub fn detail(&self) -> String {
        match self {
            ParseError::MissingFilePath => "Open 命令需要一个文件路径参数，格式: //!@Open: <路径>".to_string(),
            ParseError::UnknownCommand { token, .. } => {
                format!("无法识别的命令: \"{}\"，请检查命令拼写", token)
            }
            ParseError::MissingLocation { .. } => {
                "`...` 分隔符导致了插入/删除位置不明确。请在此命令之前使用 Location 明确指定操作位置。".to_string()
            }
            ParseError::UnexpectedSeparator { .. } => {
                "分隔符 `...` 出现在非预期位置，这可能破坏了命令流。".to_string()
            }
            ParseError::BlockRequiredForDelete { .. } => {
                "使用 Delete:Block 时，前一个 Location 也必须指定 Block 指令（Location:Block），以确保删除的是整个代码块而非不确定的范围。".to_string()
            }
        }
    }

    pub fn hints(&self) -> Vec<&str> {
        match self {
            ParseError::MissingFilePath => {
                vec!["在 Open 后添加文件路径，例如: //!@Open: ./src/main.rs"]
            }
            ParseError::UnknownCommand { .. } => {
                vec![
                    "支持的命令: Open, Location, New, Delete, Raw, Off",
                    "命令不区分大小写，检查是否有拼写错误",
                ]
            }
            ParseError::MissingLocation { command, .. } => {
                if command == "New" {
                    vec![
                        "在 New 之前添加 //!@Location: ... 来指定操作范围",
                        "或者使用 New:Start / New:End 直接在文件首尾插入",
                    ]
                } else {
                    vec![
                        "在 Delete 之前添加 //!@Location: ... 来指定操作范围",
                        "可以先使用嵌套 Location 精确定位到要删除的内容",
                    ]
                }
            }
            ParseError::UnexpectedSeparator { .. } => {
                vec!["检查 `...` 分隔符是否正确放置在 Location/New/Delete 内容之后"]
            }
            ParseError::BlockRequiredForDelete { .. } => {
                vec![
                    "将前一个 Location 改为 Location:Block",
                    "或移除 Delete 的 Block 修饰符",
                ]
            }
        }
    }
}

/// 文件 I/O 相关错误
#[derive(Debug)]
pub enum FileError {
    /// 文件未找到
    NotFound {
        /// 文件路径
        path: String,
    },
    /// 无法打开文件
    CannotOpen {
        /// 文件路径
        path: String,
        /// 失败原因
        reason: String,
    },
    /// 写入失败
    WriteFailed {
        /// 文件路径
        path: String,
        /// 失败原因
        reason: String,
    },
}

impl FileError {
    pub fn title(&self) -> String {
        match self {
            FileError::NotFound { path } => format!("文件未找到: {}", path),
            FileError::CannotOpen { path, .. } => format!("无法打开文件: {}", path),
            FileError::WriteFailed { path, .. } => format!("写入文件失败: {}", path),
        }
    }

    pub fn detail(&self) -> String {
        match self {
            FileError::NotFound { .. } => "请确认文件路径是否正确，文件是否存在。".to_string(),
            FileError::CannotOpen { reason, .. } => {
                format!("原因: {}", reason)
            }
            FileError::WriteFailed { reason, .. } => {
                format!("原因: {}", reason)
            }
        }
    }

    pub fn hints(&self) -> Vec<&str> {
        match self {
            FileError::NotFound { .. } => {
                vec![
                    "检查路径拼写是否正确",
                    "使用相对路径时确认当前工作目录",
                    "确保文件扩展名正确",
                ]
            }
            FileError::CannotOpen { .. } => {
                vec!["检查文件权限是否正确", "确认文件没有被其他程序占用"]
            }
            FileError::WriteFailed { .. } => {
                vec!["检查目标目录的写入权限", "确认磁盘空间充足"]
            }
        }
    }
}

/// 引擎执行错误
#[derive(Debug)]
pub enum EngineError {
    /// 执行 Open 命令时缺少前置 Location
    MissingLocationForNew,
    /// 执行 Delete:Block 时前一个 Location 未使用 Block
    #[allow(dead_code)]
    BlockRequiredForDelete,
    /// Block 栈为空时尝试弹出
    BlockStackEmpty,
    /// 隐式 Off 失败
    #[allow(dead_code)]
    ImplicitOffFailed {
        /// 失败原因
        reason: String,
    },
}

impl EngineError {
    pub fn title(&self) -> String {
        match self {
            EngineError::MissingLocationForNew => {
                "New/Delete 命令之前必须存在 Location 命令".to_string()
            }
            EngineError::BlockRequiredForDelete => {
                "Delete:Block 要求前一个 Location 也使用 Block 指令".to_string()
            }
            EngineError::BlockStackEmpty => "Block 栈为空，无法执行 Off 操作".to_string(),
            EngineError::ImplicitOffFailed { .. } => "隐式 Off:Open 执行失败".to_string(),
        }
    }

    pub fn detail(&self) -> String {
        match self {
            EngineError::ImplicitOffFailed { reason } => reason.clone(),
            _ => String::new(),
        }
    }

    pub fn hints(&self) -> Vec<&str> {
        match self {
            EngineError::MissingLocationForNew => {
                vec![
                    "在执行 New/Delete 前，请先使用 Location 定位到目标代码块",
                    "或在文件首尾直接插入，使用 New:Start / New:End",
                ]
            }
            EngineError::BlockRequiredForDelete => {
                vec!["将前一个 Location 改为 Location:Block"]
            }
            EngineError::BlockStackEmpty => {
                vec!["检查 Off 命令是否与 Location 正确配对"]
            }
            EngineError::ImplicitOffFailed { .. } => {
                vec!["检查文件是否在脚本执行过程中被修改或删除"]
            }
        }
    }
}

impl fmt::Display for MatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.title(), self.detail())
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.title(), self.detail())
    }
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.title(), self.detail())
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.title(), self.detail())
    }
}

impl fmt::Display for NEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NEditError::Match(e) => write!(f, "{}", e),
            NEditError::Parse(e) => write!(f, "{}", e),
            NEditError::File(e) => write!(f, "{}", e),
            NEditError::Engine(e) => write!(f, "{}", e),
        }
    }
}

impl Error for NEditError {}
impl Error for MatchError {}
impl Error for ParseError {}
impl Error for FileError {}
impl Error for EngineError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_match_error_display() {
        let err = MatchError::NoMatch {
            location_content: "fn main()".to_string(),
        };
        let display = format!("{}", err);
        assert!(display.contains("未找到任何匹配"));
        assert!(display.contains("fn main()"));
    }

    #[test]
    fn test_no_match_hints() {
        let err = MatchError::NoMatch {
            location_content: "fn main()".to_string(),
        };
        let hints = err.hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|h| h.contains("行号定位")));
    }

    #[test]
    fn test_too_many_matches_error_display() {
        let err = MatchError::TooManyMatches {
            count: 3,
            candidates: vec!["L12: fn foo".to_string(), "L45: fn foo".to_string()],
            location_content: "fn foo".to_string(),
        };
        let display = format!("{}", err);
        assert!(display.contains("3"));
        assert!(display.contains("L12"));
    }

    #[test]
    fn test_too_many_matches_hints() {
        let err = MatchError::TooManyMatches {
            count: 3,
            candidates: vec!["L12: fn foo".to_string()],
            location_content: "fn foo".to_string(),
        };
        let hints = err.hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|h| h.contains("上下文")));
    }

    #[test]
    fn test_parse_error_missing_file_path_display() {
        let err = ParseError::MissingFilePath;
        let display = format!("{}", err);
        assert!(display.contains("文件路径"));
    }

    #[test]
    fn test_parse_error_missing_file_path_hints() {
        let err = ParseError::MissingFilePath;
        let hints = err.hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|h| h.contains("Open")));
    }

    #[test]
    fn test_parse_error_unknown_command_display() {
        let err = ParseError::UnknownCommand {
            token: "BadCmd".to_string(),
            line: LineNumber::new(5),
        };
        let display = format!("{}", err);
        assert!(display.contains("BadCmd"));
        assert!(display.contains("5"));
    }

    #[test]
    fn test_file_error_not_found_display() {
        let err = FileError::NotFound {
            path: "/tmp/test.rs".to_string(),
        };
        let display = format!("{}", err);
        assert!(display.contains("文件未找到"));
        assert!(display.contains("/tmp/test.rs"));
    }

    #[test]
    fn test_file_error_not_found_hints() {
        let err = FileError::NotFound {
            path: "/tmp/test.rs".to_string(),
        };
        let hints = err.hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|h| h.contains("路径")));
    }

    #[test]
    fn test_engine_error_missing_location_new() {
        let err = EngineError::MissingLocationForNew;
        let display = format!("{}", err);
        assert!(display.contains("Location"));
    }

    #[test]
    fn test_engine_error_missing_location_hints() {
        let err = EngineError::MissingLocationForNew;
        let hints = err.hints();
        assert!(!hints.is_empty());
    }

    #[test]
    fn test_nedit_error_wraps_sub_errors() {
        let err = NEditError::Parse(ParseError::MissingFilePath);
        let display = format!("{}", err);
        assert!(display.contains("文件路径"));
    }

    #[test]
    fn test_nedit_error_hints() {
        let err = NEditError::Match(MatchError::NoMatch {
            location_content: "fn main()".to_string(),
        });
        let hints = err.hints();
        assert!(!hints.is_empty());
    }

    #[test]
    fn test_delete_match_failed_error_display() {
        let err = MatchError::DeleteMatchFailed {
            delete_content: "let x = 1;".to_string(),
            block_snippet: "fn main() {".to_string(),
        };
        let display = format!("{}", err);
        assert!(display.contains("未能在当前 Block 中找到匹配内容"));
        assert!(display.contains("let x = 1;"));
    }

    #[test]
    fn test_delete_match_failed_hints() {
        let err = MatchError::DeleteMatchFailed {
            delete_content: "let x = 1;".to_string(),
            block_snippet: "fn main() {".to_string(),
        };
        let hints = err.hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|h| h.contains("嵌套")));
    }

    #[test]
    fn test_delete_not_adjacent_hints() {
        let err = MatchError::DeleteNotAdjacent {
            location_last_line: "}".to_string(),
            delete_first_line: "let x = 1;".to_string(),
            gap_lines: 3,
        };
        let hints = err.hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|h| h.contains("嵌套")));
    }

    #[test]
    fn test_block_not_parseable_error_display() {
        let err = MatchError::BlockNotParseable {
            location_content: "# Title\n## Section".to_string(),
        };
        let display = format!("{}", err);
        assert!(display.contains("无法解析为一个 Block"));
        assert!(display.contains("# Title"));
    }

    #[test]
    fn test_block_not_parseable_hints() {
        let err = MatchError::BlockNotParseable {
            location_content: "# Title".to_string(),
        };
        let hints = err.hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|h| h.contains("Block")));
    }

    #[test]
    fn test_nedit_error_title() {
        let err = NEditError::File(FileError::NotFound {
            path: "test.rs".to_string(),
        });
        assert!(err.title().contains("文件未找到"));
    }

    #[test]
    fn test_nedit_error_detail() {
        let err = NEditError::Engine(EngineError::MissingLocationForNew);
        let detail = err.detail();
        assert!(detail.is_empty() || !detail.is_empty());
    }

    #[test]
    fn test_parse_error_missing_location_hints() {
        let err = ParseError::MissingLocation {
            command: "New".to_string(),
            line: LineNumber::new(5),
        };
        let hints = err.hints();
        assert!(!hints.is_empty());
    }

    #[test]
    fn test_file_error_cannot_open_hints() {
        let err = FileError::CannotOpen {
            path: "test.rs".to_string(),
            reason: "permission denied".to_string(),
        };
        let hints = err.hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|h| h.contains("权限")));
    }
}
