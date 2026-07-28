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

需要稳定版 Rust。Rust 代码没有第三方 crate 依赖；仓库另含许可和来源
均单独保留的公开 Rime 词典与 UD Chinese GSDSimp train/test 快照。

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
退格修改；Shift+Tab 显式打开隔离的换序候选；完整单字码上按 Tab 后，使用
`h/s/p/n/z` 继续过滤同码字。`q` 和 `t` 始终是普通双拼字母，退出使用
空输入下的 Esc，不占用编码键。

普通候选直接调用现有连续句子解码器，Tab 直接调用现有冻结单字池；实验台
不复制一套算法。首屏只计算可见项，翻页时只同步补足当前页，同时在后台
展开最多 200 项；同一按键串的缓存只升级不降级，回首页不会丢掉深候选。
换序候选直接复用现有的受限相邻颠倒恢复栏，不改普通排序，也不写入
普通候选的本轮记忆。显式选过的普通同码候选在本次运行中置顶，退出即丢弃。它仍不读取私人记录、
不写文件，也没有持久个人排序、光标移动、标点或系统输入框上屏。完整按键、
重定向回退与停止边界见[端到端连续输入实验台](docs/typing-lab.md)。

终端宿主已把当前组合状态抽成可复用的 `CompositionSession`；已提交文字仍由
宿主自己拥有。这是[自有 Windows TSF Alpha](docs/tsf-alpha.md)的地基，
不会注册输入法、修改默认键盘或读取私人记录。类工厂现在会为合成 Context
创建带 50 词公开开发候选源的服务，并已验证 `nihk` 提交“你好”；这只是
构建期闭环，不代表日用词量。任何本机注册仍需要用户再次明确确认。

发布 DLL 的架构、COM 导出、证书目录和固定 zh-CN 语言配置可以先用只读工具
核对；它不提供注册或激活命令：

```powershell
cargo build --release --lib --bin tsf-devctl
.\target\release\tsf-devctl.exe inspect --dll .\target\release\ziranma_core.dll
```

检查口径与尚未满足的安装条件见[TSF 开发检查](docs/tsf-dev-inspection.md)。

## 候选实验台

`candidate-lab` 默认只显示适合直接阅读的中文：普通候选、预计操作数、
相对完整输入省下的动作，以及每个词使用完整双拼还是简拼。容易干扰正常
输入的按键颠倒候选默认隐藏，只有显式加入 `--recovery` 才会出现。

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

上游 65,125 行中，当前 codec 导入 65,116 个词条：1,714 个零权重在
内存中升到 1，8 行不支持的拼音和 1 行重复项被明确计数跳过，原文件
不被改写。`public-index-stats` 会复核这些数字及 trie 结构；快照导入
统计还有回归测试保护。65,116 个词条分布在 39,027 个非空终点节点，
单个同码节点最多挂 312 个词条。

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
- 可注册的 Windows TSF 输入法、可换代日用候选数据层或候选窗口；
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
```

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
