# 候选排序与个人学习：公开实现和论文研究备忘

## 范围

本文记录 2026-07-29 的公开资料研究，回答四个问题：

1. 公共候选、个人词语与上下文证据怎样分层；
2. 什么行为足以触发学习，什么行为只能保留为未知；
3. 个人状态怎样遗忘、撤销、换代和回退；
4. 哪些方法适合当前本地 TSF Alpha，哪些方法仍只适合作为研究对照。

本轮没有读取私人记录，没有从真实输入估计参数，也没有改变候选排序。
文中的数值是实验设计候选，不是产品默认值。

## 从公开实现得到的工程证据

### Rime：提交学习是可撤销事务

Rime 的 `Memory` 在提交时开始用户词典事务。提交后紧接的 Backspace 可以
中止最近事务；其他未处理按键才确认事务。用户词条还支持负计数删除、复活
和随时间更新的权重。

这说明学习不必和上屏成为同一个不可逆动作。对本项目更合适的语义是：

- 候选上屏先产生 `pending` 正证据；
- 后续与该文档跨度重叠的删除或替换撤销它；
- 只有出现明确边界后才把它确认为个人证据；
- 撤销正证据不自动生成“这是错字”的负标签。

参考实现：

- [memory.cc](https://github.com/rime/librime/blob/1d0df6e40cdcac17a986adc65e4668ae84ae0ada/src/rime/gear/memory.cc)
- [user_dictionary.cc](https://github.com/rime/librime/blob/1d0df6e40cdcac17a986adc65e4668ae84ae0ada/src/rime/dict/user_dictionary.cc)

### libime：公共模型与有界个人历史混合

libime 没有让用户历史无限累计。`HistoryBigram` 使用容量为
128、8192、65536 的三层历史池；最近证据权重更高，旧句子逐级迁移，
超出容量后自然遗忘。它同时提供按文字或“文字 + 校验码”忘记的接口。

`UserLanguageModel` 在概率空间混合公共语言模型与个人历史，再返回对数分数，
而不是用一个无上限的频次直接压过公共模型。

这给出三个适合本项目的边界：

- 个人证据有容量和最大影响力；
- 同一文字可按实际码区分，避免自定义别名污染标准码；
- 公共基线始终保留，个人证据只做受限覆盖或重排。

参考实现：

- [historybigram.cpp](https://github.com/fcitx/libime/blob/7b638a433815ed7a29d9bcb8d59aed7366bd3b28/src/libime/core/historybigram.cpp)
- [userlanguagemodel.cpp](https://github.com/fcitx/libime/blob/7b638a433815ed7a29d9bcb8d59aed7366bd3b28/src/libime/core/userlanguagemodel.cpp)

### 2026-08-09 复核：冷启动需要公共静态模型，个人历史不能替代它

本轮继续复核同一固定 libime 提交。`LanguageModel` 用静态 N-gram 模型给
lattice 上的完整路径累计分数；`UserLanguageModel` 再在概率空间混入有界
个人历史。`PinyinContext` 在用户确认后更新历史，但第一次新造自定义词不会
立即写入历史。这套职责分离解释了当前 Alpha 的一个真实缺口：两个完整公共词
都能组合出来时，纯 unigram 仍可能把机械同音组合排在自然搭配前面；等待用户
亲自选过一次只能改善热态，不能解决第一次输入的冷启动排序。

libime 当前固定的简体公共模型材料是：

- `https://download.fcitx-im.org/data/lm_sc.arpa-20260629.tar.zst`；
- 上游压缩包大小：77,670,558 字节；
- 上游声明 SHA-256：
  `06808333b9173e5374cf2cb5afc12d08f5625bf9abb536489cac376fc05f2e7f`；
- 上游先把 ARPA 构建为压缩 trie 二进制，再由运行时加载，并不在每次启动或
  每次输入时解析完整 ARPA。

这份 2026-06 ARPA 声明为三元模型，包含 `<unk>`，但没有 `<s>` / `</s>`。
libime 加载静态模型时明确把 KenLM 的 `sentence_marker_missing` 设为 `SILENT`，
因此缺少句界是该公开材料的受支持格式，不是下载损坏。离线审计据此采用两种
严格模式：模型同时提供两个句界时使用它们；模型同时不提供时从空上下文开始且
不追加句末分；只提供其中一个时仍失败关闭。报告会明确打印实际采用的模式。

当前项目已经做过两个不能泛化的较小对照。用核心词典自身构造字符邻接统计时，
合理搭配仍会输给高频同音字的机械组合；用现有小型 UD 训练 word/character
bigram 时，拟合集上的改善没有稳定保持到独立测试集，部分保守档还损失了一个
原本正确的首选。因此，这两类小模型都不能直接进入 TSF 热路径，也不能仅凭几个
手工例子宣称有效。

为验证更强公共模型而不先污染运行时，`candidatectl static-context-audit` 采用下面
的只读闸门：

- 从公开 fit / held-out CoNLL-U 分别选择自然相邻的两个完整词；选样不读取解码
  结果，并排除整段文字或整串码已经是词典完整词的场景；
- 冻结当前完整双拼候选及其分词，只允许模型在固定搜索深度内把一个已有挑战者
  提到首位，不创建新路径；
- 对解压后的公开 ARPA 做两次有总字节、单行和文件身份边界的流式扫描：第一次
  确认词表、阶数和 SHA-256，第二次只保留冻结候选真正需要的 N-gram；两次材料
  不一致就失败关闭；
- 在 fit 上从预声明的搜索深度和最小平均 log10 增益中选档，held-out 只做一次
  独立评测；报告只含 Top-K、升降位、正确首选得失和非目标首选变化，不回显词句；
- 只有 held-out 净改善、没有正确首选损失、也没有非目标首选变化时，才允许继续
  研究小型、版本化 sidecar。审计本身不会生成 sidecar，也不会接入 TSF。

合成 ARPA 测试已经证明稀疏 parser、标准 backoff、单挑战者门和独立负对照按上述
口径工作。完整官方压缩包随后以 75 个互不覆盖的范围分片下载；分片连续性、总长
77,670,558 字节和上游压缩包 SHA-256 均通过。解压后的 `lm_sc.arpa` 为
196,248,782 字节，SHA-256 为
`9cbfd115c139162fbafc54a1b7a2dbd03d16ac7cd1bb127b2bfbbbfc78436dfa`。

使用当前 62,757 条核心候选、`frontier-limit=50`、`sample-limit=512` 和
`max-order=5` 运行真实审计时，模型声明阶数把实际使用阶数限制为 3。拟合集冻结
512 例，预声明档位选择 `ARPA-d8-g0.75`：正确首选 +10 / -0，但有 2 次非目标
首选变化。独立保留集冻结 372 例；同一档位正确首选 +2 / -0，但仍有 1 次非目标
首选变化。它因此没有通过预先固定的安全门；本轮没有生成 sidecar，也不得把该
配置或由它推导的排序资料接入运行时。完整 ARPA 仍不进入换代脚本、DLL 加载或
按键热路径。

后续短输入审计发现，两键单字的 500 个公开目标中有 494 个处于至少七个精确
候选的宽同码池，而上面的 `static-context-audit` 评估的是两个相邻词合并后的整串
解码，不能回答“左词已经提交、当前两键单字怎样排序”。因此新增一条同样只读的
冻结候选审计：

```powershell
cargo run --release --bin candidatectl -- single-character-context-audit `
  --model .local/research/fcitx-lm/extracted-20260629/lm_sc.arpa `
  --core-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --fit-corpus data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu `
  --held-out-corpus data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu `
  --frontier-limit 50 --sample-limit 512 --max-order 3
```

选择器只保留“核心可确认的公开左词 + 无多音规范码歧义的单个汉字”，当前输入
只使用后一个字的两键完整码；同码池少于两个候选时排除。左词和目标必须是原始语料
中不跨标点的相邻 token，这与 TSF 在标点后清空左侧锚点的实际边界一致。候选冻结
后，ARPA 只可依据 `[已提交左词, 当前单字候选]` 提升搜索深度内的一个既有挑战者，
不能创建候选。拟合侧预声明的搜索深度为 8/16/32/50，最小平均 log10 增益为
0.10～4.00；加入 1.25～4.00 的严格档发生在保留集仍只运行冻结基线之后，最终
配置选择不读取保留答案。

固定 65,116 条核心包上，拟合语料产生 71,946 个有效邻接窗、29,025 个单字目标、
26,093 个双端核心覆盖和 3,813 个句级代表；排除 185 个多音目标后冻结 512 例，
其中 2 个目标在 Top-50 外。保留语料相应为 8,709、3,484、3,116 和 485；排除
135 个多音目标后冻结 350 例，目标均在 Top-50 内。模型稀疏查询需要 30,472 条
N-gram，命中 7,015 条，2,456 个所需词型映射为 `<unk>`。拟合选择
`ARPA-d16-g1.00`：Top-1 从 301/512 增至 340/512，正确首选 `+39/-0`，但有
6 次非目标首选变化。保留集 Top-1 从 198/350 增至 226/350，正确首选
`+28/-0`，仍有 3 次非目标首选变化，因此严格安全门失败。这证明公开左上下文对
宽同码单字有可观信号，也证明当前门控还不足以无副作用上线。该 test 结果现已冻结；
不得继续用它调阈值或过滤条件，下一步必须先准备新的 fit/dev 材料或可事先声明的
置信证据，再保留另一份未观察材料作安全门。本轮没有生成 sidecar，也没有改变
TSF、个人模型或换代流程。

为避免继续复用已经观察过的 GSDSimp test，同一固定上游修订中此前未纳入仓库的
官方 dev 随后被逐字节固定，并先作为一次性独立验证集运行冻结的
`ARPA-d16-g1.00`。357 例的 Top-1 从 211 增至 232，正确首选 `+22/-1`，另有
4 次非目标首选变化；副作用因此在另一官方切分上复现，原档仍不能上线。这个结果
之后，dev 明确降为可以选择档位的开发集，不再冒充未观察测试集。

最终验证协议在读取新保留语料正文前实现并由合成测试固定：开发集只允许“正确首选
损失为 0 且非目标首选变化为 0”的预声明档位竞争，再最大化新增正确首选；最终
保留集只运行冻结基线和开发选中的唯一档位。新的域外保留集是固定修订
`2849afd946a8c01b3e9acdf3e7afa8670cf2777d` 的 UD Chinese PUD，来源、CC BY-SA
3.0 许可、README、SHA-256 和行数账目均保留在快照目录。运行命令为：

```powershell
cargo run --release --bin candidatectl -- single-character-context-validation-audit `
  --model .local/research/fcitx-lm/extracted-20260629/lm_sc.arpa `
  --core-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --development-corpus data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-dev.conllu `
  --held-out-corpus data/public/ud-chinese-pud/zh_pud-ud-test.conllu `
  --frontier-limit 50 --sample-limit 512 --max-order 3
```

开发集的 9,198 个有效邻接窗经筛选冻结 357 例；严格规则选择
`ARPA-d8-g2.00`，Top-1 从 211/357 增至 218/357，正确首选 `+7/-0`，非目标
首选变化为 0。PUD 的 15,750 个有效邻接窗、6,409 个单字目标和 4,113 个双端
核心覆盖形成 905 个句级代表；排除 220 个多音目标后冻结 512 例，其中 2 个目标
在 Top-50 外。稀疏查询需要 29,416 条 N-gram，命中 7,225 条，2,496 个所需
词型映射为 `<unk>`。最终 Top-1 从 258/512 增至 262/512，正确首选 `+4/-0`，
但仍有 1 次非目标首选变化，因此最终安全门失败。

三条公开轨道共同证明：已提交左词对宽同码单字具有稳定的纠正信号，单一 ARPA
分差阈值却无法把收益和偶发误提升完全分开。PUD 结果现已冻结，不得用它继续调
深度、阈值或过滤条件；本轮仍不生成 sidecar、不接入 TSF。下一次若继续，应改进
证据形态或学习可撤销的个人上下文，而不是围绕最后一个公开反例收紧常数。

参考实现：

- [libime language model](https://github.com/fcitx/libime/blob/7b638a433815ed7a29d9bcb8d59aed7366bd3b28/src/libime/core/languagemodel.cpp)
- [libime user language model](https://github.com/fcitx/libime/blob/7b638a433815ed7a29d9bcb8d59aed7366bd3b28/src/libime/core/userlanguagemodel.cpp)
- [libime pinyin learning boundary](https://github.com/fcitx/libime/blob/7b638a433815ed7a29d9bcb8d59aed7366bd3b28/src/libime/pinyin/pinyincontext.cpp#L1098-L1118)
- [libime pinned model material](https://github.com/fcitx/libime/blob/7b638a433815ed7a29d9bcb8d59aed7366bd3b28/data/CMakeLists.txt)

### 2026-08-04 持久化复核：有界快照承担冷启动，事务日志承担并发写入

本轮继续只读复核相同固定提交。Rime 的 LevelDB 后端用 `WriteBatch` 提交事务，
并把统一 TSV 快照作为备份、恢复边界；Mozc 的 `UserHistoryStorage` 把有界 LRU
整体序列化到加密存储，加载和保存由独立任务执行；libime 的三层历史池也按容量
保存完整快照，而不是在启动时重放无限事件。

当前 Alpha 不能直接覆盖一个共享快照：多个宿主进程可能同时保存新选择，DLL
也没有负责串行化写入的常驻服务。因此采用混合协议：选择仍进入不可变 DPAPI
小批次；达到门槛后，任一新宿主可以发布一个同样不可变的 DPAPI 检查点，内部
精确列出已覆盖的批次名。加载器只接受覆盖集合仍属于当前目录、尾部顺序严格更
晚的检查点；否则退回规范全量重放。检查点不删除日志，也不覆盖另一个宿主的
文件。这个协议借鉴成熟实现的“有界快照 + 事务尾部”结构，不复制其数据库代码，
也不在 TSF DLL 中引入常驻线程或新依赖。

参考实现：

- [Rime LevelDB backend](https://github.com/rime/librime/blob/1d0df6e40cdcac17a986adc65e4668ae84ae0ada/src/rime/dict/level_db.cc)
- [Rime uniform user DB snapshot](https://github.com/rime/librime/blob/1d0df6e40cdcac17a986adc65e4668ae84ae0ada/src/rime/dict/user_db.cc)
- [Mozc user history storage](https://github.com/google/mozc/blob/3f235b4eb6fcff7d14ef5f0fb8ee56de7ee4c732/src/prediction/user_history_storage.cc)
- [libime bounded history snapshot](https://github.com/fcitx/libime/blob/7b638a433815ed7a29d9bcb8d59aed7366bd3b28/src/libime/core/historybigram.cpp)

### Weasel 与 PIME：TSF 前端可以保持薄层

Weasel 把按键、焦点、候选选择、翻页和输入位置经命名管道交给独立服务；
PIME 的 TSF 客户端同样通过 RPC 接收组合串、候选列表、提交文字和状态。
两者都证明 Windows 文本服务、候选 UI 和解码/学习引擎可以分开演进。

这不表示本项目现在就应该增加常驻服务。当前更稳妥的阶段是：

- TSF DLL 继续只读加载不可变公共候选和已发布个人快照；
- 私人反馈和模型训练留在独立进程；
- 只有性能、换代或崩溃隔离的实测结果证明有需要时，再设计本地 IPC。

参考实现：

- [WeaselIPC.h](https://github.com/rime/weasel/blob/f9203cae5e2b0796d94575b975f62a6be9614b00/include/WeaselIPC.h)
- [WeaselClientImpl.cpp](https://github.com/rime/weasel/blob/f9203cae5e2b0796d94575b975f62a6be9614b00/WeaselIPC/WeaselClientImpl.cpp)
- [PIMEClient.h](https://github.com/EasyIME/PIME/blob/9f6a1e9161b7f609eb1fadf282048c2907da04c9/PIMETextService/PIMEClient.h)

### IME WL Converter：导入导出应是离线边界

IME WL Converter 把格式解析、过滤、简繁转换、词频、编码生成与导出组织为
显式流水线。微软自学习词库等专有格式被限制在独立 importer/exporter，
没有进入实时解码核心。

本项目以后若支持导入微软、Rime 或其他个人词库，也应先转换为一个内部、
可审计的中间格式，再构建新的不可变个人候选版本。TSF 运行时不直接解析
多种外部词库格式。

参考实现：

- [ConversionPipeline.cs](https://github.com/studyzy/imewlconverter/blob/d7f3799e4095277ef198debab94af3fec7d2e996/src/ImeWlConverter.Core/Pipeline/ConversionPipeline.cs)
- [Win10MsPinyinSelfStudyExporter.cs](https://github.com/studyzy/imewlconverter/blob/d7f3799e4095277ef198debab94af3fec7d2e996/src/ImeWlConverter.Formats/Win10MsSelfStudy/Win10MsPinyinSelfStudyExporter.cs)

这里借鉴的是边界与验证方法，不复制第三方实现。

### 成熟测试与基准工具：默认摘要和详细报告分层

Criterion.rs 默认围绕一个基准显示核心区间，把额外统计留在 verbose 输出和
HTML 报告；hyperfine 把多项结果整理成相对参考项的摘要，并把 CSV、JSON、
Markdown 等导出作为独立用途；Google Benchmark 使用稳定的名称与数值列；
cargo-nextest 则分别控制运行中状态、最终状态和成功/失败输出，默认隐藏无助
于当前判断的成功细节。

这几种实现共同说明：日常终端输出应先回答一个窄问题，完整审计指标由显式
选项展开，机器格式再单独设计。`adaptive-lab` 因此默认只显示六行对照表，
每个场景选一个直接相关的观察项；`--details` 才显示参数、事件计数和全部
辅助指标。Criterion.rs 的自动“改善/退化”判定依赖统计显著性与噪声门槛，
当前确定性合成题没有同等证据，因此没有照搬这种结论句。

参考实现：

- [Criterion.rs command-line output](https://github.com/bheisler/criterion.rs/blob/3dbc6c618acb48885066422d81d50729aa17b2b7/book/src/user_guide/command_line_output.md)
- [hyperfine result comparison](https://github.com/sharkdp/hyperfine/blob/f12f3d9f86f3643b3b7deace5e160b1f0f44d2b7/src/benchmark/scheduler.rs)
- [hyperfine export formats](https://github.com/sharkdp/hyperfine/blob/f12f3d9f86f3643b3b7deace5e160b1f0f44d2b7/README.md)
- [Google Benchmark console reporter](https://github.com/google/benchmark/blob/194098fdfc109ece3fd2a4bc3d1181244cbc1b89/src/console_reporter.cc)
- [cargo-nextest reporting levels](https://github.com/nextest-rs/nextest/blob/9e2053ea6c077f8f0479d97aaf744cd133aef6a8/site/src/docs/reporting.md)

## 从论文得到的算法和评测证据

### 退格是有噪声的观察，不是错误真值

Zheng 等人在 2011 年从 2,277,786 名用户的输入中，通过 Backspace
启发式提取了 54,309,334 对纠正样本。论文对人工样本估计的抽取精度约为
75.8%，对 15 种典型误拼的召回约为 55.6%。

这项工作证明真实错误具有拼音发音和键盘位置特征，也证明“按过退格”
不能直接当成错误标签。项目应继续保留 `未知 / 改写 / 辅助取字 / 纠正候选`
等中性类别。

来源：[Why Press Backspace? Understanding User Input Behaviors in Chinese Pinyin Input Method](https://aclanthology.org/P11-2085/)

### 双拼换序应是带惩罚的错误路径，不是静默改码

传统键入研究指出，换序错误常伴随较短的相邻键间隔，但间隔还会受二键组合、
左右手与个人技能影响；大规模键入观测也显示用户之间与二键之间的节奏差异很大。
因此固定毫秒阈值只能作为冷启动安全门，不能冒充个人错误概率。

成熟输入法同样保守。libime 为 QWERTY 相邻键替换构造带 `Correction` 标记的
拼音路径并施加额外惩罚，但在双拼表构建中明确排除会交换字母顺序的
`AdvancedTypo`，避免产生错误音节。Rime 把容错边标成 `is_correction` 并赋予
较低可信度，正常拼写保持优先；其相邻换序用例仍是禁用测试。这些实现支持
本项目继续分离“候选召回证据、显示档位、来源标注”，而不是先重写用户码串。

当前个人 Alpha 因而先采用三档可回退门：极短间隔允许唯一完整词恢复升首位，
次短间隔只放第二位，再慢一档只影子检查。个人化若继续，应在明确反馈中区分
恢复提交、后续重叠编辑和普通路径选择，用分层收缩的二键模型逐步替换全局冷启动
参数；不能用一次退格训练一个永久规则。

第一版分层收缩现已作为固定内存表接入 Alpha，不写文件也不扫描历史。每个间隔桶
先共享当前用户会话的全局接受/拒绝计数，再把具体有序二键组合向全局后验收缩；
全局先验强度为 16，二键收缩强度为 8。全局同桶不足 24 个明确标签、且该二键同桶
不足 8 个明确标签时，结果严格等于冷启动档位。跨过任一门槛后，接受概率达到
0.72 才指向高置信、达到 0.40 才指向中置信，否则指向影子；一次版本内最多只移动
一个相邻档位，不能从影子直接跳到首位或反向跳过中档。这些值是有回归测试的
Alpha 配置，不是从私人样本拟合出的普适参数。

标签也保持因果边界：恢复候选实际可见、随后同一码提交该文字才是接受；同一码
明确提交另一可见候选才是拒绝。影子命中、继续输入、取消、原码提交、不同码提交
和未完成尾部都只计未知，未知不进入概率分母。反馈帧同时保留冷启动档位与实际
采用档位，因而许愿回放可以审计校准是否改变候选；进程或反馈会话结束后内存模型
归零。跨宿主持久模型必须另走明确构建、独立评测和可回退安装流程，当前没有隐式
写入。

来源：

- [Perceptual, Cognitive, and Motoric Aspects of Transcription Typing](https://psycnet.apa.org/record/1986-21057-001)
- [Observations on Typing from 136 Million Keystrokes](https://doi.org/10.1145/3173574.3174220)
- [KNPTC: Knowledge and Neural Machine Translation Powered Chinese Pinyin Typo Correction](https://arxiv.org/abs/1805.00741)
- [Survey of Automatic Spelling Correction](https://doi.org/10.3390/electronics9101670)
- [libime `pinyincorrectionprofile.cpp`](https://github.com/fcitx/libime/blob/c8fa4906a74f3b280f9be9a1533ddc749dceaeb0/src/libime/pinyin/pinyincorrectionprofile.cpp)
- [libime `shuangpinprofile.cpp`](https://github.com/fcitx/libime/blob/c8fa4906a74f3b280f9be9a1533ddc749dceaeb0/src/libime/pinyin/shuangpinprofile.cpp)
- [Rime `syllabifier.cc`](https://github.com/rime/librime/blob/1d0df6e40cdcac17a986adc65e4668ae84ae0ada/src/rime/algo/syllabifier.cc)

### 在线词表有效，但即时真值假设不适合直接照搬

Zhang、Huang 与 Zhao 在 2019 年提出在线更新词表的神经 P2C：
比较模型 Top-1 与用户选择，寻找最长不匹配 n-gram 并加入词表；实际部署
建议每 64 个实例训练一次以控制成本。论文同时使用 Top-K MIU accuracy
与按键分数 KySS，而不只看准确率。

该方法在论文数据上有明显收益，但它假定用户本轮选择立即代表正确答案。
真实编辑可能是语句重组、自定义别名或找字流程。因此本项目只能借鉴
“个人覆盖层、先预测后学习、控制更新成本”，不能照搬即时加词规则。

来源：[Open Vocabulary Learning for Neural Chinese Pinyin IME](https://aclanthology.org/P19-1154/)

### 上下文有价值，但模型大小与延迟必须单独核算

Tan 等人在 2022 年表明，更长中文前文对简拼预测有明显帮助；同时，
12 层 PinyinGPT 在 V100 上的一组测试平均推理约为 142–145 ms，
6 层模型约为 94 ms，简拼 P@5 随缩小模型下降。论文明确没有执行
按键成本和人工评测。

因此“大模型上下文重排”可以保留为远期离线研究方向，但不适合作为当前
本地 TSF 主路径。现阶段应先验证有界 bigram、个人别名和短上下文是否已经
覆盖主要收益。

来源：[Exploring and Adapting Chinese GPT to Pinyin Input Method](https://aclanthology.org/2022.acl-long.133/)

### 个性化收益必须与交互成本一起测量

Kristensson 等人的触屏键盘研究在闭环模拟中把词错误率从 38.4% 降到
5.7%，个性化进一步降到 4.6%，说明公共模型和个人模型的组合有价值。
但 Quinn 与 Zhai 的实验又表明，更积极的候选建议虽然减少按键并受到
主观偏好，却可能因为注意、判断和选择成本而降低实际时间表现。
PhraseFlow 也发现短语级纠错先增加了认知负担，经过交互迭代后才在不降速
的情况下减少错误。

因此本项目不能把“少按几个字母”直接称为效率提升。至少要同时报告：

- 目标名次与 Top-1 / Top-5 / Top-10；
- OOV 或个人覆盖层带来的新增召回；
- 实际按键、候选选择、翻页、回删和重打；
- 提交后短时撤销率；
- 冷启动和热路径延迟；
- 最终输入时间或小规模真实体验记录。

来源：

- [Effects of Language Modeling and its Personalization on Touchscreen Typing Performance](https://doi.org/10.1145/2702123.2702503)
- [A Cost-Benefit Study of Text Entry Suggestion Interaction](https://doi.org/10.1145/2858036.2858305)
- [PhraseFlow: Designs and Empirical Studies of Phrase-Level Input](https://doi.org/10.1145/3411764.3445166)

### 生成式输入仍缺少足够的个人化证据

2024 年 GeneInput 把完整、简拼和带噪按键统一建模，并探索 RLHF-IME。
论文自己说明没有进行大规模个人化实验；其个人化部分主要是案例分析，
端侧实时推理也仍列为后续工作。

这适合作为远期研究参照，不足以推翻当前确定性、端侧、可解释和可回退的
约束。

来源：[Generative Input: Towards Next-Generation Input Methods Paradigm](https://aclanthology.org/2024.findings-acl.218/)

## 建议的分层架构

```text
按键与组合
    │
    ▼
确定性码表、词界 lattice、受限错误通道
    │
    ├── 公共不可变词典候选
    └── 已确认的个人 code → text 覆盖候选
              │
              ▼
公共语言分 + 有界个人 unigram/bigram 概率混合
              │
              ▼
候选页、选择、提交
              │
              ▼
待确认事务 ──重叠编辑──► 撤销
              │
           明确边界
              ▼
个人证据历史池
```

### 召回与排序分开

固定 Top-50 重排无法救回公共词典完全没有的个人词。个人化需要两条路径：

1. **重排证据**：个人 unigram / bigram 只调整已经存在的候选；
2. **覆盖召回**：反复确认的 `实际码 → 文字` 可以进入单独、受限的个人
   候选层。

个人覆盖候选必须保留来源、实际码、确认次数、最近确认代次和撤销状态。
它不能改写公共词典，也不能绕开全局错误预算伪装成标准码。

### 相邻精确提交形成个人短语

固定词典缺少某个合理短语时，无界扩大“双字自由组合”不是合适的替代：若两个
音节各取 N 个单字，组合数量会按 N² 增长，并把大量从未被用户表达过的机械拼接
带进候选页。更窄的个人短语路径可以把**连续、相邻、完整码、已明确选择**的若干
提交先组成一个待确认身份：组合码为各次实际完整码连接，文字为各次提交文字连接。

在线首版现在按下面的窄协议实现：

- 只连接同一合格输入范围中的连续键盘提交；标点、普通首选提交、取消、焦点切换、
  输入模式切换和宿主快捷键都会断链。当前还没有文档范围锚点，因此同一 TSF
  Context 内的鼠标光标移动不在首版连续性保证内；
- 每个组成项都由完整双拼精确解释，首版只接受一个两键完整码对应的单字，不消费简拼、
  纠错、Tab 找字、换序恢复、原码提交和未解析边；
- 第一次观察立即在当前宿主提供可撤销的会话候选；第二个组成项存活到下一按键、
  焦点离开或停用边界后，一次即可进入受限个人覆盖层，不要求重复输入；它仍只在
  精确连接码下召回；
- 第二个组成项提交后紧邻的空组合 Backspace 会同时撤销新短语和第二个组成项，
  第一组成项已经跨过自己的确认边界，继续保留。组成项的个人排序证据与组合
  短语证据分别保存，不能因忘记短语而删除单字偏好；
- 候选数量、文字长度、码长、历史容量和影响位置都有硬上限，来源标成个人覆盖，
  不伪装为公共词典频率；
- 持久化复用当前用户 DPAPI 的不可变个人排序批次；离线 `adaptive_coverage`
  仍不直接接入日用 TSF，也不从历史文本批量猜短语。

这样，“词典缺词”和“公共组合排序不足”仍可分开诊断；系统学习的是用户确实
连续提交过的短语，而不是预先枚举所有可能的双字排列。

### 进程内左侧上下文只重排既有候选

个人短语解决覆盖召回，但同码候选在不同语境下的偏好不能继续压成一个全局首选。
在线首版因此增加一张固定内存表，身份为“上一段输入法候选文字 + 当前实际码 +
当前明确选择文字”。只有选择跨过紧邻 Backspace 撤销边界后才计入；撤销会恢复提交
前的左侧锚点。普通首选可以成为下一次预测的左侧身份，但不会单独产生训练标签。

候选端只在确有相同左侧身份和码串证据时，把本次冻结深度由 6 扩到 12；随后至多
移动一个原本就在池中的候选到首个未保护位置，再裁回调用者请求的深度。固定别名、
项目保护前缀和精确 suppression 始终优先。标点、原码、宿主拥有的按键、焦点变化
与中英文模式切换都会切断锚点；同一 Context 内鼠标移动仍因缺少文档范围锚点而不作
连续性保证。明确选择更后候选时，系统只把被越过的首个未保护候选记为一次上下文
反证；正反证共用待确认与 Backspace 撤销事务，每次反证只抵消一格旧支持。表最多
2048 项，同身份正证与反证各封顶四次，文本服务停用即清空，不写文件。

这一步只证明确定性状态机与排序代数可安全接入，不宣称真实准确率已经改善。跨宿主
持久化、公共 bigram 融合和持久上下文负证据仍需独立历史/评测与回退格式。

### 概率混合优于无界加分

候选排序可以研究下面的概率混合，而不是立即固定参数：

```text
P_mix = (1 - λ) · P_public + λ · P_personal
score = log10(P_mix) - error_penalty - unresolved_penalty
```

其中：

- `λ` 必须有上限，并在没有足够重复证据时接近 0；
- `P_personal` 来自有界、分层衰减的 code-aware unigram / bigram；
- 完整无错、完整词典覆盖和未解析输入的保守层级继续由现有规则保证；
- 个人证据不能跨候选身份借分，也不能改变解释中的实际码与纠错来源。

### 正向学习与错误学习分开

第一次实现只学习“这个码下曾确认选择这段文字”的正证据。下列信息继续
作为事件证据，不进入错误概率：

- 退格；
- 没选首选；
- 提交后的文字被改写；
- 标准码与实际码不同；
- 删除后又恢复同一文字。

只有多轮独立证据能区分误触、发音混淆、手快颠倒与组织语言修改时，才为
错误通道建立单独模型。

## 个人状态的建议生命周期

### 运行时

1. 候选提交后创建带文档跨度的 `pending` 记录；
2. 同跨度的删除、替换或组合取消撤销该记录；
3. 后续不重叠提交、焦点结束或明确时间边界把它确认为正证据；
4. 每次预测在学习当前答案之前完成；
5. 显式“忘记该词/该码”写入删除标记，而不是依赖负频次碰巧抵消。

### 内存与磁盘

- 内存态使用小型 recent / medium / long 历史池；
- 私人原文事件仍由现有加密记录器保存；
- 离线训练只产生新的 DPAPI 不可变模型候选；
- `current / candidate / previous` 显式提升或回退；
- TSF 只加载已发布快照，不直接更新模型文件；
- 导入外部词库先转换、验证、去重，再构建新候选版本。

实时体验上的“刚选过马上更靠前”先由会话内存覆盖提供，不要求每次选择都同步
写盘。TSF 当前只把数字选择，以及翻页后用空格或标点确认当前页首项，视为明确
的非首选证据；第一页空格首选、Tab 找字和换序恢复不进入这层记忆。会话内提升
不得跨过显式固定候选的保护前缀。允许落盘的证据现已先进入单条待确认事务；
数字选择与空格确认后的紧邻空组合 Backspace 会撤回事务并恢复此前的会话覆盖，
其他按键、焦点离开和停用会确认。标点确认因候选后存在标点后缀，不把第一个
Backspace 猜成候选撤回。这个窄语义已经接入 TSF；任意宿主编辑的跨度追踪仍只
属于下面的研究状态机。

## 近期实验路线

### A. 公开合成实验：待确认学习语义

`src/adaptive_memory.rs` 已实现第一版公开、纯内存研究状态机，并用人工构造
测试验证：

- `pending → confirm`；
- 重叠编辑撤销；
- 删除后恢复同文不产生错误标签；
- 不重叠编辑只平移跨度；
- 相同文字的标准码与个人别名分别计数；
- 容量到达上限后的确定性淘汰；
- 清空和显式忘记。

已确认证据使用可配置的 `recent → medium → long` 三层有界历史。同一
`code → text` 只保存一份；重新确认会累加正证据并将它移回 recent，
旧证据先逐层迁移，只有越过 long 容量后才会遗忘。三层容量仍是研究配置，
不是从私人数据估计出的产品参数。

`src/adaptive_ranking.rs` 还实现了固定候选池上的只读概率混合实验。调用方
保留候选文字，返回结果只含原始下标、概率和分层证据；当前 pending 不参与
本轮预测。个人混合比例随证据增长但有硬上限，无证据时严格保持公共概率和
原始顺序。当前参数与测试数据都是人工构造值，尚未用真实输入校准。

`src/adaptive_coverage.rs` 实现了精确实际码下的受限个人覆盖查询。一个文字
至少需要两次确认才有资格；公共候选池已有文字会被排除；结果数量有硬上限
并保留“已确认选择历史”来源。结果直接借用内存文字且不实现 `Debug`，
只能对指定码查询，不能遍历或序列化整个个人词库。

`src/adaptive_merge.rs` 在合成候选上连接了公共重排与个人覆盖。个人覆盖只
能分享一小块有硬上限的概率质量，公共和个人来源始终保留，完整分布先去重
再稳定排序。即使结果上限截断了候选，也不重新归一化剩余概率，因此不会因
页面深度而暗中放大个人影响。

`src/adaptive_evaluation.rs` 实现了公开合成事件流的因果闭环评测。每次查询
严格先预测、后记录选择；pending 只有遇到确认边界才学习，重叠编辑可先撤销，
显式忘记同时移除待确认与已确认记录。报告只含 OOV 救回、Top-K、公共名次
扰动、个人候选占位、概率上限和状态迁移计数，不包含事件文字。个人候选
占位同时区分“本次选中的个人候选”与“本次未选择的个人候选”，避免把所有
个人候选出现都笼统称为错误。

`src/adaptive_comparison.rs` 在同一条公开人工合成事件流上独立回放四个固定
研究配置：

1. 当前参考参数；
2. 只把个人覆盖的确认门槛从两次提高到三次；
3. 只降低个人重排和个人覆盖的最大影响；
4. 只提高个人重排和个人覆盖的最大影响，但仍服从全局 20% 覆盖硬上限。

每一行都报告自己的完整聚合结果，以及相对参考参数的 OOV 救回、Top-K、
公共 Top-1 变化、公共名次位移、未选择个人候选占位和最大覆盖概率差值。
比较器不选择“最佳配置”，不输出产品建议，也不接受或读取磁盘会话；配置
名称描述实验变量，不代表默认值已经由真实数据证明。

`src/adaptive_scenarios.rs` 进一步提供六道互相隔离的公开合成实验题：

1. 稳定重复选择同一个公共池外文字；
2. 选择后在确认边界前删除；
3. 同一文字使用另一个实际码；
4. 两次确认后显式遗忘；
5. 已在公共池中的候选参与个人重排；
6. 个人覆盖候选出现，但本次选择了公共候选。

每道题都从空记忆开始，分别交给四个固定配置完整重放，总计 24 次配置回放。
这使确认门槛、撤回、实际码隔离、遗忘、公共重排与未选择占位可以分别观察，
不会让上一道题的证据污染下一道题。实验题只验证状态机和指标口径；固定
文字、频率和事件顺序不代表真实语言分布，也不能用于选择产品参数。

可以用一条命令查看这套固定实验：

```powershell
cargo run --release --bin adaptive-lab
```

默认输出是一张六行对照表，每个场景只显示一个直接相关的观察项。需要查看
参数、事件计数和全部辅助指标时显式添加 `--details`：

```powershell
cargo run --release --bin adaptive-lab -- --details
```

该命令不接受输入文件或会话号，不输出合成事件文字，也不在报告中选择配置
或生成研究建议。

这项实现不把候选重排接入现有 `SessionSelectionMemory` 或 TSF，不读取
私人记录，也不执行序列化、持久化或网络操作。这里的“重排”仅是研究函数
的数值结果；合并器只接受调用方提供的合成候选，尚未进入真实解码或交互
候选；加密模型槽位仍未实现。

### B. 私人离线因果回放：比较四条固定路线

只有再次明确启动私人实验时，使用旧会话训练、后续会话评测：

1. 当前公共排序；
2. 当前三位封顶词频缓存；
3. 分层衰减、code-aware unigram；
4. 同底座加分层 bigram 与个人覆盖召回。

每条路线使用同一候选深度、同一历史/评测分组和同一事件资格条件。覆盖
召回应单列“公共池外救回”，不能混入纯重排名次改善。

第一步已经补上 `capsule-replay --personal-word-comparison --compact`：
它在一次流式回放中比较公共排序、冻结历史词频和因果在线词频。冻结与
因果路线从相同旧历史起步，并共享每条策略码的一次 Top-13 候选搜索；
只有因果路线在窗口预测后更新。该工具只输出脱敏聚合，不选择配置，也不
保存个人状态。分层衰减、code-aware 证据和覆盖召回仍是后续独立路线。

第二步 `--personal-pair-comparison --compact` 把公共排序、冻结历史词频、
冻结历史词频加有序词对和因果在线词对放进同一窗口的一份 Top-13 池。
报告把冻结词对相对冻结词频的净名次变化单列出来，并保留在线学习及修订
撤销计数；因此不会再用两次独立回放间接猜测词对收益。
为隔离“三位共享上限已被词频占满”的情况，同一报告还比较两个有界预留
槽位配置：只有候选池存在合格词对时才将纯词频限制为两位，把第三位留给
词对；无词对时完全退回词频。门槛分别为一次观察和同一词对重复两次，且
两者都只读冻结历史。

第三层 `--personal-code-comparison --compact` 只加入精确
`实际窗口码 → 窗口文字` 身份，和公共、全局词频路线共享 Top-13 池。
它保留三位提升上限、冻结/因果分叉和修订撤销，但固定 `decay=none`，
也暂不参与反事实简写。报告同时增加“冻结全局词频 + 因果精确身份”的
混合完整码路线：两类提升使用 `min(3, word + code)` 合并，共享一份预算。
分层衰减仍需在同一身份定义上另做一条固定对照。
精确身份报告现同时给出冻结/因果路线相对公共排序的逐窗变化、因果相对冻结
的变化，以及目标和竞争候选覆盖。混合路线还单列词频已占满三位、因而精确
身份无法增加提升的窗口，避免误把共享上限解释成证据无效。

### C. 持久模型

只有 B 在独立会话上证明新增收益后，才实现
[加密个人模型](personal-model.md)中的 DPAPI 槽位。先支持显式离线训练和
只读加载；自动压缩、后台更新与跨应用学习仍继续后置。

### D. 运行架构

先测量 TSF 加载个人快照后的冷启动、热路径 P50/P95/P99 和内存。只有
测量显示 DLL 换代、崩溃隔离或模型大小已成为真实瓶颈，才评估
Weasel/PIME 式本地服务与 IPC。IPC 不是个性化算法成立的前提。

## 当前决定边界

窄范围的“可撤销待确认选择”现已落地；公开合成对照也接入了四次封顶的确认
支持度：一次新选择不能压过已有重复支持，达到同等封顶支持后由新近度切换，
因此旧累计量不会造成无限切换门槛。分层历史池仍是下一项独立研究。它不支持立即：

- 启用永久实时学习；
- 把所有 Backspace 或非首选选择当成错误；
- 扩大到神经网络或生成式主路径；
- 在 TSF DLL 中直接解析、训练和覆盖个人模型；
- 仅凭 Top-K 或省键投影宣称输入效率提升。

这些是研究备忘中的工程判断，不应作为固定结论出现在程序状态或用户界面。

显式忘记的生产核心现使用独立 `PersonalRankingSuppressionSnapshot`：它按精确
`code + text` 遮住个人正证据，恢复操作可逆，不修改公共候选，也不把忘记解释成
“输入错误”。结构不提供遍历或正文 `Debug`，同文字在其他码下不受影响。独立
DPAPI 不可变动作日志、TSF 刷新和候选窗 `Ctrl+Delete` 两阶段入口现已启用；
进入模式不写动作，只有数字键精确选择具有个人证据的候选才会保存，同一组合中
紧接 Backspace 可恢复。

精确码隔离之上现增加一条更窄的结构继承：只有经过当前公开候选源验证的完整整词
证据，才可影响同词、池内、保留完整首音节的连续尾简。目标短码自己的精确证据
优先；目标码或来源码的显式抑制都会关闭继承。它不把任意前缀、自由分句、私人
别名或池外文字解释成同一身份，也不创建新的持久正向事件。
