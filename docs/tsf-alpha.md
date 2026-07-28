# Windows TSF Alpha：自有输入法壳

## 状态

这是用户明确批准的新里程碑。当前已完成与宿主无关的 `CompositionSession`
抽取、TSF COM 生命周期探针，以及只在测试进程中工作的按键与真实组合范围
闭环。仓库尚未注册或安装 TSF 输入法，也没有修改 Windows 默认输入方式；
文字变更目前只在进程内的合成 Context 中验证，尚未进入记事本或 Codex。

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
- 未注册测试进程拿到的是应用 client id，Windows 会拒绝用它冒充前台文本
  服务。测试因此只绕过“前台订阅”这一项，直接调用同一个按键接收器；
- 测试按键仍经过真实的 Thread Manager、Document Manager、Context 与同步
  `RequestEditSession`。事务用 `ITfRange` 修改合成文档，同时只保留有界的
  “更新/取消/提交”结构遥测，不保存原文；
- 默认类工厂没有候选源，任何按键都会交还宿主。测试候选源只能由仓库内测试
  显式注入；
- 在用户再次确认安装前，只构建和检查，不注册。

当前构建命令为 `cargo build --release --lib`，产物是
`target/release/ziranma_core.dll`。它只生成未注册的 DLL；单元测试会在测试线程
中创建并释放系统 TSF Thread Manager，不写注册表、不创建输入配置文件，也不
改变 Windows 输入法列表。

### 2. 最小可输入闭环

- `a`～`z`、Backspace、Esc、Space / Enter 与数字键的路由和事务计划已有
  进程内覆盖；
- 已用 `ITfInsertAtSelection`、`ITfContextComposition` 和 `ITfRange` 在合成
  Context 中创建、更新、取消与提交组合；
- 测试会逐步读回合成正文，验证同一范围更新、候选替换、取消删除以及提交后
  光标落在文字末尾；
- `ITfCompositionSink` 已处理宿主主动终止并同步清理内部状态；真实激活下的
  焦点丢失和输入法切换清理仍是注册前的下一道门；
- 未激活或没有组合时，普通按键交还宿主。

### 3. 候选窗口与现有功能

- 候选窗口跟随 TSF 提供的文本范围，而不是读取屏幕或猜光标；
- `-` / `+` 翻页；
- Shift+Tab 显式换序；
- Tab 单字笔画辅助；
- DPI、暗色界面和候选窗口消失/重建单独测试。

### 4. 直接反馈与个人候选

自有输入法可以直接记录“按键、展示、选择、提交和取消”的结构化事件，不再
从 UI Automation 猜测候选行为。该阶段仍沿用当前用户 DPAPI 加密、无网络、
显式会话和可停止边界。安全输入、密码框以及不允许文本服务的输入范围一律
不学习；默认不保存宿主周围正文。

先离线评测精确的“有效按键串 → 实际提交文字”，再决定是否创建持久个人
模型。记录过程不实时改变正在使用的模型。

## 安装、升级与回退边界

- 不直接写注册表设置默认键盘；注册使用 TSF 正式接口；
- 安装、注销、升级和回退必须是不同的显式操作；
- 更新前先切换到其他输入法，并等待旧 DLL 不再被新宿主加载；
- current / candidate / previous 的版本槽位不复用正在加载的文件；
- 注册失败或候选版本启动失败时，微软拼音和 previous 必须保持可用；
- 不申请网络能力，不把解码或私人数据发送给其他进程；
- 在签名与 Windows 兼容要求没有验证前，不称为可分发安装包。

微软要求现代自定义 IME 使用 TSF，并说明输入法 DLL 会被加载进当前应用、
受到该应用容器能力约束：

- <https://learn.microsoft.com/zh-cn/windows/apps/develop/input/input-method-editor-requirements>
- <https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration>
- <https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfkeystrokemgr-advisekeyeventsink>
- <https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcontext-requesteditsession>

Weasel 与 PIME/libIME2 只用于研究成熟项目如何分离 Windows 前端、UI 与引擎；
不复制其实现。引入或链接第三方代码前必须单独审查其许可证与更新边界。

## Alpha 接受条件

- 记事本和 Codex 都能重复完成输入、取消、翻页、选择与切换输入法；
- 宿主关闭、输入法停用、焦点切换和解码失败均不使宿主崩溃；
- 候选结果与终端宿主使用相同核心，在固定输入下保持确定；
- 禁用/卸载后无残留后台采集，微软拼音始终可选；
- 性能报告区分解码、TSF 编辑会话和候选 UI，不用一次总耗时掩盖问题；
- 私人反馈只有经过再次明确授权的本地实验才会读取。

达成这些条件后，Alpha 才进入自然日用；此前仍是可构建、可测试、可卸载的
开发版本。
