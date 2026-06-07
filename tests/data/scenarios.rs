// scenarios.rs — 语法手册场景测试数据
//
// 包含 struct、impl、函数、嵌套结构，用于验证各种常见修改场景。

use std::path::PathBuf;

/// 应用程序配置
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub name: String,
    pub version: String,
    pub data_dir: PathBuf,
}

impl AppConfig {
    pub fn from_env() -> Self {
        AppConfig {
            name: String::new(),
            version: String::new(),
            data_dir: PathBuf::from("."),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("name is empty".into());
        }
        Ok(())
    }
}

/// 数据处理器
pub struct DataProcessor {
    items: Vec<String>,
    chunk_size: usize,
}

impl DataProcessor {
    pub fn new() -> Self {
        DataProcessor {
            items: Vec::new(),
            chunk_size: 64,
        }
    }

    pub fn process(&self, input: &str) -> Result<String, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("empty input".into());
        }
        Ok(trimmed.to_uppercase())
    }

    pub fn deprecated_method(&self) -> String {
        let mut result = String::new();
        for item in &self.items {
            result.push_str(&format!("[{}]", item));
        }
        result
    }

    pub fn active_count(&self) -> usize {
        self.items.len()
    }
}

/// 入口函数
pub fn run(config: AppConfig) -> Result<(), String> {
    config.validate()?;

    let processor = DataProcessor::new();
    let result = processor.process("hello")?;

    if result.is_empty() {
        return Err("empty result".into());
    }

    Ok(())
}
