//!@Open: ./tests/data/scenarios.rs
// 场景 8：行号定位 Block + 在其后插入新方法
// DataProcessor::new() 在文件中大约第 39 行
//!@Location:Block @39,39
//!@New:
    pub fn with_chunk_size(mut self, size: usize) -> Self {
        self.chunk_size = size;
        self
    }
//!@Off:Open
