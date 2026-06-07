//! 命令执行引擎 (Engine)
//!
//! 维护全局状态机，按顺序消费 Parser 输出的 AST 节点。
//!
//! ## 状态流转
//!
//! Open → Location (可嵌套) → New/Delete/Raw → Off
//!
//! ## 错误恢复
//!
//! 执行失败时保持在内存中修改，不写回原文件，
//! 确保原文件不受部分执行的影响。
//!
//! ## 对应文档
//!
//! 详见 INSTRUCTION.md 第 1.3 节 "命令状态机" 及第 3.3-3.4 节

use crate::error::{FileError, NEditError};
use crate::matcher::LocationMatcher;
use crate::model::BLOCK_SNIPPET_MAX_LINES;
use crate::model::{ContentBlock, FileContent, Line, LineNumber, MatchInfo, SearchScope};
use crate::output::{DiffLine, DiffLineKind, CONTEXT_MAX_LINES};
use crate::parser::{Command, OffTarget};
use std::collections::HashMap;

/// 命令执行引擎
///
/// 维护全局状态机，按顺序消费 Parser 输出的 AST 节点。
pub struct Engine {
    /// 当前打开的文件路径（用于最终写回）
    file_path: Option<String>,
    /// 当前打开的文件内容（Open 命令后设置）
    pub file: Option<FileContent>,
    /// Location 嵌套栈（栈顶为当前操作作用域）
    pub block_stack: Vec<ContentBlock>,
    /// 执行过程中累积的差异输出行（New=Added, Delete=Deleted）
    pub diff_lines: Vec<DiffLine>,
    /// 上一次记录 diff 时所在的 ContentBlock 标识 (start_line, end_line)
    /// 用于判断是否需要在输出中插入分隔符
    last_diff_block_key: Option<(usize, usize)>,
    /// 详细模式：打印每条命令的执行信息（Phase 6）
    verbose: bool,
}

// ============================================================
// Delete 匹配辅助函数
// ============================================================

/// 在 ContentBlock 中查找 DeleteContent 的连续匹配区间
///
/// 返回 (start_index, end_index) 在 block.lines 中的索引。
/// 要求所有行连续匹配，不可跳行。
fn find_delete_match(
    block: &ContentBlock,
    del_content: &crate::model::DeleteContent,
) -> Option<(usize, usize)> {
    let del_lines = &del_content.lines;
    if del_lines.is_empty() || block.lines.is_empty() {
        return None;
    }

    let first_del_stripped = crate::model::stripped_content(&del_lines[0].content);

    for start_idx in 0..block.lines.len() {
        if block.lines[start_idx].stripped_content() != first_del_stripped {
            continue;
        }

        if start_idx + del_lines.len() > block.lines.len() {
            continue;
        }

        if lines_continuously_match(block, del_lines, start_idx) {
            return Some((start_idx, start_idx + del_lines.len() - 1));
        }
    }

    None
}

/// 检查从 start_idx 开始，block 的行是否与 delete_content 所有行连续匹配
fn lines_continuously_match(
    block: &ContentBlock,
    del_lines: &[crate::model::DeleteLine],
    start_idx: usize,
) -> bool {
    for (offset, del_line) in del_lines.iter().enumerate() {
        let block_line = &block.lines[start_idx + offset];

        let block_stripped = block_line.stripped_content();
        let del_stripped = crate::model::stripped_content(&del_line.content);

        let block_is_empty = block_line.content.trim().is_empty();
        let del_is_empty = del_line.content.trim().is_empty();

        if block_is_empty && del_is_empty {
            continue;
        }
        if block_is_empty || del_is_empty {
            return false;
        }
        if block_stripped != del_stripped {
            return false;
        }
    }
    true
}

/// 检查 Delete 匹配位置是否与 Location 最后一行的位置紧邻
///
/// 若之间隔了非空行，说明 Delete 可能删错了位置。
fn check_delete_adjacency(block: &ContentBlock, start_idx: usize) -> Result<(), NEditError> {
    if let MatchInfo::Location { matched_line_count } = &block.match_info {
        if *matched_line_count == 0 {
            return Ok(());
        }
        let location_last_idx = matched_line_count.saturating_sub(1);
        if start_idx <= location_last_idx {
            return Ok(());
        }
        let gap_non_empty: Vec<_> = block.lines[location_last_idx + 1..start_idx]
            .iter()
            .filter(|l| !l.content.trim().is_empty())
            .collect();
        if !gap_non_empty.is_empty() {
            let loc_last = &block.lines[location_last_idx].content;
            let del_first = &block.lines[start_idx].content;
            return Err(NEditError::Match(
                crate::error::MatchError::DeleteNotAdjacent {
                    location_last_line: loc_last.clone(),
                    delete_first_line: del_first.clone(),
                    gap_lines: gap_non_empty.len(),
                },
            ));
        }
    }
    Ok(())
}

/// 记录被删除的行到 diff_lines
#[allow(dead_code)]
fn record_deleted_lines(block: &ContentBlock, start_idx: usize, end_idx: usize) -> Vec<DiffLine> {
    block.lines[start_idx..=end_idx]
        .iter()
        .map(|line| DiffLine {
            kind: DiffLineKind::Deleted,
            line_number: Some(line.line_num),
            content: line.content.clone(),
        })
        .collect()
}

/// 构建 Delete 未找到匹配时的错误信息
fn delete_not_found_error(
    del_content: &crate::model::DeleteContent,
    block: &ContentBlock,
) -> NEditError {
    let first_del_line = del_content
        .lines
        .first()
        .map(|l| l.content.as_str())
        .unwrap_or("");
    let block_snippet = block
        .lines
        .iter()
        .take(BLOCK_SNIPPET_MAX_LINES)
        .map(|l| l.content.as_str())
        .collect::<Vec<&str>>()
        .join("\n");
    NEditError::Match(crate::error::MatchError::DeleteMatchFailed {
        delete_content: first_del_line.to_string(),
        block_snippet,
    })
}

// ============================================================
// Engine 实现
// ============================================================

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// 创建新的执行引擎实例
    pub fn new() -> Self {
        Engine {
            file_path: None,
            file: None,
            block_stack: Vec::new(),
            diff_lines: Vec::new(),
            last_diff_block_key: None,
            verbose: false,
        }
    }

    /// 设置详细输出模式
    pub fn set_verbose(&mut self, verbose: bool) {
        self.verbose = verbose;
    }

    /// 执行完整的 AST 命令序列
    ///
    /// 遍历 commands，逐条调用对应的处理方法。
    /// 执行完毕后自动处理隐式 Off:Open（若脚本末尾未显式关闭）。
    pub fn execute(&mut self, commands: Vec<Command>) -> Result<(), NEditError> {
        for command in commands {
            if self.verbose {
                match &command {
                    Command::Open { file_path } => {
                        eprintln!("[verbose] 执行 Open: {}", file_path);
                    }
                    Command::Location {
                        block,
                        line_range,
                        content,
                    } => {
                        if let Some(range) = line_range {
                            eprintln!(
                                "[verbose] 执行 Location:Block={} @{},{}",
                                block, range.start, range.end
                            );
                        } else {
                            let first_line = content
                                .lines
                                .first()
                                .map(|l| l.content.as_str())
                                .unwrap_or("");
                            eprintln!(
                                "[verbose] 执行 Location:Block={} \"{}\"",
                                block,
                                if first_line.len() > 50 {
                                    format!("{}...", &first_line[..47])
                                } else {
                                    first_line.to_string()
                                }
                            );
                        }
                    }
                    Command::New { position, content } => {
                        eprintln!(
                            "[verbose] 执行 New:{:?} ({} 行)",
                            position,
                            content.lines.len()
                        );
                    }
                    Command::Delete {
                        block,
                        line_range,
                        content,
                    } => {
                        if let Some(range) = line_range {
                            eprintln!("[verbose] 执行 Delete:@{},{}", range.start, range.end);
                        } else if *block {
                            eprintln!("[verbose] 执行 Delete:Block");
                        } else {
                            let line_count = content.as_ref().map(|c| c.lines.len()).unwrap_or(0);
                            eprintln!("[verbose] 执行 Delete ({} 行)", line_count);
                        }
                    }
                    Command::Raw { content } => {
                        eprintln!(
                            "[verbose] 执行 Raw: \"{}\"",
                            if content.len() > 30 {
                                format!("{}...", &content[..27])
                            } else {
                                content.clone()
                            }
                        );
                    }
                    Command::Off { target } => {
                        eprintln!("[verbose] 执行 Off:{:?}", target);
                    }
                }
            }
            match command {
                Command::Open { file_path } => {
                    self.execute_open(&file_path)?;
                }
                Command::Location {
                    block,
                    line_range,
                    content,
                } => {
                    self.execute_location(&content, block, line_range.as_ref())?;
                }
                Command::New { position, content } => {
                    self.execute_new(&position, &content)?;
                }
                Command::Delete {
                    block,
                    line_range,
                    content,
                } => {
                    self.execute_delete(block, content.as_ref(), line_range.as_ref())?;
                }
                Command::Raw { .. } => {
                    // Raw 命令已在 Parser 阶段融入 New/Delete 内容，
                    // Engine 无需额外处理
                }
                Command::Off { target } => {
                    self.execute_off(&target)?;
                }
            }
        }

        self.handle_implicit_off()
    }

    /// 执行 Open 命令：读取文件并构建 FileContent
    fn execute_open(&mut self, file_path: &str) -> Result<(), NEditError> {
        let file = FileContent::from_path(file_path).map_err(NEditError::File)?;
        self.file_path = Some(file_path.to_string());
        self.file = Some(file);
        Ok(())
    }

    /// 执行 Location 命令：匹配定位内容，将 ContentBlock 推入栈
    fn execute_location(
        &mut self,
        location_content: &crate::model::LocationContent,
        block: bool,
        line_range: Option<&crate::model::LineRange>,
    ) -> Result<(), NEditError> {
        let search_scope = self.get_search_scope()?;

        let content_block = if let Some(range) = line_range {
            // Phase 5: 行号定位 — 直接按索引截取，跳过 matcher
            LocationMatcher::find_by_line_range(&search_scope, *range, block, false)
                .map_err(NEditError::Match)?
        } else {
            LocationMatcher::find_unique_block(&search_scope, location_content, block)
                .map_err(NEditError::Match)?
        };

        self.block_stack.push(content_block);
        Ok(())
    }

    /// 执行 Off 命令：根据目标弹出栈或写回文件
    fn execute_off(&mut self, target: &OffTarget) -> Result<(), NEditError> {
        match target {
            OffTarget::Location | OffTarget::New => {
                let popped_block = self.block_stack.pop().ok_or(NEditError::Engine(
                    crate::error::EngineError::BlockStackEmpty,
                ))?;
                self.write_back_to_parent(popped_block)?;
            }
            OffTarget::Open => {
                self.write_back_to_file()?;
            }
        }
        Ok(())
    }

    /// 执行 Delete:Block — 删除整个 ContentBlock
    ///
    /// 移除 block 中所有行，仅保留首行的行号（避免在文件中产生空行）。
    /// 删除的行会被记录到 diff_lines 中。
    fn execute_delete_block(&mut self) -> Result<(), NEditError> {
        let block = self.block_stack.last_mut().ok_or(NEditError::Engine(
            crate::error::EngineError::MissingLocationForNew,
        ))?;

        // 收集 delete 的上下文和行数据（在被修改之前）
        let total = block.lines.len();
        let diff_data = if total > 0 {
            let (changed, context_above, context_below) =
                Self::collect_deleted_diff_data(block, 0, total.saturating_sub(1));
            Some((changed, context_above, context_below))
        } else {
            None
        };

        // 保留首行的行号，清空所有行
        let first_line_num = block.start_line;
        block.lines.clear();
        block.lines.push(Line {
            line_num: first_line_num,
            taps: 0,
            diff_taps: 0,
            content: String::new(),
            stripped_content: String::new(),
        });
        // 更新 match_info，确保后续 New:Normal 能正确插入
        block.match_info = MatchInfo::DeleteAt { position: 0 };
        block.reindex();

        if let Some((changed, context_above, context_below)) = diff_data {
            self.record_diff_with_context(changed, context_above, context_below);
        }
        Ok(())
    }

    /// 按行号范围执行 Delete（Phase 5）
    ///
    /// 直接在当前 ContentBlock 的栈顶按行号删除指定范围的行。
    /// 行号相对于栈顶 block（1-based）。
    fn execute_delete_by_line_range(
        &mut self,
        line_range: crate::model::LineRange,
    ) -> Result<(), NEditError> {
        let block = self.block_stack.last_mut().ok_or(NEditError::Engine(
            crate::error::EngineError::MissingLocationForNew,
        ))?;

        let start_index = line_range.start.saturating_sub(1);
        let end_index = (line_range.end.saturating_sub(1)).min(block.lines.len().saturating_sub(1));

        if start_index >= block.lines.len() {
            return Err(NEditError::Match(crate::error::MatchError::NoMatch {
                location_content: format!(
                    "Delete 行号 {} 超出范围（Block 共 {} 行）",
                    line_range.start,
                    block.lines.len()
                ),
            }));
        }

        // 在删除之前收集上下文和删除行数据
        let (changed, context_above, context_below) =
            Self::collect_deleted_diff_data(block, start_index, end_index);

        // 执行删除
        block.lines.drain(start_index..=end_index);
        block.match_info = MatchInfo::DeleteAt {
            position: start_index,
        };
        block.reindex();

        self.record_diff_with_context(changed, context_above, context_below);
        Ok(())
    }

    /// 执行 Delete 命令：在 ContentBlock 中删除匹配内容
    ///
    /// 若 `block` 为 true（Delete:Block），删除整个 ContentBlock.
    /// 若 `line_range` 有值（Phase 5），按行号直接删除。
    /// 否则在 block 内逐行匹配并删除。
    fn execute_delete(
        &mut self,
        block: bool,
        content: Option<&crate::model::DeleteContent>,
        line_range: Option<&crate::model::LineRange>,
    ) -> Result<(), NEditError> {
        if block {
            return self.execute_delete_block();
        }

        if let Some(range) = line_range {
            return self.execute_delete_by_line_range(*range);
        }

        let del_content = content.ok_or(NEditError::Engine(
            crate::error::EngineError::MissingLocationForNew,
        ))?;

        let current_block = self.block_stack.last_mut().ok_or(NEditError::Engine(
            crate::error::EngineError::MissingLocationForNew,
        ))?;

        let (start_idx, end_idx) = match find_delete_match(current_block, del_content) {
            Some(range) => range,
            None => return Err(delete_not_found_error(del_content, current_block)),
        };

        // 检查 Delete 匹配是否紧邻 Location 的最后一行
        check_delete_adjacency(current_block, start_idx)?;

        // 在删除之前收集上下文和删除行数据
        let (changed, context_above, context_below) =
            Self::collect_deleted_diff_data(current_block, start_idx, end_idx);

        // 执行删除并更新定位信息
        current_block.lines.drain(start_idx..=end_idx);
        current_block.match_info = MatchInfo::DeleteAt {
            position: start_idx,
        };
        current_block.reindex();

        self.record_diff_with_context(changed, context_above, context_below);
        Ok(())
    }

    /// 记录新增行到 diff_lines（无上下文，用于文件级 New:Start/New:End）
    fn record_added_lines(&mut self, entries: Vec<(usize, String)>) {
        for (line_num, content) in entries {
            self.diff_lines.push(DiffLine {
                kind: DiffLineKind::Added,
                line_number: Some(LineNumber::new(line_num)),
                content,
            });
        }
    }

    /// 获取当前 Location 的搜索范围
    ///
    /// 若 block_stack 为空（顶层 Location），搜索范围为完整 FileContent。
    /// 若 block_stack 非空（嵌套 Location），搜索范围为栈顶 ContentBlock。
    fn get_search_scope(&self) -> Result<SearchScope<'_>, NEditError> {
        if let Some(block) = self.block_stack.last() {
            Ok(SearchScope::Block(block))
        } else {
            self.file
                .as_ref()
                .map(SearchScope::File)
                .ok_or(NEditError::File(FileError::NotFound {
                    path: "(no file opened)".to_string(),
                }))
        }
    }

    /// 将弹出的 ContentBlock 写回到父级
    ///
    /// Phase 4 嵌套：若 block_stack 仍有剩余 Block，将 popped 内容写回父级 Block；
    /// 否则写回 FileContent。
    fn write_back_to_parent(&mut self, block: ContentBlock) -> Result<(), NEditError> {
        if let Some(parent) = self.block_stack.last_mut() {
            apply_block_to_parent(&block, parent);
        } else if let Some(ref mut file) = self.file {
            apply_block_to_file(file, &block);
        }
        Ok(())
    }

    /// 将所有修改最终写回磁盘文件
    ///
    /// Phase 4 嵌套：从内到外逐层弹出并写回父级 Block，
    /// 最外层写回 FileContent 后落盘。
    fn write_back_to_file(&mut self) -> Result<(), NEditError> {
        while let Some(block) = self.block_stack.pop() {
            if let Some(parent) = self.block_stack.last_mut() {
                apply_block_to_parent(&block, parent);
            } else if let Some(ref mut file) = self.file {
                apply_block_to_file(file, &block);
            }
        }

        if let (Some(ref file), Some(ref path)) = (&self.file, &self.file_path) {
            file.write_back(path).map_err(NEditError::File)?;
        }

        Ok(())
    }

    /// 处理隐式 Off:Open — 脚本末尾未显式关闭时自动写回
    fn handle_implicit_off(&mut self) -> Result<(), NEditError> {
        if self.file.is_some() {
            self.write_back_to_file()?;
        }
        Ok(())
    }

    /// 执行 New 命令：在 ContentBlock 中插入新内容
    fn execute_new(
        &mut self,
        position: &crate::parser::NewPosition,
        content: &crate::model::NewContent,
    ) -> Result<(), NEditError> {
        match position {
            crate::parser::NewPosition::Start => self.execute_new_start(content),
            crate::parser::NewPosition::End => self.execute_new_end(content),
            crate::parser::NewPosition::Normal => self.execute_new_normal(content),
        }
    }

    /// 在文件/Block 开头插入新内容
    fn execute_new_start(&mut self, content: &crate::model::NewContent) -> Result<(), NEditError> {
        let new_lines = build_new_lines(content);
        let new_line_count = new_lines.len();

        if let Some(block) = self.block_stack.last_mut() {
            let mut combined = new_lines;
            combined.append(&mut block.lines);
            block.lines = combined;
            block.reindex();
            let (changed, context_above, context_below) =
                Self::collect_added_diff_data(block, 0, new_line_count);
            self.record_diff_with_context(changed, context_above, context_below);
        } else if let Some(ref mut file) = self.file {
            let mut combined = new_lines;
            combined.append(&mut file.lines);
            file.lines = combined;
            reindex_file(file);
            let added_entries = collect_new_file_line_info(file, 0, new_line_count);
            self.record_added_lines(added_entries);
        }
        Ok(())
    }

    /// 在文件/Block 末尾插入新内容
    fn execute_new_end(&mut self, content: &crate::model::NewContent) -> Result<(), NEditError> {
        let new_lines = build_new_lines(content);
        let new_line_count = new_lines.len();

        if let Some(block) = self.block_stack.last_mut() {
            let insert_start = block.lines.len();
            block.lines.extend(new_lines);
            block.reindex();
            let (changed, context_above, context_below) =
                Self::collect_added_diff_data(block, insert_start, new_line_count);
            self.record_diff_with_context(changed, context_above, context_below);
        } else if let Some(ref mut file) = self.file {
            let insert_start = file.lines.len();
            let new_lines_clone = build_new_lines(content);
            file.lines.extend(new_lines_clone);
            reindex_file(file);
            let added_entries = collect_new_file_line_info(file, insert_start, new_line_count);
            self.record_added_lines(added_entries);
        }
        Ok(())
    }

    /// 在 Location 匹配位置之后插入新内容
    fn execute_new_normal(&mut self, content: &crate::model::NewContent) -> Result<(), NEditError> {
        let insert_pos = {
            let block = self.block_stack.last_mut().ok_or(NEditError::Engine(
                crate::error::EngineError::MissingLocationForNew,
            ))?;
            match &block.match_info {
                MatchInfo::Empty => block.lines.len(),
                MatchInfo::Location { matched_line_count } => *matched_line_count,
                MatchInfo::DeleteAt { position } => *position,
            }
        };

        let new_lines = build_new_lines(content);
        let new_line_count = new_lines.len();

        let (changed, context_above, context_below) = {
            let block = self.block_stack.last_mut().ok_or(NEditError::Engine(
                crate::error::EngineError::MissingLocationForNew,
            ))?;
            if insert_pos >= block.lines.len() {
                block.lines.extend(new_lines);
            } else {
                let tail = block.lines.split_off(insert_pos);
                block.lines.extend(new_lines);
                block.lines.extend(tail);
            }
            block.reindex();
            Self::collect_added_diff_data(block, insert_pos, new_line_count)
        };

        self.record_diff_with_context(changed, context_above, context_below);
        Ok(())
    }

    /// 记录差异行，包含上下文和分隔符（Phase 6）
    ///
    /// 在记录 Added/Deleted 行之前，先插入上下文行（上下各最多 CONTEXT_MAX_LINES 行）
    /// 和分隔符（若 ContentBlock 发生变化）。
    fn record_diff_with_context(
        &mut self,
        changed_lines: Vec<DiffLine>,
        context_above: Vec<DiffLine>,
        context_below: Vec<DiffLine>,
    ) {
        self.insert_separator_if_needed();

        for line in context_above {
            self.diff_lines.push(line);
        }
        for line in changed_lines {
            self.diff_lines.push(line);
        }
        for line in context_below {
            self.diff_lines.push(line);
        }

        self.update_diff_block_key();
    }

    /// 获取 ContentBlock 的唯一标识
    fn get_block_key(block: &ContentBlock) -> (usize, usize) {
        (block.start_line.to_usize(), block.end_line.to_usize())
    }

    /// 获取当前 ContentBlock 的唯一标识
    fn get_current_block_key(&self) -> Option<(usize, usize)> {
        self.block_stack.last().map(Self::get_block_key)
    }

    /// 若当前 ContentBlock 与上一次不同，插入分隔符
    fn insert_separator_if_needed(&mut self) {
        let current_key = self.get_current_block_key();
        if current_key != self.last_diff_block_key
            && self.last_diff_block_key.is_some()
            && !self.diff_lines.is_empty()
        {
            self.diff_lines.push(DiffLine::separator());
        }
    }

    /// 更新最后一次记录的 block key
    fn update_diff_block_key(&mut self) {
        self.last_diff_block_key = self.get_current_block_key();
    }

    /// 收集新增行的 diff 数据（changed + context），供调用方传给 record_diff_with_context
    fn collect_added_diff_data(
        block: &ContentBlock,
        insert_pos: usize,
        new_line_count: usize,
    ) -> (Vec<DiffLine>, Vec<DiffLine>, Vec<DiffLine>) {
        let end_idx = (insert_pos + new_line_count).min(block.lines.len());
        let context_above = collect_block_context_above(block, insert_pos);
        let context_below = collect_block_context_below(block, end_idx.saturating_sub(1));
        let changed: Vec<DiffLine> = block.lines[insert_pos..end_idx]
            .iter()
            .map(|line| DiffLine {
                kind: DiffLineKind::Added,
                line_number: Some(line.line_num),
                content: line.content.clone(),
            })
            .collect();
        (changed, context_above, context_below)
    }

    /// 收集删除行的 diff 数据（changed + context），供调用方传给 record_diff_with_context
    fn collect_deleted_diff_data(
        block: &ContentBlock,
        start_idx: usize,
        end_idx: usize,
    ) -> (Vec<DiffLine>, Vec<DiffLine>, Vec<DiffLine>) {
        let context_above = collect_block_context_above(block, start_idx);
        let context_below = collect_block_context_below(block, end_idx);
        let changed: Vec<DiffLine> = block.lines[start_idx..=end_idx]
            .iter()
            .map(|line| DiffLine {
                kind: DiffLineKind::Deleted,
                line_number: Some(line.line_num),
                content: line.content.clone(),
            })
            .collect();
        (changed, context_above, context_below)
    }
}

// ============================================================
// Block / File 写回辅助函数
// ============================================================

/// 将 ContentBlock 的修改应用到 FileContent 中对应位置
///
/// 使用 block.start_line 和 block.end_line 确定原始范围，
/// 将其替换为 block 的当前行。
fn apply_block_to_file(file: &mut FileContent, block: &ContentBlock) {
    let start_index = block.start_line.to_index();
    let end_index = block.end_line.to_index();

    let count = end_index.saturating_sub(start_index) + 1;
    let count = count.min(file.lines.len().saturating_sub(start_index));

    let new_lines: Vec<Line> = block
        .lines
        .iter()
        .map(|line| Line {
            line_num: line.line_num,
            taps: line.taps,
            diff_taps: line.diff_taps,
            content: line.content.clone(),
            stripped_content: line.stripped_content.clone(),
        })
        .collect();

    file.lines
        .splice(start_index..start_index + count, new_lines);

    reindex_file(file);
}

/// 将内层 ContentBlock 的修改应用到父级 ContentBlock 中
///
/// 用于嵌套 Location 场景（Phase 4）：内层 Block（inner）弹出后，
/// 通过 start_line 差值计算偏移量，将内层修改合并回父级 Block（outer）。
fn apply_block_to_parent(inner: &ContentBlock, outer: &mut ContentBlock) {
    let start_offset = inner
        .start_line
        .to_index()
        .saturating_sub(outer.start_line.to_index());
    let end_offset = inner
        .end_line
        .to_index()
        .saturating_sub(outer.start_line.to_index());

    let start_offset = start_offset.min(outer.lines.len());
    let end_offset = end_offset.min(outer.lines.len().saturating_sub(1));

    let count = if end_offset >= start_offset {
        end_offset - start_offset + 1
    } else {
        0
    };

    let new_lines: Vec<Line> = inner
        .lines
        .iter()
        .map(|line| Line {
            line_num: line.line_num,
            taps: line.taps,
            diff_taps: line.diff_taps,
            content: line.content.clone(),
            stripped_content: line.stripped_content.clone(),
        })
        .collect();

    outer
        .lines
        .splice(start_offset..start_offset + count, new_lines);
    outer.reindex();
}

/// 从 NewContent 构建 Line 列表
///
/// 使用 NewContent 中各行的 diff_taps 作为绝对缩进量计算实际 taps，
/// 生成 Line 结构用于插入。line_num 设为占位值，调用方通过 reindex 重算。
fn build_new_lines(content: &crate::model::NewContent) -> Vec<Line> {
    const PLACEHOLDER_LINE_NUM: LineNumber = LineNumber::new(1);

    content
        .lines
        .iter()
        .map(|new_line| {
            let actual_taps = if new_line.is_raw {
                crate::model::count_leading_spaces(&new_line.content)
            } else {
                new_line.diff_taps
            };
            let indented_content = if new_line.is_raw {
                new_line.content.clone()
            } else if actual_taps > 0 {
                format!("{:indent$}{}", "", new_line.content, indent = actual_taps)
            } else {
                new_line.content.clone()
            };
            let stripped = crate::model::stripped_content(&indented_content);
            Line {
                line_num: PLACEHOLDER_LINE_NUM,
                taps: actual_taps,
                diff_taps: 0,
                content: indented_content,
                stripped_content: stripped,
            }
        })
        .collect()
}

/// 从 ContentBlock 中收集新增行的 (line_num, content) 信息
#[allow(dead_code)]
fn collect_new_line_info(
    block: &ContentBlock,
    insert_pos: usize,
    new_line_count: usize,
) -> Vec<(usize, String)> {
    let end = (insert_pos + new_line_count).min(block.lines.len());
    (insert_pos..end)
        .map(|i| {
            (
                block.lines[i].line_num.to_usize(),
                block.lines[i].content.clone(),
            )
        })
        .collect()
}

/// 从 FileContent 中收集新增行的 (line_num, content) 信息
fn collect_new_file_line_info(
    file: &FileContent,
    insert_pos: usize,
    new_line_count: usize,
) -> Vec<(usize, String)> {
    let end = (insert_pos + new_line_count).min(file.lines.len());
    (insert_pos..end)
        .map(|i| {
            (
                file.lines[i].line_num.to_usize(),
                file.lines[i].content.clone(),
            )
        })
        .collect()
}

/// 从 ContentBlock 中收集指定位置之前的上下文行（最多 CONTEXT_MAX_LINES 行）
fn collect_block_context_above(block: &ContentBlock, position: usize) -> Vec<DiffLine> {
    if position == 0 {
        return Vec::new();
    }
    let start = position.saturating_sub(CONTEXT_MAX_LINES);
    block.lines[start..position]
        .iter()
        .map(|line| DiffLine {
            kind: DiffLineKind::Unchanged,
            line_number: Some(line.line_num),
            content: line.content.clone(),
        })
        .collect()
}

/// 从 ContentBlock 中收集指定位置之后的上下文行（最多 CONTEXT_MAX_LINES 行）
fn collect_block_context_below(block: &ContentBlock, position: usize) -> Vec<DiffLine> {
    if position + 1 >= block.lines.len() {
        return Vec::new();
    }
    let end = (position + 1 + CONTEXT_MAX_LINES).min(block.lines.len());
    block.lines[position + 1..end]
        .iter()
        .map(|line| DiffLine {
            kind: DiffLineKind::Unchanged,
            line_number: Some(line.line_num),
            content: line.content.clone(),
        })
        .collect()
}

/// 重新为 FileContent 的所有行分配行号和重算 diff_taps，重建首行索引
fn reindex_file(file: &mut FileContent) {
    let base_taps = file.lines.first().map(|l| l.taps).unwrap_or(0);
    for (index, line) in file.lines.iter_mut().enumerate() {
        line.line_num = LineNumber::from_index(index);
        line.diff_taps = line.taps.saturating_sub(base_taps);
    }
    let mut index: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, line) in file.lines.iter().enumerate() {
        index
            .entry(line.stripped_content.clone())
            .or_default()
            .push(i);
    }
    file.first_line_index = index;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        DeleteContent, DeleteLine, LocationContent, LocationLine, NewContent, NewLine,
    };
    use crate::parser::{Command, NewPosition, OffTarget};

    /// 辅助结构：持有临时文件及其路径，确保文件在测试期间存活
    struct TempFile {
        path: String,
        _temp_dir: tempfile::TempDir,
    }

    /// 辅助函数：创建测试用的临时文件并返回包装结构
    fn create_temp_file(content: &str) -> TempFile {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test_file.txt");
        let path_str = file_path.to_str().unwrap().to_string();
        std::fs::write(&file_path, content).unwrap();
        TempFile {
            path: path_str,
            _temp_dir: dir,
        }
    }

    /// 辅助函数：构建简单的 LocationContent
    fn make_location_content(lines: &[&str]) -> LocationContent {
        if lines.is_empty() {
            return LocationContent { lines: vec![] };
        }
        let base_taps = crate::model::count_leading_spaces(lines[0]);
        let loc_lines: Vec<LocationLine> = lines
            .iter()
            .enumerate()
            .map(|(i, content)| {
                let line_taps = crate::model::count_leading_spaces(content);
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

    /// 辅助函数：构建简单的 NewContent（diff_taps 为绝对缩进量）
    fn make_new_content(lines: &[&str]) -> NewContent {
        let new_lines: Vec<NewLine> = lines
            .iter()
            .map(|content| {
                let line_taps = crate::model::count_leading_spaces(content);
                let stripped_content = content[line_taps..].to_string();
                NewLine {
                    diff_taps: line_taps,
                    content: stripped_content,
                    is_raw: false,
                }
            })
            .collect();
        NewContent { lines: new_lines }
    }

    /// 辅助函数：构建简单的 DeleteContent
    fn make_delete_content(lines: &[&str]) -> DeleteContent {
        let del_lines: Vec<DeleteLine> = lines
            .iter()
            .map(|content| DeleteLine {
                content: content.to_string(),
                is_raw: false,
            })
            .collect();
        DeleteContent { lines: del_lines }
    }

    // ============================================================
    // Engine 基本生命周期测试
    // ============================================================

    #[test]
    fn test_engine_open_reads_file() {
        let tmp = create_temp_file("line one\nline two\nline three\n");
        let commands = vec![Command::Open {
            file_path: tmp.path.clone(),
        }];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_ok(), "Unexpected error: {:?}", result.err());

        let file = engine.file.as_ref().unwrap();
        assert_eq!(file.lines.len(), 3);
        assert_eq!(file.lines[0].content, "line one");
    }

    #[test]
    fn test_engine_open_nonexistent_file_errors() {
        let commands = vec![Command::Open {
            file_path: "/nonexistent/path.xyz".to_string(),
        }];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_err());
    }

    #[test]
    fn test_engine_location_pushes_to_block_stack() {
        let tmp = create_temp_file("fn main() {\n    let x = 1;\n}\n");
        let mut engine = Engine::new();

        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&["fn main() {"]), false, None)
            .unwrap();

        assert_eq!(engine.block_stack.len(), 1);
        let current_block = &engine.block_stack[0];
        assert_eq!(current_block.start_line, 1);
    }

    #[test]
    fn test_engine_location_no_match_errors() {
        let tmp = create_temp_file("fn foo() {}\nfn bar() {}\n");
        let commands = vec![
            Command::Open {
                file_path: tmp.path.clone(),
            },
            Command::Location {
                line_range: None,
                block: false,
                content: make_location_content(&["fn nonexistent() {}"]),
            },
        ];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_err());
    }

    #[test]
    fn test_engine_off_location_pops_stack() {
        let tmp = create_temp_file("fn main() {\n    let x = 1;\n}\n");
        let commands = vec![
            Command::Open {
                file_path: tmp.path.clone(),
            },
            Command::Location {
                line_range: None,
                block: false,
                content: make_location_content(&["fn main() {"]),
            },
            Command::Off {
                target: OffTarget::Location,
            },
        ];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_ok(), "Unexpected error: {:?}", result.err());

        assert_eq!(engine.block_stack.len(), 0);
    }

    #[test]
    fn test_engine_off_open_writes_back_to_file() {
        let tmp = create_temp_file("original content\n");
        let commands = vec![
            Command::Open {
                file_path: tmp.path.clone(),
            },
            Command::Off {
                target: OffTarget::Open,
            },
        ];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_ok(), "Unexpected error: {:?}", result.err());

        let content = std::fs::read_to_string(&tmp.path).unwrap();
        assert_eq!(content, "original content\n");
    }

    // ============================================================
    // 隐式 Off:Open 测试
    // ============================================================

    #[test]
    fn test_engine_implicit_off_open_writes_back() {
        let tmp = create_temp_file("content\n");
        let commands = vec![Command::Open {
            file_path: tmp.path.clone(),
        }];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_ok(), "Unexpected error: {:?}", result.err());

        let content = std::fs::read_to_string(&tmp.path).unwrap();
        assert_eq!(content, "content\n");
    }

    // ============================================================
    // Open-Location-Off 完整流程测试
    // ============================================================

    #[test]
    fn test_engine_full_open_location_off_flow() {
        let tmp = create_temp_file(
            "// header\nfn process() {\n    do_work();\n}\n\nfn main() {\n    process();\n}\n",
        );
        let commands = vec![
            Command::Open {
                file_path: tmp.path.clone(),
            },
            Command::Location {
                line_range: None,
                block: false,
                content: make_location_content(&["fn main() {"]),
            },
            Command::Off {
                target: OffTarget::Open,
            },
        ];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_ok(), "Unexpected error: {:?}", result.err());

        let content = std::fs::read_to_string(&tmp.path).unwrap();
        assert!(content.contains("fn main()"));
        assert!(content.contains("fn process()"));
    }

    // ============================================================
    // execute_new — 插入测试
    // ============================================================

    #[test]
    fn test_new_insert_normal() {
        let tmp = create_temp_file("fn main() {\n    println!(\"hi\");\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&["fn main() {"]), false, None)
            .unwrap();

        let new_content = make_new_content(&["    let x = 1;"]);
        engine
            .execute_new(&NewPosition::Normal, &new_content)
            .unwrap();

        let current_block = engine.block_stack.last().unwrap();
        assert_eq!(current_block.lines.len(), 4);
        assert_eq!(current_block.lines[1].content, "    let x = 1;");
    }

    #[test]
    fn test_new_insert_start() {
        let tmp = create_temp_file("fn main() {\n    println!(\"hi\");\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        let new_content = make_new_content(&["// SPDX-License-Identifier: MIT"]);
        engine
            .execute_new(&NewPosition::Start, &new_content)
            .unwrap();

        let file = engine.file.as_ref().unwrap();
        assert_eq!(file.lines.len(), 4);
        assert_eq!(file.lines[0].content, "// SPDX-License-Identifier: MIT");
    }

    #[test]
    fn test_new_insert_end() {
        let tmp = create_temp_file("fn main() {\n    println!(\"hi\");\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        let new_content = make_new_content(&["// EOF"]);
        engine.execute_new(&NewPosition::End, &new_content).unwrap();

        let file = engine.file.as_ref().unwrap();
        assert_eq!(file.lines.len(), 4);
        assert_eq!(file.lines[3].content, "// EOF");
    }

    #[test]
    fn test_new_insert_preserves_indentation() {
        let tmp = create_temp_file("fn main() {\n    println!(\"hi\");\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&["fn main() {"]), false, None)
            .unwrap();

        let new_content = make_new_content(&["    let a = 1;", "        let b = 2;"]);
        engine
            .execute_new(&NewPosition::Normal, &new_content)
            .unwrap();

        let current_block = engine.block_stack.last().unwrap();
        assert_eq!(current_block.lines[1].content, "    let a = 1;");
        assert_eq!(current_block.lines[2].content, "        let b = 2;");
    }

    // ============================================================
    // execute_delete — 删除测试
    // ============================================================

    #[test]
    fn test_delete_removes_matching_lines() {
        let tmp = create_temp_file(
            "fn main() {\n    let x = 1;\n    let y = 2;\n    println!(\"{}\", x);\n}\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&["fn main() {"]), false, None)
            .unwrap();

        let del_content = make_delete_content(&["    let x = 1;", "    let y = 2;"]);
        engine
            .execute_delete(false, Some(&del_content), None)
            .unwrap();
    }

    #[test]
    fn test_delete_requires_continuous_match() {
        let tmp = create_temp_file(
            "fn main() {\n    let x = 1;\n    let y = 2;\n    println!(\"{}\", x);\n}\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&["fn main() {"]), false, None)
            .unwrap();

        let del_content = make_delete_content(&["    let x = 1;", "    println!(\"{}\", x);"]);
        let result = engine.execute_delete(false, Some(&del_content), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_content_not_found() {
        let tmp = create_temp_file("fn main() {\n    println!(\"hi\");\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&["fn main() {"]), false, None)
            .unwrap();

        let del_content = make_delete_content(&["    nonexistent content"]);
        let result = engine.execute_delete(false, Some(&del_content), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_insert_normal_without_location_errors() {
        let tmp = create_temp_file("fn main() {\n    println!(\"hi\");\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        let new_content = make_new_content(&["    let x = 1;"]);
        let result = engine.execute_new(&NewPosition::Normal, &new_content);
        assert!(result.is_err());
    }

    #[test]
    fn test_full_new_delete_pipeline() {
        let tmp = create_temp_file("fn main() {\n    old_code();\n}\n");
        let commands = vec![
            Command::Open {
                file_path: tmp.path.clone(),
            },
            Command::Location {
                line_range: None,
                block: false,
                content: make_location_content(&["fn main() {"]),
            },
            Command::Delete {
                line_range: None,
                block: false,
                content: Some(make_delete_content(&["    old_code();"])),
            },
            Command::New {
                position: NewPosition::Normal,
                content: make_new_content(&["    let x = 1;"]),
            },
            Command::Off {
                target: OffTarget::Open,
            },
        ];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_ok(), "Unexpected error: {:?}", result.err());

        let content = std::fs::read_to_string(&tmp.path).unwrap();
        assert!(content.contains("    let x = 1;"));
        assert!(!content.contains("old_code"));
    }

    // ============================================================
    // diff_lines 输出测试
    // ============================================================

    #[test]
    fn test_new_produces_added_diff_lines() {
        let tmp = create_temp_file("fn main() {\n    println!(\"hi\");\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&["fn main() {"]), false, None)
            .unwrap();
        engine
            .execute_new(&NewPosition::Normal, &make_new_content(&["    let x = 1;"]))
            .unwrap();

        // 过滤出 Added 行（忽略上下文 Unchanged 行和分隔符）
        let added: Vec<_> = engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == DiffLineKind::Added)
            .collect();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].kind, DiffLineKind::Added);
        assert_eq!(added[0].content, "    let x = 1;");
        assert!(added[0].line_number.is_some());
    }

    #[test]
    fn test_delete_produces_deleted_diff_lines() {
        let tmp = create_temp_file("fn main() {\n    let x = 1;\n    let y = 2;\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&["fn main() {"]), false, None)
            .unwrap();
        engine
            .execute_delete(false, Some(&make_delete_content(&["    let x = 1;"])), None)
            .unwrap();

        let deleted: Vec<_> = engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == DiffLineKind::Deleted)
            .collect();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].kind, DiffLineKind::Deleted);
        assert_eq!(deleted[0].content, "    let x = 1;");
        assert!(deleted[0].line_number.is_some());
    }

    #[test]
    fn test_new_delete_produces_mixed_diff_lines() {
        let tmp = create_temp_file("fn main() {\n    old_code();\n}\n");
        let commands = vec![
            Command::Open {
                file_path: tmp.path.clone(),
            },
            Command::Location {
                line_range: None,
                block: false,
                content: make_location_content(&["fn main() {"]),
            },
            Command::Delete {
                line_range: None,
                block: false,
                content: Some(make_delete_content(&["    old_code();"])),
            },
            Command::New {
                position: NewPosition::Normal,
                content: make_new_content(&["    let x = 1;"]),
            },
            Command::Off {
                target: OffTarget::Open,
            },
        ];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_ok(), "Unexpected error: {:?}", result.err());

        let added: Vec<_> = engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == DiffLineKind::Added)
            .collect();
        let deleted: Vec<_> = engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == DiffLineKind::Deleted)
            .collect();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].content, "    old_code();");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].content, "    let x = 1;");
    }

    #[test]
    fn test_new_start_produces_diff_lines() {
        let tmp = create_temp_file("fn main() {\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_new(
                &NewPosition::Start,
                &make_new_content(&["// SPDX-License-Identifier: MIT"]),
            )
            .unwrap();

        assert_eq!(engine.diff_lines.len(), 1);
        assert_eq!(engine.diff_lines[0].kind, DiffLineKind::Added);
        assert_eq!(
            engine.diff_lines[0].content,
            "// SPDX-License-Identifier: MIT"
        );
    }

    #[test]
    fn test_new_end_produces_diff_lines() {
        let tmp = create_temp_file("fn main() {\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_new(&NewPosition::End, &make_new_content(&["// EOF"]))
            .unwrap();

        assert_eq!(engine.diff_lines.len(), 1);
        assert_eq!(engine.diff_lines[0].kind, DiffLineKind::Added);
        assert_eq!(engine.diff_lines[0].content, "// EOF");
    }

    // ============================================================
    // Phase 3: Delete → New 定位修复测试
    // ============================================================

    #[test]
    fn test_empty_location_delete_then_new_replaces_deleted() {
        let tmp = create_temp_file(
            "// header\nfn process() {\n    do_work();\n}\n\nfn main() {\n    old_code();\n}\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();
        engine
            .execute_delete(
                false,
                Some(&make_delete_content(&["    old_code();"])),
                None,
            )
            .unwrap();
        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["    new_code();"]),
            )
            .unwrap();

        let current_block = engine.block_stack.last().unwrap();
        let contents: Vec<&str> = current_block
            .lines
            .iter()
            .map(|l| l.content.as_str())
            .collect();
        assert!(contents.contains(&"    new_code();"));
        assert!(!contents.contains(&"    old_code();"));
        assert!(contents.contains(&"fn main() {"));
    }

    #[test]
    fn test_empty_location_new_without_delete_inserts_at_end() {
        let tmp = create_temp_file("line1\nline2\nline3\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();
        engine
            .execute_new(&NewPosition::Normal, &make_new_content(&["line4"]))
            .unwrap();

        let current_block = engine.block_stack.last().unwrap();
        assert_eq!(current_block.lines.len(), 4);
        assert_eq!(current_block.lines[3].content, "line4");
    }

    #[test]
    fn test_delete_at_start_then_new_inserts_at_start() {
        let tmp = create_temp_file("old first\nsecond\nthird\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();
        engine
            .execute_delete(false, Some(&make_delete_content(&["old first"])), None)
            .unwrap();
        engine
            .execute_new(&NewPosition::Normal, &make_new_content(&["new first"]))
            .unwrap();

        let current_block = engine.block_stack.last().unwrap();
        assert_eq!(current_block.lines[0].content, "new first");
        assert_eq!(current_block.lines[1].content, "second");
        assert_eq!(current_block.lines.len(), 3);
    }

    #[test]
    fn test_delete_then_new_preserves_indentation() {
        let tmp = create_temp_file("impl Foo {\n    fn bar() {\n        old_inner();\n    }\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();
        engine
            .execute_delete(
                false,
                Some(&make_delete_content(&["        old_inner();"])),
                None,
            )
            .unwrap();
        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["        new_inner();"]),
            )
            .unwrap();

        let current_block = engine.block_stack.last().unwrap();
        assert!(current_block
            .lines
            .iter()
            .any(|l| l.content == "        new_inner();"));
        assert!(!current_block
            .lines
            .iter()
            .any(|l| l.content == "        old_inner();"));
        assert!(current_block
            .lines
            .iter()
            .any(|l| l.content == "    fn bar() {"));
        assert!(current_block.lines.iter().any(|l| l.content == "}"));
    }

    // ============================================================
    // Phase 4: 嵌套 Location 测试
    // ============================================================

    #[test]
    fn test_nested_location_basic() {
        let tmp = create_temp_file(
            "fn outer() {\n    let x = 1;\n    fn inner() {\n        let y = 2;\n    }\n    let z = 3;\n}\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["fn outer() {"]), false, None)
            .unwrap();

        engine
            .execute_location(&make_location_content(&["    fn inner() {"]), false, None)
            .unwrap();

        assert_eq!(engine.block_stack.len(), 2);

        let inner_block = &engine.block_stack[1];
        assert_eq!(inner_block.start_line, 3);
        assert!(inner_block.lines.len() >= 3);

        engine.execute_off(&OffTarget::Location).unwrap();
        assert_eq!(engine.block_stack.len(), 1);

        engine.execute_off(&OffTarget::Location).unwrap();
        assert_eq!(engine.block_stack.len(), 0);
    }

    #[test]
    fn test_nested_location_new() {
        let tmp =
            create_temp_file("fn outer() {\n    fn inner() {\n        let a = 1;\n    }\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["fn outer() {"]), false, None)
            .unwrap();

        engine
            .execute_location(&make_location_content(&["    fn inner() {"]), false, None)
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["        let b = 2;"]),
            )
            .unwrap();

        let inner_block = engine.block_stack.last().unwrap();
        assert!(inner_block
            .lines
            .iter()
            .any(|l| l.content.contains("let b = 2;")));
        assert!(inner_block.lines.len() >= 4);

        let outer_block = &engine.block_stack[0];
        assert!(outer_block.lines.len() >= 4);
    }

    #[test]
    fn test_nested_location_delete() {
        let tmp = create_temp_file(
            "fn outer() {\n    fn inner() {\n        let old = 1;\n        let keep = 2;\n    }\n}\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["fn outer() {"]), false, None)
            .unwrap();
        engine
            .execute_location(&make_location_content(&["    fn inner() {"]), false, None)
            .unwrap();

        engine
            .execute_delete(
                false,
                Some(&make_delete_content(&["        let old = 1;"])),
                None,
            )
            .unwrap();

        let inner_block = engine.block_stack.last().unwrap();
        assert!(!inner_block
            .lines
            .iter()
            .any(|l| l.content.contains("let old")));
        assert!(inner_block
            .lines
            .iter()
            .any(|l| l.content.contains("let keep")));
        assert!(inner_block.lines.len() >= 3);
    }

    #[test]
    fn test_nested_location_off_chain() {
        let tmp = create_temp_file(
            "fn outer() {\n    fn inner() {\n        let old = 1;\n    }\n    let z = 3;\n}\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["fn outer() {"]), false, None)
            .unwrap();
        engine
            .execute_location(&make_location_content(&["    fn inner() {"]), false, None)
            .unwrap();

        engine
            .execute_delete(
                false,
                Some(&make_delete_content(&["        let old = 1;"])),
                None,
            )
            .unwrap();
        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["        let new = 2;"]),
            )
            .unwrap();

        engine.execute_off(&OffTarget::Location).unwrap();
        assert_eq!(engine.block_stack.len(), 1);

        let outer_block = engine.block_stack.last().unwrap();
        assert!(outer_block
            .lines
            .iter()
            .any(|l| l.content.contains("let new = 2;")));
        assert!(!outer_block
            .lines
            .iter()
            .any(|l| l.content.contains("let old")));

        engine.execute_off(&OffTarget::Location).unwrap();

        let file = engine.file.as_ref().unwrap();
        assert!(!file.lines.iter().any(|l| l.content.contains("let old")));
        assert!(file.lines.iter().any(|l| l.content.contains("let new")));
        assert!(file.lines.iter().any(|l| l.content.contains("let z")));
    }

    #[test]
    fn test_nested_location_with_empty_inner() {
        let tmp = create_temp_file(
            "fn outer() {\n    fn inner() {\n        let x = 1;\n    }\n    let y = 2;\n}\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["fn outer() {"]), false, None)
            .unwrap();

        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        let inner_block = engine.block_stack.last().unwrap();
        assert_eq!(inner_block.start_line, 1);
        assert_eq!(inner_block.lines.len(), 6);
    }

    #[test]
    fn test_nested_location_new_start_end() {
        let tmp =
            create_temp_file("fn outer() {\n    fn inner() {\n        let x = 1;\n    }\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["fn outer() {"]), false, None)
            .unwrap();
        engine
            .execute_location(&make_location_content(&["    fn inner() {"]), false, None)
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Start,
                &make_new_content(&["        // start of inner"]),
            )
            .unwrap();

        let inner_block = engine.block_stack.last().unwrap();
        assert_eq!(inner_block.lines[0].content, "        // start of inner");

        engine
            .execute_new(
                &NewPosition::End,
                &make_new_content(&["        // end of inner"]),
            )
            .unwrap();

        let inner_block = engine.block_stack.last().unwrap();
        assert_eq!(
            inner_block.lines.last().unwrap().content,
            "        // end of inner"
        );
    }

    #[test]
    fn test_nested_location_via_commands() {
        let tmp =
            create_temp_file("fn outer() {\n    fn inner() {\n        let a = 1;\n    }\n}\n");
        let commands = vec![
            Command::Open {
                file_path: tmp.path.clone(),
            },
            Command::Location {
                line_range: None,
                block: false,
                content: make_location_content(&["fn outer() {"]),
            },
            Command::Location {
                line_range: None,
                block: false,
                content: make_location_content(&["    fn inner() {"]),
            },
            Command::New {
                position: NewPosition::Normal,
                content: make_new_content(&["        let b = 2;"]),
            },
            Command::Off {
                target: OffTarget::Location,
            },
            Command::Off {
                target: OffTarget::Location,
            },
        ];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_ok(), "Unexpected error: {:?}", result.err());

        let file = engine.file.as_ref().unwrap();
        assert!(file.lines.iter().any(|l| l.content.contains("let b = 2;")));
        assert!(file.lines.iter().any(|l| l.content.contains("fn outer")));
        assert!(file.lines.iter().any(|l| l.content.contains("fn inner")));
    }

    // ============================================================
    // Phase 4: 复杂工程场景 — 嵌套 Location 集成测试
    // ============================================================

    #[test]
    fn test_nested_three_level_method_match_arm() {
        let content = [
            "impl Service {",
            "    fn process(&self, status: Status) {",
            "        match status {",
            "            Status::Active => {",
            "                self.do_work();",
            "            }",
            "            Status::Inactive => {",
            "                self.skip();",
            "            }",
            "        }",
            "    }",
            "}",
        ]
        .join("\n");
        let tmp = create_temp_file(&content);
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["impl Service {"]), false, None)
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["        match status {"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["            Status::Active => {"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["                log::info!(\"processing active status\");"]),
            )
            .unwrap();

        let inner = engine.block_stack.last().unwrap();
        assert!(inner.lines.iter().any(|l| l.content.contains("log::info")));

        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();

        let file = engine.file.as_ref().unwrap();
        assert!(file.lines.iter().any(|l| l.content.contains("log::info")));
        assert!(file
            .lines
            .iter()
            .any(|l| l.content.contains("Status::Inactive")));
    }

    #[test]
    fn test_nested_three_level_async_error_refactor() {
        let content = [
            "async fn handle_request(req: Request) -> Result<Response> {",
            "    let data = fetch_data().await?;",
            "    if let Some(payload) = data.payload {",
            "        match payload.kind {",
            "            Kind::Success => process(payload).await?,",
            "            Kind::Retry => {",
            "                self.retry_count += 1;",
            "                return Err(Error::retry_exhausted());",
            "            }",
            "        }",
            "    }",
            "    Ok(Response::ok())",
            "}",
        ]
        .join("\n");
        let tmp = create_temp_file(&content);
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(
                &make_location_content(&[
                    "async fn handle_request(req: Request) -> Result<Response> {",
                ]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["    if let Some(payload) = data.payload {"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["        Kind::Retry => {"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_delete(
                false,
                Some(&make_delete_content(&[
                    "            self.retry_count += 1;",
                    "            return Err(Error::retry_exhausted());",
                ])),
                None,
            )
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&[
                    "            self.metrics.record_retry();",
                    "            self.retry_count += 1;",
                    "            if self.retry_count > 3 {",
                    "                return Err(Error::retry_exhausted());",
                    "            }",
                    "            continue;",
                ]),
            )
            .unwrap();

        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();

        let file = engine.file.as_ref().unwrap();
        let contents: Vec<&str> = file.lines.iter().map(|l| l.content.as_str()).collect();
        let joined = contents.join("\n");

        assert!(!joined.contains("self.retry_count += 1;\n            return Err"));
        assert!(joined.contains("self.metrics.record_retry()"));
        assert!(joined.contains("if self.retry_count > 3"));
        assert!(joined.contains("Kind::Success"));
    }

    #[test]
    fn test_nested_cross_level_new_delete_with_module_end() {
        let tmp = create_temp_file(
            "pub mod utils {\n\
             pub fn validate(input: &str) -> bool {\n\
             let trimmed = input.trim();\n\
             if trimmed.is_empty() {\n\
             self.log_warning();\n\
             return false;\n\
             }\n\
             true\n\
             }\n\
             }\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["pub mod utils {"]), false, None)
            .unwrap();

        engine
            .execute_new(
                &NewPosition::End,
                &make_new_content(&[
                    "",
                    "    pub fn sanitize(input: &str) -> String {",
                    "        input.trim().to_lowercase()",
                    "    }",
                ]),
            )
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["    pub fn validate(input: &str) -> bool {"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["        if trimmed.is_empty() {"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_delete(
                false,
                Some(&make_delete_content(&["            self.log_warning();"])),
                None,
            )
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["            crate::log::warn(\"empty input\");"]),
            )
            .unwrap();

        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();

        let file = engine.file.as_ref().unwrap();
        let contents: Vec<&str> = file.lines.iter().map(|l| l.content.as_str()).collect();
        let joined = contents.join("\n");

        assert!(joined.contains("pub fn sanitize"));
        assert!(joined.contains("input.trim().to_lowercase()"));
        assert!(!joined.contains("self.log_warning()"));
        assert!(joined.contains("crate::log::warn"));
        assert!(joined.contains("pub fn validate"));
    }

    #[test]
    fn test_nested_deep_indentation_preserved() {
        let tmp = create_temp_file(
            "struct Processor {\n\
             items: Vec<Item>,\n\
             }\n\
             impl Processor {\n\
             fn run(&mut self) {\n\
             for item in &self.items {\n\
             if item.is_valid() {\n\
             item.process();\n\
             }\n\
             }\n\
             }\n\
             }\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["impl Processor {"]), false, None)
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["    fn run(&mut self) {"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["        for item in &self.items {"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["            if item.is_valid() {"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&[
                    "                log::debug!(\"processing item {}\", item.id);",
                    "                metrics::increment_counter(\"items_processed\");",
                ]),
            )
            .unwrap();

        let innermost = engine.block_stack.last().unwrap();
        let logged = innermost
            .lines
            .iter()
            .find(|l| l.content.contains("log::debug"));
        assert!(logged.is_some(), "log::debug line should exist");
        assert_eq!(
            logged.unwrap().taps,
            16,
            "log::debug should have 16 spaces indent"
        );

        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();

        let file = engine.file.as_ref().unwrap();
        assert!(file
            .lines
            .iter()
            .any(|l| l.content.contains("metrics::increment_counter")));
        assert!(file
            .lines
            .iter()
            .any(|l| l.content.contains("item.process()")));
    }

    #[test]
    fn test_nested_multi_operation_inner_block() {
        let content = [
            "fn handler() {",
            "    let config = load_config();",
            "    let mut buffer = Vec::new();",
            "    process_data(&mut buffer);",
            "    let result = finalize(buffer);",
            "    log_result(&result);",
            "}",
        ]
        .join("\n");
        let tmp = create_temp_file(&content);
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["fn handler() {"]), false, None)
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["    let mut buffer = Vec::new();"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["    buffer.reserve(1024);"]),
            )
            .unwrap();

        engine.execute_off(&OffTarget::Location).unwrap();

        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();
        engine
            .execute_location(
                &make_location_content(&["    process_data(&mut buffer);"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_delete(
                false,
                Some(&make_delete_content(&["    process_data(&mut buffer);"])),
                None,
            )
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&[
                    "    validate_buffer(&buffer);",
                    "    transform_data(&mut buffer);",
                ]),
            )
            .unwrap();

        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();

        let file = engine.file.as_ref().unwrap();
        let contents: Vec<&str> = file.lines.iter().map(|l| l.content.as_str()).collect();
        let joined = contents.join("\n");

        assert!(joined.contains("buffer.reserve(1024)"));
        assert!(joined.contains("validate_buffer"));
        assert!(joined.contains("transform_data"));
        assert!(!joined.contains("process_data(&mut buffer)"));
        assert!(joined.contains("fn handler() {"));
        assert!(joined.contains("log_result(&result)"));
    }

    #[test]
    fn test_nested_location_block_delete_and_new() {
        let content = [
            "impl Calculator {",
            "    fn add(&self, a: i32, b: i32) -> i32 {",
            "        a + b",
            "    }",
            "    fn old_method(&self) {",
            "        self.deprecated_work();",
            "        self.cleanup();",
            "    }",
            "    fn multiply(&self, a: i32, b: i32) -> i32 {",
            "        a * b",
            "    }",
            "}",
        ]
        .join("\n");
        let tmp = create_temp_file(&content);
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["impl Calculator {"]), false, None)
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&[
                    "    fn old_method(&self) {",
                    "        self.deprecated_work();",
                ]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_delete(
                false,
                Some(&make_delete_content(&[
                    "    fn old_method(&self) {",
                    "        self.deprecated_work();",
                    "        self.cleanup();",
                    "    }",
                ])),
                None,
            )
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&[
                    "    fn subtract(&self, a: i32, b: i32) -> i32 {",
                    "        a - b",
                    "    }",
                ]),
            )
            .unwrap();

        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();

        let file = engine.file.as_ref().unwrap();
        let contents: Vec<&str> = file.lines.iter().map(|l| l.content.as_str()).collect();
        let joined = contents.join("\n");

        assert!(!joined.contains("fn old_method"));
        assert!(!joined.contains("deprecated_work"));
        assert!(joined.contains("fn subtract"));
        assert!(joined.contains("fn add"));
        assert!(joined.contains("fn multiply"));
    }

    #[test]
    fn test_nested_diff_lines_tracks_all_changes() {
        let tmp = create_temp_file("fn run() {\n    let x = old_calc();\n    let y = x + 1;\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&["fn run() {"]), false, None)
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["    let x = old_calc();"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_delete(
                false,
                Some(&make_delete_content(&["    let x = old_calc();"])),
                None,
            )
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["    let x = new_calc();", "    debug_assert!(x >= 0);"]),
            )
            .unwrap();

        engine
            .execute_location(
                &make_location_content(&["    debug_assert!(x >= 0);"]),
                false,
                None,
            )
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["    log::info!(\"x = {}\", x);"]),
            )
            .unwrap();

        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();

        let added: Vec<_> = engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == DiffLineKind::Added)
            .collect();
        let deleted: Vec<_> = engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == DiffLineKind::Deleted)
            .collect();

        assert_eq!(deleted.len(), 1, "Should have 1 deleted line");
        assert!(deleted[0].content.contains("old_calc"));
        assert_eq!(added.len(), 3, "Should have 3 added lines");
        assert!(added.iter().any(|d| d.content.contains("new_calc")));
        assert!(added.iter().any(|d| d.content.contains("debug_assert")));
        assert!(added.iter().any(|d| d.content.contains("log::info")));
    }

    // ============================================================
    // Phase 5: 行号 Location 测试
    // ============================================================

    #[test]
    fn test_line_number_location_basic() {
        let tmp = create_temp_file("line 1\nline 2\nline 3\nline 4\nline 5\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        let line_range = crate::model::LineRange { start: 2, end: 4 };
        engine
            .execute_location(&make_location_content(&[]), false, Some(&line_range))
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        assert_eq!(block.lines.len(), 3);
        assert_eq!(block.lines[0].content, "line 2");
        assert_eq!(block.lines[1].content, "line 3");
        assert_eq!(block.lines[2].content, "line 4");
        assert_eq!(block.start_line, 2);
        assert_eq!(block.end_line, 4);
    }

    #[test]
    fn test_line_number_location_single_line() {
        let tmp = create_temp_file("a\nb\nc\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        let line_range = crate::model::LineRange { start: 1, end: 1 };
        engine
            .execute_location(&make_location_content(&[]), false, Some(&line_range))
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        assert_eq!(block.lines.len(), 1);
        assert_eq!(block.lines[0].content, "a");
        assert_eq!(block.start_line, 1);
    }

    #[test]
    fn test_line_number_location_beyond_range_errors() {
        let tmp = create_temp_file("a\nb\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        let line_range = crate::model::LineRange { start: 10, end: 10 };
        let result = engine.execute_location(&make_location_content(&[]), false, Some(&line_range));
        assert!(result.is_err());
    }

    #[test]
    fn test_line_number_location_then_new() {
        let tmp = create_temp_file("fn main() {\n    old_a();\n    old_b();\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        let line_range = crate::model::LineRange { start: 2, end: 2 };
        engine
            .execute_location(&make_location_content(&[]), false, Some(&line_range))
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["    new_code();"]),
            )
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        assert!(block.lines.iter().any(|l| l.content == "    new_code();"));
    }

    #[test]
    fn test_line_number_location_nested() {
        let tmp = create_temp_file(
            "fn outer() {\n    fn inner() {\n        let a = 1;\n        let b = 2;\n    }\n    let c = 3;\n}\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        // 先用行号定位到 outer 函数
        let outer_range = crate::model::LineRange { start: 1, end: 7 };
        engine
            .execute_location(&make_location_content(&[]), false, Some(&outer_range))
            .unwrap();

        // 在 outer block 内用行号定位到 inner 函数
        let inner_range = crate::model::LineRange { start: 2, end: 5 };
        engine
            .execute_location(&make_location_content(&[]), false, Some(&inner_range))
            .unwrap();

        let inner_block = engine.block_stack.last().unwrap();
        assert_eq!(inner_block.start_line, 2);
        assert_eq!(inner_block.end_line, 5);
        assert!(inner_block
            .lines
            .iter()
            .any(|l| l.content.contains("fn inner()")));
    }

    // ============================================================
    // Phase 5: 行号 Delete 测试
    // ============================================================

    #[test]
    fn test_line_number_delete_basic() {
        let tmp = create_temp_file("fn main() {\n    old_a();\n    old_b();\n    old_c();\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        // 先用空 Location 获取整个文件
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        let line_range = crate::model::LineRange { start: 2, end: 3 };
        engine
            .execute_delete(false, None, Some(&line_range))
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        let contents: Vec<&str> = block.lines.iter().map(|l| l.content.as_str()).collect();
        assert!(contents.contains(&"fn main() {"));
        assert!(contents.contains(&"    old_c();"));
        assert!(contents.contains(&"}"));
        assert!(!contents.contains(&"    old_a();"));
        assert!(!contents.contains(&"    old_b();"));
    }

    #[test]
    fn test_line_number_delete_produces_diff_lines() {
        let tmp = create_temp_file("line1\nline2\nline3\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        let line_range = crate::model::LineRange { start: 2, end: 2 };
        engine
            .execute_delete(false, None, Some(&line_range))
            .unwrap();

        let deleted: Vec<_> = engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == DiffLineKind::Deleted)
            .collect();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].kind, DiffLineKind::Deleted);
        assert_eq!(deleted[0].content, "line2");
    }

    #[test]
    fn test_line_number_delete_beyond_range_errors() {
        let tmp = create_temp_file("a\nb\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        let line_range = crate::model::LineRange {
            start: 100,
            end: 100,
        };
        let result = engine.execute_delete(false, None, Some(&line_range));
        assert!(result.is_err());
    }

    // ============================================================
    // Phase 5: Raw 内容经过 Parser 融入 New 测试
    // ============================================================

    #[test]
    fn test_raw_content_in_new_via_parser() {
        // 测试 Raw 命令通过 Parser 融入 NewContent 后，is_raw 行内容被提取
        let tmp = create_temp_file("fn main() {\n    println!(\"hi\");\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&["fn main() {"]), false, None)
            .unwrap();

        // 手动构建带 is_raw=true 的 NewContent 模拟 Parser 处理 Raw 后的结果
        let raw_new_line = NewLine {
            diff_taps: 0,
            content: "...".to_string(),
            is_raw: true,
        };
        let new_content = NewContent {
            lines: vec![raw_new_line],
        };
        engine
            .execute_new(&NewPosition::Normal, &new_content)
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        assert!(block.lines.iter().any(|l| l.content == "..."));
    }

    // ============================================================
    // Phase 5: 行号 Location/Delete 多样性测试
    // ============================================================

    /// 测试：行号定位后 ContentBlock 的 start_line/end_line 正确对应原文行号
    #[test]
    fn test_line_range_content_block_boundaries() {
        let tmp = create_temp_file(
            "// L1: header\nfn main() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n}\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        let line_range = crate::model::LineRange { start: 3, end: 5 };
        engine
            .execute_location(&make_location_content(&[]), false, Some(&line_range))
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        assert_eq!(block.start_line, 3);
        assert_eq!(block.end_line, 5);
        assert_eq!(block.lines.len(), 3);
        assert_eq!(block.lines[0].content, "    let a = 1;");
        assert_eq!(block.lines[1].content, "    let b = 2;");
        assert_eq!(block.lines[2].content, "    let c = 3;");
    }

    /// 测试：行号 Delete 后 ContentBlock 中行号重新计算正确
    #[test]
    fn test_line_range_delete_reindexes_correctly() {
        let tmp = create_temp_file("A\nB\nC\nD\nE\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        engine
            .execute_delete(
                false,
                None,
                Some(&crate::model::LineRange { start: 2, end: 3 }),
            )
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        assert_eq!(block.lines.len(), 3);
        assert_eq!(block.lines[0].content, "A");
        assert_eq!(block.lines[0].line_num, 1);
        assert_eq!(block.lines[1].content, "D");
        assert_eq!(block.lines[1].line_num, 2);
        assert_eq!(block.lines[2].content, "E");
        assert_eq!(block.lines[2].line_num, 3);
    }

    /// 测试：行号 Location + New 在同一行号范围后的插入位置
    #[test]
    fn test_line_range_location_then_new_inserts_after_last_matched() {
        let tmp = create_temp_file("fn main() {\n    old1();\n    old2();\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        // 先用空 Location 获取整个文件
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        // 在整个文件 block 内用行号定位到第2行
        engine
            .execute_location(
                &make_location_content(&[]),
                false,
                Some(&crate::model::LineRange { start: 2, end: 2 }),
            )
            .unwrap();

        let inner_block = engine.block_stack.last().unwrap();
        // 行号定位的 block 只包含指定行
        assert_eq!(inner_block.lines.len(), 1);
        assert_eq!(inner_block.lines[0].content, "    old1();");

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["    new_after_old1();"]),
            )
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        let pos: Vec<_> = block.lines.iter().map(|l| l.content.as_str()).collect();
        assert_eq!(pos[0], "    old1();");
        assert_eq!(pos[1], "    new_after_old1();");
    }

    /// 测试：行号范围 Delete + New 组合（替换）
    #[test]
    fn test_line_range_delete_then_new_replace() {
        let tmp = create_temp_file("fn main() {\n    a();\n    b();\n    c();\n}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        engine
            .execute_delete(
                false,
                None,
                Some(&crate::model::LineRange { start: 2, end: 3 }),
            )
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["    x();", "    y();"]),
            )
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        let lines: Vec<&str> = block.lines.iter().map(|l| l.content.as_str()).collect();
        assert_eq!(
            lines,
            vec!["fn main() {", "    x();", "    y();", "    c();", "}"]
        );
    }

    /// 测试：行号 Location 删除首行
    #[test]
    fn test_line_range_delete_first_line() {
        let tmp = create_temp_file("first\nsecond\nthird\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        engine
            .execute_delete(
                false,
                None,
                Some(&crate::model::LineRange { start: 1, end: 1 }),
            )
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        assert_eq!(block.lines.len(), 2);
        assert_eq!(block.lines[0].content, "second");
        assert_eq!(block.lines[1].content, "third");
        let deleted: Vec<_> = engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == DiffLineKind::Deleted)
            .collect();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].content, "first");
    }

    /// 测试：行号 Delete 最后一行
    #[test]
    fn test_line_range_delete_last_line() {
        let tmp = create_temp_file("first\nsecond\nthird\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        engine
            .execute_delete(
                false,
                None,
                Some(&crate::model::LineRange { start: 3, end: 3 }),
            )
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        assert_eq!(block.lines.len(), 2);
        assert_eq!(block.lines[0].content, "first");
        assert_eq!(block.lines[1].content, "second");
    }

    /// 测试：Location:Block + 行号，BlockParser 扩展行号范围
    #[test]
    fn test_line_range_location_block_expands_range() {
        let tmp = create_temp_file(
            "// L1\nfn process() {\n    do_a();\n    do_b();\n}\n// L6\nfn other() {}\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        // 行号指向函数签名行，Block 解析器应扩展到整个函数体
        engine
            .execute_location(
                &make_location_content(&[]),
                true,
                Some(&crate::model::LineRange { start: 2, end: 2 }),
            )
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        assert_eq!(block.start_line, 2);
        assert_eq!(block.end_line, 5);
        assert_eq!(block.lines.len(), 4);
        assert_eq!(block.lines[0].content, "fn process() {");
        assert_eq!(block.lines[3].content, "}");
    }

    /// 测试：Location:Block + 行号 + New 在 Block 后插入
    #[test]
    fn test_line_range_location_block_then_new_inserts_after_block() {
        let tmp = create_temp_file("fn old_func() {\n    old_code();\n}\nfn keep_func() {}\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        // 先用空 Location 获取整个文件
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        // 在整个文件中定位 old_func Block
        engine
            .execute_location(
                &make_location_content(&[]),
                true,
                Some(&crate::model::LineRange { start: 1, end: 1 }),
            )
            .unwrap();

        engine
            .execute_new(
                &NewPosition::Normal,
                &make_new_content(&["fn new_func() {", "    new_code();", "}"]),
            )
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        let lines: Vec<&str> = block.lines.iter().map(|l| l.content.as_str()).collect();
        assert_eq!(lines[0], "fn old_func() {");
        assert_eq!(lines[1], "    old_code();");
        assert_eq!(lines[2], "}");
        // new function 插入在 Block 之后
        assert_eq!(lines[3], "fn new_func() {");
        assert_eq!(lines[4], "    new_code();");
        assert_eq!(lines[5], "}");

        // 写回父级 block 验证
        engine.execute_off(&OffTarget::Location).unwrap();

        let outer = engine.block_stack.last().unwrap();
        let outer_lines: Vec<&str> = outer.lines.iter().map(|l| l.content.as_str()).collect();
        assert!(outer_lines.contains(&"fn new_func() {"));
        assert!(outer_lines.contains(&"fn keep_func() {}"));
    }

    /// 测试：行号 Location:Block + Delete:Block 指令的效果
    #[test]
    fn test_line_range_delete_block_via_commands() {
        let tmp = create_temp_file("fn a() {}\nfn remove_me() {\n    dead_code();\n}\nfn c() {}\n");
        // 先获取整个文件作为 scope
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        // 在文件 scope 中用行号+Block 定位 remove_me 函数
        engine
            .execute_location(
                &make_location_content(&[]),
                true,
                Some(&crate::model::LineRange { start: 2, end: 2 }),
            )
            .unwrap();

        // 验证定位到 remove_me block
        let inner = engine.block_stack.last().unwrap();
        assert_eq!(inner.start_line, 2);
        assert!(inner
            .lines
            .iter()
            .any(|l| l.content.contains("fn remove_me")));

        // Delete:Block 删除整个 inner block
        engine.execute_delete_block().unwrap();

        // 写回验证
        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();

        let file = engine.file.as_ref().unwrap();
        let lines: Vec<&str> = file.lines.iter().map(|l| l.content.as_str()).collect();
        assert!(lines.contains(&"fn a() {}"));
        assert!(!lines.contains(&"fn remove_me() {"));
        assert!(!lines.contains(&"    dead_code();"));
        assert!(lines.contains(&"fn c() {}"));
    }

    /// 测试：行号范围结束超过文件长度时自动截断
    #[test]
    fn test_line_range_end_clamps_to_file_end() {
        let tmp = create_temp_file("A\nB\nC\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(
                &make_location_content(&[]),
                false,
                Some(&crate::model::LineRange { start: 2, end: 999 }),
            )
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        assert_eq!(block.start_line, 2);
        assert_eq!(block.end_line, 3);
        assert_eq!(block.lines.len(), 2);
    }

    /// 测试：行号 Location 和 Delete 与嵌套 block 协作
    #[test]
    fn test_line_range_delete_inside_nested_block() {
        let tmp = create_temp_file(
            "impl Foo {\n    fn bar(&self) {\n        let x = 1;\n        let y = 2;\n        let z = 3;\n    }\n}\n",
        );
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        // 顶层定位 impl Foo（整个文件）
        engine
            .execute_location(
                &make_location_content(&[]),
                false,
                Some(&crate::model::LineRange { start: 1, end: 7 }),
            )
            .unwrap();

        // 嵌套定位到 fn bar 内部（在外层 block 中的第2到第6行）
        engine
            .execute_location(
                &make_location_content(&[]),
                false,
                Some(&crate::model::LineRange { start: 2, end: 6 }),
            )
            .unwrap();

        // 内层 block 第3行 = 原文 "        let y = 2;"
        engine
            .execute_delete(
                false,
                None,
                Some(&crate::model::LineRange { start: 3, end: 3 }),
            )
            .unwrap();

        engine.execute_off(&OffTarget::Location).unwrap();
        engine.execute_off(&OffTarget::Location).unwrap();

        let file = engine.file.as_ref().unwrap();
        let lines: Vec<&str> = file.lines.iter().map(|l| l.content.as_str()).collect();
        let joined = lines.join("\n");
        assert!(joined.contains("let x = 1;"));
        assert!(!joined.contains("let y = 2;"));
        assert!(joined.contains("let z = 3;"));
        assert!(joined.contains("fn bar(&self) {"));
        assert!(joined.contains("impl Foo {"));
    }

    /// 测试：多行 delete 的 diff_lines 记录完整
    #[test]
    fn test_line_range_delete_multi_produces_all_diff_lines() {
        let tmp = create_temp_file("1\n2\n3\n4\n5\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();
        engine
            .execute_location(&make_location_content(&[]), false, None)
            .unwrap();

        engine
            .execute_delete(
                false,
                None,
                Some(&crate::model::LineRange { start: 2, end: 4 }),
            )
            .unwrap();

        let deleted: Vec<_> = engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == DiffLineKind::Deleted)
            .collect();
        assert_eq!(deleted.len(), 3);
        assert_eq!(deleted[0].content, "2");
        assert_eq!(deleted[1].content, "3");
        assert_eq!(deleted[2].content, "4");
    }

    /// 测试：只有行号无内容的 Location 解析后 ContentBlock 正确
    #[test]
    fn test_parse_line_range_location_via_commands() {
        let tmp = create_temp_file("// L1\n// L2\n// L3\n// L4\n// L5\n");
        let commands = vec![
            Command::Open {
                file_path: tmp.path.clone(),
            },
            Command::Location {
                block: false,
                line_range: Some(crate::model::LineRange { start: 2, end: 4 }),
                content: make_location_content(&[]),
            },
            Command::New {
                position: NewPosition::Normal,
                content: make_new_content(&["// inserted"]),
            },
            Command::Off {
                target: OffTarget::Open,
            },
        ];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_ok(), "Unexpected error: {:?}", result.err());

        let file = engine.file.as_ref().unwrap();
        let lines: Vec<&str> = file.lines.iter().map(|l| l.content.as_str()).collect();
        // ContentBlock 覆盖 L2-L4(3行) → 插入后 4 行(L2,L3,L4,inserted) → 替换回文件
        // 最终: L1, L2, L3, L4, inserted, L5 = 6 行
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0], "// L1");
        assert_eq!(lines[1], "// L2");
        assert_eq!(lines[2], "// L3");
        assert_eq!(lines[3], "// L4");
        assert_eq!(lines[4], "// inserted");
        assert_eq!(lines[5], "// L5");
    }

    /// 测试：Delete 的行号忽略后面写的匹配内容
    #[test]
    fn test_line_range_delete_ignores_content() {
        let tmp = create_temp_file("fn main() {\n    a();\n    b();\n    c();\n}\n");
        let commands = vec![
            Command::Open {
                file_path: tmp.path.clone(),
            },
            Command::Location {
                block: false,
                line_range: None,
                content: make_location_content(&["fn main() {"]),
            },
            // Delete 使用行号但附带无关内容，应忽略内容用行号
            Command::Delete {
                block: false,
                line_range: Some(crate::model::LineRange { start: 2, end: 2 }),
                content: Some(make_delete_content(&["    unrelated();"])),
            },
            Command::Off {
                target: OffTarget::Open,
            },
        ];

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_ok(), "Unexpected error: {:?}", result.err());

        let file = engine.file.as_ref().unwrap();
        let lines: Vec<&str> = file.lines.iter().map(|l| l.content.as_str()).collect();
        // a() 被行号删除
        assert!(!lines.contains(&"    a();"));
        assert!(lines.contains(&"    b();"));
        assert!(lines.contains(&"fn main() {"));
    }

    /// 测试：行号 Location + Block + 缩进语言（Python 风格）
    #[test]
    fn test_line_range_location_block_python_style() {
        let tmp = create_temp_file("def outer():\n    def inner():\n        pass\n    return 0\n");
        let mut engine = Engine::new();
        engine.execute_open(&tmp.path).unwrap();

        engine
            .execute_location(
                &make_location_content(&[]),
                true,
                Some(&crate::model::LineRange { start: 2, end: 2 }),
            )
            .unwrap();

        let block = engine.block_stack.last().unwrap();
        // 缩进语言：Block 应包含第一行和缩进更深的后行
        assert_eq!(block.start_line, 2);
        assert_eq!(block.end_line, 3);
        assert_eq!(block.lines.len(), 2);
        assert_eq!(block.lines[0].content, "    def inner():");
    }
}
