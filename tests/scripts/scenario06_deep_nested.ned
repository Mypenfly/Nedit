//!@Open: ./tests/data/scenarios.rs
// 场景 6：深层嵌套精确定位
// run 函数 → DataProcessor::new() → process 调用 → 替换为空输入的检查
//!@Location:
pub fn run(config: AppConfig) -> Result<(), String> {
//!@Location:
    let processor = DataProcessor::new();
    let result = processor.process("hello")?;
//!@Location:
    let result = processor.process("hello")?;
//!@Delete:
    let result = processor.process("hello")?;
//!@New:
    let input = format!("greeting: {}", &config.name);
    let result = processor.process(&input)?;
    log::info!("processing result: {}", result);
//!@Off:Location
//!@Off:Location
//!@Off:Location
//!@Off:Open
