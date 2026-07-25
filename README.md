# ziranma-decoder

一个面向自然码双拼的本地、隐私优先、可解释的带噪序列解码实验。

项目的长期目标，是研究怎样从可能误触、按键颠倒、漏键、混合简拼且
没有显式词界的自然码按键串中，生成可信的中文候选。当前仓库不是完整
输入法，也不收集真实输入。

## 当前里程碑

第一阶段只处理小型公开词典中的完整词或短语，并允许：

- 原样匹配；
- 一次 QWERTY 物理邻键替换；
- 一次相邻按键颠倒，包括跨双拼音节边界的颠倒；
- 输出 Top-K 候选；
- 展示每个候选的纠错操作、词频分、纠错代价和总分。

演示词典的拼音和自然码编码被显式保存在
`tests/fixtures/public/demo_lexicon.tsv`，方便人工审查。编码参照
[Rime 官方自然码双拼方案](https://github.com/rime/rime-double-pinyin/blob/master/double_pinyin.schema.yaml)
核对；当前版本尚未实现通用的“全拼转自然码”组件。

## 明确的非目标

当前阶段不实现：

- 简拼或全码、简码混输；
- 漏键、多按键或任意编辑距离；
- 无词界的整句分段；
- 神经网络或在线服务；
- Rime 插件、Windows TSF 或候选窗口；
- 自动上屏或静默修改；
- 真实聊天、个人词典或日常按键采集。

## 运行

需要稳定版 Rust。仓库没有第三方依赖。

```powershell
cargo run -- nihk
cargo run -- nigk
cargo run -- nikh 5
```

三个例子依次展示“你好”的原样输入、把 `h` 邻键误按成 `g`，以及
把末尾的 `hk` 按成 `kh`。CLI 只读取编译进程序的公开演示词典，不会
把输入写入磁盘。

开发检查：

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## 隐私边界

公开、人工构造或合成数据可以进入 `tests/fixtures/public/`。真实输入、
原始按键记录、日志、个人词典和个人模型不得提交。以下本地目录已被
Git 忽略：

```text
data/private/
data/raw/
logs/
models/private/
```

`.gitignore` 不能清除已经进入 Git 历史的数据，因此个人数据在生成前
就必须与仓库内容分离。

## 下一项最小任务

在不采集个人数据的前提下，扩大公开合成评测集并报告不同错误类型的
`Recall@K`、干净输入候选变化和确定性延迟。完成评测基线以后，再决定
是否加入漏键和多按键。
