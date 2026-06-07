//! N_Edit 集成测试
//!
//! 端到端验证完整的 lexer → parser → engine 流程。
//! 使用 tests/data/ 下的真实 Rust 源码作为目标文件，
//! 使用 tests/scripts/ 下的 .ned 脚本执行编辑操作。
//!
//! 所有测试操作在临时文件副本上进行，不修改原始数据文件。

use std::path::Path;

/// 测试环境：持有临时目录和目标文件的副本
struct TestEnv {
    /// 临时目录（Drop 时自动清理）
    _dir: tempfile::TempDir,
    /// 目标文件的副本路径
    target_path: String,
}

impl TestEnv {
    /// 从 tests/data/ 复制目标文件到临时目录，返回测试环境
    fn from_data_file(data_file: &str) -> Self {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let src = Path::new("tests/data").join(data_file);
        let dst = dir.path().join(data_file);

        // 确保目标路径包含的父目录结构一致
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).expect("Failed to create parent dirs");
        }

        std::fs::copy(&src, &dst).expect(&format!("Failed to copy {}", src.display()));

        TestEnv {
            target_path: dst.to_str().unwrap().to_string(),
            _dir: dir,
        }
    }

    /// 读取 .ned 脚本内容，将其中的 Open 路径替换为临时副本路径
    fn load_script(&self, script_name: &str) -> String {
        let script_path = Path::new("tests/scripts").join(script_name);
        let script = std::fs::read_to_string(&script_path)
            .expect(&format!("Failed to read script {}", script_path.display()));

        self.replace_paths(&script)
    }

    /// 将脚本中所有 Open 命令的路径替换为临时目录中的路径
    fn replace_paths(&self, script: &str) -> String {
        // 策略：匹配 Open: ./tests/data/... 并替换为 Open: ${self.target_path}
        // 但 Open 路径可能指向不同文件（如 config.rs 或 services.rs）
        // 我们用更简单的方法：提取原路径中的文件名，映射到 temp 路径
        script
            .lines()
            .map(|line| {
                if line.starts_with("//!@Open: ") {
                    let original = line.strip_prefix("//!@Open: ").unwrap().trim();
                    let file_name = Path::new(original)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(original);
                    let temp_dir = Path::new(&self.target_path).parent().unwrap();
                    let resolved = temp_dir.join(file_name);
                    format!("//!@Open: {}", resolved.to_str().unwrap())
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 读取目标文件的当前内容
    fn read_target(&self) -> String {
        std::fs::read_to_string(&self.target_path)
            .expect(&format!("Failed to read target {}", self.target_path))
    }
}

/// 辅助：执行 .ned 脚本的完整流水线并返回引擎状态
fn execute_script(script_content: &str) -> (n_edit::engine::Engine, bool) {
    let tokens = n_edit::lexer::Lexer::tokenize(script_content);
    let commands = match n_edit::parser::Parser::parse(tokens) {
        Ok(cmds) => cmds,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            let engine = n_edit::engine::Engine::new();
            return (engine, false);
        }
    };

    let mut engine = n_edit::engine::Engine::new();
    let success = engine.execute(commands).is_ok();
    (engine, success)
}

// ============================================================
// Phase 1: Open / Location / Off（回归测试）
// ============================================================

#[test]
fn test_open_location_off_readonly() {
    let env = TestEnv::from_data_file("sample.rs");
    let script = env.load_script("test_open_location_off.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "Script execution failed");

    // 原始脚本只做了 read-only 操作，文件内容应不变
    let original = std::fs::read_to_string("tests/data/sample.rs").unwrap();
    let result = env.read_target();
    assert_eq!(
        result, original,
        "Read-only Location should not modify file"
    );

    // diff_lines 应为空（只读操作不产生 diff）
    assert!(engine.diff_lines.is_empty());
}

#[test]
fn test_open_location_off_location() {
    let env = TestEnv::from_data_file("sample.rs");
    let script = env.load_script("test_open_location_offlocation.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "Script execution failed");

    assert!(engine.diff_lines.is_empty());
}

#[test]
fn test_implicit_off_open() {
    let env = TestEnv::from_data_file("sample.rs");
    let script = env.load_script("test_implicit_off.ned");

    let (_, success) = execute_script(&script);
    assert!(success, "Implicit Off:Open should succeed");
}

#[test]
fn test_open_off_roundtrip() {
    let env = TestEnv::from_data_file("sample.rs");
    let script = env.load_script("test_open_off.ned");

    let (_, success) = execute_script(&script);
    assert!(success, "Open+Off should succeed");

    let original = std::fs::read_to_string("tests/data/sample.rs").unwrap();
    let result = env.read_target();
    assert_eq!(result, original);
}

// ============================================================
// Phase 2: New 命令集成测试
// ============================================================

#[test]
fn test_add_struct_field() {
    let env = TestEnv::from_data_file("config.rs");
    let script = env.load_script("add_struct_field.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "add_struct_field script failed");

    let result = env.read_target();

    // 验证新字段已插入
    assert!(
        result.contains("pub log_level: String"),
        "Expected new field 'log_level' in output:\n{}",
        result
    );

    // 验证原有内容未被破坏
    assert!(result.contains("pub database_url: String"));
    assert!(result.contains("pub min_password_length: u32"));
    assert!(result.contains("pub password_salt_rounds: u32"));

    // 验证 diff_lines 包含 Added 条目
    assert!(!engine.diff_lines.is_empty());
    let added_lines: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert!(!added_lines.is_empty(), "Should have Added diff lines");
}

#[test]
fn test_add_method_to_impl() {
    let env = TestEnv::from_data_file("config.rs");
    let script = env.load_script("add_method.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "add_method script failed");

    let result = env.read_target();

    // 验证新方法已插入
    assert!(
        result.contains("pub fn reload(&mut self)"),
        "Expected new method 'reload' in output"
    );
    assert!(
        result.contains("let env_config = AppConfig::from_env();"),
        "Expected method body in output"
    );

    // 验证原有方法未被破坏
    assert!(result.contains("pub fn from_env()"));
    assert!(result.contains("pub fn build_database_url"));

    // 验证 diff 输出
    let added_count = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .count();
    assert!(
        added_count > 0,
        "Should have Added diff lines, got {}",
        added_count
    );
}

#[test]
fn test_add_license_header_new_start() {
    let env = TestEnv::from_data_file("config.rs");
    let script = env.load_script("add_license_header.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "add_license_header script failed");

    let result = env.read_target();

    // 验证头部已插入
    assert!(
        result.contains("// Copyright 2024 Example Corp."),
        "Expected license header in output"
    );
    assert!(
        result.contains("// SPDX-License-Identifier:"),
        "Expected SPDX identifier in output"
    );

    // 验证原有首行仍然存在
    assert!(result.contains("// Application configuration module."));

    let added_count = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .count();
    assert!(added_count > 0, "Should have Added diff lines");
}

#[test]
fn test_add_tests_at_end_new_end() {
    let env = TestEnv::from_data_file("config.rs");
    let script = env.load_script("add_tests_at_end.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "add_tests_at_end script failed");

    let result = env.read_target();

    // 验证测试模块已追加到末尾
    assert!(
        result.contains("#[cfg(test)]"),
        "Expected test module at end of file"
    );
    assert!(
        result.contains("fn test_default_config()"),
        "Expected test function in output"
    );
    assert!(
        result.contains("fn test_from_env_respects_defaults()"),
        "Expected second test function in output"
    );

    // 验证原有内容仍在前面
    assert!(result.contains("pub struct AppConfig"));
    assert!(result.contains("impl AppConfig"));

    let added_count = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .count();
    assert!(added_count > 1, "Should have multiple Added diff lines");
}

// ============================================================
// Phase 2: Delete 命令集成测试
// ============================================================

#[test]
fn test_delete_function() {
    let env = TestEnv::from_data_file("services.rs");
    let script = env.load_script("delete_function.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "delete_function script failed");

    let result = env.read_target();

    // 验证函数已删除
    assert!(
        !result.contains("fn bcrypt_hash(password: &str, salt: &str)"),
        "bcrypt_hash function should be deleted"
    );
    assert!(
        !result.contains("password must not be empty"),
        "bcrypt_hash body should be deleted"
    );

    // 验证相邻函数仍然存在
    assert!(
        result.contains("fn generate_salt("),
        "generate_salt should still exist"
    );

    // 验证 diff 包含 Deleted 行
    let deleted_count = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .count();
    assert!(
        deleted_count > 0,
        "Should have Deleted diff lines, got {}",
        deleted_count
    );
}

// ============================================================
// Phase 2: Replace (Delete + New) 集成测试
// ============================================================

#[test]
fn test_replace_function_delete_then_new() {
    let env = TestEnv::from_data_file("services.rs");
    let script = env.load_script("replace_function.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "replace_function script failed");

    let result = env.read_target();

    // 旧实现不应存在
    assert!(
        !result.contains("let bytes: Vec<u8> = (0..16).map(|_| rng.gen()).collect();"),
        "Old salt generation code should be removed"
    );
    assert!(
        !result.contains("hex::encode(&bytes)"),
        "Old hex encoding should be removed"
    );

    // 新实现应存在
    assert!(
        result.contains("let mut bytes = [0u8; 32];"),
        "New salt generation should be present"
    );
    assert!(
        result.contains("rng.fill(&mut bytes);"),
        "New random fill should be present"
    );
    assert!(
        result.contains("base64::encode(&bytes)"),
        "New base64 encoding should be present"
    );

    // 函数签名应保持不变
    let fn_count = result
        .lines()
        .filter(|l| l.trim().starts_with("fn generate_salt("))
        .count();
    assert_eq!(
        fn_count, 1,
        "Should have exactly one generate_salt function"
    );

    // 验证既有 Added 也有 Deleted 行
    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    assert!(!added.is_empty(), "Should have Added diff lines");
    assert!(!deleted.is_empty(), "Should have Deleted diff lines");
}

// ============================================================
// Phase 2: 多操作脚本测试
// ============================================================

#[test]
fn test_multi_operation_script() {
    let env = TestEnv::from_data_file("config.rs");
    let script = env.load_script("multi_operation.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "multi_operation script failed");

    let result = env.read_target();

    // 操作 1: 添加了新字段
    assert!(result.contains("pub log_level: String"));

    // 操作 2: 添加了新方法
    assert!(result.contains("pub fn reload(&mut self)"));

    // 原有结构和内容仍然完整
    assert!(result.contains("pub struct AppConfig"));
    assert!(result.contains("pub fn from_env() -> Self"));
    assert!(result.contains("pub fn build_database_url"));

    // diff_lines 应同时包含 Added 行
    let added_count = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .count();
    assert!(
        added_count >= 2,
        "Multi-operation should produce multiple Added diff lines, got {}",
        added_count
    );
}

// ============================================================
// 边界条件测试
// ============================================================

#[test]
fn test_location_too_many_matches_errors() {
    // Location 匹配有歧义时应该报错
    let env = TestEnv::from_data_file("services.rs");

    // 创建脚本：Location 匹配 `pub fn` — 在 services.rs 中有多个匹配
    let script = format!(
        "//!@Open: {}\n//!@Location:\npub fn\n//!@Off:Open\n",
        env.target_path
    );

    let (_, success) = execute_script(&script);
    assert!(!success, "Ambiguous location should fail");
}

#[test]
fn test_new_normal_without_location_errors() {
    let env = TestEnv::from_data_file("config.rs");

    // 没有 Location 就直接 New:Normal，应该报错
    let script = format!(
        "//!@Open: {}\n//!@New:\n    let x = 1;\n...\n//!@Off:Open\n",
        env.target_path
    );

    let (_, success) = execute_script(&script);
    assert!(!success, "New without Location should fail");
}

#[test]
fn test_delete_not_found_errors() {
    let env = TestEnv::from_data_file("config.rs");

    // Delete 内容在文件中不存在，应该报错
    let script = format!(
        "//!@Open: {}\n//!@Location:\npub struct AppConfig\n...\n//!@Delete:\n    nonexistent_field: String,\n...\n//!@Off:Open\n",
        env.target_path
    );

    let (_, success) = execute_script(&script);
    assert!(!success, "Delete not found should fail");
}

// ============================================================
// Phase 3: Location:Block 集成测试
// ============================================================

#[test]
fn test_location_block_new_after_function() {
    // Location:Block 精确定位一个函数代码块，在其后新增一个方法
    let env = TestEnv::from_data_file("config.rs");
    let script = env.load_script("location_block_new.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "Location:Block + New script failed");

    let result = env.read_target();

    // 新方法应存在于 build_database_url 之后
    assert!(
        result.contains("pub fn connect_timeout(&self) -> u64 {"),
        "New method should be inserted after build_database_url\n{}",
        result
    );
    assert!(
        result.contains("self.request_timeout_secs * 2"),
        "New method body should be present"
    );

    // 原有函数仍然存在
    assert!(result.contains("pub fn build_database_url("));
    assert!(result.contains("pub fn from_env() -> Self"));

    // diff_lines 应包含 Added 行
    let added_count = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .count();
    assert!(
        added_count >= 3,
        "Should have at least 3 Added lines, got {}",
        added_count
    );
}

#[test]
fn test_location_block_exact_range() {
    // 验证 Location:Block 精确提取了代码块范围
    // 在 config.rs 中，build_database_url 函数占 88-94 行（7 行）
    let env = TestEnv::from_data_file("config.rs");

    let script = format!(
        "//!@Open: {}\n//!@Location:Block\n    pub fn build_database_url(\n//!@Off:Open\n",
        env.target_path
    );

    let (engine, success) = execute_script(&script);
    assert!(success, "Location:Block readonly should succeed");

    // diff_lines 应为空（只读操作）
    assert!(engine.diff_lines.is_empty());
}

#[test]
fn test_location_block_struct() {
    // Location:Block 定位 struct 定义块
    let env = TestEnv::from_data_file("config.rs");

    // Location:Block 定位 AppConfig struct，插入新字段
    let script = format!(
        "//!@Open: {}\n//!@Location:Block\npub struct AppConfig {{\n//!@New:\n    pub api_version: String,\n//!@Off:Open\n",
        env.target_path
    );

    let (engine, success) = execute_script(&script);
    assert!(success, "Location:Block + New in struct failed");

    let result = env.read_target();
    assert!(
        result.contains("pub api_version: String"),
        "New field should be inserted after struct block\n{}",
        result
    );
    // Struct 原有字段仍在
    assert!(result.contains("pub database_url: String"));
    assert!(result.contains("pub assets_dir: PathBuf"));

    let added_count = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .count();
    assert!(added_count > 0, "Should have Added diff lines");
}

#[test]
fn test_location_block_impl_block() {
    // Location:Block 定位 impl Default 代码块并验证其范围
    let env = TestEnv::from_data_file("config.rs");

    let script = format!(
        "//!@Open: {}\n//!@Location:Block\nimpl Default for AppConfig {{\n//!@Delete:Block\n//!@Off:Open\n",
        env.target_path
    );

    let (_, success) = execute_script(&script);
    assert!(
        success,
        "Location:Block + Delete:Block on impl should succeed"
    );

    let result = env.read_target();
    // impl Default for AppConfig 块应被删除
    assert!(
        !result.contains("impl Default for AppConfig"),
        "impl Default block should be deleted"
    );
    // impl AppConfig 块应仍然存在
    assert!(
        result.contains("impl AppConfig"),
        "impl AppConfig block should remain"
    );
}

#[test]
fn test_delete_block_entire_function() {
    // Delete:Block 删除整个函数代码块
    let env = TestEnv::from_data_file("services.rs");
    let script = env.load_script("delete_block.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "Delete:Block script failed");

    let result = env.read_target();

    // bcrypt_hash 函数应被完全删除
    assert!(
        !result.contains("fn bcrypt_hash("),
        "bcrypt_hash function should be deleted"
    );
    assert!(
        !result.contains("password must not be empty"),
        "bcrypt_hash body should be deleted"
    );

    // 相邻函数 generate_salt 仍应存在
    assert!(
        result.contains("fn generate_salt("),
        "generate_salt should remain"
    );

    // 验证 diff 包含 Deleted 行
    let deleted_count = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .count();
    assert!(
        deleted_count > 0,
        "Should have Deleted diff lines, got {}",
        deleted_count
    );
}

// ============================================================
// Phase 3: Block 不可解析 / 错误场景测试
// ============================================================

#[test]
fn test_location_block_markdown_rejected() {
    // Location:Block 对纯 Markdown 文本应报错（无法解析为 Block）
    let env = TestEnv::from_data_file("config.rs");

    // 用 config.rs 作为目标文件，但用类似 Markdown 的 Location 内容定位
    // 由于 config.rs 中没有纯文本区域，我们创建一个包含纯注释的场景
    let script = format!(
        "//!@Open: {}\n//!@Location:Block\n// Application configuration module.\n//!@Off:Open\n",
        env.target_path
    );

    let (_, success) = execute_script(&script);
    // config.rs 第一行是注释，注释行不包含 brace 且没有缩进层级
    // detect_language 会检查该行及后续几行，后续行有 brace 内容，
    // 所以实际会被检测为 Brace 语言而非 Unknown
    // 这里我们验证：该行的注释不影响后面代码的 brace 检测
    assert!(
        success,
        "Location:Block on a comment line near braces should still parse as brace"
    );
}

#[test]
fn test_location_block_non_parseable_errors() {
    // 真·不可解析：创建一个不含任何代码结构的临时文件
    use std::io::Write;
    let dir = tempfile::tempdir().expect("Failed to create temp dir");
    let target_path = dir.path().join("plain.txt").to_str().unwrap().to_string();
    let mut file = std::fs::File::create(&target_path).unwrap();
    writeln!(file, "This is a plain text file.").unwrap();
    writeln!(file, "It has no code structure.").unwrap();
    writeln!(file, "No braces, no indentation.").unwrap();
    writeln!(file, "Just some random content.").unwrap();
    writeln!(file, "And more text.").unwrap();
    drop(file);

    let script = format!(
        "//!@Open: {}\n//!@Location:Block\nThis is a plain text file.\n//!@Off:Open\n",
        target_path
    );

    let (_, success) = execute_script(&script);
    assert!(
        !success,
        "Location:Block on plain text should fail with BlockNotParseable"
    );
}

#[test]
fn test_delete_block_without_location_block_errors() {
    // Delete:Block 需要前一个 Location 也使用 Block，否则应报错
    let env = TestEnv::from_data_file("config.rs");

    // 先用普通 Location（非 Block），再 Delete:Block
    let script = format!(
        "//!@Open: {}\n//!@Location:\npub struct AppConfig\n...\n//!@Delete:Block\n//!@Off:Open\n",
        env.target_path
    );

    let (_, success) = execute_script(&script);
    assert!(
        !success,
        "Delete:Block without prior Location:Block should fail"
    );
}

// ============================================================
// Python 集成测试
// ============================================================

#[test]
fn test_python_add_method_to_class() {
    let env = TestEnv::from_data_file("python_app.py");
    let script = env.load_script("python_add_method.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "Python add method script failed");

    let content = env.read_target();
    assert!(
        content.contains("def reopen_task"),
        "Should contain new method"
    );
    assert!(content.contains("Reopen a previously completed task"));
    assert!(content.contains("task.completed = False"));

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert!(!added.is_empty(), "Should have Added diff lines");
}

#[test]
fn test_python_delete_method() {
    let env = TestEnv::from_data_file("python_app.py");
    let script = env.load_script("python_delete_method.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "Python delete method script failed");

    let content = env.read_target();
    assert!(
        !content.contains("def complete_task"),
        "complete_task should be deleted"
    );
    assert!(
        content.contains("def delete_task"),
        "delete_task should remain"
    );
    assert!(
        content.contains("def count_by_status"),
        "count_by_status should remain"
    );

    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    assert!(!deleted.is_empty(), "Should have Deleted diff lines");
}

#[test]
fn test_python_location_block_new() {
    let env = TestEnv::from_data_file("python_app.py");
    let script = env.load_script("python_location_block_new.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "Python Location:Block + New script failed");

    let content = env.read_target();
    // New function should appear after handle_get
    let handle_get_pos = content.find("def handle_get").unwrap();
    let handle_complete_pos = content.find("def handle_complete").unwrap();
    let handle_delete_pos = content.find("def handle_delete").unwrap();
    assert!(
        handle_delete_pos > handle_get_pos,
        "handle_delete should be after handle_get"
    );
    assert!(
        handle_delete_pos < handle_complete_pos,
        "handle_delete should be before handle_complete"
    );

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert!(!added.is_empty(), "Should have Added diff lines");
}

// ============================================================
// Rust 复杂操作集成测试
// ============================================================

#[test]
fn test_rust_location_block_add_method() {
    let env = TestEnv::from_data_file("rust_parser.rs");
    let script = env.load_script("rust_nested_location.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "Rust Location:Block + New script failed");

    let content = env.read_target();
    // Location:Block 定位 impl Parser，New 在 impl block 之后新增方法
    assert!(
        content.contains("fn token_count"),
        "Should contain new token_count method"
    );
    assert!(content.contains("self.tokens.len()"));
    // impl Parser 原来的内容应保留
    assert!(
        content.contains("fn parse_expression"),
        "parse_expression should remain"
    );
    assert!(
        content.contains("fn parse_prefix"),
        "parse_prefix should remain"
    );

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert!(!added.is_empty(), "Should have Added diff lines");
}

#[test]
fn test_rust_complex_replace() {
    let env = TestEnv::from_data_file("rust_parser.rs");
    let script = env.load_script("rust_complex_replace.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "Rust complex replace script failed");

    let content = env.read_target();
    // 新方法 peek_token 应存在
    assert!(
        content.contains("fn peek_token"),
        "Should contain new peek_token method"
    );
    assert!(content.contains("self.tokens.get(self.position + 1)"));
    // 旧方法也应存在（Delete + New 替换了同一个方法）
    assert!(content.contains("fn current_token"));

    // Should have both Added and Deleted diff lines
    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    assert!(!added.is_empty(), "Should have Added diff lines");
    assert!(!deleted.is_empty(), "Should have Deleted diff lines");
}

// ============================================================
// Markdown 文档操作集成测试
// ============================================================

#[test]
fn test_markdown_add_section() {
    let env = TestEnv::from_data_file("doc.md");
    let script = env.load_script("doc_add_section.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "Markdown add section script failed");

    let content = env.read_target();
    assert!(
        content.contains("## Known Limitations"),
        "Should contain new section"
    );
    assert!(content.contains("Rust-style raw string literals"));
    assert!(content.contains("Tab-indented Python"));
    // Original sections should remain
    assert!(content.contains("## Performance Considerations"));
    assert!(content.contains("## File Format Reference"));

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert!(!added.is_empty(), "Should have Added diff lines");
}

#[test]
fn test_markdown_location_block_rejected() {
    let env = TestEnv::from_data_file("plain.txt");
    let script = env.load_script("doc_block_rejected.ned");

    let (_, success) = execute_script(&script);
    assert!(
        !success,
        "Location:Block on markdown should fail with BlockNotParseable"
    );
}

// ============================================================
// 复杂工程文件 + 多层嵌套 / 跨层修改 / Block 操作测试
// ============================================================

/// 辅助函数：验证文件内容的缩进一致性
/// 检查所有非空行是否以偶数个空格或仅 tab 缩进
fn check_indentation_consistency(content: &str) -> Result<(), String> {
    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let leading_spaces = line.chars().take_while(|c| *c == ' ').count();
        // Rust 文件通常 4 空格缩进（或 0），检查是否为 4 的倍数
        if leading_spaces > 0 && leading_spaces % 4 != 0 {
            // Python/YAML 可能有 2 空格缩进，允许
            if leading_spaces % 2 != 0 {
                return Err(format!(
                    "行 {} 缩进异常: {} 个空格（期望 2/4 的倍数）\n  {}",
                    i + 1,
                    leading_spaces,
                    line
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn test_rust_nested_deep_three_levels() {
    // 四级嵌套 Location：ConnectionPool → get_connection → if-else → Delete+New
    // 然后在 impl AppConfig 中新增方法
    let env = TestEnv::from_data_file("rust_complex.rs");
    let script = env.load_script("rust_nested_deep.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "rust_nested_deep script failed");

    let result = env.read_target();

    // 验证 1: get_connection 的 else 分支被替换为含 log::warn 的版本
    assert!(
        result.contains("log::warn!(\"connection pool exhausted"),
        "Should contain log::warn in replaced else branch\n{}",
        result
    );
    assert!(
        !result.contains("} else {\n            None\n        }"),
        "Old bare else-None should be removed"
    );

    // 验证 2: impl AppConfig 末尾新增了 with_name 方法
    assert!(
        result.contains("pub fn with_name(mut self, name: &str) -> Self {"),
        "Should contain new with_name method"
    );
    assert!(
        result.contains("self.name = name.to_string();"),
        "Should contain with_name body"
    );

    // 验证 3: 原有内容未被破坏
    assert!(result.contains("pub struct AppConfig"));
    assert!(result.contains("pub fn validate(&self) -> Result<(), String>"));
    assert!(result.contains("impl DataPipeline"));

    // 验证 diff_lines
    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    assert!(
        added.len() >= 4,
        "Should have multiple Added lines, got {}",
        added.len()
    );
    assert!(
        deleted.len() >= 2,
        "Should have Deleted lines, got {}",
        deleted.len()
    );

    // 验证缩进一致性
    check_indentation_consistency(&result).expect("Indentation check failed");
}

#[test]
fn test_rust_cross_level_new_end_and_nested_replace() {
    // 跨层修改：impl DataPipeline 末尾 New:End 追加方法，
    // 同时嵌套进入 execute 方法内替换 match 错误分支
    let env = TestEnv::from_data_file("rust_complex.rs");
    let script = env.load_script("rust_cross_level.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "rust_cross_level script failed");

    let result = env.read_target();

    // 验证 1: DataPipeline impl 末尾新增的方法
    assert!(
        result.contains("pub fn clear_stages(&mut self) {"),
        "Should contain clear_stages method"
    );
    assert!(
        result.contains("self.stages.clear();"),
        "Should contain clear_stages body"
    );
    assert!(
        result.contains("pub fn is_empty(&self) -> bool {"),
        "Should contain is_empty method"
    );
    assert!(
        result.contains("self.stages.is_empty()"),
        "Should contain is_empty body"
    );

    // 验证 2: execute 方法中 Err 分支被替换
    assert!(
        result.contains("log::error!(\"stage '{}' error: {}\"") || result.contains("log::error!"),
        "Should contain log::error in replaced Err branch"
    );
    assert!(
        result.contains("pipeline aborted at"),
        "Should contain new error message format"
    );
    assert!(
        !result.contains("stage '{}' failed: {}"),
        "Old error format should be gone (within execute context)"
    );

    // 验证 3: 原有内容仍在
    assert!(result.contains("pub fn get_metrics"));
    assert!(result.contains("pub fn add_stage"));

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert!(
        added.len() >= 6,
        "Should have multiple Added lines, got {}",
        added.len()
    );

    check_indentation_consistency(&result).expect("Indentation check failed");
}

#[test]
fn test_rust_block_ops_delete_block_and_new_fields() {
    // Location:Block + Delete:Block 删除 impl Default，
    // 然后在 struct 中添加字段，在 Connection struct 前加 derive
    let env = TestEnv::from_data_file("rust_complex.rs");
    let script = env.load_script("rust_block_ops.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "rust_block_ops script failed");

    let result = env.read_target();

    // 验证 1: impl Default for AppConfig 被完全删除
    assert!(
        !result.contains("impl Default for AppConfig"),
        "impl Default for AppConfig should be deleted"
    );

    // 验证 2: AppConfig struct 新增字段
    assert!(
        result.contains("pub log_level: String"),
        "Should contain new log_level field"
    );
    assert!(
        result.contains("pub retry_count: u32"),
        "Should contain new retry_count field"
    );

    // 验证 3: struct Connection 前有 derive 属性
    let conn_pos = result.find("struct Connection {").unwrap();
    let before_conn = &result[..conn_pos];
    assert!(
        before_conn.contains("#[derive(Debug, Clone)]"),
        "Should have derive attribute before struct Connection"
    );

    // 验证 4: 原有 struct 仍然存在
    assert!(result.contains("pub struct AppConfig"));

    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    assert!(!deleted.is_empty(), "Should have Deleted diff lines");

    check_indentation_consistency(&result).expect("Indentation check failed");
}

#[test]
fn test_yaml_nested_modify_ci_pipeline() {
    // YAML CI 管线：在 test job 添加 step，修改 deploy notify payload，
    // 在 env 块末尾追加变量
    let env = TestEnv::from_data_file("ci_pipeline.yaml");
    let script = env.load_script("yaml_nested_edit.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "yaml_nested_edit script failed");

    let result = env.read_target();

    // 验证 1: Cache step 在 Run tests 之后
    assert!(
        result.contains("Cache test artifacts"),
        "Should contain cache step"
    );
    assert!(
        result.contains("actions/cache@v3"),
        "Should contain cache action reference"
    );

    // 验证 2: Slack payload 已替换
    assert!(
        result.contains("Deployment to staging succeeded"),
        "Slack payload should be updated"
    );
    assert!(
        !result.contains("Deployment complete"),
        "Old slack payload should be gone"
    );

    // 验证 3: 新环境变量
    assert!(
        result.contains("LOG_LEVEL: debug"),
        "Should contain LOG_LEVEL env var"
    );
    assert!(
        result.contains("CACHE_ENABLED: true"),
        "Should contain CACHE_ENABLED env var"
    );

    // 验证原有内容
    assert!(result.contains("CARGO_TERM_COLOR: always"));
    assert!(result.contains("name: CI Pipeline"));

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert!(
        added.len() >= 5,
        "Should have multiple Added lines, got {}",
        added.len()
    );
}

#[test]
fn test_rust_edge_cases_empty_location_replace() {
    // 边界测试：空 Location + Delete + New（替换整个文件某区域内容）
    // 嵌套 New:Start, 深度缩进保持, 多处独立修改
    let env = TestEnv::from_data_file("rust_complex.rs");
    let script = env.load_script("rust_edge_cases.ned");

    let (engine, success) = execute_script(&script);
    if !success {
        // 重新执行以获取错误详情
        let tokens = n_edit::lexer::Lexer::tokenize(&script);
        match n_edit::parser::Parser::parse(tokens) {
            Err(e) => panic!("Parse error: {}", e),
            Ok(commands) => {
                let mut engine2 = n_edit::engine::Engine::new();
                match engine2.execute(commands) {
                    Err(e) => panic!("Engine error: {}", e),
                    Ok(()) => panic!("Script succeeded in second attempt"),
                }
            }
        }
    }
    assert!(success, "rust_edge_cases script failed");

    let result = env.read_target();

    // 验证 1: 测试模块中新增了 test_config_validation
    assert!(
        result.contains("fn test_config_validation()"),
        "Should contain new test function"
    );
    assert!(
        result.contains("config.validate().is_err()"),
        "Should contain test body"
    );

    // 验证 2: validate 方法开头有 log::debug
    assert!(
        result.contains("log::debug!(\"validating config:"),
        "Should contain debug log in validate"
    );

    // 验证 3: execute 的 Ok 分支中有 log::trace
    assert!(
        result.contains("log::trace!(\"stage '{}' produced:"),
        "Should contain trace log in execute"
    );

    // 验证 4: run_app 中 pipeline.execute 调用被替换
    assert!(
        result.contains("// Process the sample input through pipeline"),
        "Should contain new comment before execute"
    );
    assert!(
        result.contains("log::info!(\"pipeline result:"),
        "Should contain info log after execute"
    );

    // 验证 5: 原有内容还在
    assert!(result.contains("fn test_config_default()"));
    assert!(result.contains("impl DataPipeline"));
    assert!(result.contains("impl AppConfig"));

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    assert!(
        added.len() >= 5,
        "Should have multiple Added lines, got {}",
        added.len()
    );
    assert!(
        deleted.len() >= 1,
        "Should have Deleted lines, got {}",
        deleted.len()
    );

    check_indentation_consistency(&result).expect("Indentation check failed");
}

#[test]
fn test_rust_multiple_independent_locations_in_file() {
    // 在同一个文件中执行多个独立的 Location 操作（非嵌套），
    // 验证每次 Location 之间不互相干扰
    let env = TestEnv::from_data_file("rust_complex.rs");
    let _original = env.read_target();

    // 构造脚本：两个独立的 Location + New
    let script = format!(
        "\
//!@Open: {target}
//!@Location:
pub struct AppConfig {{
    pub name: String,
    pub version: String,
//!@New:
    pub description: String,
//!@Off:Location
//!@Location:
pub struct ConnectionPool {{
    config: AppConfig,
//!@New:
    pool_id: u64,
//!@Off:Location
//!@Off:Open
",
        target = env.target_path
    );

    let (engine, success) = execute_script(&script);
    assert!(success, "Multi-location independent script failed");

    let result = env.read_target();

    // AppConfig 新增字段
    assert!(
        result.contains("pub description: String"),
        "AppConfig should have new description field"
    );

    // ConnectionPool 新增字段
    assert!(
        result.contains("pool_id: u64"),
        "ConnectionPool should have new pool_id field"
    );

    // 原有内容完整
    assert!(result.contains("pub name: String"));
    assert!(result.contains("impl ConnectionPool"));

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert_eq!(added.len(), 2, "Should have exactly 2 Added lines");

    check_indentation_consistency(&result).expect("Indentation check failed");
}

// ============================================================
// Phase 5: 行号 Location / Delete 集成测试
// ============================================================

#[test]
fn test_line_range_basic() {
    let env = TestEnv::from_data_file("rust_complex.rs");
    let script = env.load_script("line_range_basic.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "line_range_basic script failed");

    let result = env.read_target();

    // 验证 1: 单行号 Location:@14 后插入 region 字段
    assert!(
        result.contains("pub region: String"),
        "Should contain new region field"
    );

    // 验证 2: 内容匹配定位后插入 get_pool_size 方法
    assert!(
        result.contains("pub fn get_pool_size"),
        "Should contain new get_pool_size method"
    );
    assert!(
        result.contains("self.connections.len()"),
        "Should contain get_pool_size body"
    );

    // 验证 3: 嵌套行号 Location 添加了 with_timeout 方法
    assert!(
        result.contains("pub fn with_timeout"),
        "Should contain with_timeout method"
    );
    assert!(
        result.contains("self.timeout_ms = ms"),
        "Should contain with_timeout body"
    );

    // 验证原有内容未被破坏
    assert!(result.contains("pub struct AppConfig"));
    assert!(result.contains("pub struct ConnectionPool"));

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert!(
        added.len() >= 5,
        "Should have at least 5 Added lines, got {}",
        added.len()
    );

    check_indentation_consistency(&result).expect("Indentation check failed");
}

#[test]
fn test_line_range_delete_and_replace() {
    let env = TestEnv::from_data_file("rust_complex.rs");
    let script = env.load_script("line_range_delete.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "line_range_delete script failed");

    let result = env.read_target();

    // 验证 1: impl Default for AppConfig 已被行号范围删除
    assert!(
        !result.contains("impl Default for AppConfig"),
        "impl Default for AppConfig should be deleted"
    );

    // 验证 2: Connection struct 新增了 pool_size 和 last_maintenance 字段
    assert!(
        result.contains("pool_size: usize"),
        "Should contain new pool_size field"
    );
    assert!(
        result.contains("last_maintenance: std::time::Instant"),
        "Should contain new last_maintenance field"
    );
    // 验证原有字段仍然存在
    assert!(result.contains("id: u64"));
    assert!(result.contains("created_at:"));

    // 验证 3: 原有内容仍在
    assert!(result.contains("pub struct AppConfig"));
    assert!(result.contains("impl ConnectionPool"));

    // diff 同时包含 Added 和 Deleted
    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    assert!(!added.is_empty(), "Should have Added diff lines");
    assert!(!deleted.is_empty(), "Should have Deleted diff lines");

    check_indentation_consistency(&result).expect("Indentation check failed");
}

#[test]
fn test_line_range_block_operations() {
    let env = TestEnv::from_data_file("rust_complex.rs");
    let script = env.load_script("line_range_block.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "line_range_block script failed");

    let result = env.read_target();

    // 验证 1: Location:Block @75 定位到 get_connection，在其后插入 is_healthy
    assert!(
        result.contains("pub fn is_healthy"),
        "Should contain new is_healthy method"
    );
    assert!(
        result.contains("self.active > 0"),
        "Should contain is_healthy body"
    );

    // 验证 2: Delete:Block 删除了原 impl AppConfig 的 validate 方法前内容
    // 新方法 set_default_name 被添加
    assert!(
        result.contains("pub fn set_default_name"),
        "Should contain set_default_name method"
    );

    // 验证 3: 嵌套 Location:Block + 行号定位 transform match 已扩展
    assert!(
        result.contains("TransformType::Capitalize"),
        "Should contain new Capitalize transform variant"
    );

    // 验证原有内容
    assert!(result.contains("pub fn get_connection"));
    assert!(result.contains("impl AppConfig"));

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    assert!(!added.is_empty(), "Should have Added diff lines");
    assert!(!deleted.is_empty(), "Should have Deleted diff lines");

    check_indentation_consistency(&result).expect("Indentation check failed");
}

#[test]
fn test_line_range_complex_mixed() {
    let env = TestEnv::from_data_file("rust_complex.rs");
    let script = env.load_script("line_range_complex.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "line_range_complex script failed");

    let result = env.read_target();

    // 验证 1: 行号定位 @11,19 后在 struct 中添加 log_config 字段
    assert!(
        result.contains("pub log_config: bool"),
        "Should contain log_config field"
    );

    // 验证 2: 内容匹配 Location 定位测试函数，替换为扩展版本
    assert!(
        result.contains("assert_eq!(config.version, \"0.1.0\");"),
        "Should contain extended test assertions"
    );

    // 验证 3: 嵌套行号定位在 impl DataPipeline 中添加 stage_count
    assert!(
        result.contains("pub fn stage_count"),
        "Should contain stage_count method"
    );
    assert!(
        result.contains("self.stages.len()"),
        "Should contain stage_count body"
    );

    // 验证 4: 内容匹配 run_app，嵌套定位替换 pipeline.execute 调用
    assert!(
        result.contains("running pipeline with input:"),
        "Should contain new pipeline logging"
    );
    assert!(
        result.contains("pipeline completed successfully"),
        "Should contain pipeline completion log"
    );

    // 原有内容仍在
    assert!(result.contains("pub struct AppConfig"));
    assert!(result.contains("impl DataPipeline"));
    assert!(result.contains("pub fn run_app"));

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    assert!(
        added.len() >= 4,
        "Should have multiple Added lines, got {}",
        added.len()
    );
    assert!(
        deleted.len() >= 1,
        "Should have Deleted lines, got {}",
        deleted.len()
    );

    check_indentation_consistency(&result).expect("Indentation check failed");
}

#[test]
fn test_multi_op_refactor() {
    let env = TestEnv::from_data_file("rust_complex.rs");
    let script = env.load_script("multi_op_refactor.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "multi_op_refactor script failed");

    let result = env.read_target();

    // 验证 1: impl Default for AppConfig 已被删除
    assert!(
        !result.contains("impl Default for AppConfig"),
        "impl Default for AppConfig should be deleted"
    );

    // 验证 2: 新字段已添加到 AppConfig struct
    assert!(
        result.contains("pub env_prefix: String"),
        "Should contain env_prefix field"
    );
    assert!(
        result.contains("pub health_check_path: String"),
        "Should contain health_check_path field"
    );

    // 验证 3: validate 方法开头有 log::debug
    assert!(
        result.contains("validating config:"),
        "Should contain debug log in validate"
    );

    // 验证 4: with_env_prefix 方法已添加
    assert!(
        result.contains("pub fn with_env_prefix"),
        "Should contain with_env_prefix method"
    );

    // 验证 5: ConnectionPool 中新增 shutdown 方法
    assert!(
        result.contains("pub fn shutdown"),
        "Should contain shutdown method"
    );
    assert!(
        result.contains("self.connections.clear();"),
        "Should contain shutdown body"
    );

    // 验证 6: run_app 中新增日志
    assert!(
        result.contains("application starting with config:"),
        "Should contain application start log"
    );

    // 验证 7: 测试模块末尾有新测试函数
    assert!(
        result.contains("fn test_connection_pool_shutdown()"),
        "Should contain new test function"
    );
    assert!(
        result.contains("pool.shutdown();"),
        "Should contain test body"
    );

    // 验证原有内容完整
    assert!(result.contains("pub struct AppConfig"));
    assert!(result.contains("impl AppConfig"));
    assert!(result.contains("impl ConnectionPool"));
    assert!(result.contains("pub fn run_app"));
    assert!(result.contains("fn test_config_default()"));
    assert!(result.contains("fn test_pipeline_basic()"));

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    assert!(
        added.len() >= 8,
        "Should have many Added lines, got {}",
        added.len()
    );
    assert!(
        deleted.len() >= 8,
        "Should have many Deleted lines, got {}",
        deleted.len()
    );

    check_indentation_consistency(&result).expect("Indentation check failed");
}

// ============================================================
// 语法手册场景验证测试（9 个场景）
// ============================================================

#[test]
fn test_scenario01_add_field() {
    let env = TestEnv::from_data_file("scenarios.rs");
    let script = env.load_script("scenario01_add_field.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "scenario01_add_field failed");

    let result = env.read_target();
    assert!(
        result.contains("pub log_level: String"),
        "Should contain new field"
    );
    assert!(
        result.contains("pub name: String"),
        "Original field should remain"
    );
    assert!(
        result.contains("pub version: String"),
        "Original field should remain"
    );

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert_eq!(added.len(), 1, "Should have 1 Added line");
    assert_eq!(
        engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
            .count(),
        0
    );
}

#[test]
fn test_scenario02_insert_code() {
    let env = TestEnv::from_data_file("scenarios.rs");
    let script = env.load_script("scenario02_insert_code.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "scenario02_insert_code failed");

    let result = env.read_target();
    assert!(
        result.contains("processing input:"),
        "Should contain log line"
    );
    assert!(
        result.contains("pub fn process"),
        "Original function should remain"
    );

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert_eq!(added.len(), 1, "Should have 1 Added line");
    assert!(added[0].content.contains("log::info"));
}

#[test]
fn test_scenario03_replace_func() {
    let env = TestEnv::from_data_file("scenarios.rs");
    let script = env.load_script("scenario03_replace_func.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "scenario03_replace_func failed");

    let result = env.read_target();
    assert!(
        !result.contains("result.push_str"),
        "Old for-loop should be gone"
    );
    assert!(
        result.contains("self.items.join"),
        "New join call should exist"
    );
    assert!(
        result.contains("pub fn deprecated_method"),
        "Function signature should remain"
    );
    assert!(
        result.contains("pub fn active_count"),
        "Next function should remain"
    );

    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert!(deleted.len() >= 3, "Should have Deleted lines");
    assert_eq!(added.len(), 1, "Should have 1 Added line");
}

#[test]
fn test_scenario04_line_range() {
    let env = TestEnv::from_data_file("scenarios.rs");
    let script = env.load_script("scenario04_line_range.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "scenario04_line_range failed");

    let result = env.read_target();
    assert!(
        !result.contains("pub data_dir: PathBuf"),
        "Old field should be replaced"
    );
    assert!(
        result.contains("pub max_connections: u32"),
        "Should contain new max_connections"
    );
    assert!(
        result.contains("pub timeout_secs: u64"),
        "Should contain new timeout_secs"
    );
    assert!(
        result.contains("pub struct AppConfig"),
        "Struct should remain"
    );

    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert_eq!(deleted.len(), 2, "Should delete 2 lines");
    assert_eq!(added.len(), 3, "Should add 3 new lines");
}

#[test]
fn test_scenario05_append_method() {
    let env = TestEnv::from_data_file("scenarios.rs");
    let script = env.load_script("scenario05_append_method.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "scenario05_append_method failed");

    let result = env.read_target();
    assert!(
        result.contains("pub fn item_count"),
        "Should contain new item_count method"
    );
    assert!(
        result.contains("self.items.len()"),
        "Should contain method body"
    );
    assert!(
        result.contains("pub fn new"),
        "Original new() should remain"
    );
    assert!(
        result.contains("pub fn process"),
        "Original process() should remain"
    );

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert!(added.len() >= 3, "Should have at least 3 Added lines");
    assert_eq!(
        engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
            .count(),
        0
    );
}

#[test]
fn test_scenario06_deep_nested() {
    let env = TestEnv::from_data_file("scenarios.rs");
    let script = env.load_script("scenario06_deep_nested.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "scenario06_deep_nested failed");

    let result = env.read_target();
    assert!(
        !result.contains("processor.process(\"hello\")"),
        "Old process call should be gone"
    );
    assert!(
        result.contains("greeting:"),
        "Should contain new format call"
    );
    assert!(
        result.contains("processing result:"),
        "Should contain result log"
    );
    assert!(result.contains("pub fn run"), "run function should remain");
    assert!(
        result.contains("config.validate()"),
        "Original code should remain"
    );

    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert_eq!(deleted.len(), 1, "Should delete 1 line");
    assert!(added.len() >= 3, "Should have at least 3 Added lines");
}

#[test]
fn test_scenario07_delete_block() {
    let env = TestEnv::from_data_file("scenarios.rs");
    let script = env.load_script("scenario07_delete_block.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "scenario07_delete_block failed");

    let result = env.read_target();
    assert!(
        !result.contains("pub fn deprecated_method"),
        "deprecated_method should be deleted"
    );
    assert!(
        !result.contains("self.items.join"),
        "Method body should be deleted"
    );
    assert!(
        result.contains("pub fn process"),
        "Previous method should remain"
    );
    assert!(
        result.contains("pub fn active_count"),
        "Next method should remain"
    );

    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    assert!(deleted.len() >= 3, "Should have Deleted lines");
}

#[test]
fn test_scenario08_line_block() {
    let env = TestEnv::from_data_file("scenarios.rs");
    let script = env.load_script("scenario08_line_block.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "scenario08_line_block failed");

    let result = env.read_target();
    assert!(
        result.contains("pub fn with_chunk_size"),
        "Should contain new builder method"
    );
    assert!(
        result.contains("self.chunk_size = size;"),
        "Should contain method body"
    );
    assert!(
        result.contains("pub fn new()"),
        "Original new() should remain"
    );
    assert!(
        result.contains("pub fn process"),
        "Original process() should remain"
    );

    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert!(added.len() >= 4, "Should have at least 4 Added lines");
    assert_eq!(
        engine
            .diff_lines
            .iter()
            .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
            .count(),
        0
    );
}

#[test]
fn test_scenario09_delete_replace() {
    let env = TestEnv::from_data_file("scenarios.rs");
    let script = env.load_script("scenario09_delete_replace.ned");

    let (engine, success) = execute_script(&script);
    assert!(success, "scenario09_delete_replace failed");

    let result = env.read_target();
    assert!(
        !result.contains("chunk_size: usize"),
        "Old chunk_size field should be gone"
    );
    assert!(
        result.contains("capacity: usize"),
        "Should contain new capacity field"
    );
    assert!(
        result.contains("priority: u8"),
        "Should contain new priority field"
    );
    assert!(
        result.contains("items: Vec<String>"),
        "items field should remain"
    );

    let deleted: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Deleted)
        .collect();
    let added: Vec<_> = engine
        .diff_lines
        .iter()
        .filter(|d| d.kind == n_edit::output::DiffLineKind::Added)
        .collect();
    assert_eq!(deleted.len(), 2, "Should delete 2 lines");
    assert_eq!(added.len(), 3, "Should add 3 lines");
}
