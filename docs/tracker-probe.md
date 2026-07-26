# Codex 单应用输入追踪探针

## 这是什么

`tracker-probe` 是兼容性体检通过后的一个 **B 级局部差分实验**。它只
验证下面这条链能否由我们自己的代码重复得到：

```text
自然码按键（可选） → UIA 组合拼音 → 同一输入框的文本变化 → 上屏差分
```

它不是完整输入法、通用聊天记录器、后台服务或训练数据管线。没有完成
另外两个常用环境的兼容性体检以前，不扩大应用范围。

## 强制边界

- 每次运行必须同时给出目标进程 PID 和显式 `--arm`。
- 只查找该 PID 下、`ControlType=Edit`、名称精确等于“随心输入”的元素；
  结果不是恰好一个时拒绝替用户猜选。
- Chromium 重建 UIA 树时可能瞬时返回零个结果；探针只对“零个”按相同
  PID、控件类型和精确名称做五次、每次间隔 100 ms 的有界重试。出现多个
  结果时不重试且立即拒绝，重试不会放宽任何条件。
- `IsPassword=true` 时拒绝运行。
- 输入框未启用或不可获得键盘焦点时拒绝运行。
- 必须同时支持 `TextEditPattern` 和 `ValuePattern`。
- UIA 事件只注册在找到的输入框本身，不监听整个聊天窗口。
- 默认不安装键盘钩子，不显示文字，只报告变化长度。
- 即使显式给出 `--capture-keys`，按键配对也要在目标输入框内再按一次
  `Ctrl+Shift+F11`；探针同步确认该精确元素持有键盘焦点后才进入 READY。
- 没有文件输出、数据库、剪贴板、截图、网络或自动启动功能。
- 进程内只保留当前字段值；对外输出只有单段最小差分，不输出整份草稿。
- `Ctrl+Shift+F12` 随时停止并移除监听。

## 为什么需要两种事件

Codex/Chrome 会产生 `TextEditChangeType_Composition`，载荷是微软输入法
规范化后的完整拼音，如 `mao'mao`，但体检中没有产生
`CompositionFinalized`。

同一个输入框会产生普通 `Text_TextChanged`。组合期间 `Value.Value`
依次为拉丁拼音，空格上屏后变为“猫猫”。状态机同时保留组合前文档基线
和最后一份内存值，用最长公共前后缀求两份字符级变化段：

```text
preedit_change:  before="mao'mao", after="猫猫",
                 start=0, deleted="mao'mao", inserted="猫猫"
document_change: before="", after="猫猫",
                 start=0, deleted="", inserted="猫猫"
```

`preedit_change` 解释输入法组合如何上屏；`document_change` 解释这一次
组合对原文产生的净编辑。若在“甲错乙”中选中“错”并直接输入“在”，
前者仍是 `zai → 在`，后者则是 `错 → 在`。两份差分都只包含单个最小
变化段，不复制未变化的周围草稿。

空 ProseMirror 会把带换行的控件名暴露为 `Value.Value`。状态机在没有
活跃组合时把这个“空白包裹的精确控件名”归一为空串，包括从最后一个字
删到空框的转变；精确字面值“随心输入”仍视为用户文字。这条窄规则避免
把占位符算成五个新字符，又不按模糊名称删除真实内容。

句中组合时，两种事件不保证同步交替：专用组合事件可能已经报告
`mao'mao`，普通值却还依次追赶到 `mao'm`、`mao'mao`。只包含小写拼音和
分隔撇号的追赶差分仍属于 preedit，不结束组合；中文上屏或明确取消后才
产生 `Commit`/`Revision`。

重复的相同 `Value.Value` 不产生第二条记录。没有活跃组合时的删除或插入
作为独立 `Revision` 报告；显式开启按键捕获后，触发这次修订的退格、
Delete 和光标移动键也随记录保留。组合被 Esc 取消时同样输出一个有界
修订，而不是悄悄丢掉这次失败尝试。

底层只输出有顺序的原子事实，不在这里猜测用户意图。比如“提交错字 →
退格删除 → 重新组合 → 提交新字”会表现为 `Commit → Revision → Commit`。
选中文字后直接组合替换则表现为一条 `document_change` 同时含删除与
插入的 `Commit`。上层可以按时间、位置和非歧义证据归并为纠错候选，
同时仍能回查原始事实，避免把正常改写误标成打字错误。

纯文本差分在重复字符中可能有多个同样合法的位置。例如从八个“猫”删成
七个“猫”，只比较前后值无法知道删的是哪一个。探针因此读取同一目标的
`TextPattern.GetSelection`，只把选择范围换算为字符偏移，不输出范围内
全文；输出中的位置证据分为：

- `UniqueText`：文本本身只有一个合法差分位置；
- `Caret`：文本位置不唯一，但编辑后的光标验证了其中一个；
- `Ambiguous`：没有可靠光标证据，保留传统差分位置但明确标为有歧义。

## 原始按键为什么是单独开关

UIA 只给出规范化拼音，无法证明用户究竟按了 `mkmk`，还是经历了简写、
回删、错序和候选数字键。`--capture-keys` 因此提供一个明确可见的实验
开关，默认关闭。

Windows 的低级键盘接口在技术上是全局回调。探针用三层条件收窄：

1. 前台窗口 PID 必须等于显式白名单 PID；
2. UIA 焦点必须精确落在目标非密码输入框；唯一宽限是组合已经从该输入框
   开始但候选窗暂时取得 UIA 焦点，此时只延续到本次提交或取消；
3. 只接受字母、数字、退格、Delete、空格、Esc、方向键和 Home/End；
   Shift 与下一枚上述按键配成 `Shift(...)`，单独 Shift 不记；带
   Ctrl/Alt/Win 的快捷键以及注入事件直接丢弃。

焦点处理器不会打印或保存其他控件名称。普通失焦会立即停止新增按键；
已经由精确目标输入框触发的活跃组合可以在候选窗临时取焦时继续，但前台
PID 仍须匹配，并在提交或取消的文本事件到达时立即结束宽限。普通英文
编辑、取消组合和下一次不相关组合会限制或清理遗留按键。组合提交和普通
修订都会携带自上一条文字变化以来的受限按键数组；每条记录只带最小文本
差分，不复制周围草稿。第一版仍只用于人工短句，不能常驻运行。

每条提交或修订还带 `keys_complete`。READY 后若目标组合已经出现但没有
观察到首键，探针打印警告并把该条标为 `false`，不让残缺样本混入后续
评测。READY 之前的按键一律不捕获。

## 构建与人工运行

先关闭 AccEvent，避免同时存在两个 UIA 客户端。确定 Codex 的 PID 后，
可以先只验证目标属性；这个命令不读取输入文字，也不安装监听：

```powershell
cargo run --bin tracker-probe -- --pid <PID> --check
```

通过后，第一次建议只验证事件和长度：

```powershell
cargo run --bin tracker-probe -- --pid <PID> --arm
```

在明确使用人工短句时，可以显示变化内容：

```powershell
cargo run --bin tracker-probe -- --pid <PID> --arm --preview-text
```

最后才测试原始按键配对：

```powershell
cargo run --bin tracker-probe -- --pid <PID> --arm --preview-text --capture-keys
```

若要在原子事件之后预览纯内存纠错候选，必须同时显式开启候选层并给出
本次实验的最大删补间隔：

```powershell
# 内容继续脱敏，只显示候选类型、位置证据、长度和按键数量
cargo run --bin tracker-probe -- --pid <PID> --arm `
  --preview-candidates --candidate-gap-ms 5000

# 只对人工短句同时显示文字与限域按键
cargo run --bin tracker-probe -- --pid <PID> --arm --preview-text --capture-keys `
  --preview-candidates --candidate-gap-ms 5000

# 结束时额外创建一份脱敏汇总；目标文件必须尚不存在
cargo run --bin tracker-probe -- --pid <PID> --arm --capture-keys `
  --preview-candidates --candidate-gap-ms 5000 `
  --save-summary data/private/session-summaries/run-001.json

# 更高权限：STOP 时一次性保存 READY 后的私人明文事件；未加密
cargo run --bin tracker-probe -- --pid <PID> --arm --capture-keys `
  --save-capsule data/private/event-capsules/manual-001.zic `
  --allow-private-plaintext
```

`5000` 只是这次运行由操作者明确选择的实验边界，不是默认值，也不是
“人类纠错必定在五秒内”的结论。只给其中一个参数会直接拒绝启动。候选
计时使用进程内单调时钟；启用按键配对时，按下 READY 会同时清空旧候选
状态并重新起算。

命令启动后先点击精确目标输入框并按 `Ctrl+Shift+F11`；只有看到
`KEY_CAPTURE_READY` 才开始人工输入。按 `Ctrl+Shift+F12` 停止。探针会打印
`STOPPED records_were_memory_only=true`，不会生成日志文件。开启候选层时，
停止前还会打印一条 `SESSION_SUMMARY`，其状态同样只存在于本次进程内。
显式给出 `--save-summary` 时，停止行改为 `summary_saved=true`，并在它
之前打印 `SUMMARY_SAVED`；原子记录与候选明细仍只在内存和终端。
只有显式私人胶囊模式会令 `records_were_memory_only=false`，并打印
`PRIVATE_CAPSULE_SAVED contains_private_text=true encryption=none`。它仍
不是流式日志：READY 后的有界原子事件只在 STOP 时一次性新建文件。

提交输出使用 `preedit_*` 与 `document_*` 前缀区分两份差分。
候选预览不会替换 `COMMIT/REVISION`，而是在证据足够时追加
`CORRECTION_CANDIDATE`。没有 `--preview-text` 时，它不显示文字、拼音或
具体按键值，只显示字符数和按键数。
`--preview-text` 的终端输出属于私人数据，不复制进 issue、提交、测试
fixture 或 Git。仓库测试只使用人工构造的 `mkmk → mao'mao → 猫猫`、
组合内退格、事件流错拍、Esc 取消、Shift 选择、直接替换和上屏后删改
事件序列。

## 会话汇总口径

`SESSION_SUMMARY` 永远不包含文字、拼音或具体按键值，只累计：

- `commits`、`revisions` 与会话经过毫秒数；
- `keys_complete_records`、`keys_incomplete_records` 和逻辑按键动作总数；
- 含 Backspace/Delete 的组合提交数；
- 文档净差分位置为 `Ambiguous` 的原子记录数；
- `DirectReplacement`、`DeleteThenInsert` 两种候选数；
- `RestoredSameText`、`ReplacedWithDifferentText` 两种内容分类数；
- 能回指相邻来源提交的候选数；
- 只针对 `DeleteThenInsert` 的间隔数量、最小值、最大值、整数均值与总和。

直接替换的 `gap_ms=0` 是单事件结构，不进入删补间隔统计。整数均值只是
会话描述，不是速度或准确率指标。汇总也不提供“纠错率”：候选不等于
错误，分母更没有经过行为学定义。若按键捕获已开启，READY 之前的事件、
候选与汇总会一起清空。

## 显式保存脱敏汇总

`--save-summary` 默认不存在，也不会从时间或 PID 自动生成路径。它只有在
同时显式开启 `--preview-candidates` 和正数 `--candidate-gap-ms` 时可用，
并且只接受：

```text
data/private/session-summaries/<非隐藏新文件>.json
```

目录由仓库根 `.gitignore` 的 `/data/private/` 规则排除。程序拒绝子目录、
其他扩展名、符号链接目录和已存在目标；不会覆盖或追加。它先在同一私有
目录创建仅新建临时文件，写入、刷新并同步完整内容，再用不覆盖的硬链接
建立最终名称，最后删除临时名称。若最终名称在会话期间被别的进程占用，
保存失败且原文件不变。

文件是一行固定字段顺序的 `ziranma-session-summary-v1` JSON。除终端汇总
的数字与布尔值外，只增加静态的 schema 名和 `contains_text=false`；
没有墙上时钟、PID、目标名称、文字、拼音、具体按键或候选列表。即便
如此，它仍是私人行为元数据：不提交、不上传、不复制进 issue。只有
另一个显式只读命令 `summary-report` 能打开操作者逐个点名的摘要；它不
扫描目录，也不写输出文件，详见[脱敏会话摘要汇总](summary-report.md)。
程序没有自动保留期限；不再需要时由宝宝对精确文件执行普通本地删除。

## 显式保存私人事件胶囊

私人胶囊和脱敏摘要是两种不同权限。胶囊保存真实文字与按键，必须同时
给出 `--capture-keys`、`--save-capsule <新 .zic>` 和
`--allow-private-plaintext`；缺少任一项就拒绝启动。READY 会清除之前的
胶囊事件，STOP 后才一次性落盘。固定路径、容量上限、严格格式、删除边界
与无文本离线回放见[私人事件胶囊与离线回放](event-capsules.md)。

## 已验证与当前停止条件

Codex 中已经用纯人工字符串验证：

- 组合内输入错误、退格后重输再提交；
- 上屏后退格删除，再输入补回；
- Esc 取消尚未完成的组合；
- 候选窗临时取得 UIA 焦点时，组合期按键仍完整且提交后立即结束宽限。
- 重复字符中移动光标后退格，位置由 `Caret` 证据正确消歧。
- 在目标框内用 READY 热键启动后，第一枚自然码按键完整且
  `keys_complete=true`。
- 输入框失焦到同进程普通消息区域时，方向键不进入下一条提交；切到其他
  前台进程时，字母键同样不进入记录。
- 数字键选择非首选候选时，完整记录自然码按键、`Digit(2)`、组合拼音和
  最终上屏字，且只产生一次提交。
- 长草稿中用 Home/方向键定位、Shift 选择、Delete 删除后，位置与动作
  完整；句中补入组合即使两类 UIA 事件错拍，也只产生一条最终提交。
- 选中“错”后不先删除、直接组合输入“在”，两次人工运行都同时得到
  `preedit: zai → 在` 与 `document: 错 → 在`，位置为 `UniqueText`，
  限域自然码按键与候选键完整。
- 一次人工准备文本时观察到同一组合会从纯拼音逐段变为
  `jia'cuo'yi → 甲cuo'yi → 甲错yi`。它曾被误判为新会话并截断按键；
  状态机现只在旧组合仍显示且新旧载荷含非 ASCII 字符时延续会话，并用
  这条精确事件形态做合成回归。后续普通整段提交人工复测的七枚按键和
  `document: "" → 甲错乙` 完整；逐段转字形态本身尚未第二次人工复现。
- 显式使用 `--preview-candidates --candidate-gap-ms 15000` 的人工短会话
  同时产生两种保守候选：直接把“错”换成“在”得到
  `DirectReplacement/ReplacedWithDifferentText`；随后 Backspace 删除
  “在”再补回得到 `DeleteThenInsert/RestoredSameText`，实际删补间隔
  2218 ms。两条位置均为 `UniqueText`、`keys_complete=true`，来源提交
  序号也能回指相邻原子事实。
- 第二次补入中人工短句故意保留了一次真实发生的组合内误键：
  `z, k, Backspace, l, Space`。候选仍以最终 UIA 组合 `zai` 上屏为“在”，
  同时完整保存限域按键动作，证明归并没有抹平组合内部的回删重打。
- 第一次脱敏汇总复测从单个字 Backspace 到空框时，原子修订异常显示
  “删除 1、插入 5”，定位到 Chrome 重新暴露了带换行的“随心输入”
  占位符。状态机现会在无活跃组合时统一归一这个窄空框表示，并用
  “单字 → 占位空框 → 再输入单字”的合成轨迹保护。
- 修复后的脱敏人工复测确认相同动作恢复为“删除 1、插入 0”，随后产生
  `DeleteThenInsert/RestoredSameText`；实际间隔 999 ms，位置
  `UniqueText`、`keys_complete=true`，来源回指初次提交。终端全程只显示
  字符数与按键数，没有显示文字、拼音或具体按键值。停止汇总正确报告
  2 次提交、2 次修订、8 个逻辑动作、1 条先删后补候选和零条直接替换；
  候选之后额外发生的一条独立删除只计入修订，没有被误归并。
- 显式汇总导出已用一次人工单字会话真实验证：READY 后 1 次提交、
  3 个逻辑动作、完整按键证据，停止时依次打印 `SESSION_SUMMARY`、
  `SUMMARY_SAVED schema=ziranma-session-summary-v1 contains_text=false` 和
  `summary_saved=true`。最终文件仅新建于
  `data/private/session-summaries/manual-001.json`；`git check-ignore`
  确认它由 `/data/private/` 规则排除，普通 `git status` 不显示该文件。
  未读取或删除这份私人文件。

Codex 单输入框的人工兼容性清单至此完成。继续扩大到其他应用、鼠标候选
或持久化私人会话以前，必须单独做新的能力与隐私评审；不把本探针悄悄
扩成常驻通用记录器。

若最小单段差分在句中替换或复杂选择中不能无歧义表达，停止扩大探针，
改用 `TextPattern` 的光标附近有界范围；不退回全窗口焦点日志或无限制
全文快照。
