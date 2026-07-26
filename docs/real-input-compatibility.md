# 真实输入兼容性体检

## 目的

在开发任何长期运行的输入追踪器以前，先回答一个更小的问题：

> 宝宝常用的 Windows 输入框，能否通过公开的 UI Automation 接口，
> 可靠报告微软输入法正在组合和最终提交的文本？

这项体检只判断系统能力，不收集真实聊天，不评价解码准确率，也不把
当前仓库扩展成输入法或全局键盘记录器。

## 为什么先做体检

Windows UI Automation 的 `TextEdit` 模式可以报告输入法组合事件。
正确实现该模式的输入框应产生 `TextEditTextChanged`；输入法完成一次
上屏时，`CompositionFinalized` 的载荷是本次最终提交的字符串。

但这些事件由输入框的 UI Automation provider 提供，并非所有软件都
实现。因此，不能先假设 Codex、浏览器或聊天软件都能报告它，再围绕
这个假设开发采集器。

官方参考：

- [TextEdit Control Pattern](https://learn.microsoft.com/en-us/windows/win32/winauto/textedit-control-pattern)
- [TextEditChangeType](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationcore/ne-uiautomationcore-texteditchangetype)
- [Accessibility Insights for Windows](https://accessibilityinsights.io/docs/windows/overview/)
- [Event monitoring](https://accessibilityinsights.io/docs/windows/getstarted/eventmonitoring/)

## 安全边界

- 只在专门创建的测试输入框中使用人工短句。
- 不打开或输入真实聊天、密码、验证码、密钥、地址和个人词典。
- 不截屏，不读取剪贴板，不保存整份文档。
- 不把任何真实按键、提交文本或派生个人模型加入 Git。
- 体检结束后只保留下面的能力矩阵，不保留测试内容日志。
- 若以后开发采集器，必须由用户手动开始会话，并且只允许白名单进程；
  `IsPassword=true` 时必须拒绝记录。

## 准备

当前电脑在 2026-07-26 的只读检查中没有发现 Accessibility Insights，
但随后在 Windows 11 SDK 的版本目录中找到了 64 位 `Inspect.exe`：

```text
C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\inspect.exe
```

先前只检查旧版 SDK 的常见路径而漏报，不能据此判断工具不存在。
Accessibility Insights 的安装属于系统变更，已在宝宝明确同意后进行。

Accessibility Insights 官方安装说明：

<https://accessibilityinsights.io/docs/windows/getstarted/setup/>

首次体检不需要启用永久性的 UIAccess，也不需要让目标程序以管理员
权限运行。工具的可选遥测可以关闭。

## 要测的三类输入环境

不要追求覆盖所有软件。先选择宝宝实际最常使用的三个环境：

1. Codex 桌面应用的消息输入框；
2. Chrome 或 Edge 中的普通单行输入框和网页聊天输入框；
3. 另一个日常聊天或写作软件。

记事本可以作为接口基线，但不计入“常用环境通过数”。

## 每个输入框怎样测

### 一、检查属性和模式

1. 打开 Accessibility Insights，进入 `Inspect`。
2. 将焦点放进目标输入框。
3. 用 `Shift+F9` 返回检查工具并暂停 UIA Tree。
4. 记录以下项目，不记录输入框内容：
   - `ProcessId` 或进程名；
   - `ControlType`；
   - `IsEnabled`；
   - `IsKeyboardFocusable`；
   - `IsPassword`；
   - 是否支持 `TextEdit`、`Text`、`Value`。

### 二、监听事件

在目标元素的菜单中选择 `Listen to Events`，优先监听：

- `TextEditTextChanged`；
- `TextChanged`；
- `TextSelectionChanged`；
- 焦点变化事件。

依次做下面的人工动作：

1. 用微软自然码双拼输入“麻烦”，按空格确认；
2. 一次组合输入一个较长的人工短句；
3. 分两次提交“麻烦”和“猫猫”；
4. 用数字键选择一次非首选候选；
5. 若候选栏允许，再用鼠标选择一次候选；
6. 上屏后退格删除一个字，再重新输入；
7. 移动光标到句中，插入和删除一个字；
8. 按 `Esc` 取消一次尚未完成的组合。

每个动作只记录事件类型、事件顺序、载荷是否准确，不复制私人文本。

## 能力分级

| 等级 | 观察结果 | 工程含义 |
|---|---|---|
| A | 能收到 `CompositionFinalized`，载荷等于本次上屏文本 | 可以直接配对“按键段 → 提交文本”，不做全文差分 |
| B | 没有最终组合事件，但有可靠的 `TextChanged`，并能读取光标附近的小范围文本 | 只为该控件实现有界的局部差分 |
| C | 没有可靠文本事件，或只能不稳定地读取整份内容 | 不做通用外部追踪；需要应用适配器或自有输入法 |
| X | `IsPassword=true`，或内容属于明确敏感输入 | 永远不记录 |

单纯能读到 `Value` 不足以判为 A；它可能迫使采集器反复读取整个输入框，
也不能可靠区分输入法组合、上屏和后续编辑。

## 结果表

体检时复制此表，只填写能力，不填写输入内容：

| 日期 | 应用/版本 | 输入框类型 | TextEdit | Text | Value | Finalized | 普通变更 | 分级 | 备注 |
|---|---|---|---:|---:|---:|---:|---:|---|---|
| 2026-07-26 | Codex 桌面应用（Chrome provider） | ProseMirror `Edit`，名称“随心输入” | 是 | 是 | 是 | 未观察到 | 是，提交后 `Value.Value` 为最终文本 | B | 组合事件载荷为完整无声调拼音；普通文本事件可确认上屏 |

### Codex 的人工短句结果

使用 Windows SDK 自带的 `Inspect.exe` 和 `AccEvent.exe`，在一个人工
短句上观察到：

```text
实际自然码按键     mkmk
组合事件           m → mao → mao'm → mao'mao
普通文本值         m → mao → mao'm → mao'mao → 猫猫
最终组合事件       未观察到
```

这证明 Codex 输入框不是 A，但已经是稳定的 B：`TextEditTextChanged`
提供组合拼音，`Text_TextChanged` 后的 `Value.Value` 提供最终文本。
同一最终值会重复报告，必须按值去重。空 ProseMirror 还可能把
`"\n随心输入"` 暴露成空编辑框的值。原型在没有活跃组合且值是“被空白
包裹的精确控件名”时把这个窄特例归一为空，包括从非空删到最后一个字；
用户真正输入精确字面值“随心输入”不会被过滤。

仓库自己的探针已用同一段人工输入完成一次端到端验证：

```text
按键记录    [m, k, m, k, Space]
组合记录    m → mao → mao'm → mao'mao
提交差分    deleted="mao'mao", inserted="猫猫"
```

这条记录只在终端预览并随进程退出消失，没有写入文件。退格、取消和句中
删改目前已有合成状态机覆盖。第一次人工纠错复测还发现：候选窗会在组合
期间暂时取得 UIA 焦点，若只按“当前焦点必须是输入框”过滤，会漏掉后续
组合键。探针因此只在“组合从精确目标开始、前台 PID 仍匹配、组合尚未
提交或取消”三个条件同时成立时延续捕获。第二次人工复测确认组合内退格、
提交后删除并补回、Esc 取消三条轨迹的按键与文字差分都完整，修复有效。

重复的相同汉字还暴露了第二类歧义：仅凭全文前后值会把句中 Backspace
误定位到重复串末尾。加入 `TextPattern.GetSelection` 后，人工复测已用
编辑后光标把该删除正确恢复为句中位置，并以 `position=Caret` 区别于
文本唯一位置和无法消歧的位置。

从终端切回输入框时，异步焦点事件仍可能晚于第一个按键。探针现在不会
猜回这个键，而是用 `keys_complete=false` 排除残缺样本；同时改成两段式
启动，只有在目标框内按 `Ctrl+Shift+F11` 并同步通过精确焦点复核后才开始
按键配对。人工复测确认 `KEY_CAPTURE_READY` 先于第一段组合，完整记录
`mkmk` 且输出 `keys_complete=true`，没有首键缺口警告。

失焦边界也已用人工按键复测：目标框提交后，焦点先移到同一应用的普通
消息区域并按方向键，再移到其他前台进程输入字母；重新聚焦目标后的下一
条提交只包含新的自然码按键，没有混入上述失焦按键，也没有产生额外文本
修订。

数字候选也已验证：人工输入 `ui` 产生组合 `sh → shi`，按数字 `2` 后，
探针把 `[u, i, Digit(2)]`、组合 `shi` 和上屏“十”配成唯一一条完整提交。

为了继续验证句中选择，受限按键模型已保留 Shift 修饰语义：
`Shift+Left` 输出为 `Shift(Left)`，而不是普通 `Left`；单独 Shift 不记，
Ctrl/Alt/Win 仍拒绝。重复字符选择删除已有合成状态机覆盖，等待一次人工
长草稿复测。

第一次人工长草稿复测确认 Shift 选择、Delete 和 `position=Caret` 正确，
同时发现专用组合事件会领先普通 Value：前者已到 `mao'mao` 时，后者仍按
`mao'm → mao'mao` 追赶。状态机现将纯小写拼音/撇号差分保留为 preedit，
不再提前生成伪提交。第二次人工复测确认句中 `Home + Right` 后补入
`mkmk + Space` 只产生一条完整提交，五枚输入键、组合、`start=1` 和
“猫猫”全部配对，没有碎片修订。

直接选中替换又验证了组合提交必须保留两层差分。两次人工运行中，选中
“错”后不先删除、直接输入“在”，都得到 `preedit: zai → 在` 和
`document: 错 → 在`；后一层才是可用于纠错候选的原文净变化。第一次
准备人工短句时，微软输入法还把同一次组合依次报告为
`jia'cuo'yi → 甲cuo'yi → 甲错yi`，暴露出局部转字会被误切成新会话。
状态机已用精确合成轨迹覆盖这种形态；普通整段提交的后续人工复测确认
七枚按键以及 `document: "" → 甲错乙` 完整。由于后续运行没有再次产生
逐段转字载荷，不把这一分支写成已经二次人工复现。

纯内存候选预览也已用同一人工短句完成一次双形态验证。在显式
`max_gap_ms=15000` 的会话中，直接选中替换产生
`DirectReplacement/ReplacedWithDifferentText`；随后 Backspace 删除
“在”再补回，产生 `DeleteThenInsert/RestoredSameText`，观察到的实际
间隔为 2218 ms。两条候选的文字位置均为 `UniqueText`，按键证据完整，
来源序号能回指相邻提交。第二次补入还自然包含
`z, k, Backspace, l, Space` 的组合内误键与修正，最终 UIA 组合仍为
`zai`；探针同时保留原始动作和规范化组合，没有把其中任一层冒充成另一层。

后续脱敏汇总人工验证启动时，精确目标查询曾瞬时返回零个元素；同一 PID
稍后用 `--check` 又得到唯一安全目标，说明是 UIA 树重建竞态而非 PID
迁移。发现逻辑现只对零结果以完全相同的白名单条件做五次 100 ms 有界
重试；多个匹配仍立即拒绝，不能借重试变成模糊选择。

同一次脱敏汇总复测还发现，从单个字 Backspace 到空框时，Chrome 不是
报告空串，而是重新暴露带换行的控件名，导致原型一度输出“删除 1、
插入 5”并把随后补回误分为直接替换。上述占位归一规则已扩展到这条
非空到空转变，并加入“单字 → 占位空框 → 再输入单字”的合成回归。

修复后的同动作复测已完成：删除事件为“删除 1、插入 0”，补回产生
`DeleteThenInsert/RestoredSameText`，间隔 999 ms，位置唯一、按键证据
完整且能回指初次提交。关闭 `--preview-text` 后，原子事件、候选与
`SESSION_SUMMARY` 均未出现文字、拼音或具体按键值。汇总正确保留候选后
额外发生的一条独立删除修订，但没有把它误接到已经完成的候选上；占位
修复和脱敏会话汇总至此都有真实 UIA 证据。

显式私有汇总导出随后也完成一次最小人工验证。READY 后的单字会话报告
1 次提交、3 个逻辑动作和完整按键证据；停止时成功创建
`ziranma-session-summary-v1`，并明确报告 `contains_text=false`、
`records_were_memory_only=true`、`summary_saved=true`。目标
`data/private/session-summaries/manual-001.json` 经 `git check-ignore`
确认命中 `/data/private/`，没有出现在普通 Git 状态中。验证只核对终端
证据与忽略规则，没有读取或删除私人文件。

AccEvent 的全局 `FocusChangedEvent` 还会把聊天区域的辅助功能名称写进
调试窗口。这个结果不是采集需求，而是隐私警报：正式原型不得保存任意
焦点元素名称，只能在内存中判断“是否为白名单输入框”。上述完整
AccEvent 日志不会进入仓库。

## 继续与停止判据

完成三个常用环境后再决定，不边测边扩张需求：

- 至少两个环境为 A：继续做一个小型、事件驱动的通用采集原型。
- 一个环境为 A，其他为 B/C：只为 A 环境做最小原型，用它采集第一批
  私有但不入库的真实评测；暂不追求通用。
- 没有 A，但至少一个稳定 B：只允许做一次、一个应用的局部差分实验。
  若不能无歧义重建提交与修改，立即停止。
- 三个环境大多为 C：停止通用外部 tracker，不再增加全局钩子和全文
  快照；转向应用插件，或提前研究自有 Windows TSF 输入法壳。

无论结果如何，都不因为 tracker 困难而阻塞解码器的公开评测。真实采集
是补充证据，不是让现有研究继续运行的前置条件。

## 通过以后才讨论的最小原型

只有通过上述判据后，才设计实现：

1. C#/.NET 小进程通过 UIA3 监听焦点元素和文本事件；
2. 仅在手动会话和白名单进程中暂存带时间戳的按键；
3. 收到 `CompositionFinalized` 后生成一条按键段与提交文本的配对；
4. 提交后的光标移动、删除和插入作为独立修订事件保存；
5. 原始内容使用本机加密的私有存储，与 Rust 仓库和公开测试完全隔离；
6. Rust 解码器只读取经过宝宝明确挑选、脱敏或临时挂载的评测输入。

实现组件可优先评估 MIT 许可的
[FlaUI](https://github.com/FlaUI/FlaUI)。如果 FlaUI 没有封装需要的
`TextEdit` 事件，再通过它暴露的原生 UIA3 对象调用 Windows 接口，而
不是另造一套 UI Automation 封装。

Codex 已满足“一个稳定 B”的分支，因此仓库现在只实现
[一个 Codex 白名单、内存预览的局部差分探针](tracker-probe.md)。
它不是通用追踪器；另外两个常用环境没有完成体检以前，不扩成后台常驻
采集器。
