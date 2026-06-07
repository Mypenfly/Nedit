# N_Edit

**基于语义级命令的精确代码编辑工具。** 通过 `.ned` 脚本实现格式感知、缩进安全的源码修改。

## 为什么选择 N_Edit

传统搜索替换对代码格式敏感，一行缩进差异就匹配不到。`.ned` 脚本用 **去空白内容 + 缩进差异** 双重匹配，让 LLM 和人类都能**一次写对、精准修改**。

```
搜索替换:  "    fn main() {" → 缩进差一格就失败
N_Edit:    //!@Location: \n fn main() { → 去空白匹配，忽略缩进差异
```

## 核心能力

| 特性 | 实现 |
|------|------|
| **语义匹配** | Location 命令通过 stripped_content + diff_taps 精确找到目标 |
| **Block 解析** | Location:Block / Delete:Block 识别花括号和缩进语言的完整代码块 |
| **格式保留** | 插入内容保持缩进层级，不做额外格式化 |
| **安全写入** | 全在内存中修改，失败时原文件零影响 |
| **详细报错** | 匹配歧义时给出候选列表，帮助快速调整 |
| **管道友好** | 彩色输出自动检测终端能力，重定向时关闭颜色 |

## 快速开始

```bash
cargo build --release
./target/release/n_edit script.ned           # 执行 .ned 脚本
./target/release/n_edit script.ned --verbose  # 显示执行详情
./target/release/n_edit script.ned --quiet    # 只输出错误
```

## .ned 脚本示例

### 基本替换

```ned
//!@Open: src/main.rs
//!@Location:
fn main() {
//!@Delete:
    old_code();
...
//!@New:
    new_code();
...
//!@Off:Open
```

### 嵌套定位（精确操作深层代码）

```ned
//!@Open: src/handler.rs
//!@Location:
impl RequestProcessor {
//!@Location:
    fn handle_active(&self) {
//!@Location:
        match self.state {
            State::Running => {
//!@Delete:
                self.old_logic();
...
//!@New:
                self.new_pipeline();
...
//!@Off:Location
//!@Off:Location
//!@Off:Location
//!@Off:Open
```

### 在类中新增方法（Python）

```ned
//!@Open: app.py
//!@Location:
    def complete_task(self, task_id: str) -> bool:
//!@New:
    def reopen_task(self, task_id: str) -> bool:
        task = self._tasks.get(task_id)
        if task is None:
            return False
        task.completed = False
        return True

//!@Off:Open
```

### 按行号精确定位

```ned
//!@Open: src/main.rs
//!@Location:@42,58
//!@Delete:@3,5
//!@New:
    log::info!("processing");
    validate_input()?;
//!@Off:Location
//!@Off:Open
```

### 删除整个函数并替换（Rust）

```ned
//!@Open: src/parser.rs
//!@Location:Block
fn old_parser(input: &str) -> ParseResult {
//!@Delete:Block
//!@New:
fn new_parser(input: &str) -> ParseResult {
    let tokens = lexer::tokenize(input)?;
    let ast = grammar::parse(&tokens)?;
    Ok(ast)
}
...
//!@Off:Open
```

## 命令速查

| 命令 | 说明 |
|------|------|
| `Open:` | 打开目标文件 |
| `Location:` | 定位代码位置（支持嵌套，逐级缩小范围） |
| `Location:@行号` | 按行号直接定位，跳过匹配流程 |
| `Location:Block` | 定位完整代码块 |
| `New:` | 在定位位置后插入 |
| `New:Start` | 在文件/Block 开头插入 |
| `New:End` | 在文件/Block 末尾追加 |
| `Delete:` | 删除匹配的连续行 |
| `Delete:@行号` | 按行号直接删除 |
| `Delete:Block` | 删除整个代码块 |
| `Raw:` | 字面量内容（解决 `...` 二义性） |
| `Off:Open` / `Off:Location` / `Off:New` | 关闭作用域 |

> 完整语法和错误处理见 [docs/grammar.md](docs/grammar.md)

## 实现状态

```
Phase 1: Open / Location / Off        ✅ 已完成
Phase 2: New / Delete                 ✅ 已完成
Phase 3: Location:Block / Delete:Block ✅ 已完成
Phase 4: 嵌套 Location                 ✅ 已完成
Phase 5: 行号定位 + Raw 命令           ✅ 已完成
Phase 6: 错误美化 / 彩色输出           ✅ 已完成
Phase 7: 扩展命令（Include 等）        待实现
```

## 支持的语言

| 语言 | 普通 Location | Block 操作 |
|------|:---:|:---:|
| Rust | ✓ | ✓ |
| C / C++ / JS / TypeScript / Java | ✓ | ✓ |
| Python / YAML | ✓ | ✓ |
| Markdown / 纯文本 | ✓ | — |

## 开发

```bash
cargo build              # 构建
cargo test               # 297 个测试
cargo fmt --check        # 格式检查
cargo clippy -- -D warnings  # Lint

# Nix 环境
nix develop
cargo nextest run        # 更快运行测试
```

## 项目结构

```
src/
  main.rs      # CLI 入口
  lexer.rs     # 词法分析：//!@ 识别 → Token 流（含 @行号 和 Raw）
  parser.rs    # 语法分析：Token → Command AST
  engine.rs    # 执行引擎：状态机，逐条执行命令（含行号/嵌套 Location）
  matcher.rs   # 核心匹配算法（含 SearchScope 抽象 + 行号直取）
  block.rs     # Block 解析（花括号/缩进）
  model.rs     # 数据结构定义（含 LineRange、LineNumber newtype）
  error.rs     # 错误类型集中定义
  output.rs    # 彩色终端输出
tests/
  data/        # 测试用真实源码（Rust/Python/YAML/Markdown）
  scripts/     # .ned 测试脚本（含行号定位和多操作复合场景）
docs/
  grammar.md   # .ned 语法手册
  phases.md    # 实现阶段拆分
  INSTRUCTION.md  # 详细实现指令
```
