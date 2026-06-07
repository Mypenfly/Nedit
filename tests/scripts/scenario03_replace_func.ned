//!@Open: ./tests/data/scenarios.rs
// 场景 3：替换函数实现 — DataProcessor::deprecated_method
//!@Location:
    pub fn deprecated_method(&self) -> String {
//!@Delete:
        let mut result = String::new();
        for item in &self.items {
            result.push_str(&format!("[{}]", item));
        }
        result
//!@New:
        self.items.join(", ")
//!@Off:Open
