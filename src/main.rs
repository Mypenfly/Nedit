//! CLI 入口 (Main)
//!
//! 解析命令行参数，读取 .ned 脚本文件，驱动词法分析 → 语法分析 → 执行引擎流水线。
//!
//! ## 对应文档
//!
//! 详见 INSTRUCTION.md 第 1 节 "总体设计路径"

use clap::Parser;
use n_edit::engine::Engine;
use n_edit::lexer::Lexer;
use n_edit::output::OutputFormatter;
use n_edit::parser::Parser as ScriptParser;
use std::io::IsTerminal;

/// N_Edit — 基于注解的代码编辑工具
///
/// 读取 .ned 脚本文件，解析其中的编辑指令，
/// 对目标文件执行精确的代码修改操作。
#[derive(Parser)]
#[command(name = "n_edit")]
#[command(version = "0.1.0")]
#[command(about = "基于注解的代码编辑工具")]
struct Cli {
    /// .ned 脚本文件路径
    script_path: String,

    /// 详细输出模式
    #[arg(short, long)]
    verbose: bool,

    /// 静默模式（只输出错误）
    #[arg(short, long)]
    quiet: bool,
}

fn main() {
    let cli = Cli::parse();

    let script_content = match std::fs::read_to_string(&cli.script_path) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("读取脚本文件 {} 失败: {}", cli.script_path, e);
            std::process::exit(1);
        }
    };

    // 词法分析：脚本文本 → Token 流
    let tokens = Lexer::tokenize(&script_content);
    if cli.verbose {
        eprintln!("[verbose] 词法分析完成，共 {} 个 Token", tokens.len());
    }

    // 语法分析：Token 流 → AST (Command 序列)
    let commands = ScriptParser::parse(tokens).unwrap_or_else(|e| {
        eprint!(
            "{}",
            n_edit::output::format_error_with_color(
                &e.title(),
                &e.detail(),
                &e.hints(),
                std::io::stdout().is_terminal(),
            )
        );
        std::process::exit(1);
    });
    if cli.verbose {
        eprintln!("[verbose] 语法分析完成，共 {} 条命令", commands.len());
    }

    // 执行引擎：逐条执行 Command
    let mut engine = Engine::new();
    if cli.verbose {
        engine.set_verbose(true);
    }
    match engine.execute(commands) {
        Ok(()) => {
            if !cli.quiet {
                eprintln!("脚本执行成功: {}", cli.script_path);
                // quiet 模式下不输出 diff
                if !engine.diff_lines.is_empty() {
                    let formatter = OutputFormatter::new();
                    print!("{}", formatter.format_diff_lines(&engine.diff_lines));
                }
            }
        }
        Err(e) => {
            eprint!(
                "{}",
                n_edit::output::format_error_with_color(
                    &e.title(),
                    &e.detail(),
                    &e.hints(),
                    std::io::stdout().is_terminal(),
                )
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use n_edit::engine::Engine;
    use n_edit::lexer::{self, Lexer};
    use n_edit::model::LineNumber;
    use n_edit::parser::Parser as ScriptParser;

    /// 辅助结构：持有临时目录，确保测试文件存活
    struct TestEnv {
        dir: tempfile::TempDir,
    }

    impl TestEnv {
        fn new() -> Self {
            TestEnv {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn create_file(&self, name: &str, content: &str) -> String {
            let path = self.dir.path().join(name);
            std::fs::write(&path, content).unwrap();
            path.to_str().unwrap().to_string()
        }
    }

    #[test]
    fn test_full_pipeline_open_location_off() {
        let env = TestEnv::new();

        let target_content =
            "// header\nfn process() {\n    do_work();\n}\n\nfn main() {\n    process();\n}\n";
        let target_path = env.create_file("sample.rs", target_content);

        let ned_script = format!(
            "//!@Open: {}\n//!@Location:\nfn main() {{\n...\n//!@Off:Open\n",
            target_path
        );
        let ned_path = env.create_file("test_open_location.ned", &ned_script);

        let script_content = std::fs::read_to_string(&ned_path).unwrap();

        let tokens = Lexer::tokenize(&script_content);
        assert_eq!(tokens.len(), 4); // Open, Location, Separator, Off

        let commands = ScriptParser::parse(tokens).unwrap();
        assert_eq!(commands.len(), 3);

        let mut engine = Engine::new();
        engine.execute(commands).unwrap();

        let result_content = std::fs::read_to_string(&target_path).unwrap();
        assert_eq!(result_content, target_content);
    }

    #[test]
    fn test_full_pipeline_open_location_implicit_off() {
        let env = TestEnv::new();

        let target_content = "fn foo() {}\nfn bar() {}\n";
        let target_path = env.create_file("implicit.rs", target_content);

        let ned_script = format!("//!@Open: {}\n//!@Location:\nfn bar() {{}}\n", target_path);
        let ned_path = env.create_file("test_implicit.ned", &ned_script);

        let script_content = std::fs::read_to_string(&ned_path).unwrap();
        let tokens = Lexer::tokenize(&script_content);
        assert_eq!(tokens.len(), 2); // Open, Location (no Off)

        let commands = ScriptParser::parse(tokens).unwrap();
        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(
            result.is_ok(),
            "Implicit Off should succeed: {:?}",
            result.err()
        );

        let result_content = std::fs::read_to_string(&target_path).unwrap();
        assert_eq!(result_content, target_content);
    }

    #[test]
    fn test_full_pipeline_location_not_found() {
        let env = TestEnv::new();

        let target_content = "fn foo() {}\nfn bar() {}\n";
        let target_path = env.create_file("nomatch.rs", target_content);

        let ned_script = format!(
            "//!@Open: {}\n//!@Location:\nfn nonexistent() {{}}\n//!@Off:Open\n",
            target_path
        );
        let ned_path = env.create_file("test_nomatch.ned", &ned_script);

        let script_content = std::fs::read_to_string(&ned_path).unwrap();
        let tokens = Lexer::tokenize(&script_content);
        let commands = ScriptParser::parse(tokens).unwrap();

        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_err(), "Should fail for non-matching location");
    }

    #[test]
    fn test_full_pipeline_open_missing_file() {
        let tokens = vec![lexer::Token::Open {
            file_path: "/nonexistent/file.rs".to_string(),
            line: LineNumber::new(1),
        }];
        let commands = ScriptParser::parse(tokens).unwrap();
        let mut engine = Engine::new();
        let result = engine.execute(commands);
        assert!(result.is_err());
    }
}
