//!@Open: ./tests/data/scenarios.rs
// 场景 1：在 struct 中添加字段
//!@Location:
pub struct AppConfig {
    pub name: String,
    pub version: String,
    pub data_dir: PathBuf,
//!@New:
    pub log_level: String,
//!@Off:Open
