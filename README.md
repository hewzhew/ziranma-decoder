# ziranma-decoder

一个面向自然码双拼的本地、隐私优先、可解释的带噪序列解码实验。

长期目标，是研究怎样从可能误触、按键颠倒、漏键、混合简拼且没有
显式词界的自然码按键串中，生成可信的中文候选。当前仓库是离线研究
基线，不是完整输入法，也不收集真实输入。

## 现在能够做什么

当前版本包含一条可以完整运行和测量的链路：

```text
无声调全拼
  ↓ 自然码 codec
逐音节的标准两键编码
  ↓ 每个音节选择两键全码或一键简拼
可变长候选编码
  ↓ 允许至多一次局部按键错误
单词 Top-K，或按键位置 × 全局错误预算的多词动态规划
  ↓
词界、简拼位置、纠错操作和分数拆解
```

支持的局部错误包括：

- 一次 QWERTY 物理邻键替换；
- 一次相邻按键颠倒，包括跨音节边界；
- 一次漏键；
- 一次多按。

简拼和纠错可以联合出现。例如输入 `ni` 可以解释为“你好”的候选编码
`nih`（第二个音节使用一键简拼），随后末尾的 `h` 又漏按了。程序只
生成并解释候选，不自动上屏。

## 运行

需要稳定版 Rust。仓库没有第三方依赖。

```powershell
cd D:\IME\ziranma-decoder

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

# 兼容最初的简写形式
cargo run -- nigk

# 生成公开合成评测
cargo run -- evaluate
```

CLI 只读取编译进程序的公开演示词典，不会把输入写入磁盘。

## 自然码 codec

公开词典只保存中文、空格分隔的无声调拼音和合成相对权重。标准自然码
由 `src/codec.rs` 自动生成，不再手工重复维护。映射参照
[Rime 官方自然码双拼方案](https://github.com/rime/rime-double-pinyin/blob/master/double_pinyin.schema.yaml)
核对。

codec 支持 `v`、`ü` 和 `u:` 三种 ü 写法，并保留逐音节边界供混合简拼
使用。它验证映射结构，但还没有内置完整的普通话合法音节表，因此不能
把“能够映射”误认为“语言学上一定存在这个音节”。

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

当前会产生 1,754 个样例，并报告各类 `Recall@1/5/10` 及干净输入首选
是否仍为原样解释。样例由被测词典自身生成，因此这些结果只用于回归
检查和比较算法版本，**不代表真实中文覆盖率或实际输入准确率**。

评测方法和偏差详见 `docs/evaluation.md`。

## 设计边界

当前已经可以在全局一次错误预算下联合推断多个词的边界和简拼方式，
但仍未实现：

- 两次及以上按键错误；
- 上下文语言模型或个人词频；
- 从真实打字中估计错误概率；
- 神经网络、在线服务或遥测；
- Rime 插件、Windows TSF 或候选窗口；
- 自动上屏或静默修改。

当前词典很小，所以每个输入位置仍会枚举词条及其全部简拼组合，再由
动态规划按“输入位置 × 是否已经用过错误”保留一个有界 beam。这样容易
审查，适合验证联合解码，但不是生产规模方案。下一次算法扩展应使用
词典 trie 共享前缀，并加入公开语料估计的词序概率。

## 开发检查

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
就必须与仓库内容分离。即使未来创建 Private GitHub 仓库，这条规则也
不会放宽。
