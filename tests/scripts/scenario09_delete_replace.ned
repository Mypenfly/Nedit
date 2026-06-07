//!@Open: ./tests/data/scenarios.rs
// 场景 9：删除特定行并替换
// DataProcessor struct 的字段: items 和 chunk_size
//!@Location:
pub struct DataProcessor {
    items: Vec<String>,
    chunk_size: usize,
//!@Delete:
    items: Vec<String>,
    chunk_size: usize,
//!@New:
    items: Vec<String>,
    capacity: usize,
    priority: u8,
//!@Off:Open
