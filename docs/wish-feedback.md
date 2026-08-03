# “向猫猫许愿”：本地反馈与现场快照

## 目标

用户遇到候选、排序、闪烁、延迟、输入模式或兼容性问题时，应能用一次明确动作
留下自己的说明，并把问题发生前后的输入法内部现场绑定到同一个本地反馈包。
反馈首先服务于本人和本地开发，不是遥测系统，也不自动上传 GitHub 或任何服务。

当前 Alpha 已有默认关闭、显式开始的仅内存语义事件：候选展示、选择、提交、
取消和上下文资格都会经过输入法自身记录。它不写文件，应用退出即丢失。“许愿”
功能应在这条边界上增加短时回看、用户留言、加密保存和可审阅导出，而不是另建
全局键盘钩子或读取宿主周围正文。

## 参考模式

成熟反馈工具的共同点不是“尽量多收”，而是把用户描述与可选择的现场证据绑定：

- Windows Feedback Hub 允许先写问题或建议，再选择附件、截图以及
  “Recreate my problem”；录制前还能选择包含哪些诊断数据；
- VS Code Issue Reporter 把问题类型、标题和描述放在首位，并把系统信息与扩展
  信息作为可见复选项；附加信息可由用户查看或下载；
- Sentry User Feedback 可以在没有异常事件时由用户主动打开；与 Session Replay
  结合时，会在打开反馈入口前保留最多约 30 秒的短时缓冲；Breadcrumbs 则用
  类型、类别、级别、时间和键值数据组成结构化事件轨迹。

这些设计不能原样照搬：本项目处理真实输入文字，默认边界应比通用应用反馈更
严格。可借鉴的是“主动入口、短时前情、分层授权、提交前可见”，不是其联网
后端。

参考资料：

- [Windows Feedback Hub：发送反馈与复现问题](https://support.microsoft.com/en-us/windows/send-feedback-to-microsoft-with-the-feedback-hub-app-f59187f8-8739-22d6-ba93-f66612949332)
- [VS Code Issue Reporter model（固定提交）](https://github.com/microsoft/vscode/blob/d9d6728092f8be8bba69bbbb5cc97beb953ae465/src/vs/workbench/contrib/issue/browser/issueReporterModel.ts)
- [VS Code Issue Reporter page（固定提交）](https://github.com/microsoft/vscode/blob/d9d6728092f8be8bba69bbbb5cc97beb953ae465/src/vs/workbench/contrib/issue/browser/issueReporterPage.ts)
- [Sentry User Feedback](https://docs.sentry.io/platforms/javascript/user-feedback/)
- [Sentry Breadcrumbs](https://docs.sentry.io/product/issues/issue-details/breadcrumbs/)

## 推荐交互

控制入口不注册新的全局快捷键。语言栏菜单若被宿主显示可以继续使用；Windows
11 现代输入指示器不保证展示第三方项目，因此日用入口改为独立的单实例许愿面板，
关闭后继续在托盘待命：

1. 用户选择“开始内测反馈”；此时才建立有界的内存环形缓冲；
2. 遇到问题时选择“向猫猫许愿…”；程序冻结此前最多 30 秒，并继续收取最多
   5 秒尾迹，避免只看到问题后的结果；
3. 一个独立的前台小程序显示留言框与类别：候选、排序、显示、延迟、输入模式、
   兼容性或其他；
4. 页面逐项显示将要保存的证据。正文、具体按键/码串、候选文字和截图分别授权，
   截图默认关闭；
5. 用户确认后，程序生成一个只属于当前 Windows 用户的 DPAPI 加密反馈包；
   可以立即删除、稍后补充说明，或显式导出脱敏摘要；
6. 保存成功后继续内测缓冲，停止与清除仍由用户控制。

`wishpad` 面板只通过 TSF 通道发送开始、保存和停止三种无正文命令；托盘右键
还提供清除命令。快捷方式和托盘左键直接唤回已预热的同一面板，右键菜单只承担
打开、清除与退出。另有独立说明小窗，可为打开窗口时选定的最近一条已保存许愿
添加类别和留言。留言
不会进入 TSF 通道。证据预览仍由后续前台界面承担；它不会顺带扫描其他私人会话。

输入法内另有一个严格限域的助记入口：只有组合码精确等于 `xuy` 后按 `Tab` 才
进入操作确认；普通 `xuy + 空格` 继续输入文字，其他码的 `Tab` 继续走找字辅助
请求。确认栏按当前宿主状态只显示一个动作：未开始时分栏显示“开始反馈 / 暂不保存”，
记录中显示“向猫猫许愿 / 近 30 秒”，停止后显示“清除反馈 / 已停止”。空格或
数字 1 执行；退格、Esc 或再次 Tab 返回普通候选。若继续输入字母，确认栏立即
退出并把该字母加入原组合，不吞键。

## 当前 V1

当前实现先完成一条只传无正文控制字的安全短链路：

1. 语言栏、`wishpad`、`wish-ime.cmd start` 或明确的 `xuy + Tab` 确认动作开始
   反馈；默认仍关闭并保留原有上下文资格门；
2. 记录期间从许愿面板选择“保存近 30 秒”，或运行 `wish-ime.cmd mark`，
   同步冻结此前最多 30 秒、
   最多 1024 条带单调时间的事件；
3. 绝对单调时钟不会进入包，每条事件只保存距离点击时刻的毫秒数；未带时间的
   兼容事件会明确计为省略，不会猜测；
4. 快照在内存中形成严格有界的 `ziranma-wish-v1`，立即由 Windows 当前用户
   DPAPI 保护，再通过只含密文的临时文件原子发布到
   `.local/tsf-alpha/user-data/wishes`；
5. `wish-ime.cmd start/mark/stop/clear` 只发送控制命令；`status/list` 默认不解密；`show` 只有显式加入
   `--confirm-show-private-text` 才显示；`annotate` 把类别和留言另存为绑定同一
   随机内容 ID 的 DPAPI 文件；`trash` 需要确认并移动到可恢复目录；
6. 所有操作都不联网、不写模型、不读取光标周围正文，也不扫描既有私人会话。

V1 的明确保存动作会做一次有界 DPAPI 与本地写入，但它不是按键或候选刷新
路径；失败只形成脱敏状态，不会改变输入结果或停止反馈。当前 `wishpad` 是轻量
许愿面板、托盘驻留器与说明编辑器：它只从固定本地许愿目录选择打开窗口时最新的一条，类别和留言
经校验后另存为绑定同一 ID 的 DPAPI 文件，既不覆盖旧说明，也不解密现场。
逐层证据预览、5 秒尾迹和截图选择仍属于后续前台窗口，复杂 UI 不驻留在每个
TSF 宿主中。

## 两层证据

默认诊断层不含文字，但仍属于私人行为元数据：

- Alpha 版本、DLL 摘要、候选包 revision 与反馈 schema；
- Windows 版本、DPI、主题、横/竖布局和当前页；
- 组合长度、候选数量、请求深度、缓存命中、视图类型；
- 解码、布局、窗口定位与绘制耗时；当前 Alpha 已在显式反馈开启时记录候选窗
  首个完整帧与完全可见耗时，立即显示模式下两者相同；
- 候选窗创建、显示、隐藏、移动、尺寸/圆角区域变化和重绘原因；
- 输入模式、焦点/组合生命周期、失败阶段与 HRESULT；
- 全部时间只保存相对反馈标记的单调时钟，不保存无关的长期活动时间线。

显式内容层可以额外包含：

- 反馈留言；
- 有效按键串、规范码、展示过的候选与实际提交；
- 用户主动选择的一张截图。

内容层必须在每个反馈包上单独确认，不能依赖永久的“一次同意”。无论哪一层，
都不读取光标周围正文、剪贴板、窗口标题、文件路径或其他输入法的按键。

## 进程与存储边界

TSF DLL 被加载进记事本、Codex 等宿主，不应在宿主 UI 线程里弹复杂窗口、加密
大包或写磁盘。推荐拆成两个角色：

- Alpha 只维护显式开启的有界内存环，并在用户点击“许愿”时冻结一个快照；
- 独立前台 `wishpad` 负责留言、未来的证据预览、DPAPI 和原子写入。

当前控制层使用 TSF 当前用户全局 compartment，只传四种动作、一个有界序号和
脱敏确认状态。输入码、候选、留言、路径与快照都不进入 compartment；每个宿主
仍只在自身进程内冻结并使用 DPAPI 保存。未来若前台编辑器需要交换预览内容，
再单独为当前用户本地管道设计 ACL、随机 nonce、固定 schema、消息长度上限和
超时；它不能监听网络，也不能接受自由格式控制消息。

## 前台入口的交互依据

- [Windows 通知区指南](https://learn.microsoft.com/en-us/windows/win32/shell/notification-area)
  明确区分左键主动作与右键快捷菜单：左键应展示最适合内容的弹窗、对话框或程序
  窗口，右键才打开普通快捷菜单。因此许愿的主要流程不再放进 `TrackPopupMenuEx`。
- [PowerToys Run](https://learn.microsoft.com/en-us/windows/powertoys/run) 使用后台常驻
  与重复唤醒的前台面板来承载快速任务。`wishpad` 只借鉴单实例和预热复用，不注册
  默认全局快捷键。
- VS Code 的 Issue Reporter 在独立辅助窗口中承载类别、说明和可选证据，而不是
  塞入上下文菜单；参考其固定版本的
  [`IssueFormService`](https://github.com/microsoft/vscode/blob/9afe2783a7239c915d5fc6d1bd9c842f9ca06c2e/src/vs/workbench/contrib/issue/browser/issueFormService.ts)。
- 面板的视觉层级遵循 Fluent 的
  [字体](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/typography)、
  [颜色](https://learn.microsoft.com/en-us/windows/apps/design/style/color)与
  [按钮](https://learn.microsoft.com/en-us/windows/apps/design/controls/buttons)原则：
  标题、正文和说明使用不同字重与字号，只让“保存近 30 秒”成为强调按钮，
  状态和辅助说明降低视觉权重。颜色取自 Windows 系统角色，继续适配系统强调色
  与高对比度模式；原生 `BUTTON` 仍保留键盘焦点和无障碍名称。
- 独立伴随程序使用项目内生成的闭眼冥想猫图标；多尺寸 ICO 被嵌入
  `wishpad.exe`，标题栏和通知区复用同一资源。图标来源、处理过程与尺寸清单见
  [`assets/wishpad/README.md`](../assets/wishpad/README.md)。

这些参考只决定入口层级：许愿面板仍是有界本地工具，不加入遥测、网络提交、
隐式保存、自动启动或驻留在 TSF 宿主里的复杂界面。

反馈包使用 `ziranma-wish-v1`，写入 Git 已忽略的
`.local/tsf-alpha/user-data/wishes/`。先在内存形成完整明文，再由 Windows 当前用户 DPAPI
保护，最后以“不覆盖既有文件”的原子发布方式保存；临时文件也只能包含密文。
任何解密、原文预览或导出都需要单独命令和明确提示。

## 必须保持的停止条件

- 默认关闭；没有明确开始就没有事发前环形缓冲；
- 密码、PIN、`IS_PRIVATE`、键盘禁用、空 Context 和未知 InputScope 全部拒绝；
- 达到事件或字节上限时停止并标记不完整，不覆盖旧事件冒充完整；
- 伴随进程失败不能影响上屏、候选窗或宿主稳定性；
- 没有真实发布签名、管道威胁模型和恢复测试前，不随系统启动；
- 不把“保存到本机”与“发送给猫猫/GitHub”合并为一个动作。

## 实施顺序

1. 已完成候选窗首帧/完全显示耗时、显式 30 秒冻结、`ziranma-wish-v1`、
   DPAPI 原子写入、本地管理命令、无正文 TSF 控制通道与单实例 `wishpad` 面板；
2. 已完成托盘左键/快捷方式直接唤回面板、右键次要菜单和独立分类留言小窗；
   下一步把将保存证据的预览放进同一前台进程；
3. 只有预览确实需要跨进程内容时，才为短期当前用户命名管道完成 ACL、nonce、
   超时和卸载恢复测试；
4. 再增加可选 5 秒尾迹与逐项内容授权；
5. 最后才讨论快捷键、截图、跨宿主汇合或显式 GitHub 导出。

这条顺序让当前 Alpha 可以继续日用测试，同时不会为了“反馈更方便”把私人输入、
宿主稳定性和网络权限一次性混在一起。
