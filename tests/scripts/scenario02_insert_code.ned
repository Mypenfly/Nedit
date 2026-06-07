//!@Open: ./tests/data/scenarios.rs
// 场景 2：在函数内插入代码
//!@Location:
    pub fn process(&self, input: &str) -> Result<String, String> {
//!@New:
        log::info!("processing input: {}", input);
//!@Off:Open
