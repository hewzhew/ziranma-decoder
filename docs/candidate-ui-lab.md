# 候选窗交互实验室

## 要解决的问题

候选窗的颜色、字号、间距和选中标记需要反复比较，但不能维护一套网页近似图，
再靠人工把 CSS 数值抄回 GDI。字体度量、DPI 取整、ClearType、矩形边界与实际
TSF 宿主位置都会使两套渲染逐渐分叉。

实验室应当复用 Alpha 的真实候选模型、布局计算与 GDI 绘制函数。网页若以后
存在，只适合充当参数面板或反馈浏览器，不作为效果真值。

## 参考结论

- Fluent 2 将颜色、字阶和间距组织为语义 token。界面应由“正文、次要信息、
  选中状态、边界”等角色驱动，而不是让每个控件保存一组互不相关的 RGB 值。
- Windows 排版指南强调清晰的信息层级与一致的字号/字重；候选文字是内容，
  数字与页码是元数据，不应争夺第一视觉焦点。
- 固定版本的 Weasel TSF 将候选列表语义、`UIStyle` 和候选 UI 更新分开，并把
  点击/翻页作为 UI 回调送回输入法；它说明“可配置样式”不必侵入解码器。
- 固定版本的 Mozc Windows TIP 先形成 `RendererCommand`，再交给独立 renderer；
  它说明宿主生命周期与候选绘制可以有明确消息边界。

本项目只借鉴这些结构，不复制 Weasel 的 GPL-3.0 代码。只读参考版本和许可证
记录在 `.local/research/upstreams/README.md`。

公开资料：

- [Fluent 2 color](https://fluent2.microsoft.design/color)
- [Fluent 2 typography](https://fluent2.microsoft.design/typography)
- [Windows typography](https://learn.microsoft.com/windows/apps/design/signature-experiences/typography)
- [Windows content layout and spacing](https://learn.microsoft.com/windows/apps/design/style/spacing)
- [Motion in Windows](https://learn.microsoft.com/windows/apps/design/signature-experiences/motion)
- [SPI_GETCLIENTAREAANIMATION](https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-systemparametersinfow)

生产 Alpha 的紧凑横式遵循同一组约束：外框和正文内边距只使用少量一致的逻辑
像素；短候选保留自然宽度，长候选共享一个有上限的横向预算并在各自区域内省略，
不再让六个长短语把窗口撑满编辑器。真实记录的无正文字符数聚合表明，日常压力
主要来自六个 3–4 字候选与页脚共同挤进旧的 640 逻辑像素，而不是大量超长句子；
因此横排上限提高到 760，普通项最低宽度提高到 70，使带页脚的六个四字候选仍可
完整呈现。横排首选保留最多约 16 个汉字的自然宽度，普通项才参与压缩；首选超过
横排预算时改用按最长可见项扩宽的竖排，最多容纳 32 个汉字及一个明确的截断标记，
而不是仍在固定 360 像素竖窗里过早省略。必须截断时使用一个 Unicode 省略号，
超过 32 字的来源也不会无提示消失。候选窗首次出现即以
完整透明度显示，逐键内容更新也不做透明度动画；提交、取消或失焦时立即隐藏，
避免高频输入界面把通用弹窗动效变成可读延迟或残影。

逐键公开审计还发现，双拼声母暂态到完整韵母帧之间，目标排名总体改善，
但候选文字会大面积替换。因此实验层另有一个不连接 TSF 的
`HalfPairPaintCoalescer`：它允许已经算好的奇数键声母帧短暂等待，韵母
及时到达便跳过这次中间绘制；停顿或显式翻页则强制显示最新帧。状态机
使用单调版本号拒绝迟到的异步结果，固定 0/16/24/32/48 ms 只用于合成
节奏对照，不是产品配置。

未来实验室应把“候选语义已经更新”和“自有 GDI 弹窗何时重绘”显示为
两条时间线。即使试验延后自有弹窗，TSF UI Element 的候选、选择和分页
状态也必须立即保持最新，不能让屏幕阅读器或显式选词读取旧列表。生产
Alpha 当前仍逐次更新，没有启用这项合并。持续研究的只读回顾会按当前 DLL
聚合真实奇数帧到偶数帧的时延分桶、首选变化与候选保留率；只有这些真实聚合
显示短等待能覆盖足够多帧时，才重新评估生产计时器，不能从合成节奏直接选值。

为了不用合成节奏猜等待值，显式启动的 TSF 内存反馈现在会额外汇总普通
候选首页从奇数键声母帧到后一键偶数帧的呈现间隔。它只保留
`<8`、`8–15`、`16–23`、`24–31`、`32–47`、`48–63`、`64–95`、
`96–159`、`≥160 ms` 九个计数，不保留逐次时间；翻页、提交、取消、
非普通候选、码串不连续、受限上下文或时间倒退都会切断配对。语言栏反馈
菜单可只读查看这些计数，仍不写文件、不联网。该间隔是在候选成功呈现时
观察的，包含解码和宿主调度影响，不能冒充原始键盘硬件间隔，也不会自动
选择或启用绘制等待值。

## 推荐结构

```text
候选与页码（不含私人文字）
          │
          ▼
CandidateVisualSpec ── 语义颜色、字号、行高、内边距、圆角、横/竖布局
          │
          ▼
CandidateScene ─────── 已完成 DPI 取整的元素矩形与语义标识
          │
          ├── TSF Alpha 的真实 GDI 弹窗
          └── candidate-ui-lab 的真实 GDI 预览
                                      │
                                      └── 圈选、批注、方案比较
```

`CandidateScene` 中每个可见区域应有稳定的语义标识，例如
`candidate.selected.surface`、`candidate.rank`、`candidate.text`、
`selection.accent` 和 `footer.page`。实验室的圈选命中这些标识，而不是只保存一
块日后失去含义的屏幕坐标。

## 第一版交互

第一版是独立的、仅开发使用的 Windows 程序，不从真实输入框读取内容：

1. 使用公开合成候选，可切换 96/120/144/192 DPI、横式/竖式、短词/长词和翻页；
2. 拖动整个预览检查跟随位置，不直接改变生产窗口位置策略；
3. 在参数面板调节语义颜色、行高、字号、内边距、圆角与选中竖条；
4. 同屏冻结 A/B 两个 `CandidateVisualSpec`，避免凭短时记忆比较；
5. 在预览上拖出区域并写一句建议，程序同时记录命中的语义标识与当时 token；
6. 导出一份本地 JSON 建议，不自动修改 Rust 源码、不构建、不安装、不联网。

反馈文件放在 Git 已忽略的 `.local/ui-lab/wishes/`。它只含公开合成候选、视觉
参数、DPI、语义区域和用户批注；若未来允许真实候选或截图，必须接入
`docs/wish-feedback.md` 的逐项内容授权与 DPAPI，而不能沿用这一公开实验格式。

## 与生产绘制保持一致

- 把当前散落在 `tsf_alpha.rs` 的视觉常量先收拢为无窗口句柄的
  `CandidateVisualSpec`；
- 把布局结果收拢为 `CandidateScene`，用整数逻辑像素输入、DPI 后的物理像素
  输出；
- GDI 绘制只消费 scene，不在绘制过程中重新决定间距；
- Alpha 和实验室都调用同一个 `paint_candidate_scene(HDC, ...)`；
- 对 96/144/192 DPI 保存矩形级测试，不用像素截图替代全部结构测试；
- 生产 Alpha 只内置一份已审阅默认 spec，不在宿主进程读取实验室文件。

这样实验室调出的方案不会自动影响日常输入。采用某个方案仍是一次普通、可审阅、
可回退的源代码改动。

## 分阶段实施

1. 先提取 `CandidateVisualSpec` 和 `CandidateScene`，保持现有画面逐矩形等价；
2. 再让现有 Alpha 从 scene 绘制，并保留闪烁与 DPI 回归测试；
3. 增加只显示公开合成数据的原生 GDI 实验室；
4. 增加 A/B、圈选和本地批注；
5. 最后才评估是否与“向猫猫许愿”伴随程序合并。

当前不直接做拖拽式生产编辑器。先共享真实渲染内核，才能避免实验室本身成为
第二套需要维护的候选窗。

### 当前实现边界

第一阶段的无窗口数据边界已经建立：`src/candidate_ui.rs` 保存一份生产默认
`CandidateVisualSpec`，并在无 `HWND`、`HDC` 或候选文字依赖的情况下构造完成
DPI 取整的 `CandidateScene`。生产 Alpha 已经从该 scene 读取客户区、横/竖候选项、
选中底、选中竖条、数字序号、正文、个人记忆标记、提示图标、页脚和分隔线矩形。
GDI 仍负责测量真实字体，但测得的高度与基线会作为纯数值送进 scene，因此数字与
正文的垂直对齐不再由 painter 重算。单项操作提示会先由 GDI 测出标签与说明宽度，
再由 scene 决定是否分栏；页脚内部的模式与页码也由 scene 分出互不重叠的区域。
候选文字的有界逻辑宽度估算、横/竖项目宽度和横排空间不足时的压缩策略也已移入
同一无窗口模块，生产 TSF 与未来实验室不会分别维护两套宽度算法。
`candidate.rank`、`candidate.text`、`candidate.action-detail`、
`candidate.personal-mark`、`notice.icon`、`footer.mode` 和 `footer.page` 均已有稳定
语义名。96/144 DPI 的精确矩形测试以及生产 96/120/144/192 DPI 布局联结测试固定了
这条边界。此次提取没有修改视觉 token、候选数量、截断或排序行为。

第二阶段的共享真实绘制边界已经建立：`src/candidate_ui_gdi.rs` 提供
`paint_candidate_scene`，生产 Alpha 的背景、候选选中底和竖条、序号与正文、
省略、个人记忆标记、提示图标、操作说明、页脚分隔线、模式和页码均经过这一条
GDI 路径。`tsf_alpha.rs` 仍负责 TSF 与窗口生命周期、字体创建和真实文字测量、
双缓冲以及兼容系统的圆角外框；共享 painter 只消费已测量的 scene、字体句柄与有界
显示文字，并释放自己创建的所有帧内画刷和 region。此次提取保持原来的颜色、尺寸、
排序和截断行为。

`CandidateScene::semantic_hits_at` 还提供右、下边界排除的物理像素 hit-testing，并按
“具体绘制特征在前、所属容器在后”返回全部命中；个人标记与数字、蓝条与选中底、
页脚标签与页脚等重叠关系因此具有固定、可测试的优先级。每个候选命中同时携带稳定
候选索引，未来批注不必从屏幕坐标反猜语义。

原生 Windows 骨架现已位于 `src/bin/candidate-ui-lab.rs`。它通过同一源文件直接复用
scene、宽度策略和 GDI painter，只装载三组编译期公开合成候选，不连接 TSF，不读取
输入框、反馈记录或实验室文件，也不写盘、不联网。窗口可用 `H` 切换横/竖布局、`D`
轮换 96/120/144/192 DPI、`S` 轮换短词/长候选/个人标记场景；点击预览会在标题中显示
最具体的稳定语义与候选序号。所有场景、布局和 DPI 组合都有无窗口几何测试。

这仍只是可运行的渲染与命中骨架：尚无参数面板、A/B 冻结、拖动 token、区域圈选、
文字批注或 JSON 导出，也尚未加入日用用户工具槽。下一步应先把“点命中”扩展为有界
矩形圈选和语义集合，再增加仅保存公开场景的本地批注；不能让骨架的存在冒充第一版
全部交互已经完成。

## 圆角与抗锯齿边缘

微软文档把自绘弹窗分成两条路线：Windows 11 可以通过
`DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE)` 请求由 DWM 合成圆角；
需要完全自定义透明形状时，则可以用 `UpdateLayeredWindow` 和 32 bpp 预乘 Alpha
位图提供逐像素透明度。

当前 Alpha 优先选择第一条路线：候选窗不再保留已经没有动画用途的
`WS_EX_LAYERED`，Windows 11 使用 DWM 的圆角和边框；不支持这些属性的旧系统才
使用确定性的 GDI 圆角 region 与同心边框。DWM 路线不再叠加二值 window region，
避免系统抗锯齿之后又被像素级裁剪破坏。

逐像素 Alpha 暂不作为第一实现，因为 `UpdateLayeredWindow` 总是更新整窗，并要求
源位图使用预乘 Alpha；现有 GDI 文字、双缓冲位图与透明通道还需要一套明确的合成
测试。若真实宿主验证表明 DWM 对某类弹窗不生效，再实现 32 bpp top-down DIB、圆角
覆盖率遮罩与 `AC_SRC_ALPHA` 回退，而不是继续增加二值 region 的半径。

参考：

- [Apply rounded corners in desktop apps](https://learn.microsoft.com/windows/apps/desktop/modernize/apply-rounded-corners)
- [UpdateLayeredWindow](https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-updatelayeredwindow)
- [BLENDFUNCTION](https://learn.microsoft.com/windows/win32/api/wingdi/ns-wingdi-blendfunction)
