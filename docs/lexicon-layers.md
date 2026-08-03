# 词汇层与个人学习边界

## 范围

本文记录 2026-08-02 对公开输入法实现和公开词汇数据的复核，并把目前的
`conversation overlay` 放回一个可扩展的整体结构中。本轮没有读取私人会话，
没有生成私人模型，也没有把第三方词表复制进项目。

## 为什么不能把所有缺词都堆进一张会话 TSV

`ziranma-conversation-overlay-v1.tsv` 目前承担的是很窄的职责：为已经人工确认
的完整自然码补少量项目自有短语，并在公共快照产生自由简拼噪声前提供整词候选。
它适合修复“呜哇”“一大串”这一类明确缺口，但不适合作为无限增长的用户词典：

- 公开通用词、领域术语、显式手癖别名和自动学习证据有不同来源与许可证；
- 一次上屏不等于稳定偏好，提交后的编辑还可能撤销这次证据；
- 同一文字的规范码与个人别名不能混为一个频次；
- 运行时记忆需要遗忘和清空，发布词典则需要固定版本、摘要和回退。

因此“会话词层”应当是词汇分层中的一个项目自有静态包，而不是所有个性化功能的
总称。

## 公开实现给出的边界

### Rime

Rime 允许多个 translator 生成候选后再合并，并把固态词典与用户词典分开。
用户词典支持 `NewTransaction`、`CommitPendingTransaction` 和
`RevertRecentTransaction`；提交后的紧邻退格可以撤回尚未确认的学习事务。
这支持本项目继续使用“先 pending、遇明确边界再确认、重叠编辑可撤销”的语义。

- [Rime 方案与 translator](https://github.com/rime/home/wiki/RimeWithSchemata)
- [Rime 用户词典说明](https://github.com/rime/home/wiki/UserGuide)
- [Rime Memory 实现](https://github.com/rime/librime/blob/1d0df6e40cdcac17a986adc65e4668ae84ae0ada/src/rime/gear/memory.cc)
- [Rime UserDictionary 接口](https://github.com/rime/librime/blob/1d0df6e40cdcac17a986adc65e4668ae84ae0ada/src/rime/dict/user_dictionary.h)

### Mozc

Mozc 把显式 `UserDictionary` 与 `UserHistoryPredictor` 分开。历史预测器提供
`Finish`、`Revert`、按条清除和全部清除；排序信号包括新近度/LRU、文字长度与
bigram 连接，而低置信度候选可被标记为不得越过默认首选。

这说明显式加词、短期会话偏好和长期历史模型应是三个独立能力；它们可以在候选
合并处相遇，但不能共享一份无界频次。

- [Mozc UserHistoryPredictor](https://github.com/google/mozc/blob/3f235b4eb6fcff7d14ef5f0fb8ee56de7ee4c732/src/prediction/user_history_predictor.h)
- [Mozc UserDictionary](https://github.com/google/mozc/blob/3f235b4eb6fcff7d14ef5f0fb8ee56de7ee4c732/src/dictionary/user_dictionary.cc)

## 建议的词汇层

| 层 | 内容 | 生命周期 | 隐私与来源 | 候选作用 |
| --- | --- | --- | --- | --- |
| 公共核心层 | 固定拼音词典与基础频率 | 固定版本 | 第三方公开数据，锁定提交与摘要 | 通用召回与公共基线 |
| 公共语言层 | 更大词汇表、n-gram 或语言模型 | 独立可替换包 | 单独许可证与来源清单 | 公共排序和长句覆盖 |
| 项目领域层 | 会话、电子、课程等人工审阅包 | 随版本发布 | 项目原创、MPL-2.0 | 完整码精确补词 |
| 显式别名层 | `wua → 呜哇` 一类用户指定映射 | 用户主动增删 | 私人配置，不进入 Git | 精确码最高优先级召回 |
| 会话记忆层 | 当前进程中近期已确认选择 | 进程内、有容量 | 只在内存，随时清空 | 同码重排与很小的短期覆盖 |
| 私人确认层 | 多次确认的 code-aware unigram/bigram | 加密版本槽 | DPAPI、显式训练/提升/回退 | 有界重排与受限 OOV 覆盖 |
| 抑制层 | 显式“忘记/隐藏此候选” | 可撤销版本 | 只记录身份，不推断错误原因 | 屏蔽指定来源或个人证据 |

“课程式”“主题式”不需要新造一种学习算法；它们更适合作为项目领域包的激活标签。
同一包可以有 `general`、`electronics`、`course:<id>` 等作用域，运行时只合并当前
明确启用的包。

## 召回与排序协议

1. 先按实际输入码查询显式别名和完整码项目包；
2. 查询不可变公共快照；
3. 只有达到确认门槛的私人 `code → text` 才能提供公共池外候选；
4. 按 `(文字, 实际码, 来源层)` 保留身份，显示前再按文字稳定去重；
5. 公共候选和已有个人证据使用有上限的概率混合，个人分数不能无界覆盖公共模型；
6. 完整无错、已解析与全局纠错预算等保守层级仍由解码器保证；
7. 调试报告可显示来源层，日常候选窗不显示研究标签。

这把“缺词召回”和“已有候选重排”分开，也避免用逐条加词掩盖自由简拼、词界或
语言模型本身的问题。

## 可评估的公开数据

现有固定 `rime-pinyin-simp` 快照继续作为公共核心层。可进一步评估
[rime/rime-essay](https://github.com/rime/rime-essay)：它是 Rime 默认词汇表与语言
模型，但采用 LGPL-3.0。若采用，应作为带原始许可证、固定提交、SHA-256 和独立
provenance 的数据包加载，不能把内容直接粘进项目的 MPL-2.0 overlay。

任何新公共数据先经过同一闸门：

- 明确许可证、来源 URL、固定提交和校验和；
- 用 `src/codec.rs` 生成自然码，不维护第二份手工双拼映射；
- 统计导入、去重、无效拼音、字符集和词长分布；
- 分开报告 OOV 召回、Top-K、公共候选位移、冷/热延迟与包体积；
- 数据包不自动获得高于项目显式别名或已审阅完整码词条的优先级。

## 最近的工程顺序

1. 保持现有两个项目 overlay 的窄职责，并为后续领域包设计来源清单；
2. 将已经实现的纯内存 `pending → confirm/revert` 研究状态机接到公开合成的真实
   候选合并测试，但暂不接入日用 TSF；
3. 已实现本地、可列出、可删除、可回退的 DPAPI 显式别名三槽，并移除 TSF
   源码中的个人别名数组；支持该版本的宿主在新组合边界安全热刷新。日常
   `aliaspad` 面板一次固定、移除或撤销，私密正文经标准输入进入独立管理器，
   不出现在子进程命令行；CLI 继续保留先暂存、后提升的审查路径；
4. 独立评估 `rime-essay` 等公共语言包的许可证、体积、召回和延迟；
5. 最后才把评测通过的私人确认层做成 DPAPI `candidate/current/previous` 槽位。

这些步骤延续现有隐私和换代边界，不要求在 TSF 宿主里实时写数据库。
