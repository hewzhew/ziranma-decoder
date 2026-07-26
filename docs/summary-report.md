# 脱敏会话摘要汇总

`summary-report` 是 `tracker-probe --save-summary` 的离线只读配套工具。它只
合并已经脱敏的 `ziranma-session-summary-v1` 数字，不读取原子事件、候选
明细、文字、拼音或具体按键。

## 显式使用

每一个输入文件都必须由操作者逐个点名：

```powershell
cargo run --bin summary-report -- `
  --input data/private/session-summaries/run-001.json `
  --input data/private/session-summaries/run-002.json
```

程序不会扫描 `data/private/session-summaries/`，不会寻找“最新文件”，也
不会写报告文件。成功时只在终端打印一行
`SUMMARY_REPORT contains_text=false ...`。

输入护栏如下：

- 至少一个重复的 `--input` 参数，没有默认输入；
- 只接受固定私有目录的直接、非隐藏 `.json` 子文件；
- 拒绝目录、符号链接、重复路径和大于 64 KiB 的文件；
- 严格核对 v1 的字段顺序、字段集合、数字格式和内部计数不变量；
- 不同 `candidate_gap_limit_ms` 或不同 `key_capture_requested` 的会话拒绝
  合并，因为这两类汇总的观察口径不同。

v1 是程序生成的机器文件，不是供手工整理的通用 JSON。重新排版、调整
字段顺序、增加字段或修改冗余均值都会被拒绝，以免格式漂移被静默接受。
错误信息可以包含操作者点名的文件路径和语法位置，但不会回显文件内容。

## 汇总口径

跨会话合并会相加会话数、经过时间、原子记录、逻辑按键动作和各类候选
数量；删后补间隔使用所有事件的总和与数量重新计算整数均值，并保留全局
最小值和最大值，不会平均各会话均值。`key_capture_ready_sessions` 单独
报告真正按过 READY 的会话数。

这些仍然只是观察量：

- `CORRECTION_CANDIDATE` 不是已经确认的输入错误；
- `commits` 不是句子数或词数；
- `logical_key_actions / commits` 不是未经定义就可使用的“效率”；
- 候选数量不能直接命名为错误率或准确率。

因此，这一层适合回答“我们收到了多少完整证据、哪些形态值得进一步
抽样”，还不能比较不同解码方案的真实节省。下一层若要做模型回放，需要
使用经过隐私审查、显式保存的[私人事件胶囊](event-capsules.md)；不能从
这份纯数字摘要还原输入内容。
