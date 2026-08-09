# ziranma-decoder

一个面向自然码双拼的本地、隐私优先、可解释的带噪序列解码实验。

长期目标，是研究怎样从可能误触、按键颠倒、漏键、混合简拼且没有
显式词界的自然码按键串中，生成可信的中文候选。当前仓库是离线研究
基线，不是完整输入法。实验性的 Codex 单应用追踪探针必须手动武装，
默认不采集按键、不显示文本，也不写入磁盘。

如果只想先弄清“项目在做什么、做到哪一步、何时停止”，请看
[项目地图](docs/project-map.md)。

## 现在能够做什么

当前版本包含一条可以完整运行和测量的链路：

```text
无声调全拼
  ↓ 自然码 codec
逐音节的标准两键编码
  ↓ 每个词只按音节序列写入一次
紧凑逐音节 trie
  ↓ 联合选择一键/两键并消费至多一次局部按键错误
单词 Top-K，或流式词前缀扫描产生的句子 lattice
  ↘ 每个活跃位置还有一条逐键、重罚、原样保留的未解析边
  ↓ 按键位置 × 全局错误预算 × 前词状态的记忆化 k-best
  ↓ unigram / 平滑 bigram 插值
  ↓
词界、简拼位置、纠错操作、未解析按键和分数拆解
```

支持的局部错误包括：

- 一次 QWERTY 物理邻键替换；
- 一次相邻按键颠倒，包括跨音节边界；
- 一次漏键；
- 一次多按。

搜索器能够联合枚举简拼和纠错。例如输入 `ni` 可以解释为“你好”的候选
编码 `nih`（第二个音节使用一键简拼），随后末尾的 `h` 又漏按了。这个
能力用于研究路径完整性，不表示任意混合简拼已被验证为可用输入协议。
程序只生成并解释候选，不自动上屏。

## 运行

需要稳定版 Rust。Windows TSF 使用官方 `windows` crate；候选包材料绑定使用
RustCrypto `sha2`，候选发布验签使用关闭默认功能的 `ed25519-dalek`，避免手写
密码学原语。仓库另含许可和来源均单独保留的公开 Rime 词典与
UD Chinese GSDSimp train/test 快照。

```powershell
Set-Location -LiteralPath 'C:\path\to\ziranma-decoder'

# 查看全拼怎样映射成自然码
cargo run -- encode ni hao

# 完整码、混合简拼、漏键和多按
cargo run -- decode nihk
cargo run -- decode nhk
cargo run -- decode nik
cargo run -- decode niihk

# 没有显式词界的多词联合解码
cargo run -- sentence zrmurf
cargo run -- sentence zrnurf
cargo run -- sentence nigkz

# 对照不使用 bigram 的排序
cargo run -- sentence-unigram ajjp

# 审查句子 lattice 的 trie 扫描、对齐状态和词边
cargo run -- sentence-stats zrmurf

# 审查紧凑索引的实际存储量与隐式拼写量
cargo run -- index-stats

# 审查一次单词联合搜索的路径与对齐工作量
cargo run -- search-stats nhk

# 兼容最初的简写形式
cargo run -- nigk

# 生成公开合成评测
cargo run -- evaluate

# 审计自由混合简拼的编码理论边界
cargo run --release -- abbreviation-audit

# 只用公开 train 学上下文，再在独立 test 校准首选与拒识分差
cargo run --release -- public-calibrate

# 使用固定的 6.5 万词公开快照
cargo run -- public-index-stats
cargo run -- public-decode nihk 10
cargo run -- public-sentence zrmurf 10

# 连续尾部简写，并单独保留一次相邻颠倒恢复
cargo run -- public-compose mafkmm 3

# 使用固定公开词典的只读候选实验台：默认显示简洁中文
cargo run --release -- candidate-lab mafmkm 3

# 课程式检查：直接报告目标文字在当前显示候选中的名次
cargo run --release -- candidate-lab mafmkm 3 --expect 麻烦猫猫

# 只有明确请求时才显示按键颠倒候选
cargo run --release -- candidate-lab mafkmm 3 --recovery --expect 麻烦猫猫

# 研究细节与机器可读结果使用独立输出层
cargo run --release -- candidate-lab mafmkm 3 --verbose
cargo run --release -- candidate-lab mafmkm 3 --json

# 端到端逐键实验：连续双拼、候选选择、退格与 Tab 单字辅助
cargo run --release -- typing-lab

# 固定公开 2～6 字双词短语的连续输入评测
cargo run --release -- public-compose-evaluate

# 深查首屏外样本，并用 train-only 上下文重排同一冻结候选池
cargo run --release -- public-compose-audit

# 在公开 train 内固定切 fit/dev，对照四种受限简写协议
cargo run --release -- public-protocol-audit

# 深查锚定尾简首屏外样本，并模拟一个明确词界
cargo run --release -- public-protocol-failure-audit

# 用 fit-only 词上下文重排冻结候选池，同时检查完整码是否退化
cargo run --release -- public-protocol-context-audit

# 比较冻结候选池上的公开词/字上下文与保守首选门（只读，不接入 TSF）
cargo run --release -- public-context-hybrid-audit

# 公开同码单字课程：测一至三画能否把首屏外目标筛回 Top-10
cargo run --release -- public-shape-audit

# 公开 Tab 笔画实验台：Windows 终端直接按 Tab、笔画键和数字，不用 Enter
cargo run --release -- shape-lab shi --expect 事
cargo run --release -- shape-lab da --expect 龘 --prefix n

# 连续公开课程：默认混合一画、两画、三画题；Enter 跳过，q 结束
cargo run --release -- shape-course --count 10 --level mixed

# release 模式固定工作负载、预热后重复采样
cargo run --release -- benchmark 3
```

CLI 程序自身只读取编译进程序的公开演示数据或固定公开快照，不会主动把
输入持久化；命令行历史、终端回滚和重定向输出属于程序之外的明文边界。

## 连续输入实验台

`typing-lab` 是静态候选报告与未来系统输入法之间的最小交互层。Windows
终端中可以直接输入连续双拼，空格或 Enter 选首项，数字选择其他候选，
减号向前翻页，加号（或等号）向后翻页，PageUp / PageDown 作为备用，
退格修改当前双拼；没有双拼时可从已选文字末尾退格，复现“先选较长候选再裁短”的
流程。Shift+Tab 暂留作开发期换序对照，不作为计划中的日用纠错入口；完整单字码上按 Tab 后，使用
`h/s/p/n/z` 继续过滤同码字。`q` 和 `t` 始终是普通双拼字母，退出使用
空输入下的 Esc，不占用编码键。

普通候选直接调用现有连续句子解码器，Tab 直接调用现有冻结单字池；实验台
不复制一套算法。首屏只计算可见项，翻页时只同步补足当前页，同时在后台
展开最多 200 项；同一按键串的缓存只升级不降级，回首页不会丢掉深候选。
换序候选直接复用现有的受限相邻颠倒恢复栏，不改普通排序，也不写入
普通候选的本轮记忆。显式选过的普通同码候选在本次运行中置顶，退出即丢弃。它仍不读取私人记录、
不写文件，也没有持久个人排序、光标移动、标点或系统输入框上屏。完整按键、
重定向回退与停止边界见[端到端连续输入实验台](docs/typing-lab.md)。真实 TSF
练习区、结构化标记和加密轨迹的分阶段方案见[打字练习实验室](docs/typing-practice-lab.md)。

终端宿主已把当前组合状态抽成可复用的 `CompositionSession`；已提交文字仍由
宿主自己拥有。这是[自有 Windows TSF Alpha](docs/tsf-alpha.md)的地基，
不会注册输入法、修改默认键盘或读取私人记录。类工厂现在会为合成 Context
创建服务，并已验证 `nihk` 提交“你好”；未配置外部数据时使用 50 词公开开发
包，这只是构建期闭环，不代表日用词量。候选源经过有界的
[只读候选快照](docs/candidate-snapshots.md)构造；快照损坏时不会创建文本服务，
宿主保留原输入路径。固定公开用例也已在注册后的 Windows 记事本中完成
`nihk` →“你好”首选上屏、切回微软拼音和禁用闭环。新候选 UI 使用标准
`ITfCandidateListUIElement` 与不抢焦点的六项弹窗。首项使用略亮圆角底和
左侧柔和天蓝色短条；首选汉字为近白半粗体，普通汉字、序号和分页依次降低
明度与字号，避免六项同亮。数字键明确选过的普通同码候选会在当前文本服务
实例中置顶；两键完整单字码上按普通 `Tab` 会进入标有“找字”的公开笔画筛选，
`h/s/p/n/z` 稳定缩小同码单字池；空格首选、换序与 Tab 候选不学习，
停用即清空，且不写磁盘。前台文本服务默认维护有界的滚动内存现场；输入法内的
“向猫猫许愿”优先按最近完成的输入片段冻结重点与上下文，第二项和命令行仍
保留近 30 秒兜底。明确保存后才使用 Windows 当前用户 DPAPI 加密写入本地。
达到内存上限时分批淘汰最旧事件，不会阻塞输入；保存失败也不影响输入和候选窗。
Windows 11 现代输入指示器并不可靠展示第三方语言栏项目，因此语言栏菜单只作
可选入口；`wish-ime.cmd mark` 通过 TSF 当前用户全局 compartment 发送无正文整数
命令，`wishpad` 则是按需打开的本地“猫猫应愿”管理器。输入码、候选、留言、
路径和快照都不进入控制通道，不注册全局快捷键，也不连接网络。真实 Codex 与
密码控件仍需逐步验收；任何本机注册仍需要用户明确确认。

候选弹窗的内容刷新使用内存双缓冲；只有位置或尺寸真正变化时才重新定位，只有
尺寸变化时才重设圆角区域，避免快速组合更新反复显示、擦除和塑形窗口。更完整的
问题留言、事发前短时轨迹、证据预览与 DPAPI 本地包边界见
[“向猫猫许愿”：本地反馈与现场快照](docs/wish-feedback.md)。

若要让日常使用持续积累证据，而不是每次都主动许愿，可以单独开启
[持续研究收件箱](docs/research-feedback.md)。它默认关闭，只记录猫猫输入法在普通
输入域里已经产生的结构化事件；密码、PIN、私密、受限和未知 InputScope 继续
fail closed。V8 批次用进程内连续流、批次序号和事件序号把存储边界接回，主动许愿
会锚到同一条流；批次写入独立的 `research-inbox`，使用 Windows 当前用户 DPAPI，
不联网、不自动训练，也不会自动进入 Codex 对话：

```powershell
cargo build --release --lib --bin tsf-devctl --bin candidatectl --bin researchctl
.\update-ime.cmd
.\research-ime.cmd on
.\research-ime.cmd status
.\research-ime.cmd review
.\research-ime.cmd off
```

不带参数的 `research-ime.cmd` 等同于 `status`。开启或关闭后，已加载宿主会在后续
输入中自动发现；关闭不会删除已有加密批次。`review` 会在当前终端解密并汇总
真实编码和提交文字，列出非首选提交、翻页、先选后升为首选、取消、原码上屏和
候选窗耗时，并按 DLL 与候选数据版本区分新批次。它还会先连接 V8 批次，再按失焦、
宿主结束和自适应停顿形成自然片段，将有锚点的许愿连接回现场，并把实际采用的自动
换序与重复改码分别列为证据和待确认线索；它不写模型、不联网，也不修改排序。旧
批次没有连续链或版本身份时会明确标为未记录，不根据文件时间推断。

分析更新与输入法换代已经分层：`cargo build --release --bin researchctl` 后，下一次
`research-ime.cmd review` 立即使用新版，不需要关闭任何 TSF 宿主或运行
`update-ime.cmd`。当前单 crate 构建可能顺带刷新 `target/release` 里的待安装 DLL，
但不会替换系统已注册的按内容寻址副本。候选数据和显式别名也可独立切换。只有按键
处理、候选窗和新增采集字段仍需替换宿主内 DLL；系统不会冒险强制热卸载仍被 Codex、
Typora 等进程使用的旧 DLL。

日常工具还可以完全避开 `target/release` 与 TSF DLL：

```powershell
.\refresh-ime.cmd
.\refresh-ime.cmd status
.\refresh-ime.cmd rollback
```

默认命令离线、锁定依赖地构建八个别名、候选、个人排序、研究、许愿和桌面启动 EXE，
并发布到 Git 忽略的不可变 `current / previous` 用户工具槽。现有工具进程不被
关闭，新打开的 `aliaspad` / `wishpad` 和下一次管理命令使用 current；状态只读，
回退先复核完整摘要。这个入口不构建 `--lib`，不生成或替换 TSF DLL，也不读取
私人数据根。`candidate-data.cmd` 可调用当前候选管理器，`personal-ime.cmd` 则把
个人排序命令固定到既有当前用户数据根；具体生效边界见
[用户态工具独立刷新](docs/user-tool-refresh.md)。

刷新还会原子更新固定的原生启动器
`.local/tsf-alpha/desktop-launcher/ziranma-launcher.exe`。桌面上的“向猫猫许愿”、
“自定义短语”和“自然码换代”快捷方式可以分别传入 `wish`、`alias`、`update`；
前两项直接启动摘要校验通过的 current GUI，不再经过 `.cmd` 或闪出命令窗口，
换代仍保留可见控制台、诊断输出和既有确认边界。启动器只接受这三个固定动作，
不会执行调用者提供的程序名或路径。

本地许愿放在 Git 忽略的 `.local/tsf-alpha/user-data/wishes`。默认管理命令只显示
数量和随机内容 ID；查看输入原文必须显式确认，说明另存为绑定同一 ID 的加密
文件，移除则进入可恢复的本地 `trash`：

```powershell
cargo build --release --bin wishctl --bin wishpad
.\wish-ime.cmd
.\wish-ime.cmd mark
.\wish-ime.cmd status
.\wish-ime.cmd list
.\wish-ime.cmd annotate --latest --category display --text "候选窗边角不自然"
.\wish-ime.cmd show --latest --confirm-show-private-text
```

不带参数的 `wish-ime.cmd` 会打开单实例“猫猫应愿”管理器。它不再承担捕获，也不
驻留托盘：左侧按时间列出本地许愿，右侧显示说明和重点现场，并可按需展开前后
记录。选择“整理这条记录…”后，可以补充或修改类别与说明；原始现场保持不可修改，
整理内容继续由当前 Windows 用户加密保存。关闭窗口即退出，不设置开机启动或
全局快捷键。成功执行 `update-ime.cmd` 后也会打开或唤回管理器。
在输入法内也可以精确输入 `xuy` 后按 `Tab`。候选栏只提供“记录刚才的情况”和
“记录更多内容”两项：空格确认自动分段的默认现场，数字 `2` 选择近 30 秒兜底。
分类和文字说明留到之后审阅，保存后会短暂显示结果；退格、Esc 或再次 Tab 返回，
`xuy + 空格` 仍按普通候选上屏，所有非 `xuy` 的普通 Tab 仍进入找字路径。新版许愿记录
把参考上下文、重点片段和触发入口分开保存，旧记录仍可读取。

类别可用 `candidates`、`ranking`、`display`、`latency`、`input-mode`、
`compatibility` 和 `other`。这些命令不上传、不写模型，也不会扫描其他私人会话。

猫猫完成 release 构建后，本地 Alpha 可从任意 PowerShell 目录一键换代：

```powershell
.\prepare-ime.cmd
.\update-ime.cmd
.\update-ime.cmd status
```

`prepare-ime.cmd` 是不需要管理员权限的离线准备步骤：它使用锁定依赖构建 TSF DLL
及日用候选、别名、个人学习、持续研究和许愿工具，然后自动执行只读 `status`。
它只更新仓库的 `target/release`，不禁用、注册、启用或安装输入法；构建失败和候选
数据验证失败都保持现有系统版本不变。开发可以先完成这一步，真正换代仍由宝宝以后
方便时单独运行不带参数的 `update-ime.cmd`。

不带参数才执行换代；`status` 只读比较 release 与已安装 DLL、验证候选槽和当前用户
状态，并报告相对 release 的新旧宿主数量，不弹 UAC、不打开管理器、不写文件。
默认入口只定位并调用仓库内既有的安全换代脚本；真正换代仍会显示 Windows 管理员确认，
不会自行编译、联网或更改默认输入法。已安装版本的 DLL 摘要与候选槽完全相同
时走只读快路径，不再注销、注册或弹出管理员确认；真正的新版本仍保留完整的
安全换代事务。`tsf-devctl inspect` 会额外汇总可见宿主正在加载的同版与旧版
Alpha 数量，不显示进程名、窗口标题、路径或输入内容，便于判断是否只是应用
仍缓存着换代前 DLL。换代前已经活动的 Alpha 可以由既有宿主继续使用；脚本会
用一轮完整的一秒稳定窗口验证持久启用。若换代前未活动，验收仍要求更新过程不得
自行激活它。成功摘要会列出预检、禁用、管理员/UAC、后验检查和宿主扫描耗时，
方便区分真实 TSF 等待与脚本开销；系统 PowerShell 5.1 是固定兼容边界，本机测得
可选 `pwsh 7.6.4` 冷启动反而更慢。

TSF Alpha 的日用按键与终端实验台刻意分开：Space 确认首选，Enter 原样提交
当前字母码；中文模式下 `,` / `.` / `;` 与 `Shift+;`、`Shift+1`、`Shift+6`、
`Shift+9/0`、`Shift+/` 分别输入常用全角标点（含 `……`），有组合时先确认首选
再追加标点。其他可打印键
会先结束预编辑再交还宿主；带 Shift 的数字不会误选候选。交互候选会先列任意长度的完整精确码；
恰为四键、两个完整音节时，再按需分页加入最多 50 个有界单字拼合候选，随后才补自由简写结果；
`ju` / `qu` / `xu` / `yu` 可使用通常的 `u` 写法匹配自然码
规范 `v` 词条。开发 DLL 另带两份项目自有、MPL-2.0 的小型覆盖词表：技术层
补充“简拼、丝印、焊盘、过孔、数据手册、使能、片选”等公开快照缺词，会话层补充
“呜哇”。两层都不修改固定第三方词典，也不从私人输入隐式学习。

个人排序只接受明确的非首选操作：数字选择，或翻页后以空格、标点确认当前页
首项。第一页空格首选、Tab 找字和换序恢复不会被猜成偏好。当前宿主会立即记住
这次选择；普通文本等明确允许的输入范围先把 `code + text` 保留为一条待确认事务。
数字选择或翻页后空格确认若紧接空组合 Backspace，会撤回这次事务并恢复此前的
会话偏好；下一个按键、焦点离开或输入法停用才确认它。标点确认因为候选后还有
标点后缀，第一次 Backspace 通常只删除标点，因此不会被误当成候选撤回。确认后
才分批写入 `.local/tsf-alpha/user-data/personal-ranking`，并使用 Windows 当前用户
DPAPI 加密，使偏好能跨应用和换代继续使用。密码、PIN、私人、受限和无法确认的
输入范围不落盘。确认次数以四次为排序支持上限：没有既有证据时第一次选择即可
生效，已有重复支持的候选不会被一次偶然选择推翻，而新候选重复达到同等支持后
仍可切换。显式固定候选始终位于个人排序之前。

完整码下的个人证据还会有界继承到同词的规范尾简。例如公开词典能证明
`jdjd → 讲讲` 是完整整词、而 `jdj` 的普通候选池本来就含“讲讲”时，这份证据
可以把短码中的“讲讲”提前。它不会传播到 `jd`、任意字符串前缀、显式别名或
自由分句，也不会凭空加入短码候选；短码自身的明确选择与固定候选始终优先。
在短码视图中忘记继承候选只屏蔽该短码，不删除完整码证据。

相邻的两个明确单字选择还能形成一个很窄的个人短语层。例如分别从完整双拼
`ui` 和 `ub` 的普通候选中明确选出“试”和“手”后，`uiub` 会立即出现“试手”；
第二次选择若紧接空组合 Backspace 则整条新短语撤回，否则跨过下一按键、焦点离开
或停用边界后，一次即可进入同一套当前用户加密个人排序，不要求再输入一遍。
首版只连接两个经过公开候选源验证的“两键完整码 + 单字”，第一页空格首选、标点、
简拼、纠错、Tab、换序、原码和宿主快捷键都不会自动造词。标点、取消、焦点或
输入模式变化会断链；同一 TSF Context 内的鼠标光标移动仍是需要文档范围锚点的
后续边界。

同一宿主还会维护一个不落盘的短左侧上下文层。它只在明确选择跨过可撤销边界后，
记录“上一段已提交候选 + 当前实际码 + 当前选择文字”；立即 Backspace 会恢复此前
上下文且不训练。再次遇到相同左侧候选与码串时，程序最多查看现有 Top-12，并只把
其中一项移到首个未保护位置，不生成新候选、不越过明确别名或固定词。精确忘记始终
优先于上下文证据，标点、原码、宿主按键、焦点和输入模式切换会切断当前左侧锚点。
明确选择更后候选时，同一待确认事务还会把被越过的首个未保护候选记为一次反证；
每次反证只抵消一格同身份支持，重复选择才逐渐换位，立即 Backspace 会连同正证一起
撤销。表内最多 2048 组身份，正证与反证各封顶四次；文本服务停用即清空，尚未跨
宿主保存。

普通候选还提供显式的两阶段忘记：组合未提交时按 `Ctrl+Delete`，再用数字键选择
当前页候选；翻页键仍可使用，Esc 或 Backspace 取消。程序只接受确实具有长期或
本轮个人证据的候选，固定词和纯公共候选不会产生动作。成功后当前候选页立即重排，
紧接 Backspace 可恢复；忘记与恢复作为独立的当前用户 DPAPI 加密不可变动作追加，
不删除正向证据，也不改公共词典。

宿主复用已经验证的批次，并为增长的历史自动建立当前用户加密检查点；新进程从
检查点加少量尾部批次恢复，不必逐个解密全部旧日志。检查点不删除原始批次，
因此不会与仍在运行的旧宿主争夺文件。

`personalctl status` 只显示加密批次、明确选择、排序条目与忘记计数；不会显示码串
或文字。`forget` / `restore` 只在进程启动后逐行读取正文，不把它放进命令参数或
shell 历史。显式清空会把整个正向排序目录原子移到同级可恢复归档，不直接删除：

```powershell
cargo run --release --bin personalctl -- status `
  --root .local/tsf-alpha/user-data/personal-ranking

cargo run --release --bin personalctl -- forget `
  --root .local/tsf-alpha/user-data/personal-ranking

cargo run --release --bin personalctl -- restore `
  --root .local/tsf-alpha/user-data/personal-ranking

cargo run --release --bin personalctl -- clear `
  --root .local/tsf-alpha/user-data/personal-ranking `
  --confirm-clear-personal-ranking
```

清空前应先停用 Alpha 或关闭正在使用它的宿主，否则旧宿主仍可能补刷尚未写出的
一小批选择并重新建立目录。旧加密会话训练、持久词对和跨宿主上下文模型仍是
独立的后续工作，不与这条在线个人排序或临时左侧上下文混为一层。

固定候选包的清单与载荷可以在不加载 TSF、不显示候选正文的情况下单独检查：

```powershell
cargo run --release --bin candidatectl -- inspect `
  --manifest tests/fixtures/public/demo_candidate_manifest.zcm `
  --payload tests/fixtures/public/demo_lexicon.tsv `
  --provenance tests/fixtures/public/demo_candidate_provenance.zcp
```

检查器只读三个明确指定的普通文件，不扫描目录、不写文件、不学习、不联网。
公开 TSV 也可以在显式来源、许可和源文件 SHA-256 钉住后，确定性生成一个
全新的候选包目录；目标目录必须尚不存在：

```powershell
cargo run --release --bin candidatectl -- build `
  --source tests/fixtures/public/demo_lexicon.tsv `
  --output .local/candidate-demo-v1 `
  --revision tsf-public-demo-v1 `
  --source-id ziranma-demo-v1 `
  --source-license MPL-2.0 `
  --source-url https://github.com/hewzhew/ziranma-decoder `
  --source-sha256 b7b65f5b9e826fdb4075089f26c4051575fa6a7b197be0d1da8d6ff8d714e100 `
  --public
```

`build` 会输出绑定三份包材料的“发布 SHA-256”。发布者应把它放在与候选包
分开的可信渠道；使用者不能从同一个待验证目录读取摘要再称为可信。拿到独立
摘要后，可以先只读验证。这个值是 `candidatectl` 对三份规范文件计算的域分隔
摘要，不是 ZIP 文件摘要，不能拿 `Get-FileHash` 对压缩包的结果直接替代：

```powershell
cargo run --release --bin candidatectl -- verify `
  --package .local/candidate-demo-v1 `
  --expected-sha256 1f2f3c81280641d9963b0ea0fac1fcdaf749d76bae778034037f015f8b8434c2
```

`candidatectl verify-signature` 还能只读核对一份严格的 Ed25519 脱离签名声明。
它要求明确指定包目录、声明文件，以及从独立可信渠道取得的公钥；公钥若与包和
签名来自同一未验证来源，就没有建立信任。成功报告中的发布 SHA-256 可继续作为
`adopt` / `stage` 的 `--expected-sha256`；只读验签不会自动安装或改变槽位。
也可以显式使用 `adopt-signed` / `stage-signed`，同时给出槽根、包、签名文件
和可信公钥。它们先完成验签，再把同一份已加载材料交给现有安装与预检路径，
避免验签后重新读取外部包。验签失败时不创建或改变槽位。所有签名命令都不
回显公钥和签名正文，也不会发现或保存信任密钥。

项目目前尚未发布真实发布公钥，也没有签名命令、私钥存储、密钥轮换或吊销
策略，因此这里不提供可误执行的示例密钥命令。

候选包可以在不注册输入法的情况下经过真实 Windows TSF 合成 Context 预检：

```powershell
cargo run --release --bin candidatectl -- preflight `
  --package .local/candidate-demo-v1
```

预检从包内确定性选择一个完整码首选，逐键验证预编辑，再用空格确认上屏；
报告只显示版本、按键数、上屏字数和结果，不显示候选正文。预检凭据使用
SHA-256 绑定来源侧车、清单、载荷、宿主和解码兼容标识。

显式指定的本地槽库支持 `status`、摘要驱动或签名驱动的 `adopt` / `stage`、
`promote` 和 `rollback`。它只接受通过完整校验和 TSF 预检的公开明文包；槽位
状态原子替换，包文件不就地改写。预检凭据以 SHA-256 绑定三份包材料和兼容
标识，缺失、损坏或包被改写都会阻止提升：

```powershell
cargo run --release --bin candidatectl -- adopt `
  --root .local/candidate-slots `
  --package .local/candidate-demo-v1 `
  --expected-sha256 1f2f3c81280641d9963b0ea0fac1fcdaf749d76bae778034037f015f8b8434c2
cargo run --release --bin candidatectl -- status --root .local/candidate-slots
```

`adopt` 和 `stage` 强制要求该独立摘要，并在创建或改变槽位前核对；摘要缺失、
格式错误或不匹配时不安装、不预检、不改变状态。对应的 `adopt-signed` /
`stage-signed` 强制要求显式签名文件和可信公钥，并在任何槽位写入前完成
Ed25519 验证。它们认证候选包发布声明，不代替 Windows DLL / EXE 的
PE/Authenticode 签名。

若槽根固定为 DLL 同目录的 `candidate-data`，新类工厂会读取经过预检的
`current`。目录完全不存在时才使用编译进 DLL 的公开演示包；目录存在但损坏、
未配置、缺少预检凭据或含明文私人数据时拒绝建立类工厂，不静默回退。已有
类工厂继续持有旧的不可变快照，提升后创建的新类工厂才观察到新版本。
早期双文件包和 v1 预检凭据不会原地升级；Alpha 开发环境应重新构建包并采用
全新的 `candidate-data` 根，避免旧格式或半迁移状态被误认为当前包。

更大的公开词库不替换核心包。默认关闭的补充完整词层使用
`.local/tsf-alpha/user-data/public-supplement` 独立槽根：先用普通 `adopt` 验证并
准备公开包，再以 `candidatectl supplement-enable --exact-promotions 1` 显式开启；
`supplement-status` 只读检查，`supplement-disable` 原子关闭但保留候选包。补充层
只加入核心完整词集合中不存在的新整词；一旦命中，第一页保持在完整词通道，
不再用自由简拼句子凑满空位。状态或包失效时回退到仅核心候选。
完整命令与固定热路径口径见[候选快照文档](docs/candidate-snapshots.md#独立公开补充根)。

用户主动指定的精确别名与公开候选包分开保存。`alias-ime.cmd` 把它们写入
Git 忽略的 `.local/tsf-alpha/user-data/aliases`，正文始终先经过 Windows
当前用户 DPAPI 加密，再进入不可变包和 current / candidate / previous 三槽。
日常使用时，不带参数运行脚本会打开“自定义短语”面板。填写小写字母触发码和
候选内容后一次保存并切换，旧版本留作撤销；移除和回退也在同一面板完成。面板通过私密
标准输入把内容交给独立管理器，码和文字不出现在子进程命令行：

```powershell
cargo build --release --bin aliasctl --bin aliaspad
.\alias-ime.cmd
```

需要分阶段审查时仍可使用原有 CLI：第一条别名直接建立 current，后续 `set`
或 `remove` 只进入 candidate，确认后再提升：

```powershell
.\alias-ime.cmd set --code wua --text 呜哇
.\alias-ime.cmd set --code wuu --text 呜呜
.\alias-ime.cmd set --code vtrayn --text v2rayN
.\alias-ime.cmd promote
.\alias-ime.cmd status
```

触发码保持为 1–64 个小写 ASCII 字母，避免占用候选数字键和宿主的 Shift 行为；
候选内容则可以混合中文、英文、大小写和数字。因此软件名可显式设为
`vtrayn → v2rayN`，但程序不会擅自替用户选择或写入这个私人映射。

`remove --code <码>` 同样先暂存；`unstage` 放弃暂存，`rollback` 交换 current 与
previous。只有显式加入 `--confirm-show-private-text` 的 `list` 才会在当前终端
显示别名正文。面板与命令都不联网、不从打字中学习，也不会把私人配置复制进
安装 DLL 目录。支持这一格式的文本服务只在“当前没有组合、即将输入第一个
字母”时读取小型槽指针；指针改变后先完整解密校验，再为这一次新组合替换内存
快照。损坏的更新保留该服务最后一次成功加载的版本，不会在组合中途偷换，也
不需要重装 DLL。界面或 TSF 代码变化仍需正常换代，并等待已有宿主自然释放旧 DLL。

发布 DLL 的架构、COM 导出、证书目录、固定 CLSID 的标准 COM 注册位置、
zh-CN 语言配置和键盘类别可以先用只读工具核对：

```powershell
cargo build --release --lib --bin tsf-devctl
.\target\release\tsf-devctl.exe inspect --dll .\target\release\ziranma_core.dll
```

获得明确确认后，可以在管理员 PowerShell 中执行本机范围、64 位、默认关闭的
开发注册。它把 DLL 复制到按 SHA-256 寻址的 `.local` 不可变目录，不启用、
不激活、不设为默认，也不修改微软拼音：

```powershell
.\target\release\tsf-devctl.exe register-machine `
  --dll .\target\release\ziranma_core.dll `
  --confirm-machine-wide-development-alpha

.\target\release\tsf-devctl.exe unregister-machine `
  --confirm-machine-wide-development-alpha
```

注册完成后，可以在普通 PowerShell 中显式更改当前用户的“可选”状态。启用
不会把 Alpha 设为默认，也不请求进程或桌面范围激活；Windows 可能只在执行
命令的短生命周期辅助线程内短暂激活该配置。换代脚本会等辅助进程退出后用
独立进程复查“已启用、未活动”；禁用仍是测试后的安全退路：

```powershell
.\target\release\tsf-devctl.exe enable-current-user `
  --confirm-enable-current-user-development-alpha

.\target\release\tsf-devctl.exe disable-current-user `
  --confirm-disable-current-user-development-alpha
```

四层注册、当前用户启用边界、失败回滚、安装记录和反向注销口径见
[TSF Alpha 开发注册](docs/tsf-dev-inspection.md)。

## 候选实验台

`candidate-lab` 默认只显示适合直接阅读的中文：普通候选、预计操作数、
相对完整输入省下的动作，以及每个词使用完整双拼还是简拼。容易干扰正常
输入的按键颠倒候选默认隐藏，只有显式加入 `--recovery` 才会出现。

这里的名次只属于固定研究解码器，不能外推成当前 TSF 现场。核对当前核心包
和公开补充包可使用 `candidatectl runtime-query`；它会明确标注仍未包含的
别名、项目覆盖、会话记忆、个人学习与上下文层。实机候选只能由候选窗或
许愿中的候选帧证明。

`--expect <目标文字>` 增加一次课程式精确检查，直接报告目标在普通栏或
已开启恢复栏中的名次。它只检查每栏当前实际显示的候选（最多 10 项）；
没找到时不会声称整个候选空间都没有，恢复栏未开启时也会明确写“未检查”。

当前 `candidate-lab` 只应用于公开、人工构造或合成材料：连续按键串和
`--expect` 目标都会作为命令行参数进入 shell，输入、目标和候选也会原样
出现在终端或 JSON 中。程序自身不会学习或持久化这些内容，但 PowerShell
历史、终端回滚、重定向和自动化日志仍可能保存明文；在提供不经过命令行
参数的私人入口前，不要用它输入私人聊天内容。

算法评分、纠错预算与语言模型证据收进 `--verbose`；自动评测则使用
`--json` 输出的稳定英文字段，并以 `contains_text: true` 明示其中含输入、
候选及可选目标文字。`--verbose` 与 `--json` 互斥，三个层次不会再混进
同一个终端界面。

预计操作仍只是统一口径的投影，不估算翻页与视觉查找。普通候选仍可能
包含自由混合简拼，也不等于最终输入协议。完整边界、首份实例和停止条件见
[候选实验台：从程序指标到真实手感](docs/candidate-lab.md)。

`shape-lab` 是更窄的单字课程与沙盒。默认界面只显示可选目标、规范双拼、
当前笔画前缀和候选。在 Windows 交互终端里直接按 `Tab` 进入辅助，再按
`h/s/p/n/z`、数字、退格或 `Esc`；每一键立即刷新，不需要按 Enter。选择后
只输出选中的字，不替使用者总结手感。若输入或输出被重定向，程序自动回退
为行命令 `t`、`hspnz`、数字、`-`、`esc`、`q`。`--prefix` 可直接查看一个
公开前缀。

`shape-course` 把同一冻结协议串成一次连续会话，不必每题重新运行命令。
每题从空输入开始：先按目标读音输入完整自然码双拼，普通候选出现后再按
Tab 和笔画键，最后用数字选择。这比直接从 Tab 开始多覆盖了真实输入中的
拼音编辑与候选出现边界；双拼输错可以退格修改，普通候选阶段退格会回到
双拼，Tab 内退格才撤回笔画。
`easy`、`medium`、`hard` 分别选择在全部公开替代笔顺下最少一、二、三画
进入首屏的目标，`mixed` 按三个级别轮流出题。选中目标后直接进入下一题，
不输出赞美或替使用者判断是否省力；Enter 跳过，输入双拼时 Esc 结束，
普通候选或 Tab 阶段用 `q` 结束。结束页只给
进度、双拼键、Tab、笔画、退格与误选计数，不保存文件，也不读取私人记录。

候选池大小、原排名、动作投影与完整公开笔画码不属于体验界面，只有显式
加入 `--details` 才会输出并立即退出。筛选仍只删除不匹配项、不重排；
动作投影也仍只是研究口径，不代表真实翻页、视觉搜索或输入法延迟。拼音、
目标和候选会出现在终端，因此只应用于公开或合成材料；程序不学习、不写
文件。

## Tab 音形辅助原型

对于“已经知道读音，但目标生僻字在同音候选中很靠后”的情况，仓库现有
一个独立的显式 Tab 筛选原型。它在普通解码完成后冻结候选池，使用
`h/s/p/n/z` 五类笔画前缀或有序部件拼音首字母做稳定过滤；不改变原分数、
原顺序，也不自动上屏。两套字母解释同时命中时会保留两条可审查证据，
不会暗中猜优先级。

现在已有一份固定到确切提交、保留 CC BY 4.0 署名与完整账目的真实五类
笔画快照，以及拒绝畸形或越界输入的严格导入器；一个字的替代笔顺不会被
静默丢掉。`public-shape-audit` 现已在固定公开同码单字池中专测原排名
第 11 名以后的 13,285 个目标：最多三画时，12,589 个目标在**全部**上游
替代笔顺下都能进入 Top-10，平均候选池从 91.94 缩到 5.47；29,906 个
笔顺试次全部保留目标。这个强结构信号支持继续研究 Tab 笔画筛选，但公开
词典权重不等于现实候选顺序，评测也尚未计入 Tab、选词、翻页和视觉寻找
的净成本。公开 `shape-lab` 已能在 Windows 终端用真实 Tab 逐键查看同一
冻结池与笔画过滤，但真实部件数据库、已安装输入法候选和系统级 Tab 热键
仍未接入。设计与停止条件见
[Tab 音形辅助：显式的第二阶段筛选](docs/tab-shape-refinement.md)。

## 自然码 codec

公开词典只保存中文、空格分隔的无声调拼音和合成相对权重。标准自然码
由 `src/codec.rs` 自动生成，不再手工重复维护。映射参照
[Rime 官方自然码双拼方案](https://github.com/rime/rime-double-pinyin/blob/master/double_pinyin.schema.yaml)
核对。

这里采用宝宝实际键盘的规范写法：`sh → u`、`ch → i`、`uang → d`、
韵母 `an → j`，所以 `shuang → ud`、`chan → ij`。零声母两字母音节
保留原拼音，例如 `an → an`、`ai → ai`；单字母 `a → aa`，三字母
`ang → ah`。Rime 代数中还会派生 `aj` 一类兼容拼法，但它不是本项目
codec 输出的规范写法。

codec 支持 `v`、`ü` 和 `u:` 三种 ü 写法，并保留逐音节边界供混合简拼
使用。它验证映射结构，但还没有内置完整的普通话合法音节表，因此不能
把“能够映射”误认为“语言学上一定存在这个音节”。

## 简拼协议的理论边界

当前“每个音节任意选择一键或两键”的自由混合规则已经完成独立的
[形式化可行性审计](docs/abbreviation-feasibility.md)。固定公开码本中
26/26 个字母都能作为一键简拼，410/410 个完整码也都能切成两个一键
简拼；`aa` 既是完整音节 `a`，又是两个简拼音节。重复 `a` 的 32 键
输入仅一键/两键边界就有 3,524,578 种解释。

因此自由混合不是唯一可解码协议。动态规划能避免真的展开全部路径，
语言模型能给路径排序，但二者都不能恢复被一键简拼丢掉的信息。它从
现在起只作为搜索压力测试，不再默认是未来输入界面。近期优先研究：

- 常用词或短语的显式快捷码白名单；
- 至少一个完整音节锚定后，只允许连续尾部简拼；
- 把简拼当候选补全，由用户选择提供缺失信息；
- 锚定尾部简写的一次相邻颠倒单列恢复候选，其他简写纠错继续隔离研究。

可用 `cargo run --release -- abbreviation-audit` 复核码本计数与直接反例。

`public-protocol-audit` 已进一步把公开 train 固定切成 fit/dev，对照
显式快捷短语白名单、每词最多省一键、锚定尾简和显式全简模式。128 条
held-out 短语中，完整码、保守尾简、锚定尾简、显式全简的 Top-10
分别为 `122/104/104/25`。另有 116 条 3+ 音节词压力层：完整码、
保守尾简、锚定尾简为 `115/113/113`；后两者分别少 160/309 个字母，
首选为 76/72。
fit-only 的 826 条重复且不互撞快捷短语没有覆盖任何一条所选 dev
短语。结论是：保守尾简与锚定尾简都值得进入真实操作成本对照；全简
模式不能成为通用默认；快捷码应学习宝宝真正重复的表达，而不是期待
公开短语白名单自动泛化。完整方法、冲突口径和停止条件见
[受限简写协议审计](docs/protocol-audit.md)。

## 连续组合：不按词确认

当前最贴近实际输入的主线是“每个词先打一个完整音节，后续音节只打
首键，词与词之间不按空格”。例如：

```text
麻烦：ma + f  = maf
猫猫：mk + m  = mkm
整段：           mafmkm
```

固定公开大词典已能把 `mafmkm` 自动切成 `maf | mkm`，并将“麻烦猫猫”
排在主候选第一。若第一组 `mk` 相邻打反为 `km`，输入成为 `mafkmm`，
单词搜索也能解释为一次相邻颠倒；但大量零错误简拼切分会把它挤出普通
首屏。

`public-compose` 因此保留两栏：主候选仍使用稳定的零错误优先顺序；
另一栏专门展示“完整首音节 + 连续尾部简写”内部的一次相邻颠倒。它
不会为了抬高纠错而打坏干净输入 Top-1，也不要求用户切换输入模式。
实现与固定公开评测见[连续组合设计](docs/continuous-composition.md)。

`public-compose-evaluate` 不再使用高度重叠的“源序前 64 个窗口”：
它先从固定 UD test 的每个句子至多保留一个合格双词窗口，再从 489 个
独立句代表中均匀取 64 条。尾部简写把 418 个完整码字母降到 337 个，
节省 19.4%；主榜 Top-1/3/5/10 为 `18/31/37/40`，相邻颠倒恢复栏为
`18/35/36/40`。

`public-compose-audit` 继续深查 24 条 Top-10 外样本：16 条在
第 11～100，8 条仍在 Top-100 外，错误首选 24/24 与答案同字数。
在冻结的同一 Top-100 池中，train-only 词级 bigram 只把 5 条送回
Top-10、1 条送到第一；纯字符平均 bigram 为 4 条和 0 条。因此当前
证据支持“同长度排序是瓶颈”，却不支持把这两个弱模型接入生产排序。

## 固定公开词典

`data/public/rime-pinyin-simp/` 原样保存 Rime 官方
`rime-pinyin-simp` 的固定快照，提交为
`0c6861ef7420ee780270ca6d993d18d4101049d0`，SHA-256 为
`e341598343a0f0f2035bb1aafc34a7f3bb7887deeecb3f60796262aaa2983e6b`。
上游许可证、作者说明、下载地址和转换说明都与数据放在一起，并汇总于
`THIRD_PARTY_NOTICES.md`。

上游 65,125 行中，当前简体专用导入器保留 62,757 个词条：1,714 个零
权重在内存中升到 1，2,360 条被同音高频简体字遮蔽的繁体单字读音、
7 行不支持的拼音和 1 行重复项被明确计数跳过，原文件不被改写。保守
清单由固定 Rime 快照与固定 OpenCC `TSCharacters.txt` 映射派生；不会
删除多字词，也保留没有满足规则的其他读音。`public-index-stats` 会复核
这些数字及 trie 结构；快照导入统计还有回归测试保护。62,757 个词条
分布在 39,027 个非空终点节点，单个同码节点最多挂 282 个词条。

`public-sentence` 仍只使用上游权重作为 unigram 实验配置，不应把它的
首选当作成熟输入法质量；例如全简拼 `zrmurf` 在大词典下会出现许多
合法但语义不对的完整覆盖。另有一个仅供 `public-calibrate` 使用的
train-only bigram 基线；它只做双路径诊断，没有接入完整生产搜索。

## 固定公开句子校准集

`data/public/ud-chinese-gsdsimp/` 原样保存 Universal Dependencies
Chinese GSDSimp 的官方 train/test splits，固定到提交
`4231dfd59866fa5999ad4a6bc1fdecd7985b3b59`。test 含 500 句、12,010 个
语法 token，SHA-256 为
`3af8046a6f32477b4d5cf3dd06bbf38682a380fe77aade3f68de97e51ab94900`，
train 含 3,997 句、98,614 个语法 token，SHA-256 为
`956636fe612a1166e8b19e7413fee2e73d68231aca2f0455be2c616b947d629d`。
采用 CC BY-SA 4.0；许可证、上游 README、来源和限定说明与数据一起
保存。

`public-calibrate` 只把 train 映射成 Rime 分词序列，用 add-0.5
估计 bigram；test 从不参与训练。训练保留 2,339 个纯汉字可覆盖序列、
51,712 个词实例，形成 40,299 种二元组。test 按上游顺序筛选 8～24 个
纯汉字的句子：词在 Rime 中存在时使用最高权重整词读音，否则逐字使用
确定的 Rime 读音。111 句可覆盖，固定取前 64；另从 Rime 未收整词但
逐字可表达的 699 个唯一 UD token 中固定取前 128。每份文本同时生成
完整码和全简拼轨道。文本组合来自 UD，读音和权重来自独立的 Rime
快照；没有用户数据，也不按解码结果挑样例。

当前双路径诊断只比较公开答案路径和现行 unigram Top-1。自然句完整码
的 60 个原始错误中，答案路径只有 1 次被 bigram 提到更高；未收整词
完整码为 7/123；两条全简拼轨道均为 0。这个负结果说明当前稀疏词级
bigram 不值得直接接入昂贵的完整大词表搜索。它不是完整搜索准确率，
但给下一步设下了明确门槛：先改善上下文覆盖与候选路径，再谈集成。

现行 unigram 的只读候选召回进一步定位了问题：自然句完整码
Top-1/5/10 为 `4/10/12`（共 64），未收整词完整码为 `5/15/24`
（共 128）；两条全简拼轨道直到 Top-10 仍均为 0。公开答案路径由
Rime 词条直接构造，在 lattice 中结构可达；低召回表示它被大量合法
组合压到可见窗口之外，不是词典无法表达。

train-only 字级 bigram 又提供了一层长度审计。模型用 74,381 个字实例
观察到 43,048 种相邻关系。累加字级概率看似把自然句完整码双文本上限
推到 46/64，但原始错误中有 25 个公开答案比 Top-1 更短；等长的 35 个
样本实际只以 18:17 微弱偏好答案。全简拼的公开答案在自然句 64/64 中
都更长，因而累加分全部落败；按字符转移取平均后答案胜 47/64。这个
对照揭示了简拼输出字数先验的问题，却尚未给出可直接接入动态规划的
可靠分数，因此同样保持只读。

## trie 与本地 bigram

Decoder 创建时把每个词按标准两键音节序列写入 trie 一次，不预先生成
`2^n` 个全码、简拼混合字符串。查询时，每经过一条音节边，都可以消费
两键完整码或只消费第一键，并同步记录简拼音节位置。因此仍然不必逐词
扫描，又把指数拼写集合留成了隐式路径。

`cargo run -- index-stats` 可以审查这个区别：当前 50 词演示词典只存 50
个词条终点、96 个节点和 95 条音节边，却隐式表示 212 种全码/简拼拼写。
这些数值也有结构回归测试保护。

错误通道也在同一次 trie 遍历里推进。对齐状态记录实际输入位置、是否
已经使用错误预算，以及是否正在等待完成一次跨边界颠倒；它能直接消费
邻键替换、漏键、多按和相邻颠倒，不再预先生成整批错误编码假设。到达
词条终点后才生成稳定的纠错说明。一次错误下同时可达的状态最多只有
5 个，因此热路径使用固定容量状态集和终点长度集，不为每条 trie 边
反复申请小 `Vec`。同一扫描内，规范化状态集会被实习化为整数 ID；
`状态 ID × 下一键` 的逐键结果只计算一次，之后直接查 26 列转移表。

`cargo run -- search-stats nhk` 会显示一次单词搜索访问的 trie 路径状态、
实际检查/精确复用的按键对齐状态和去重前终点匹配数。这些计数用于理解
实现，不是稳定性能基准。

多词搜索可以加载 `demo_bigram_corpus.tsv` 中独立保存的人工分词序列。
它使用 add-0.5 平滑，并把 unigram 与 bigram 对数概率按 35% / 65%
插值。CLI 会显示共现次数、前词总次数、平滑值和最终语言分。

句子动态规划在每个活跃词界只启动一次 trie 前缀扫描。扫描沿后续输入
流式前进，每到达一个词条终点先产生一条轻量记录，不再把每种片段长度
分别送入单词解码器。同一位置的零错误边还会在两个全局错误预算层之间
复用。同一终点路径只保存一份 `Spelling/Correction`，挂在节点上的多个
同码词条用路径编号、词条编号和分数这些标量参与后续聚合，不再各自克隆
解释。无 bigram 时，索引记录先完成与完整候选相同的结构去重、错误层
折叠和 Top-K 子状态缩减，只有留下的记录才复制中文、拼音和解释对象；
bigram 模式仍物化全部记录。

同一无 bigram Top-K 边界还会先进行一次很窄的纯精确预扫，完整确认同一
规范词条身份在每个终点是否存在精确解释。trie 节点保存子树最高词频和
剩余音节范围；搜索把“子树最高 `ln(词频)` 减去已经支付的简拼代价”
作为所有后代的乐观上界。未用错误的精确边、未用错误的纠错边、已用
错误后的精确边各维护独立的 K 个稳定文本阈值；只有上界严格低于该子树
可能进入的每一个阈值时才跳过子树，分数相等不会剪枝。word 解码和
bigram 解码不启用这项优化。

同一终点节点的词条按频率从高到低保存。某条拼写路径上的所有词条共享
简拼与纠错代价，所以词频顺序也是总分的非增顺序；当当前词条已经严格
低于它可能进入或通过跨错误层替换而影响的所有稳定阈值时，后续词条可以
整段跳过。精确词条和纠错词条都必须同时尊重未用错误状态的两个阈值，
精确词条在存在已用错误前缀时还要尊重第三个精确阈值。等分词条继续
展开，绕过解析器构造的重复身份也保持原行为。

每个活跃位置还加入一条逐键未解析边。它把无法诚实解释的按键显示为
`〔x〕`，没有拼音或虚构中文，按键本身原样保留；每键显式扣 8.00 分，
不消耗一次纠错预算，并重置 bigram 上下文。即使词典为空，句子解码也
会返回这种可审查结果，而不是整串消失。

`cargo run -- sentence-stats zrmurf` 可以审查活跃词界扫描数、累计 trie
路径、精确证据预扫的路径/词条量、子树剪枝与对齐状态、终点拼写路径/
实际展开/上界跳过的词条数、lattice 逻辑生成/完整物化/最终保留的词边
与未解析边、求解的 k-best 状态、缓存命中和路径组合数。

lattice 完成后，排序状态只包含输入位置、全局错误预算和前一个词。相同
状态的未来语言分完全一致，因此解码器可以记忆化求出每个状态的 Top-K
唯一后缀，再由根状态得到完整句子；不同前词不再被塞进同一个经验 beam。
这个结论针对当前固定词典 lattice 和一阶 bigram 状态。

没有加载 bigram 时，候选文本不会成为下一状态的一部分。因此生成每个
位置的边后，还可以按完整子状态提前保留至多 K 个唯一前缀；组内第 K 名
以下的边与完全相同的后缀组合后也不可能进入最终 Top-K。这是精确缩减，
不是 beam。加载 bigram 时当前词会成为下一状态的“前一个词”，所以代码
明确跳过这层提前缩减，留给完整状态的 k-best 排名器处理。

排序首先最小化未解析键数，再偏好零纠错，最后比较语言和局部代价。
因此任何完整词典覆盖（即使需要一次支持的纠错）都在含 `〔x〕` 的路径
之前；完整零纠错分词又一定在纠错路径之前。语言模型只在相同保守层级
里比较“键盘”还是“简拼”。

## 公开合成评测

`tests/fixtures/public/demo_lexicon.tsv` 当前包含 50 个手工公开词条。
`cargo run -- evaluate` 会确定性生成：

- 干净完整码；
- 所有非空的全码、简拼混合方式；
- 每个键的全部 QWERTY 邻键替换；
- 每组不同字符的相邻颠倒；
- 每个位置的一次漏键；
- 每个间隙的一次确定性重复键；
- 相邻两个词的全简拼无词界拼接。

当前会产生 1,749 个样例，并报告各类 `Recall@1/5/10` 及干净输入首选
是否仍为原样解释。样例由被测词典自身生成，因此这些结果只用于回归
检查和比较算法版本，**不代表真实中文覆盖率或实际输入准确率**。

另有 13 条与训练语料分开存放的人工句例。当前 unigram 的 Top-1 是
12/13，bigram 是 13/13；样本很小，只能说明上下文评分链路确实改变了
预期的歧义排序。

另有 5 条更长的人工句例和 12 个独立于演示词典的人工词外探针。当前
长句在小词典中为 5/5 Top-1；词外探针有 9/12 的首选显式含未解析键，
3/12 被其他词典路径完整覆盖，全部原样保留为 0/12，未解析键为
17/48（35.4%）。这组不漂亮的数字被刻意保留，因为它揭示了大词典和
全简拼下最需要解决的误解释风险。

评测还会运行只读的“拒识影子”扫描，不改变候选或排序。它比较最佳完整
词典路径和全部逐键原样回退：

```text
每键分差 = (最佳完整路径分 - 全部原样回退分) / 输入键数
```

没有完整词典覆盖时直接计为拒识；有覆盖时扫描 0～8 的每键最低分差，
同时报告 18 条已知句会保留多少、12 个词外探针会拒识多少。当前人工
小样本中，已知句范围为 `5.090～5.910`，3 个有完整覆盖的词外探针为
`4.657～4.715`，门槛 `5.0` 恰好得到 `18/18` 保留和 `12/12` 拒识。
独立公开校准已经否定了这个干净分界：自然句完整码只有 4/64 首选相符，
相符分差为 `5.639～5.876`，60 个不符首选却跨越 `4.601～6.231`；
门槛 `5.0` 只拒绝其中 4/60。未收整词完整码也只有 5/128 相符，且
门槛 `5.0` 会拒绝全部 5 个相符结果。全简拼两组分别为 0/64 和
0/128 相符。**当前没有采用产品阈值**：unigram 下的上下文歧义不能靠
一个分差标量修好。

`cargo run --release -- benchmark 3` 会用固定公开快照、固定查询、一次
预热和重复采样分别报告单词、短句、长句的 min/median/mean/p95/max。
一次本机运行中单词 median 约 0.9 ms、短句约 8.5 ms、长句约
20.4 ms；这些数字不能跨机器比较。结构去重曾把短句/长句中位数从约
`1.21 s / 2.56 s` 降到 `128.9 / 251.1 ms`，随后轻量终点、转移复用、
共享终点解释、子树上界与终点词频上界继续降低工作量。

当前含分型恢复前沿的短句/长句路径访问为 10,760 / 11,622；其中
536 / 594 条精确预扫路径核对 6,009 / 7,187 个词条，并剪掉
19,154 / 18,827 个子树。对齐工作为 `5,623 实查 + 859,991 复用` /
`5,641 + 798,787`；终点拼写路径为 7,555 / 8,538，正式展开
2,319 / 2,459 个词条并跳过 70,455 / 68,633 个词条。逻辑、物化、
最终 lattice 边为 `2,198 → 575 → 564` / `2,378 → 604 → 594`，
排名转移为 751 / 806，路径组合为 7,751 / 10,367。分型恢复有意多
保留锚定颠倒证据，因此比只保留普通前沿的旧记录稍重；标准评测候选、
结果校验和与不裁剪 oracle 仍不变，固定负载仍在当前预算内。

评测方法和偏差详见 `docs/evaluation.md`。

## 设计边界

当前搜索器可以在全局一次错误预算下联合推断多个词的边界和自由简拼
路径，但自由简拼只作为研究压力测试。仍未实现：

- 两次及以上按键错误；
- 能推断拼音或中文的未知词模型（当前只逐键原样保留）；
- 足以改善大词典排序的密集公开上下文模型或个人词频；
- 从真实打字中估计错误概率；
- 神经网络、在线服务或遥测；
- 可注册的 Windows TSF 输入法、带版本槽位的日用候选数据层或候选窗口；
- 自动上屏或静默修改。

底层性能阶段至此封口。独立公开评测已经证明：直接把“完整词典覆盖”
换成固定分差阈值，会同时漏掉高分错误结果并拒绝低分正确结果。
train-only 词级 bigram 基线也已完成，但双路径诊断几乎没有翻转错误
首选，因此不直接接入完整搜索。字级 bigram 揭示了输出字数偏置，但
等长文本上的语义信号仍弱。更根本的码本审计又证明自由混合不是唯一
可解码协议，因此下一阶段暂停加模型，先在理论上选定歧义有界的快捷码
或锚定简拼协议。由于当前 test 已用于架构诊断，后续把它视为持续
benchmark；新的确认结论需要尚未用于选择的公开 holdout。未知输入仍
必须原样保留。

## 开发检查

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
& .\scripts\release-audit.ps1
```

GitHub Actions 会在 Linux 与 Windows 上重复运行 Rust 检查，并在 Windows
任务中执行严格发布审计。工作流只有仓库内容读取权限，不保留检出凭据。

提交补丁和首次公开前的完整要求见[贡献指南](CONTRIBUTING.md)。最终发布
候选还必须在干净工作树上通过
`& .\scripts\release-audit.ps1 -RequireClean`；脚本只审查 Git 候选文件、
历史路径名和固定公开快照，不扫描 Git 已忽略的私人目录。

## Codex 单应用追踪探针

Codex 的 ProseMirror 输入框已通过一次 B 级兼容性体检：UIA 能报告完整
拼音组合，普通文本变化能确认最终上屏，但没有观察到
`CompositionFinalized`。仓库因此只提供一个手动、内存态、PID 白名单的
局部实验，不把它当作通用采集器：

```powershell
# 只核对目标属性，不读文字、不监听
cargo run --bin tracker-probe -- --pid <PID> --check

# 默认只报告长度，不显示文本，不捕获原始按键
cargo run --bin tracker-probe -- --pid <PID> --arm

# 只对人工短句显式开启内容预览和限域按键配对
cargo run --bin tracker-probe -- --pid <PID> --arm --preview-text --capture-keys

# 纯内存候选预览；时间边界必须由本次实验明确给出
cargo run --bin tracker-probe -- --pid <PID> --arm --preview-candidates `
  --candidate-gap-ms 5000

# 可选：仅新建一份脱敏汇总，不保存原子事件或候选明细
cargo run --bin tracker-probe -- --pid <PID> --arm --capture-keys `
  --preview-candidates --candidate-gap-ms 5000 `
  --save-summary data/private/session-summaries/run-001.json

# 可选：只读合并操作者逐个点名的脱敏摘要；不扫描目录、不写文件
cargo run --bin summary-report -- `
  --input data/private/session-summaries/run-001.json `
  --input data/private/session-summaries/run-002.json

# 私人明文事件胶囊：必须显式捕获按键、给出新路径并单独确认未加密保存
cargo run --bin tracker-probe -- --pid <PID> --arm --capture-keys `
  --save-capsule data/private/event-capsules/manual-001.zic `
  --allow-private-plaintext

# 只读回放逐个点名的胶囊；紧凑模式只输出核心脱敏聚合
cargo run --release --bin capsule-replay -- `
  --input data/private/event-capsules/manual-001.zic `
  --window-gap-ms 5000 `
  --public-context `
  --compact

# 明确授权后：只输出 Tab 笔画、单字上屏与短语裁剪的脱敏动作账
cargo run --release --bin capsule-replay -- `
  --session <SESSION_ID> `
  --shape-audit

# 最后一个公开对照：同一冻结候选池改用平均字符 bigram 分数重排
# 与 --public-context 互斥，仍只读、脱敏且不使用胶囊训练
cargo run --release --bin capsule-replay -- `
  --input data/private/event-capsules/manual-001.zic `
  --window-gap-ms 5000 `
  --public-character-context `
  --compact

# 成对排序摘要：一次流式读取，同窗比较现行频率、公开词 bigram 与字 bigram
# 只输出脱敏计数，不写文件，也不替研究者给出采用或停止建议
cargo run --release --bin capsule-replay -- `
  --session <SESSION_ID> `
  --window-gap-ms 15000 `
  --ranking-report

# 因果个人词缓存：预测后学习，文档重叠编辑可撤销，纯内存、不导出
cargo run --release --bin capsule-replay -- `
  --input data/private/event-capsules/manual-001.zic `
  --window-gap-ms 5000 `
  --personal-cache `
  --compact

# 有序个人词对：包含相同词频底座，跨连续提交学习顺序，总提升仍封顶三位
cargo run --release --bin capsule-replay -- `
  --input data/private/event-capsules/manual-001.zic `
  --window-gap-ms 5000 `
  --personal-pair-cache `
  --compact

# 严格过去/未来分离：旧胶囊只训练，最终成绩只统计后续评测胶囊
cargo run --release --bin capsule-replay -- `
  --history-input data/private/event-capsules/natural-001.zic `
  --input data/private/event-capsules/natural-002.zic `
  --window-gap-ms 15000 `
  --personal-pair-cache `
  --compact

# 同场比较：公共排序、冻结历史词频、评测期先预测后学习的因果词频
cargo run --release --bin capsule-replay -- `
  --history-session <OLDER_SESSION_ID> `
  --session <NEWER_SESSION_ID> `
  --window-gap-ms 15000 `
  --personal-word-comparison `
  --compact

# 精确身份与混合对照：旧历史词频 + 评测期同码重复，共享三位提升上限
cargo run --release --bin capsule-replay -- `
  --history-session <OLDER_SESSION_ID> `
  --session <NEWER_SESSION_ID> `
  --window-gap-ms 15000 `
  --personal-code-comparison `
  --compact
```

历史选择器只建立因果缓存，不计算不会进入最终评测报告的历史候选名次。
个人缓存与 `--compact` 组合时，也只计算紧凑报告实际包含的窗口指标；
学习、撤销、窗口边界及所有显示字段与完整路径保持等价。
`--personal-word-comparison` 从同一份历史计数分出冻结与因果在线两条路线；
每个策略码只生成一次公共 Top-13 候选池，两条路线共享该池后分别重排。
冻结路线在整个评测期不更新，因果路线则在每个窗口完成全部预测后学习。
报告只含聚合名次与计数，不显示原文、不写个人模型、不联网。
`--personal-code-comparison` 在同一公共池上再并列完整码的冻结/因果精确
code-text 缓存，并增加“冻结历史词频 + 因果精确 code-text”混合路线。
两类证据的提升先相加、再共同封顶三位，不会各自获得一份提升预算。它仍
不启用衰减，也不评估简写路线，以免把多个实验变量混在一起。报告另列
混合路线相对冻结词频逐窗改善、持平、退化和 Top-1 得失，避免只比较汇总
命中数时被相互抵消的变化误导。

按键配对模式启动后，要在精确目标输入框内按 `Ctrl+Shift+F11` 并看到
`KEY_CAPTURE_READY` 才开始输入；用 `Ctrl+Shift+F12` 停止。完整隐私
边界、事件证据与人工测试清单见[追踪探针说明](docs/tracker-probe.md)。
原子事件之上的纯内存、非意图标签归并见
[纠错候选说明](docs/correction-candidates.md)；提交同时保留拼音上屏差分
与组合前后文档净差分，因此能区分普通补入和非歧义的直接选中替换。
候选预览默认继续脱敏且没有磁盘输出；显示内容仍需另加
`--preview-text`。停止时会附加一条不含文字、拼音或具体按键值的
`SESSION_SUMMARY`，汇总原子记录、证据完整性、候选形态和先删后补间隔。
只有显式给出 `--save-summary` 时，这一条聚合才会以
`ziranma-session-summary-v1` JSON 保存到 Git 已忽略的固定私有目录；
目标必须尚不存在，程序不覆盖、不追加，也不保存候选明细。
保存后的多次会话可由只读 `summary-report` 显式合并；它只打开逐个
`--input` 点名的文件，拒绝不兼容口径，输出仍不含文字。完整边界见
[脱敏会话摘要汇总](docs/summary-report.md)。
若要比较真实按键与有限的反事实简写，必须另行显式创建包含私人明文的
事件胶囊；其知情开关、上限、删除边界和只读回放见
[私人事件胶囊与离线回放](docs/event-capsules.md)。

## Codex 专用持续记录器

短时探针之外，仓库现在提供一个独立的 `codex-recorder`，用于一次启动后
持续积累 Codex 输入框的新差分。它不依赖固定 PID；Codex 退出、重启或
重建输入元素时会解除旧绑定并等待唯一的精确目标重新出现；若重建恰好
发生在监听挂接窗口，已注册部分也会回滚并重试，不会结束整个会话。先做完全
只读的目标体检：

```powershell
cargo run --bin codex-recorder -- --check
```

确认 `candidates=1` 后，手动启动一个日常会话：

```powershell
cargo build --release --bin codex-recorder
.\target\release\codex-recorder.exe --run --session-kind daily
```

已经建立本地版本槽后，也可以用 `recorderctl` 管理后台运行而不依赖滚动的
终端：

```powershell
cargo build --release --bin recorderctl
.\target\release\recorderctl.exe status
.\target\release\recorderctl.exe run --session-kind daily --background
```

`status` 默认用中文显示“正在运行、当前版、待升级版、可回退版、下一步”；
本地脚本需要稳定字段时使用 `status --machine`；机器输出也不公开绝对
路径，但其中的会话、时长和数量仍是行为元数据，不宜原样分享。两者都只读取
`.local/recorder/` 中的版本指针并查询同名进程，不读取 `data/private/`、
不初始化 UIA、不写盘。支持 `active-v1` 的记录器还会显示会话号、运行
时长、连接/暂停状态、已安全保存的分段与事件数和最近刷新时间。这份状态
不含正文或按键，只在启动、连接变化、暂停和非空分段落盘时原子更新，
没有每秒心跳。已停止的历史会话只显示结束状态与“开始于多久前”，不会
继续显示停止前的输入框连接状态。`drain` 是显式的正常排空操作；
它只向唯一且路径受管理的记录器发送固定停止消息，等待记录器自行解绑、
加密刷新和退出，从不强杀进程：

```powershell
.\target\release\recorderctl.exe drain
```

候选版用 `stage <明确路径>` 复制到不可变本地构建并执行 `--check`。
`promote` 和 `rollback` 只在没有任何记录器进程时更新一个原子版本指针，
不会改写旧会话；完整命令和失败边界见
[持续记录器的本地换代协议](docs/recorder-lifecycle.md)。

记录器默认每 128 个事件或 60 秒轮换一次 `.zcs`。每段在内存中成形后先
由 Windows 当前用户 DPAPI 保护，再原子发布到
`data/private/continuous-capture/`；磁盘临时文件也已经加密。已有输入
只作内存基线，不保存为事件。`Ctrl+Shift+F10` 暂停/恢复并在暂停时刷新，
`Ctrl+Shift+F12` 停止并刷新。终端状态不显示具体文字、按键或本地路径，
分段回执只声明 `path_disclosed=false`；但会话号、时间点和数量仍是行为
元数据，不应当成匿名报告随意公开。
发送消息或切换任务造成的短暂编辑框重建会合并成一条 `REBOUND`，不会
把新框的既有草稿重复保存。

加密段可由原回放器逐个点名读取：

```powershell
cargo run --release --bin capsule-replay -- `
  --input data/private/continuous-capture/segment-<会话号>-00000000.zcs `
  --window-gap-ms 15000 `
  --compact
```

正常停止会打印包含会话号、段数和事件数的脱敏
`CODEX_RECORDER_FEEDBACK`，以及一条可直接运行的 `FEEDBACK_COMMAND`。
整次会话也可以用一个显式 selector 回放；它只按连续的可预测文件名展开，
不扫描私有目录：

```powershell
cargo run --release --bin capsule-replay -- `
  --session <会话号> `
  --window-gap-ms 15000 `
  --compact
```

需要人工核对记录到的原文、按键和修改时，使用独立的私人查看入口：

```powershell
cargo run --release --bin personal-lab -- review `
  --session <会话号>
```

它会明确把私人文字打印到当前终端及其滚动记录中，但不写新文件、不学习、
不联网，也不寻找“最新会话”。默认只显示 40 条事件并缩短超过 160 字的
字段；默认页头只有会话号和范围，时间线每条事件只占一行，不显示内部位置
和定位证据，无法确认完整性的按键以“待确认”短标记区分。
`--from`/`--limit` 用于翻页，`--details` 才展开分段、计时、位置、预编辑
与定位证据，`--full-text` 则完整显示长字段。加载时仍校验当前用户 DPAPI、
连续分段名、加密元数据、时间顺序和会话连续性。

个人缓存目前仍是纯内存实验。若独立后续会话证明词频或有序词对确有收益，
下一层才会创建可回退的 DPAPI 加密个人模型；拟议的保存内容、显式训练、
只读评测、原子更新与删除边界见
[加密个人模型设计](docs/personal-model.md)。
[词汇层与个人学习边界](docs/lexicon-layers.md)进一步区分公共词典、项目领域包、
显式别名、会话记忆和加密私人确认层，并记录公开数据的许可证与导入闸门。

回放器按显式顺序一次只打开、解密并聚合一个文件；该段处理完就释放其
原子事件，不再把整次长会话的私人正文同时留在内存里。脱敏计数报告的
字段和段边界语义保持不变；个人缓存模式仍会按设计保留历史词/词对状态，
但不会保留所有原始事件。段边界仍是胶囊边界，不会因“流式”而偷偷连接
跨轮换点的窗口。

需要直接比较三条公开排序基线时，可把上例的 `--compact` 换成
`--ranking-report`。它在同一次私人输入流式遍历中，针对完全相同的合格
连续窗口并列统计现行词频排序、公开 train-only 词 bigram 和公开
train-only 字 bigram。两条公开模型都只重排现行排序生成的固定 Top-50
候选池，不从私人输入学习，也不扩大召回池。输出包含各自 Top-1/5/10，
以及相对现行排序的 Top-1 得失、名次改善/持平/变差和 Top-10 掉出/救回；
不显示正文、拼音、按键串、路径或会话号，不写文件，也不输出研究建议。
该模式首行声明 `private_input_passes=1`、`ranking_lanes=3` 和固定候选池
深度；它与 `--report`、`--compact`、单模型上下文开关、个人缓存、历史
学习、形码审计及健康模式互斥。

记录过程中只想快速检查数量和采集完整性时，可跳过候选解码：

```powershell
cargo run --release --bin capsule-replay -- `
  --session <会话号> `
  --health-only
```

新版健康输出保留既有 `CAPTURE_HEALTH`，并另加一行
`CAPTURE_INTEGRITY contains_text=false contains_behavioral_metadata=true`。
两行都显式标记 `contains_behavioral_metadata=true`，因为即使没有正文，
提交、修订、逻辑按键和连续性数量也会描述输入行为。
它只聚合 `codex-uia-v2` 加密段里的低分辨率管线证据；旧 v1 段和明文
胶囊单列为 `legacy_inputs_without_integrity`，不会把缺失字段冒充成 0。

完整报告里的 `noncanonical_code_observations` 是中性观察，不会把用户
自定义词码直接当作打错。

记录器空闲轮询使用 Windows 消息等待，热键仍会立即唤醒；完整回放会在
一次提交内复用相同码串，并复用逐提交阶段已经获得的窗口分词。两者都不
持久化候选缓存，也不跨历史/评测边界学习。

每个加密段还在密文内保存记录器版本与采集口径。新记录器写
`ziranma-continuous-segment-v2` / `codex-uia-v2`，在不改变 v1 原子事件
格式的前提下加入回调计数、读取失败、基线代数和粗粒度边界原因；新版
回放器仍严格读取旧 `ziranma-continuous-segment-v1` / `codex-uia-v1`，
不迁移或重写历史。根据旧报告做出的改动不能再用同一会话证明有效；旧会话用
`--history-session` 学习，下一次独立会话才用 `--session` 评测。完整
接受、回退和停止判据见
[反馈驱动升级闭环](docs/feedback-upgrade-loop.md)。

当前代码**没有**给 Windows 安装启动项，也没有托盘、跨应用监听、自动
保留期限或私人模型导出；`daily/course/theme` 只是加密会话类别骨架。
完整目标策略、加密限制、崩溃边界、回放和后续自启动决策见
[Codex 专用持续记录器](docs/continuous-recorder.md)。版本候选、会话
排空、提升和回滚见
[持续记录器的本地换代协议](docs/recorder-lifecycle.md)。

## 开源许可

除文件或相邻目录另有说明外，本项目原创的源代码、测试、文档和配置采用
[Mozilla Public License 2.0](LICENSE)：

> This Source Code Form is subject to the terms of the Mozilla Public License,
> v. 2.0. If a copy of the MPL was not distributed with this file, You can
> obtain one at https://mozilla.org/MPL/2.0/.

`data/public/` 中的第三方快照不重新许可为 MPL；它们继续采用各自目录
记录的上游许可证、署名与使用条件。完整归属见
[第三方通知](THIRD_PARTY_NOTICES.md)和
[开源边界审计](docs/open-source-boundary-audit.md)。
参与方式与素材来源要求见[贡献指南](CONTRIBUTING.md)。

MPL 授权不包含真实聊天、按键记录、个人词典、私人模型或其他没有进入
公开发行包的用户数据。详细边界见 [隐私政策](PRIVACY.md)。

## 隐私边界

公开、人工构造或合成数据可以进入 `tests/fixtures/public/`。真实输入、
原始按键记录、探针预览、日志、个人词典和个人模型不得提交。追踪探针
默认没有磁盘输出；只有显式脱敏摘要、同时通过知情开关的私人明文事件
胶囊，或明确运行的 DPAPI 加密持续记录器能写入以下已被 Git 忽略的
本地目录：

```text
data/private/
data/raw/
logs/
models/private/
```

`.gitignore` 不能清除已经进入 Git 历史的数据，因此个人数据在生成前
就必须与仓库内容分离。即使未来创建 Private GitHub 仓库，这条规则也
不会放宽。
