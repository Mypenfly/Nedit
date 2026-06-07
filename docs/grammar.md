# N_Edit 脚本语法手册

`.ned` 脚本使用 `//!@` 注释前缀命令精确修改源代码文件。每条命令从 `//!@` 行开始，内容提取到下一个 `//!@` 命令或独立的 `...` 分隔符为止。

---

## 快速入门

```ned
//!@Open: src/main.rs
//!@Location:
fn main() {
//!@New:
    println!("hello");
//!@Off:Open
```

1. 打开 `src/main.rs`
2. 定位到 `fn main() {` 这一行
3. 在该位置之后插入 `println!("hello");`
4. 关闭文件，写回修改

---

## 核心概念

### taps 与 diff_taps

- **taps**：行首的 ASCII 空格数（tab 不计为空格）
- **diff_taps**：当前行 taps 减去所在 ContentBlock 首行 taps 的差值

匹配时同时校验**去空白内容**和 **diff_taps**——内容相同但缩进层级不同也会被排除。

```
Location 内容:
    fn foo():       # taps=4, diff_taps=0
        pass        # taps=8, diff_taps=4

文件中:
    fn foo():       # taps=4 ✓  diff_taps=0 ✓  stripped="fnfoo()" ✓
        pass        # taps=8 ✓  diff_taps=4 ✓  stripped="pass" ✓
```

### 匹配流程

1. Location 首行去空白 → 通过哈希索引 O(1) 找到所有候选行
2. 每个候选逐行比对去空白内容 + diff_taps（跳过空行）
3. 恰好 1 个候选 → 返回 ContentBlock；否则报错

### ContentBlock 边界

- **普通 Location**：从匹配首行到**搜索范围末尾**（顶层=文件末，嵌套=父 Block 末）
- **Location:Block**：通过 BlockParser 精确解析花括号/缩进语言代码块边界

---

## 命令参考

### `//!@Open:` — 打开文件

```
//!@Open: <文件路径>
```

打开目标文件，读取并解析为 `FileContent`。所有后续修改先在内存中进行，只有显式 `Off:Open` 或脚本成功结束时才写回磁盘。

**错误：**

| 错误 | 原因 | 解决 |
|------|------|------|
| `Open 命令缺少文件路径参数` | `Open:` 后为空 | 补充文件路径 |
| `文件未找到: <path>` | 路径不存在 | 检查拼写 |
| `无法打开文件 <path>: <reason>` | 权限不足等 | 检查文件权限 |

---

### `//!@Location:` — 定位代码位置

后续 `New` / `Delete` 都在此 Location 返回的 ContentBlock 内操作。支持三种定位方式。

#### 1. 内容匹配（默认）

```ned
//!@Location:
fn process_data(items: &[Item]) -> Vec<Output> {
    let mut results = Vec::new();
```

提取 `fn process_data(...` 和 `    let mut results...` 两行作为定位内容，在文件中查找唯一匹配。

**匹配校验：**
- 逐行去空白内容必须一致
- diff_taps 必须一致（缩进层级感知）
- 空行自动跳过

#### 2. 行号定位

```ned
// 单行号：定位到第 66 行
//!@Location:@66

// 行号范围：定位到第 66 到 120 行
//!@Location:@66,120
```

行号定位**跳过匹配流程**，直接按索引截取。行号相对于**当前搜索范围**（顶层=文件行号，嵌套=父 Block 内行号）。行号必须 > 0，end >= start。

> **优先级**：当 `@行号` 和匹配内容同时存在时，行号优先，后面的内容行被忽略。

#### 3. Block 修饰符

```ned
// 定位完整代码块（自动解析花括号或缩进边界）
//!@Location:Block
fn my_function(data: &Data) -> Result<()> {
```

`Location:Block` 使用 BlockParser 自动确定代码块边界（非"到文件末尾"）：
- **花括号语言**（Rust/C/JS/Java）：逐字符扫描 `{` `}`，正确处理字符串、`//` 行注释、`/* */` 块注释、`\"` `\\` 转义
- **缩进语言**（Python/YAML）：基于缩进层级，跳过空行和注释行
- **纯文本/Markdown**：无法解析为 Block，报错

可与行号结合：`//!@Location:Block @66`

**错误：**

| 错误 | 原因 | 解决 |
|------|------|------|
| `Location 命令未找到任何匹配` | 内容在搜索范围中完全不存在 | 检查拼写 |
| `Location 命令匹配到 N 个结果` | 内容太短导致歧义 | 增加更多上下文行 |
| `Location 被指定为一个 Block 但无法解析` | 内容无花括号/缩进结构 | 去掉 `:Block` |

---

### `//!@New:` — 插入内容

| 变体 | 说明 | 需要 Location? |
|------|------|:---:|
| `//!@New:` (Normal) | 在定位位置之后插入 | 是 |
| `//!@New:Start` | 在文件/当前 Block 开头插入 | 否 |
| `//!@New:End` | 在文件/当前 Block 末尾追加 | 否 |

**缩进规则：**
- New 内容每行的 `diff_taps` = 该行的行首空格数（绝对缩进量）
- 插入时以插入位置的 taps 为基准，加上各行的 diff_taps 构建最终缩进
- `is_raw` 行（来自 `Raw` 命令）保持原始内容不计算缩进

**New:Normal 插入位置：**
- Location 最后匹配行之后（block 内偏移为 matched_line_count）
- 空 Location（matched_line_count=0）→ 插入到 block 末尾
- Delete 之后（match_info=DeleteAt）→ 插入到删除位置

**注意**：`New:Start` / `New:End` 前面若有 Location，则在**当前 Block** 的开头/末尾操作，而非整个文件。

**错误：**

| 错误 | 原因 | 解决 |
|------|------|------|
| `New 命令前缺少 Location 定位` | `New:Normal` 前无 Location（或被 `...` 截断） | 先使用 `Location:` |
| `New/Delete 命令之前必须存在 Location 命令` | 执行时 block_stack 为空 | 检查 Location 是否成功 |

---

### `//!@Delete:` — 删除内容

删除定位范围内的**连续匹配行**。匹配逻辑与 Location 一致（去空白比对）。

```ned
//!@Location:
fn update_user(&self, user_id: u64) {
//!@Delete:
    log::warn!("deprecated call");
    self.deprecated_update(user_id)
```

**变体：**

| 命令 | 说明 |
|------|------|
| `//!@Delete:` | 内容匹配删除（要求连续、紧邻） |
| `//!@Delete:@start,end` | 按行号直接删除（行号相对于当前 Block） |
| `//!@Delete:Block` | 删除整个 ContentBlock（要求前一个 Location 也使用 Block） |

**Delete 邻接规则：**
- Delete 首行必须紧邻 Location 最后匹配行之后（中间不能隔非空行）
- 中间隔了代码时 → `DeleteNotAdjacent` 错误
- **解决方法**：用嵌套 Location 桥接

```ned
// ❌ 错误：中间隔了其他行
//!@Location:
pub fn run_app(config: AppConfig) {
//!@Delete:
    let result = pipeline.execute("test")?;

// ✅ 正确：用嵌套 Location 精确定位
//!@Location:
pub fn run_app(config: AppConfig) {
//!@Location:
    let result = pipeline.execute("test")?;
//!@Delete:
    let result = pipeline.execute("test")?;
```

**Delete:Block 之后跟 New:Normal**：New 会插入到删除位置（引擎自动记录 `DeleteAt` 位置）。

**错误：**

| 错误 | 原因 | 解决 |
|------|------|------|
| `Delete 命令未能在当前 Block 中找到匹配内容` | 要删的内容在范围内不存在 | 检查内容拼写或调整 Location |
| `Delete 匹配位置与 Location 不紧邻` | 中间隔了非空行 | 加嵌套 Location 或扩大 Location |
| `Delete:Block 要求前一个 Location 也使用 Block 指令` | Location 没用 Block 修饰 | 改为 `Location:Block` |

---

### `//!@Raw:` — 字面量内容

解决 `...` 分隔符的二义性。当需要在 New/Delete 内容中写入真正的 `...` 字符时使用。

```ned
//!@New:
    println!("start");
//!@Raw: ...
...
```

Raw 的内容会**融入上一个** `New` 或 `Delete` 命令：
- **New 上下文**：作为 `is_raw=true` 行加入 NewContent，写入时保留原始内容不计算缩进
- **Delete 上下文**：作为 `is_raw=true` 行加入 DeleteContent，按字面量逐字符匹配
- Raw 必须在 `New` 或 `Delete` 之后，否则报 `UnknownCommand` 错误

---

### `//!@Off:` — 关闭作用域

| 命令 | 效果 |
|------|------|
| `//!@Off:Location` | 弹出栈顶 ContentBlock，将修改写回父级（嵌套时自动合并） |
| `//!@Off:New` | 效果等同于 `...`（退出 New 作用域） |
| `//!@Off:Open` | 逐层弹出所有 Block 并写回文件，最终落盘 |

**重要规则：**
- 每个 `Off:Location` 关闭一层嵌套——N 层嵌套需要 N 个
- 脚本结尾若未显式 `Off:Open`，引擎自动执行隐式 `Off:Open`
- `Off:Location` 时若 block_stack 为空 → `Block 栈为空` 错误

---

### 嵌套 Location

在一个 Location 作用域内再次使用 Location，逐级缩小操作范围。

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
//!@New:
                self.new_pipeline();
//!@Off:Location
//!@Off:Location
//!@Off:Location
//!@Off:Open
```

**原理：**
- 栈顶已有 ContentBlock → 搜索范围自动缩小为该 Block
- ContentBlock 行号保持绝对文件行号，写回时通过 `start_line` 偏移计算
- `Off:Location` 逐层弹出，内层修改先合并回外层，最外层最终写回文件

---

## 分隔符 `...`

`...` 有两种含义：

| 上下文 | 含义 |
|--------|------|
| 独立一行（行内容完全等于 `...`） | **分隔符**，终止上一个命令的内容提取 |
| 需要写入字面量 `...` | 使用 `//!@Raw: ...` |

### 内容提取规则

Lexer 从命令行剩余文本开始收集内容行，直到遇到：
1. 下一行以 `//!@` 开头（下一个命令）
2. 独立的 `...` 行（分隔符）

`...` 分隔符自身**不出现在**任何命令的内容中。

### 关键陷阱：`...` 重置 Location 追踪

在 Location 和 New/Delete 之间出现独立的 `...` 会切断上下文，导致 `MissingLocation` 错误：

```ned
// ❌ ... 在 Location 和 Delete 之间，切断了上下文
//!@Location:
fn example() {
    let old_code = 1;
...
//!@Delete:
    let old_code = 1;

// ✅ 正确：让下一个 //!@ 命令自然终止 Location 内容
//!@Location:
fn example() {
    let old_code = 1;
//!@Delete:
    let old_code = 1;
```

### 记忆法则

| 场景 | 正确做法 |
|------|----------|
| Location 后跟 New/Delete/嵌套Location | 不用 `...`，下一个 `//!@` 自动终止 |
| New 内容后跟其他命令 | 不用 `...`，下一个 `//!@` 自动终止 |
| New/Delete 内容作为末尾 | 用 `...` 终止 |
| 需要写入字面量 `...` | 使用 `//!@Raw: ...` |

---

## 复杂示例

以下脚本对测试文件执行多次编辑，涵盖内容匹配、行号 Delete、嵌套 Location、Block 操作：

```ned
//!@Open: ./tests/data/rust_complex.rs

// 操作 1：行号 Delete 删除 impl Default for AppConfig（原文件行 22-34）
//!@Location:
//!@Delete:@22,34
//!@Off:Location

// 操作 2：内容匹配定位 AppConfig struct，添加新字段
//!@Location:
pub struct AppConfig {
    pub name: String,
    pub version: String,
    pub features: Vec<String>,
    pub settings: HashMap<String, String>,
    pub data_dir: PathBuf,
    pub max_connections: u32,
    pub timeout_ms: u64,
//!@New:
    pub env_prefix: String,
    pub health_check_path: String,
//!@Off:Location

// 操作 3：嵌套 Location 在 validate 方法开头插入日志
//!@Location:
impl AppConfig {
//!@Location:
    pub fn validate(&self) -> Result<(), String> {
//!@New:Start
        log::debug!("validating config: {}", self.name);
//!@Off:Location
//!@Off:Location

// 操作 4：Location:Block 定位，在 get_connection 后插入新方法
//!@Location:Block @75,75
//!@New:
    pub fn is_healthy(&self) -> bool {
        self.active > 0 && self.active <= self.config.max_connections as usize
    }
//!@Off:Location

// 操作 5：在文件末尾追加新的测试函数
//!@Location:
#[cfg(test)]
mod tests {
//!@New:End

    #[test]
    fn test_connection_pool_shutdown() {
        let config = AppConfig::default();
        let mut pool = ConnectionPool::new(config);
        assert_eq!(pool.active, 0);
        pool.shutdown();
        assert!(pool.connections.is_empty());
    }
//!@Off:Location

//!@Off:Open
```

**预期输出（示例）：**

```
脚本执行成功: multi_op_refactor.ned
- L22: impl Default for AppConfig {
- L23:     fn default() -> Self {
- L24:         AppConfig {
...
- L34: }
+ L20:     pub env_prefix: String,
+ L21:     pub health_check_path: String,
+ L32:         log::debug!("validating config: {}", self.name);
+ L91:     pub fn is_healthy(&self) -> bool {
+ L92:         self.active > 0 && self.active <= self.config.max_connections as usize
+ L93:     }
+ L285:     #[test]
+ L286:     fn test_connection_pool_shutdown() {
...
+ L292:     }
```

---

## 常用修改场景

### 场景 1：在 struct 中添加字段

```ned
//!@Open: src/config.rs
//!@Location:
pub struct AppConfig {
    pub name: String,
    pub version: String,
//!@New:
    pub log_level: String,
//!@Off:Open
```

### 场景 2：在函数内插入代码

```ned
//!@Open: src/handler.rs
//!@Location:
fn process_request(&self, req: Request) -> Response {
//!@New:
    log::info!("processing request: {:?}", req.id);
//!@Off:Open
```

### 场景 3：替换函数实现

```ned
//!@Open: src/services.rs
//!@Location:
fn generate_salt(rounds: u32) -> String {
//!@Delete:
    let bytes: Vec<u8> = (0..16).map(|_| rng.gen()).collect();
    format!("$2b${}${}", rounds, hex::encode(&bytes))
//!@New:
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes);
    format!("$2b${}${}", rounds, base64::encode(&bytes))
//!@Off:Open
```

### 场景 4：行号精确定位修改

```ned
//!@Open: src/main.rs
//!@Location:@42,58
//!@Delete:@3,5
//!@New:
    log::info!("processing");
    validate_input()?;
//!@Off:Open
```

### 场景 5：在 impl 块末尾追加新方法

```ned
//!@Open: src/pipeline.rs
//!@Location:Block
impl DataPipeline {
//!@New:End
    pub fn clear_stages(&mut self) {
        self.stages.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }
//!@Off:Open
```

### 场景 6：深层嵌套精确定位

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
//!@New:
                self.new_pipeline();
//!@Off:Location
//!@Off:Location
//!@Off:Location
//!@Off:Open
```

### 场景 7：Delete:Block 删除整个函数

```ned
//!@Open: src/services.rs
//!@Location:Block
fn deprecated_function(input: &str) -> ParseResult {
//!@Delete:Block
//!@Off:Open
```

### 场景 8：行号定位 Block + 在其后插入新方法

```ned
//!@Open: src/complex.rs
//!@Location:Block @75,75
//!@New:
    pub fn new_method(&self) -> bool {
        self.value > 0
    }
//!@Off:Open
```

### 场景 9：删除特定行并替换

```ned
//!@Open: src/config.rs
//!@Location:
pub struct AppConfig {
    pub name: String,
//!@Delete:@3,4
//!@New:
    pub title: String,
    pub description: String,
//!@Off:Open
```

---

## 输出格式

### Diff 输出

修改成功后输出差异标记：

```
+ L12:     let new_field: String,      ← 绿色 + 新增行
- L15:     old_code();                 ← 红色 - 删除行
```

- `+` 绿色：新增行，带行号前缀
- `-` 红色：删除行，带行号前缀
- 管道/重定向时自动关闭颜色（检测 `is_terminal`）

### CLI 标志

| 标志 | 效果 |
|------|------|
| （默认） | 输出 `脚本执行成功` + diff |
| `-v` / `--verbose` | 额外输出词法分析 Token 数 + 语法分析命令数 |
| `-q` / `--quiet` | 只输出错误，不输出成功提示 |

---

## 错误类型参考

### ParseError — 脚本解析阶段

| 错误 | 触发条件 |
|------|----------|
| `MissingFilePath` | `Open:` 后无路径 |
| `UnknownCommand` | 无法识别的命令头（如 `Off:Invalid`）或 `Raw` 无上下文 |
| `MissingLocation` | `New:Normal` / `Delete:` 前无 Location（或被 `...` 截断） |
| `BlockRequiredForDelete` | `Delete:Block` 前未使用 `Location:Block` |

### MatchError — 匹配阶段

| 错误 | 触发条件 |
|------|----------|
| `NoMatch` | Location 内容完全找不到（含行号超出范围） |
| `TooManyMatches` | 匹配到 ≥2 个候选（附带前 3 个候选信息） |
| `DeleteMatchFailed` | Delete 内容在 Block 中找不到连续匹配 |
| `DeleteNotAdjacent` | Delete 首行与 Location 末行之间隔了非空行 |
| `BlockNotParseable` | `Location:Block` 内容无法解析为代码块 |

### FileError — 文件 I/O

| 错误 | 触发条件 |
|------|----------|
| `NotFound` | 文件路径不存在（仅 engine 层使用） |
| `CannotOpen` | 文件无法读取（权限、编码等） |
| `WriteFailed` | 写回文件失败 |

### EngineError — 引擎执行阶段

| 错误 | 触发条件 |
|------|----------|
| `MissingLocationForNew` | New/Delete 执行时 block_stack 为空 |
| `BlockStackEmpty` | `Off:Location` 时栈已空（Off 数量多于嵌套层数） |
| `BlockRequiredForDelete` | 引擎层 Block 指令不一致 |

---

## 常见误用与排查

### 1. Location 和 New/Delete 之间误放 `...`

```ned
// ❌ ... 切断了 Location 状态
//!@Location:
fn foo() {
    let x = 1;
...
//!@Delete:
    let x = 1;

// ✅ 让下一个命令自然终止
//!@Location:
fn foo() {
    let x = 1;
//!@Delete:
    let x = 1;
```

### 2. 注释行混入 Location 内容

```ned
// ❌ 注释在 Location 和下一个命令之间，被当作匹配内容
//!@Location:
fn foo() {
    bar();
// 这是一条注释
//!@New:

// ✅ 注释放在命令之前
// 这是一条注释
//!@Location:
fn foo() {
    bar();
//!@New:
```

### 3. 纯文本/Markdown 使用 Location:Block

只使用普通 `Location:`，Block 修饰符对纯文本无效。

### 4. Delete:Block 之前忘记 Location:Block

```ned
// ❌
//!@Location:
fn foo() {
//!@Delete:Block

// ✅
//!@Location:Block
fn foo() {
//!@Delete:Block
```

### 5. Off:Location 数量与嵌套层数不一致

N 层嵌套需要 N 个 `Off:Location`。多了报 `Block 栈为空`，少了内层修改不写回。

### 6. 行号定位在修改文件后继续使用

行号是相对于文件的绝对位置。第一次修改后（如在新行插入），后续行号会偏移，导致定位错误。建议行号定位只用于单次操作，多次操作优先使用内容匹配。

### 7. 缩进不一致导致匹配失败

Location 可以**不关心绝对缩进**——它以首行为基准计算 diff_taps。所以定位内容可以省去首行缩进：

```ned
// ✅ 首行 fn foo() 本身没有缩进，后面 diff_taps=4
//!@Location:
fn foo() {
    bar();

// ✅ 也可以带缩进写，效果相同
//!@Location:
    fn foo() {
        bar();
```

---

## 支持语言

| 语言 | 普通 Location | Block 操作 |
|------|:---:|:---:|
| Rust / C / C++ / JS / TS / Java | ✓ | ✓ (花括号) |
| Python / YAML | ✓ | ✓ (缩进) |
| Markdown / 纯文本 | ✓ | — |

Block 解析不支持 Rust raw string (`r#"..."#`)、模板字符串等特殊语法。
