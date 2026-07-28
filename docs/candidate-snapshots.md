# 候选快照：把数据换代与 TSF 生命周期分开

## 当前状态

`CandidateSnapshot` 是纯内存、只读的候选数据边界。调用者显式提供词典文字和
描述符，快照完成校验后才建立现有 `Decoder` 索引。它不寻找文件、不解密、
不学习、不写磁盘，也不联网。

当前 TSF 开发类工厂先解析固定的候选包清单，再通过这个入口加载仓库内的
50 词公开演示词典。每个进程只构造一次，随后由类工厂创建的文本服务共享
同一个不可变快照。它仍不是日用词典，也没有从磁盘选择
current/candidate/previous 的能力。

## 不可变候选包

`ziranma-candidate-package-v1` 清单固定为八行、LF 结尾，不接受未知字段、调换
顺序或非规范数字：

```text
schema=ziranma-candidate-package-v1
snapshot_schema=ziranma-candidate-snapshot-v1
revision=<版本>
contains_private_text=<true|false>
payload_format=ziranma-lexicon-tsv-v1
payload_bytes=<字节数>
payload_fingerprint_fnv1a64=<16 位小写十六进制>
entry_count=<词条数>
```

清单不含候选正文，载荷单独保存为 UTF-8 TSV。核心库只接收调用者已经明确
提供的两段内存，不解析路径。`candidatectl inspect` 才负责只读打开用户点名的
两个普通文件；它拒绝符号链接、空文件、非 UTF-8 和超过固定上限的文件，不
扫描相邻目录，也不尝试猜测配对文件。

```powershell
cargo run --release --bin candidatectl -- inspect `
  --manifest tests/fixtures/public/demo_candidate_manifest.zcm `
  --payload tests/fixtures/public/demo_lexicon.tsv
```

报告只显示版本、公开/私人标记、词条数、载荷字节数和“校验通过”，不显示
文件路径、指纹值或任何候选文字。检查器不写文件、不学习、不修改 TSF 配置、
不联网，也不会替操作者安排下一步。

## 固定边界

一个描述符包含：

- schema：当前只能是 `ziranma-candidate-snapshot-v1`；
- revision：1～64 字节的 ASCII 字母、数字、点、下划线、加号或连字符；
- `contains_private_text`：标记载荷是否含私人文字；
- UTF-8 词典 TSV；
- 精确的载荷字节数、FNV-1a 64 位指纹和解析后词条数。

载荷与描述符中较大的字节数不得超过 16 MiB，标注词条数必须位于
1～131,072。字节数、指纹或实际词条数任一不符都会拒绝；候选接口只接受
第 1～10 名。

FNV-1a 只用于发现损坏、拿错版本或构建材料漂移。它不是密码学摘要、数字签名
或来源认证；`contains_private_text` 也是调用者声明，不可能从文字本身推断隐私
归属。未来若加载外部公开包，仍需固定来源和 SHA-256；私人包则必须先经过
既定的 DPAPI 外层校验与解密，再把内存明文交给快照解析器。

## 输入失败时怎样处理

- 小词典不能完整解释首选时，确认操作提交原始组合串，不把研究报告里的
  “未解析”标记写进宿主；
- 数字指定的候选不存在时不伪造结果；
- 快照自身无法加载时，类工厂拒绝创建文本服务，让宿主保留原输入路径；
- 文本服务内部若没有候选源，也会拒绝处理按键，作为第二道防线；
- 解析错误只报告结构类别，不回显词典行、拼音或候选文字。

这些规则首先保证不吞字。它们不声称未知双拼已经被正确转换，也不把按键直通
包装成候选质量。

## 尚未实现的换代层

包清单、载荷校验和只读检查器已经完成。下一阶段才实现包生成与槽位，而不是
让 `CandidateSnapshot` 自己扫描目录：

1. 构建工具从固定公开来源生成清单与不可变载荷，并另行记录上游 SHA-256 和
   解码兼容指纹；
2. 开发控制器显式把包放入 current/candidate/previous 槽位；
3. 新文本服务只在创建时取得一个 `Arc<CandidateSnapshot>`，活动组合期间绝不
   偷换模型；
4. candidate 先做独立合成宿主检查，再显式 promote；加载失败继续保留 current；
5. 私人模型仍沿用独立的 DPAPI、因果评测和显式授权流程，不因为公开候选包
   可换代就自动启用学习。

是否把完整公开词典编译进 DLL、映射只读数据文件，或使用独立引擎进程，需要
在真实宿主权限、启动延迟、内存重复量和升级可靠性都有测量后再决定。当前快照
核心刻意不提前锁死这一选择。
