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

实时体验上的“刚选过马上更靠前”可以先由会话内存覆盖提供，不要求每次
选择都同步写盘。

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

第二层 `--personal-code-comparison --compact` 只加入精确
`实际窗口码 → 窗口文字` 身份，和公共、全局词频路线共享 Top-13 池。
它保留三位提升上限、冻结/因果分叉和修订撤销，但固定 `decay=none`，
也暂不参与反事实简写。报告同时增加“冻结全局词频 + 因果精确身份”的
混合完整码路线：两类提升使用 `min(3, word + code)` 合并，共享一份预算。
分层衰减仍需在同一身份定义上另做一条固定对照。

### C. 持久模型

只有 B 在独立会话上证明新增收益后，才实现
[加密个人模型](personal-model.md)中的 DPAPI 槽位。先支持显式离线训练和
只读加载；自动压缩、后台更新与跨应用学习仍继续后置。

### D. 运行架构

先测量 TSF 加载个人快照后的冷启动、热路径 P50/P95/P99 和内存。只有
测量显示 DLL 换代、崩溃隔离或模型大小已成为真实瓶颈，才评估
Weasel/PIME 式本地服务与 IPC。IPC 不是个性化算法成立的前提。

## 当前决定边界

本轮证据支持下一项公开实现聚焦于“可撤销的待确认选择记忆 + 有界分层
历史池”的研究性状态机。它不支持立即：

- 启用永久实时学习；
- 把所有 Backspace 或非首选选择当成错误；
- 扩大到神经网络或生成式主路径；
- 在 TSF DLL 中直接解析、训练和覆盖个人模型；
- 仅凭 Top-K 或省键投影宣称输入效率提升。

这些是研究备忘中的工程判断，不应作为固定结论出现在程序状态或用户界面。
