# Windows TSF Alpha：自有输入法壳

## 状态

这是用户明确批准的新里程碑。当前已完成与宿主无关的 `CompositionSession`
抽取、TSF COM 生命周期探针，以及只在测试进程中工作的按键、真实组合范围、
焦点清理和公开候选上屏闭环。仓库也提供本机范围、64 位、默认关闭的显式
开发注册和反向注销事务；它不会启用、激活或设为默认输入法。真实宿主输入
已开始验证：固定公开用例在 Windows 记事本完成 `nihk` →“你好”、切回微软
拼音和禁用闭环。另有显式的当前用户启用和禁用命令；启用不设为默认，也不
请求进程或桌面范围激活。Windows 可能只在执行命令的辅助线程内短暂激活配置，
因此换代脚本会在辅助进程退出后另起进程确认它已经持久启用且不再活动。上一
构建已在记事本中显示系统静态控件原型；下一构建改为不抢焦点的自绘 Alpha
面板并绑定完整公开候选包，仍需真实宿主复核外观、翻页和上屏。

## 目标

第一版 Alpha 只回答一个问题：现有解码器能否在真实 Windows 编辑框中完成
一轮稳定、可回退的日常输入，并让后续功能直接依据自有输入法事件评测。

首批宿主限定为：

- Windows 记事本，用于验证标准 TSF 编辑生命周期；
- Codex 输入框，用于验证主要日常场景；
- 一个仓库内的合成宿主测试，用于覆盖异常激活、焦点切换和组合取消。

Alpha 不取代微软拼音，不设置默认输入法。安装后只能由用户通过 Windows
输入法切换器主动选择；任何时候都必须能切回微软拼音。

## 分层

```text
宿主编辑框
  ↕ Windows TSF 编辑会话
极薄 TSF 适配层
  ↕ CompositionInput / CompositionEffect
CompositionSession
  ↕ 候选请求与选择
现有 Decoder / ShapeLab / 本轮选择记忆
```

`CompositionSession` 只拥有当前组合串、换序/Tab 模式和候选页位置。已提交
文字属于宿主文档；TSF 适配层提交成功后调用 `finish_commit`。候选解码、
候选窗口、Windows 注册、私人学习和磁盘存储都不进入该状态机。

没有活跃组合时，Space、Backspace、Tab、翻页键和 Esc 返回 `PassThrough`，
由 TSF 交还宿主；终端适配器只把空输入下的 Esc 解释为退出实验台。

这条边界让终端实验台和 TSF 共用同一组按键语义，又避免把终端的退出行为、
滚动输出或累计文字上限带进真实输入框。

## 实施顺序

### 0. 可复用组合核心

- 从 `typing-lab` 抽出宿主无关状态；
- 终端继续作为回归宿主；
- 所有既有逐键场景保持通过；
- 不执行任何系统注册。

### 1. TSF 生命周期探针

- 创建最小 Windows `cdylib` 边界；
- 实现 COM 类工厂、激活、停用、服务器锁和安全卸载边界；
- 用真实的系统 `ITfThreadMgr` 在测试进程内验证生命周期与失败清理；
- 当前 DLL 只导出 `DllGetClassObject` 和 `DllCanUnloadNow`；刻意不提供
  `DllRegisterServer` / `DllUnregisterServer`，因此不能被 `regsvr32` 注册；
- 文本服务已实现 `ITfKeyEventSink`，正式激活路径会申请前台按键接收，并在
  停用时成对释放；
- 文本服务也会通过 `ITfSource` 成对订阅和释放 `ITfThreadMgrEventSink`，用来
  识别文档焦点离开当前组合所在 Context；
- 未注册测试进程拿到的是应用 client id，Windows 会拒绝用它冒充前台文本
  服务。测试因此只绕过“前台订阅”这一项，直接调用同一个按键接收器；
- 测试按键仍经过真实的 Thread Manager、Document Manager、Context 与同步
  `RequestEditSession`。事务用 `ITfRange` 修改合成文档，同时只保留有界的
  “更新/取消/提交”结构遥测，不保存原文；
- DLL 同目录没有 `candidate-data` 时，默认类工厂使用进程内只初始化一次的
  开发候选源。它复用 50 词公开演示词典，先解析严格的候选包清单，再通过
  [只读候选快照](candidate-snapshots.md)校验 schema、版本、字节数、指纹和
  词条数后建立现有 `Decoder`；`candidate-data` 存在时，新类工厂只读其中
  经过预检的公开 current。两条路径都不学习、不联网；
- 注册前仍可只构建和检查；获得用户明确确认后，`tsf-devctl` 可以把经过检查
  的 DLL 复制到按 SHA-256 寻址的本地不可变目录，并完成本机开发注册。

当前构建命令为 `cargo build --release --lib`，产物是
`target/release/ziranma_core.dll`。它只生成未注册的 DLL；单元测试会在测试线程
中创建并释放系统 TSF Thread Manager，不写注册表、不创建输入配置文件，也不
改变 Windows 输入法列表。

注册前可运行[TSF 开发检查](tsf-dev-inspection.md)，只读核对 DLL 的 PE 架构、
COM/注册入口、证书目录、固定 CLSID 的标准 COM 注册位置、固定 zh-CN
语言配置和键盘类别。显式注册与注销命令都要求
`--confirm-machine-wide-development-alpha`；注册固定为本机 64 位且默认关闭，
需要管理员权限，不包含启用或激活。“证书目录存在”也不冒充签名验证。
候选包的 Ed25519 脱离验签是另一条数据发布边界，不验证 DLL / EXE 的
PE/Authenticode 签名。

### 2. 最小可输入闭环

- `a`～`z`、Backspace、Esc、Space、Enter、中文逗号/句号与数字键的路由和
  事务计划已有进程内覆盖。Space 确认首选，活跃组合上的 Enter 原样提交当前
  字母码；没有组合时 Enter 交还宿主。中文模式下未按 Shift 的 `,` / `.` 分别
  上屏 `，` / `。`，活跃组合会先确认首选再追加标点，Shift 组合仍交还宿主；
- 已用 `ITfInsertAtSelection`、`ITfContextComposition` 和 `ITfRange` 在合成
  Context 中创建、更新、取消与提交组合；
- 测试会逐步读回合成正文，验证同一范围更新、候选替换、取消删除以及提交后
  光标落在文字末尾；
- 另一个测试从 `IClassFactory::CreateInstance` 开始，在合成 Context 中输入
  `nihk`，确认预编辑为 `nihk`、首选提交为“你好”。它覆盖正式对象创建路径，
  不使用系统注册；
- 词典不能完整解释的首选会原样提交按键串，不插入研究解码器用于报告的
  未解析标记，也不吞掉已有预编辑；
- 开发快照若校验失败，正式类工厂会拒绝创建文本服务；文本服务内部若没有
  候选源也不接管按键。合成测试分别覆盖这两道失败边界；
- `ITfCompositionSink` 已处理宿主主动终止：使用回调给出的写入权限删除未提交
  的预编辑文字、归位光标并清理内部状态；
- 文档焦点离开、前台输入法切换和文本服务停用都会为旧 Context 排入异步取消。
  取消任务绑定到发起时的确切组合对象；如果宿主先终止旧组合、用户随后开始
  新组合，迟到的旧任务不会删除新输入；
- 合成测试会直接触发同一焦点接收器并覆盖上述竞态。注册后的真实宿主是否按
  预期时序投递焦点事件和异步编辑会话，仍需继续扩大测试；
- 未激活或没有组合时，普通按键交还宿主。
- 默认进入中文模式。没有按其他键的单独一次 `Shift` 在中文与英文模式之间
  切换；若仍有预编辑，先提交当前首选，再完成切换。`Shift` 参与组合键时不
  切换模式；
- 英文模式不接管普通按键或宿主快捷键。中文模式下的 `Shift + 字母` 与
  Caps Lock 字母也交还宿主；若切换发生在预编辑中，先结束当前首选，避免
  英文字母落入未完成的拼音组合；
- 模式目前属于每个宿主中的文本服务实例，不修改 Windows 默认输入法，也
  尚未接入系统输入模式指示器。
- 注册后的 Windows 记事本已完成一次固定公开 `nihk` →“你好”首选提交；
  随后切回微软拼音并通过显式命令禁用，未修改默认输入法。

### 3. 候选窗口与现有功能

- 首次组合只解码 Top-14；翻到已加载末页后按七项一页逐步扩展，最多 Top-50。
  同一组合缓存当前最深的确定性前缀，返回前页和确认候选不会重复缩小或重新
  解码；日常首屏不承担深候选的固定延迟。快照对任意长度先合并完整、零纠错、
  不简写的词典项，再补连续句子候选并按文字去重，避免准确完整码被大量自由
  简写路径挤出 Top-50。`ju` / `qu` / `xu` / `yu` 中无歧义的 `u` 写法也可
  沿用自然码规范 `v` 词条，因此 `ju` 能显示“句”，`uuyu` 能优先显示“属于”；
- 实现标准 `ITfCandidateListUIElement`，普通宿主与 UIless TSF 宿主读取同一
  候选、页位置和当前选择；
- 普通宿主使用最多七项的不抢焦点弹窗，通过 `ITfContextView::GetTextExt`
  跟随真实组合范围，而不是读取屏幕或猜光标；
- `GetTextExt` 的 clipped 标志只作为宿主布局提示，不再直接隐藏弹窗；部分宿主
  会在刚更新组合时暂时报告 clipped，但仍返回可用的定位矩形；
- `-` / `+` 与 PageUp / PageDown 逐页切换，最多按需展开至十页；数字键按
  当前页选择；
- 数字键明确选择的普通候选会进入当前文本服务实例的有界内存记忆；再次输入
  完全相同的码串时，该候选稳定移到首位。空格确认首项、Enter 原码提交、换序
  候选和 Tab 找字都不产生记忆。最多保留 128 组，文本服务停用即清空，不序列化、
  不写文件，也不改变公开候选包；
- DLL 还内嵌两份项目自有的 MPL-2.0 小词表，只在完整码精确匹配时补充公开
  Rime 快照：电子硬件层包含“丝印、焊盘、过孔、数据手册、使能、片选”等词，
  会话层包含“呜哇”。它们不改写固定第三方快照，并与公开候选按文字去重；
  多来源候选包拥有独立来源声明前，这两份小词表保持为明确的开发 Alpha 覆盖层；
- 提交、取消、宿主终止组合、焦点离开和文本服务停用都会结束 UI 元素并隐藏
  弹窗；
- 自绘 Alpha 面板每页最多显示 7 个候选。首项使用轻微提亮、上下留白的圆角底
  和左侧柔和天蓝色短条标记，避免选中态变成一块沉重按钮；首选汉字采用近白
  半粗体，普通汉字降为柔和浅灰。序号使用更小的中灰元数据字体，并通过固定
  基线补偿与候选文字对齐，只有首选序号沿用天蓝。分页再退一级，并以低对比
  分隔线进入固定尾部区域。这个“候选文字优先、选择状态其次、操作元数据最后”
  的层级避免七个等亮标签同时争抢注意力；普通文字与背景至少保持 4.5:1 对比度。
  短候选在可用空间内默认横排，候选过长或屏幕太窄时确定性回落为竖排。弹窗按
  宿主 DPI 缩放并限制在当前显示器工作区。“换序”模式标记使用独立强调色，与
  右侧页码分开显示。内容更新先在兼容内存 DC 完成，再一次贴到屏幕；位置或尺寸
  不变时不重复 `SetWindowPos`，尺寸不变时也不重设圆角区域，减少快速组合更新中
  的空白帧。系统主题、高对比度、屏幕阅读器与候选窗口消失/重建仍需真实宿主
  验证；
- Shift+Tab 打开单独标有“换序”的恢复候选，Esc 返回普通候选。它只允许一次
  相邻按键颠倒；交换后的候选不能再使用第二次纠错，并且只能由完整双拼或
  首音节完整、尾部简写的片段组成。恢复搜索限于 16 键，普通候选仍保持
  零错误优先，不会因容错视图而偷偷换首选；
- Tab 单字笔画辅助；
- 当前安装仍指向上一份不可变 DLL；新候选窗口必须在 Alpha 保持禁用时完成
  注销、重新注册，再由用户显式启用进行下一轮记事本测试。

### 4. 直接反馈与个人候选

自有输入法可以直接记录“按键、展示、选择、提交和取消”的结构化事件，不再
从 UI Automation 猜测候选行为。若后续进入持久化阶段，仍须沿用当前用户
DPAPI 加密、无网络、显式会话和可停止边界。安全输入、密码框以及不允许文本
服务的输入范围一律不学习；默认不保存宿主周围正文。

先离线评测精确的“有效按键串 → 实际提交文字”，再决定是否创建持久个人
模型。记录过程不实时改变正在使用的模型。

第一步的宿主无关事件核心和显式前台控制已经完成，但仍只属于开发 Alpha：

- 正常激活的 TSF Alpha 默认关闭反馈；不会因激活文本服务、显示候选或输入
  文字而自动开始；
- 启动内存会话必须在调用点构造 `explicit_memory_only` 授权；默认构造、激活
  文本服务或显示候选都不会顺带开启；
- 内存会话具有明确的 `Disabled`、`Recording` 和 `Stopped` 状态。重复开始不会
  清掉正在记录或已经停止但尚未处理的事件；必须先停止，才能显式清空并开始
  下一轮。文本服务停用会结束正在记录的会话，但保留其内存内容直至服务对象
  释放；
- 前台文本服务把控制放在 TSF 原生输入模式语言栏项目中，不注册新的全局
  快捷键，也不与持续记录器的 `Ctrl+Shift+F10/F11/F12` 冲突。项目平时显示
  “中”或“英”；现代任务栏使用随系统文字颜色变化的 16×16“自”或“A”
  单色图标，仅内存反馈正在记录时显示一个圆点；
- 菜单只提供当前状态允许的动作：未开始时为“开始反馈（仅内存）”，记录中为
  “停止反馈”，停止后为“清除本轮”。停止后的会话不能被再次开始覆盖；先清除
  后才能开始下一轮；
- 语言栏项目在文本服务激活时添加、停用时移除。若宿主没有提供该可选界面，
  输入法仍可使用，但反馈保持关闭；
- 事件只存在于调用进程内存，不写文件、不使用 DPAPI、不建立 IPC，也不连接
  网络；
- 只接收输入法自身已经展示的候选页，以及文档编辑成功后的选择、提交和取消；
  不读取密码、宿主周围正文或其他应用内容；
- 空格确认首选、Enter 原码提交、标点确认首选、数字选择、普通候选、换序候选、
  候选绝对名次、退格取消和 Esc 取消分别保留结构化身份；
- 单页最多保留 7 个候选，单次会话默认最多 4096 条事件和 1 MiB 私人文字。
  达到上限时停止接收并标记为不完整，不会丢弃旧事件后继续声称记录完整；
- 私人事件不实现调试输出或序列化；对外状态只包含条数、类型计数和是否完整。

下一阶段的用户留言、事发前短时缓冲、证据分层授权、独立前台伴随进程和 DPAPI
反馈包见[“向猫猫许愿”：本地反馈与现场快照](wish-feedback.md)。在该边界完成前，
当前菜单仍只保留内存事件，不把“开始反馈”冒充可持久提交的许愿入口。

每条事件还必须通过独立的上下文资格门。前台 TSF 路径会先读取键盘禁用和空
上下文 compartment，再读取当前组合范围的 `GUID_PROP_INPUTSCOPE`：

- 只有明确报告普通文本、聊天、搜索、中文文本或 native-script 的范围进入
  允许状态；
- 密码、数字密码、PIN 和 `IS_PRIVATE` 分别归入敏感状态并拒绝；
- 属性缺失、读取失败、未来未知值以及 URL、邮箱、姓名、地址等其他专用范围
  都不会按普通文本猜测，而是拒绝；
- 翻页和换序这类没有编辑 cookie 的 UI 操作只复用同一组合最近一次成功检查，
  码串不一致或没有检查结果时拒绝；
- 拒绝只增加脱敏计数，不保存被拒绝事件；合成测试覆盖允许、密码、私人、
  键盘禁用、空上下文、受限和未知状态。

首轮真实 Windows 11 验收确认：只有文字而返回空 `HICON` 的项目不会出现在
现代任务栏；Typora 已加载目标 DLL，因此该结果不是旧进程缓存造成。源码现已
补齐主题感知单色图标，下一轮换代只验证按钮可见性、默认关闭以及受限输入框
拒绝。Codex 仍可能继续持有换代前 DLL，必须用新启动的宿主验收，不能强制从
正在运行的进程卸载。再下一阶段才讨论跨进程生命周期和当前用户 DPAPI 落盘。

上述读取顺序与 Microsoft 的
[ITfInputScope](https://learn.microsoft.com/en-us/windows/win32/api/inputscope/nn-inputscope-itfinputscope)
示例一致：通过 `ITfContext::GetAppProperty(GUID_PROP_INPUTSCOPE)` 和当前范围
取得接口。官方
[InputScope](https://learn.microsoft.com/en-us/windows/win32/api/inputscope/ne-inputscope-inputscope)
说明还明确指出 `IS_PASSWORD` 本身不提供安全性，密码字段应禁用文本服务；
因此实现不会把“看见密码 scope”当成唯一防线。

语言栏生命周期遵循 Microsoft 的
[Language Bar (Text Services)](https://learn.microsoft.com/en-us/windows/win32/tsf/language-bar)
约定：文本服务在 `Activate` 添加项目，在 `Deactivate` 移除，并用
`ITfLangBarItemSink` 通知状态变化。Windows 8 以后的
[TF_LANGBARITEMINFO](https://learn.microsoft.com/en-us/windows/win32/api/ctfutb/ns-ctfutb-tf_langbariteminfo)
要求输入模式项目使用 `GUID_LBI_INPUTMODE`；因此这里复用同一个“中/英”入口，
没有伪造第二个可能被系统忽略的反馈按钮。微软公开
[SampleIME `LanguageBar.cpp`](https://github.com/microsoft/Windows-classic-samples/blob/77f217b3f89d4dac7864a62cc91ff7b569f26a50/Samples/IME/cpp/SampleIME/LanguageBar.cpp)
为 `TF_LBI_STYLE_SHOWNINTRAY` 项目返回真实的 16×16 图标；现代任务栏不会可靠
绘制只有文字而返回空图标的项目，因此 Alpha 同样从 `GetIcon` 返回透明背景
单色图标，并用 `TF_LBI_STYLE_TEXTCOLORICON` 交给系统适配明暗主题。

## 安装、升级与回退边界

日常本地开发换代使用仓库根目录的 `update-ime.cmd`。它按自身位置寻找底层
`scripts/replace-tsf-alpha.ps1`，因此可以从任意当前目录运行，也可以由资源
管理器双击；不接受额外参数，不自行编译或下载内容。PowerShell 执行策略参数
和重新启用开关由包装入口固定传入。底层脚本先验证 release DLL 相邻的公开
候选槽，把它复制到 DLL SHA-256 对应的不可变构建目录并再次只读检查，然后
禁用当前用户配置、在单独管理员阶段完成注销和注册，最后重新启用。候选槽
缺失时脚本拒绝继续，不会安装后静默使用 50 词开发包。

`tsf-devctl inspect` 还会汇总可见进程中已经加载的 Alpha DLL：只区分与本次
检查 DLL 相同或不同的不可变版本并报告数量，不输出进程名、窗口标题、模块
路径或输入内容。这个只读提示用于区分“安装没有更新”和“既有宿主仍缓存旧
DLL”；无法枚举的受保护进程不冒充已检查。

入口会先核对 release DLL 摘要、严格安装凭据、候选槽状态和实际注册布局。
如果它们已经对应同一版本，只验证并确保当前用户启用，不再复制、注销、注册
或请求管理员权限。确有新 DLL 时仍执行完整事务；注册工具本身已经同步回读
注册结果并轮询注销传播，因此包装脚本不再额外固定等待两秒。相同 DLL 摘要下
若候选槽状态不同，入口拒绝改写不可变安装目录，要求先产生新的 DLL 构建。
控制器分别使用系统安装枚举和当前用户启用状态查询确认语言配置；刚注册但尚未
启用的配置不会因未出现在当前用户配置枚举中而被误判为缺失。当前用户切换会
先调用旧 `EnableLanguageProfile` 作为兼容通知，但不会再把它返回的 `S_OK`
当作持久成功证据；随后总是调用 Vista 起推荐的
`ITfInputProcessorProfileMgr::ActivateProfile`，以
`TF_IPPMF_ENABLEPROFILE` / `TF_IPPMF_DISABLEPROFILE` 的结果为准。调用不包含
设为默认、进程范围或桌面会话范围的标志。

现代接口可能在执行命令的短生命周期辅助线程内暂时把配置标成活动，因此同一
进程内的启用事务只验证注册完整且启用位已经出现。换代脚本会等该进程退出后，
再由独立进程确认“已启用、未活动”并观察一个稳定窗口；摘要只报告已经通过的
持久状态，不把请求冒充结果。若本次登录的 Windows 文本服务仍缓存换代前的
profile，它可能在维护进程退出后撤销刚写入的当前用户状态；此时机器注册保持
完整且当前用户保持安全禁用，应在下次重新登录后只运行当前用户启用步骤，不必
重新构建 DLL。成功路径只显示结果、DLL 摘要、当前用户启用状态和默认输入法
边界；底层检查的详细输出仅在失败时展开。

在任意 PowerShell 当前目录中均可运行：

```powershell
D:\IME\ziranma-decoder\update-ime.cmd
```

- 不直接写注册表设置默认键盘；注册使用 TSF 正式接口；
- 安装、注销、升级和回退必须是不同的显式操作；
- 更新前先切换到其他输入法，并等待旧 DLL 不再被新宿主加载；
- current / candidate / previous 的版本槽位不复用正在加载的文件；
- 注册失败或候选版本启动失败时，微软拼音和 previous 必须保持可用；
- 不申请网络能力，不把解码或私人数据发送给其他进程；
- 在 Windows PE/Authenticode 签名与兼容要求没有验证前，不称为可分发安装包。
- Vista 以后优先使用 `ITfInputProcessorProfileMgr` 管理语言配置；键盘 TIP 类别
  通过 `ITfCategoryMgr` 明确注册。标准 COM 激活信息、TSF 配置和类别必须各自
  有可验证的反向清理；
- DLL 同目录没有 `candidate-data` 时，默认类工厂只有 50 词开发候选源。它足以
  验证对象创建、解码和上屏，不足以日用；只读检查通过也不等于适合安装。
- 独立 `candidatectl` 能确定性生成公开包并原子管理
  current/candidate/previous。新类工厂会从 DLL 相邻的固定槽根读取 current；
  已有类工厂继续持有旧快照，活动组合不会热换。配置目录存在却无效时拒绝
  建立类工厂，不静默使用演示包。
- `candidatectl preflight` 会用包内确定性探针创建真实系统 Thread Manager 与
  合成 Context，经过同一个类工厂、预编辑和首选上屏。`adopt` / `stage` 只有在
  独立可信包摘要、来源、许可、解码兼容性和该预检全部通过后才写材料绑定
  凭据；提升和回退会复核凭据与当前包内容。
- `candidatectl verify-signature` 可以用独立渠道取得的公钥只读验证候选包
  Ed25519 脱离签名，并给出供 `--expected-sha256` 使用的发布摘要；
  `adopt-signed` / `stage-signed` 也可在任何槽位写入前完成相同验签，并把同一
  份已验证材料交给安装与预检。所有命令都要求显式公钥，不发现或保存信任根；
  当前也没有真实发布公钥、签名命令或密钥轮换与吊销策略。
- `tsf-devctl register-machine` 先把 DLL 固定到
  `.local/tsf-alpha/builds/<sha256>`，再依次注册本机 64 位 COM、TSF
  文本服务身份、默认关闭的 zh-CN 配置和键盘类别。任何失败都会逆序撤回；
  注销也会先复核严格安装记录与系统状态，再执行反向事务。注册、启用、激活
  和设为默认仍不合并。

微软要求现代自定义 IME 使用 TSF，并说明输入法 DLL 会被加载进当前应用、
受到该应用容器能力约束：

- <https://learn.microsoft.com/zh-cn/windows/apps/develop/input/input-method-editor-requirements>
- <https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration>
- <https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfinputprocessorprofilemgr>
- <https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-registerprofile>
- <https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-activateprofile>
- <https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeystrokemgr-advisekeyeventsink>
- <https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontext-requesteditsession>

注册与启用边界还对照了固定版本的 Microsoft SampleIME、Weasel 和 Mozc：
SampleIME 与 Weasel 都用现代 profile manager 注册，并显式传入默认启用选择；
Mozc 则把 profile 注册与另一个显式的默认/进程/会话激活路径分开。Alpha 保留
默认关闭，不能照抄示例的默认启用值。SampleIME 的 MIT 实现和 Mozc 的 BSD
风格实现只作结构核对；Weasel 的 GPL-3.0 实现只作行为参考，没有复制代码。

- [SampleIME `Register.cpp`（固定提交）](https://github.com/microsoft/Windows-classic-samples/blob/77f217b3f89d4dac7864a62cc91ff7b569f26a50/Samples/IME/cpp/SampleIME/Register.cpp)
- [Weasel `Register.cpp`（固定提交）](https://github.com/rime/weasel/blob/f9203cae5e2b0796d94575b975f62a6be9614b00/WeaselTSF/Register.cpp)
- [Mozc `tsf_registrar.cc`（固定提交）](https://github.com/google/mozc/blob/3f235b4eb6fcff7d14ef5f0fb8ee56de7ee4c732/src/win32/base/tsf_registrar.cc)
- [Mozc `imm_util.cc`（固定提交）](https://github.com/google/mozc/blob/3f235b4eb6fcff7d14ef5f0fb8ee56de7ee4c732/src/win32/base/imm_util.cc)

本地稀疏只读副本及其路径也记录在 Git 忽略的
`.local/research/upstreams/README.md`；引入或链接第三方代码前仍必须单独审查
许可证与更新边界。

## Alpha 接受条件

- 记事本和 Codex 都能重复完成输入、取消、翻页、选择与切换输入法；
- 宿主关闭、输入法停用、焦点切换和解码失败均不使宿主崩溃；
- 候选结果与终端宿主使用相同核心，在固定输入下保持确定；
- 禁用/卸载后无残留后台采集，微软拼音始终可选；
- 性能报告区分解码、TSF 编辑会话和候选 UI，不用一次总耗时掩盖问题；
- 私人反馈只有经过再次明确授权的本地实验才会读取。

达成这些条件后，Alpha 才进入自然日用；此前仍是可构建、可测试、可卸载的
开发版本。
