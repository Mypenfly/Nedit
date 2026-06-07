//!@Open: ./tests/data/scenarios.rs
// 场景 5：在 impl 块末尾追加新方法
//!@Location:Block
impl DataProcessor {
//!@New:End
    pub fn item_count(&self) -> usize {
        self.items.len()
    }
//!@Off:Open
