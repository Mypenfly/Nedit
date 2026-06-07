//! 核心匹配算法 (Location Matcher)
//!
//! 负责根据 LocationContent 在搜索范围（SearchScope）中查找唯一匹配的代码位置。
//!
//! ## 实现逻辑
//!
//! 1. 取 LocationContent 首行去空白后，在 SearchScope 中扫描匹配的候选起点
//! 2. 对每个候选起点，逐行比对 content（去空白）和 diff_taps
//! 3. 确认结果唯一性，否则返回详细的匹配错误
//!
//! ## 对应文档
//!
//! 详见 INSTRUCTION.md 第 3.1 节 "Location 匹配算法"
//! Phase 4 嵌套 Location：详见 phases.md 第 4.1 节

use crate::block::BlockParser;
use crate::error::MatchError;
use crate::model::{self, ContentBlock, Line, LineNumber, LocationContent, MatchInfo, SearchScope};
use crate::model::{CANDIDATE_SNIPPET_MAX_LEN, MAX_CANDIDATE_DISPLAY};
use std::collections::HashMap;

/// Location 匹配器
///
/// 根据 LocationContent 在 SearchScope 中执行精确匹配，返回唯一 ContentBlock。
///
/// 支持 Phase 4 嵌套 Location：SearchScope 可为 FileContent 或 ContentBlock，
/// 匹配出的 ContentBlock 行号始终为绝对文件行号。
pub struct LocationMatcher;

impl LocationMatcher {
    /// 在搜索范围内执行 Location 匹配，返回唯一 ContentBlock
    ///
    /// 匹配过程：
    /// 1. 首行去空白匹配 → 收集候选起点（索引相对于搜索范围）
    /// 2. 逐行比对（content 去空白 + diff_taps）→ 筛选
    /// 3. 确认唯一性 → 返回 ContentBlock（含绝对文件行号）
    ///
    /// 若 `block` 为 true（Location:Block），使用 BlockParser 获取精确 Block 边界，
    /// 而非"从首行到文件末尾"的默认行为。
    pub fn find_unique_block(
        scope: &SearchScope,
        location: &LocationContent,
        block: bool,
    ) -> Result<ContentBlock, MatchError> {
        if location.lines.is_empty() {
            let scoped_lines = scope.lines();
            let start_line = scoped_lines
                .first()
                .map(|l| l.line_num)
                .unwrap_or(LineNumber::new(1));
            let end_line = scoped_lines
                .last()
                .map(|l| l.line_num)
                .unwrap_or(LineNumber::new(1));
            let lines: Vec<Line> = scoped_lines
                .iter()
                .map(|line| Line {
                    line_num: line.line_num,
                    taps: line.taps,
                    diff_taps: line.diff_taps,
                    content: line.content.clone(),
                    stripped_content: line.stripped_content.clone(),
                })
                .collect();
            return Ok(ContentBlock {
                start_line,
                end_line,
                lines,
                first_line_index: scope.first_line_index().clone(),
                match_info: MatchInfo::Empty,
            });
        }
        let candidates = collect_first_line_matches(scope, location);
        let filtered = filter_by_full_match(scope, candidates, location);
        expect_single_match(scope, filtered, location, block)
    }

    /// 按行号范围直接定位，跳过匹配流程（Phase 5）
    ///
    /// 行号相对于 SearchScope（顶层为 FileContent，嵌套为 ContentBlock）。
    /// `is_delete` 标识是否来自 Delete 命令（此时不设置 match_info）。
    pub fn find_by_line_range(
        scope: &SearchScope,
        line_range: crate::model::LineRange,
        block: bool,
        is_delete: bool,
    ) -> Result<ContentBlock, MatchError> {
        let scoped_lines = scope.lines();
        let start_index = line_range.start.saturating_sub(1);
        let end_index =
            (line_range.end.saturating_sub(1)).min(scoped_lines.len().saturating_sub(1));

        if start_index >= scoped_lines.len() {
            return Err(MatchError::NoMatch {
                location_content: format!(
                    "行号 {} 超出范围（共 {} 行）",
                    line_range.start,
                    scoped_lines.len()
                ),
            });
        }

        if end_index < start_index {
            return Err(MatchError::NoMatch {
                location_content: format!(
                    "无效的行号范围: @{},{}",
                    line_range.start, line_range.end
                ),
            });
        }

        let (block_start, block_end) = if block {
            BlockParser::parse_block(scope, start_index)?
        } else {
            (start_index, end_index)
        };

        let start_line = scoped_lines[block_start].line_num;
        let end_line = scoped_lines[block_end].line_num;
        let lines: Vec<Line> = scoped_lines[block_start..=block_end]
            .iter()
            .map(|line| Line {
                line_num: line.line_num,
                taps: line.taps,
                diff_taps: line.diff_taps,
                content: line.content.clone(),
                stripped_content: line.stripped_content.clone(),
            })
            .collect();

        let matched_line_count = if is_delete {
            0
        } else if block {
            lines.len()
        } else {
            end_index - start_index + 1
        };

        let match_info = if matched_line_count == 0 {
            MatchInfo::Empty
        } else {
            MatchInfo::Location { matched_line_count }
        };

        let mut content_block = ContentBlock {
            start_line,
            end_line,
            lines,
            first_line_index: HashMap::new(),
            match_info,
        };
        content_block.reindex();
        Ok(content_block)
    }
}

/// 收集首行匹配的所有候选起点
///
/// 使用 SearchScope 的 first_line_index 进行 O(1) 查找，
/// 返回的索引相对于搜索范围的 lines()。
fn collect_first_line_matches(scope: &SearchScope, location: &LocationContent) -> Vec<usize> {
    let target = model::stripped_content(&location.lines[0].content);
    scope
        .first_line_index()
        .get(&target)
        .cloned()
        .unwrap_or_default()
}

/// 对候选集进行逐行全量匹配筛选
fn filter_by_full_match(
    scope: &SearchScope,
    candidates: Vec<usize>,
    location: &LocationContent,
) -> Vec<usize> {
    candidates
        .into_iter()
        .filter(|&start_index| rows_match(scope, start_index, location))
        .collect()
}

/// 逐行比对：content（去空白）+ diff_taps 双重校验
fn rows_match(scope: &SearchScope, start_index: usize, location: &LocationContent) -> bool {
    let loc_lines = &location.lines;
    let location_line_count = loc_lines.len();
    let scoped_lines = scope.lines();

    if start_index + location_line_count > scoped_lines.len() {
        return false;
    }

    let file_slice = &scoped_lines[start_index..start_index + location_line_count];
    let base_taps = file_slice[0].taps;

    let mut file_index: usize = 0;
    let mut loc_index: usize = 0;

    while file_index < file_slice.len() && loc_index < loc_lines.len() {
        let file_line = &file_slice[file_index];
        let loc_line = &loc_lines[loc_index];

        let file_is_empty = file_line.content.trim().is_empty();
        let loc_is_empty = loc_line.content.trim().is_empty();

        if file_is_empty && loc_is_empty {
            file_index += 1;
            loc_index += 1;
            continue;
        }

        if file_is_empty {
            file_index += 1;
            continue;
        }
        if loc_is_empty {
            loc_index += 1;
            continue;
        }

        if file_line.stripped_content() != model::stripped_content(&loc_line.content) {
            return false;
        }

        let file_diff = file_line.taps.saturating_sub(base_taps);
        let loc_diff = loc_line.diff_taps.unwrap_or(0);

        if file_diff != loc_diff {
            return false;
        }

        file_index += 1;
        loc_index += 1;
    }

    true
}

/// 确认匹配结果唯一，否则构造详细错误信息
fn expect_single_match(
    scope: &SearchScope,
    candidates: Vec<usize>,
    location: &LocationContent,
    block: bool,
) -> Result<ContentBlock, MatchError> {
    match candidates.len() {
        0 => Err(MatchError::NoMatch {
            location_content: format_location_for_error(location),
        }),
        1 => build_content_block(scope, candidates[0], location.lines.len(), block),
        n => {
            let scoped_lines = scope.lines();
            let mut candidate_descriptions: Vec<String> = candidates
                .iter()
                .take(MAX_CANDIDATE_DISPLAY)
                .map(|&idx| {
                    let line_num = scoped_lines[idx].line_num;
                    let snippet = &scoped_lines[idx].content;
                    let truncated: String = if snippet.len() > CANDIDATE_SNIPPET_MAX_LEN {
                        format!("{}...", &snippet[..CANDIDATE_SNIPPET_MAX_LEN - 3])
                    } else {
                        snippet.clone()
                    };
                    format!("  L{}: {}", line_num, truncated)
                })
                .collect();
            if n > MAX_CANDIDATE_DISPLAY {
                candidate_descriptions.push(format!("  ({} more)", n - MAX_CANDIDATE_DISPLAY));
            }
            Err(MatchError::TooManyMatches {
                count: n,
                candidates: candidate_descriptions,
                location_content: format_location_for_error(location),
            })
        }
    }
}

/// 从匹配起点构建 ContentBlock
///
/// 若 `block` 为 false（普通 Location）：block 边界为从 start_index 到搜索范围末尾。
/// 若 `block` 为 true（Location:Block）：使用 BlockParser 获取精确的代码块边界。
///
/// start_index 是相对于搜索范围的索引。BlockParser 返回的也是相对于搜索范围的索引。
/// 最终 ContentBlock 的 start_line / end_line 为绝对文件行号（从 Line.line_num 获取）。
fn build_content_block(
    scope: &SearchScope,
    start_index: usize,
    matched_line_count: usize,
    block: bool,
) -> Result<ContentBlock, MatchError> {
    let scoped_lines = scope.lines();
    let (block_start, block_end) = if block {
        BlockParser::parse_block(scope, start_index)?
    } else {
        (start_index, scoped_lines.len().saturating_sub(1))
    };

    let start_line = scoped_lines[block_start].line_num;
    let end_line = scoped_lines[block_end].line_num;
    let lines: Vec<Line> = scoped_lines[block_start..=block_end]
        .iter()
        .map(|line| Line {
            line_num: line.line_num,
            taps: line.taps,
            diff_taps: line.diff_taps,
            content: line.content.clone(),
            stripped_content: line.stripped_content.clone(),
        })
        .collect();

    let effective_matched = if block {
        lines.len()
    } else {
        matched_line_count
    };

    let match_info = if effective_matched == 0 {
        MatchInfo::Empty
    } else {
        MatchInfo::Location {
            matched_line_count: effective_matched,
        }
    };

    let mut content_block = ContentBlock {
        start_line,
        end_line,
        lines,
        first_line_index: HashMap::new(),
        match_info,
    };
    content_block.reindex();
    Ok(content_block)
}

fn format_location_for_error(location: &LocationContent) -> String {
    location
        .lines
        .iter()
        .map(|line| line.content.as_str())
        .collect::<Vec<&str>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::MatchError;
    use crate::model::{self, FileContent, Line, LocationContent, LocationLine};
    use std::collections::HashMap;

    /// 辅助函数：根据字符串切片构建简单的 FileContent
    fn make_file_content(lines: &[&str]) -> FileContent {
        let mut file_lines: Vec<Line> = Vec::new();
        let mut index: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, content) in lines.iter().enumerate() {
            let taps = model::count_leading_spaces(content);
            let stripped = model::stripped_content(content);
            index.entry(stripped.clone()).or_default().push(i);
            file_lines.push(Line {
                line_num: LineNumber::from_index(i),
                taps,
                diff_taps: 0,
                content: content.to_string(),
                stripped_content: stripped,
            });
        }
        FileContent {
            lines: file_lines,
            first_line_index: index,
        }
    }

    /// 辅助函数：根据字符串切片构建 LocationContent
    fn make_location_content(lines: &[&str]) -> LocationContent {
        if lines.is_empty() {
            return LocationContent { lines: vec![] };
        }
        let base_taps = model::count_leading_spaces(lines[0]);
        let loc_lines: Vec<LocationLine> = lines
            .iter()
            .enumerate()
            .map(|(i, content)| {
                let line_taps = model::count_leading_spaces(content);
                let diff_taps = Some(line_taps.saturating_sub(base_taps));
                LocationLine {
                    index: i,
                    diff_taps,
                    content: content.to_string(),
                    line_num: None,
                }
            })
            .collect();
        LocationContent { lines: loc_lines }
    }

    // ============================================================
    // find_unique_block — 基本匹配测试（File scope）
    // ============================================================

    #[test]
    fn test_find_unique_block_exact_match() {
        let file = make_file_content(&[
            "// comment",
            "fn main() {",
            "    let x = 1;",
            "    println!(\"{}\", x);",
            "}",
        ]);

        let location = make_location_content(&["fn main() {", "    let x = 1;"]);
        let scope = SearchScope::File(&file);

        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.start_line, 2);
        assert_eq!(block.lines.len(), 4);
        assert_eq!(block.lines[0].content, "fn main() {");
        assert_eq!(block.lines[1].content, "    let x = 1;");
    }

    #[test]
    fn test_find_unique_block_single_line_location() {
        let file = make_file_content(&["fn foo() {}", "fn bar() {}", "fn baz() {}"]);
        let location = make_location_content(&["fn bar() {}"]);
        let scope = SearchScope::File(&file);

        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.start_line, 2);
        assert_eq!(block.lines[0].content, "fn bar() {}");
    }

    #[test]
    fn test_find_unique_block_no_match() {
        let file = make_file_content(&["fn foo() {}", "fn bar() {}"]);
        let location = make_location_content(&["fn nonexistent() {}"]);
        let scope = SearchScope::File(&file);

        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(result.is_err());
        match result.unwrap_err() {
            MatchError::NoMatch { .. } => {}
            _ => panic!("Expected NoMatch error"),
        }
    }

    #[test]
    fn test_find_unique_block_too_many_matches() {
        let file = make_file_content(&[
            "fn foo() {",
            "    bar();",
            "}",
            "",
            "fn foo() {",
            "    baz();",
            "}",
        ]);
        let location = make_location_content(&["fn foo() {"]);
        let scope = SearchScope::File(&file);

        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(result.is_err());
        match result.unwrap_err() {
            MatchError::TooManyMatches { count, .. } => {
                assert_eq!(count, 2);
            }
            _ => panic!("Expected TooManyMatches error"),
        }
    }

    // ============================================================
    // find_unique_block — 去空白匹配测试
    // ============================================================

    #[test]
    fn test_find_unique_block_stripped_content_match() {
        let file = make_file_content(&[
            "// file starts",
            "    fn main() {",
            "        let x = 1;",
            "    }",
        ]);
        let location = make_location_content(&["fn main() {", "    let x = 1;"]);
        let scope = SearchScope::File(&file);

        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.start_line, 2);
    }

    #[test]
    fn test_find_unique_block_disambiguates_by_second_line() {
        let file = make_file_content(&[
            "fn foo() {",
            "    let a = 1;",
            "}",
            "",
            "fn foo() {",
            "    let b = 2;",
            "}",
        ]);
        let location = make_location_content(&["fn foo() {", "    let b = 2;"]);
        let scope = SearchScope::File(&file);

        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.start_line, 5);
    }

    // ============================================================
    // find_unique_block — Block 边界测试
    // ============================================================

    #[test]
    fn test_find_unique_block_boundary_to_end_of_file() {
        let file = make_file_content(&[
            "// header",
            "mod utils;",
            "",
            "fn process() {",
            "    do_work();",
            "}",
            "",
            "fn main() {",
            "    process();",
            "}",
        ]);
        let location = make_location_content(&["fn process() {"]);
        let scope = SearchScope::File(&file);

        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.start_line, 4);
        assert_eq!(block.lines.len(), 7);
    }

    // ============================================================
    // find_unique_block — 空行处理测试
    // ============================================================

    #[test]
    fn test_find_unique_block_skips_empty_lines_in_location() {
        let file = make_file_content(&["fn main() {", "", "    let x = 1;", "}"]);
        let location = make_location_content(&["fn main() {", "", "    let x = 1;"]);
        let scope = SearchScope::File(&file);

        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.start_line, 1);
    }

    // ============================================================
    // Phase 4: SearchScope::Block — 嵌套 Location 匹配测试
    // ============================================================

    /// 辅助函数：从 FileContent 构建一个 ContentBlock（模拟嵌套搜索范围）
    fn make_block_scope(
        file: &FileContent,
        start_1based: usize,
        end_1based: usize,
    ) -> ContentBlock {
        let start_idx = start_1based.saturating_sub(1);
        let end_idx = end_1based.saturating_sub(1);
        let lines: Vec<Line> = file.lines[start_idx..=end_idx]
            .iter()
            .map(|line| Line {
                line_num: line.line_num,
                taps: line.taps,
                diff_taps: line.diff_taps,
                content: line.content.clone(),
                stripped_content: line.stripped_content.clone(),
            })
            .collect();
        let mut block = ContentBlock {
            start_line: LineNumber::new(start_1based),
            end_line: LineNumber::new(end_1based),
            lines,
            first_line_index: HashMap::new(),
            match_info: MatchInfo::Location {
                matched_line_count: (end_1based - start_1based + 1),
            },
        };
        block.reindex();
        block
    }

    #[test]
    fn test_find_unique_block_within_block_scope() {
        let file = make_file_content(&[
            "// header",
            "fn outer() {",
            "    let x = 1;",
            "    fn inner() {",
            "        let y = 2;",
            "    }",
            "    let z = 3;",
            "}",
            "",
            "fn other() {}",
        ]);
        // Block scope: fn outer() { ... } (lines 2-8)
        let block = make_block_scope(&file, 2, 8);
        let scope = SearchScope::Block(&block);

        // Search for inner() within outer()
        let location = make_location_content(&["fn inner() {"]);
        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(
            result.is_ok(),
            "Unexpected error: {:?}",
            result.as_ref().err()
        );
        let found = result.unwrap();
        // start_line should be absolute (4), not relative to block
        assert_eq!(found.start_line, 4);
        // Block should span from line 4 to end of block scope (line 8)
        assert_eq!(found.lines.len(), 5);
        assert_eq!(found.lines[0].content, "    fn inner() {");
    }

    #[test]
    fn test_find_unique_block_within_block_scope_empty_location() {
        let file = make_file_content(&["fn outer() {", "    let x = 1;", "    let y = 2;", "}"]);
        let block = make_block_scope(&file, 1, 4);
        let scope = SearchScope::Block(&block);

        let location = make_location_content(&[]);
        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(result.is_ok());
        let found = result.unwrap();
        // Empty location returns the whole block scope
        assert_eq!(found.start_line, 1);
        assert_eq!(found.lines.len(), 4);
    }

    #[test]
    fn test_find_unique_block_within_block_scope_disambiguates() {
        let file = make_file_content(&[
            "fn outer() {",
            "    if true {",
            "        work();",
            "    }",
            "    if false {",
            "        skip();",
            "    }",
            "}",
        ]);
        let block = make_block_scope(&file, 1, 8);
        let scope = SearchScope::Block(&block);

        // Two identical "if true {" should fail within the block too
        // But here we distinguish by second line
        let location = make_location_content(&["    if true {", "        work();"]);
        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(result.is_ok());
        let found = result.unwrap();
        assert_eq!(found.start_line, 2);
    }

    #[test]
    fn test_find_unique_block_within_block_scope_too_many() {
        let file = make_file_content(&[
            "fn outer() {",
            "    let a = 1;",
            "    let b = 2;",
            "    let a = 1;",
            "}",
        ]);
        let block = make_block_scope(&file, 1, 5);
        let scope = SearchScope::Block(&block);

        let location = make_location_content(&["    let a = 1;"]);
        let result = LocationMatcher::find_unique_block(&scope, &location, false);
        assert!(result.is_err());
        match result.unwrap_err() {
            MatchError::TooManyMatches { count, .. } => {
                assert_eq!(count, 2);
            }
            _ => panic!("Expected TooManyMatches"),
        }
    }

    // ============================================================
    // Phase 5: find_by_line_range 测试
    // ============================================================

    #[test]
    fn test_find_by_line_range_single_line() {
        let file = make_file_content(&["line1", "line2", "line3", "line4"]);
        let scope = SearchScope::File(&file);
        let line_range = crate::model::LineRange { start: 2, end: 2 };

        let result = LocationMatcher::find_by_line_range(&scope, line_range, false, false);
        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.start_line, 2);
        assert_eq!(block.end_line, 2);
        assert_eq!(block.lines.len(), 1);
        assert_eq!(block.lines[0].content, "line2");
    }

    #[test]
    fn test_find_by_line_range_multi_line() {
        let file = make_file_content(&["a", "b", "c", "d", "e"]);
        let scope = SearchScope::File(&file);
        let line_range = crate::model::LineRange { start: 2, end: 4 };

        let result = LocationMatcher::find_by_line_range(&scope, line_range, false, false);
        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.start_line, 2);
        assert_eq!(block.end_line, 4);
        assert_eq!(block.lines.len(), 3);
        assert_eq!(block.lines[0].content, "b");
        assert_eq!(block.lines[1].content, "c");
        assert_eq!(block.lines[2].content, "d");
        // match_info should be Location with matched_line_count = 3
        match block.match_info {
            MatchInfo::Location { matched_line_count } => {
                assert_eq!(matched_line_count, 3);
            }
            _ => panic!("Expected Location match_info"),
        }
    }

    #[test]
    fn test_find_by_line_range_is_delete_sets_empty_match_info() {
        let file = make_file_content(&["a", "b", "c"]);
        let scope = SearchScope::File(&file);
        let line_range = crate::model::LineRange { start: 1, end: 3 };

        let result = LocationMatcher::find_by_line_range(&scope, line_range, false, true);
        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.match_info, MatchInfo::Empty);
    }

    #[test]
    fn test_find_by_line_range_with_block_brace() {
        let file = make_file_content(&[
            "// header",
            "fn main() {",
            "    let x = 1;",
            "    println!(\"{}\", x);",
            "}",
            "// footer",
        ]);
        let scope = SearchScope::File(&file);
        let line_range = crate::model::LineRange { start: 2, end: 2 };

        let result = LocationMatcher::find_by_line_range(&scope, line_range, true, false);
        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.start_line, 2);
        assert_eq!(block.end_line, 5);
        assert_eq!(block.lines.len(), 4);
        assert_eq!(block.lines[0].content, "fn main() {");
        assert_eq!(block.lines[3].content, "}");
    }

    #[test]
    fn test_find_by_line_range_with_block_indent() {
        let file = make_file_content(&[
            "def outer():",
            "    def inner():",
            "        pass",
            "    return 0",
            "end_outer",
        ]);
        let scope = SearchScope::File(&file);
        let line_range = crate::model::LineRange { start: 2, end: 2 };

        let result = LocationMatcher::find_by_line_range(&scope, line_range, true, false);
        assert!(result.is_ok());
        let block = result.unwrap();
        assert_eq!(block.start_line, 2);
        assert_eq!(block.end_line, 3);
        assert_eq!(block.lines.len(), 2);
    }

    #[test]
    fn test_find_by_line_range_out_of_bounds() {
        let file = make_file_content(&["a", "b"]);
        let scope = SearchScope::File(&file);
        let line_range = crate::model::LineRange { start: 10, end: 20 };

        let result = LocationMatcher::find_by_line_range(&scope, line_range, false, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_by_line_range_within_block_scope() {
        let file = make_file_content(&[
            "fn outer() {",
            "    let x = 1;",
            "    let y = 2;",
            "    let z = 3;",
            "}",
        ]);
        let block = make_block_scope(&file, 1, 5);
        let scope = SearchScope::Block(&block);
        // 行号相对 block scope: 第2行对应 "    let x = 1;"
        let line_range = crate::model::LineRange { start: 2, end: 3 };

        let result = LocationMatcher::find_by_line_range(&scope, line_range, false, false);
        assert!(result.is_ok());
        let found = result.unwrap();
        // start_line 仍为绝对行号
        assert_eq!(found.start_line, 2);
        assert_eq!(found.lines.len(), 2);
        assert_eq!(found.lines[0].content, "    let x = 1;");
        assert_eq!(found.lines[1].content, "    let y = 2;");
    }
}
