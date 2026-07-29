# 候选快照：把数据换代与 TSF 生命周期分开

## 当前状态

`CandidateSnapshot` 是纯内存、只读的候选数据边界。调用者显式提供词典文字和
描述符，快照完成校验后才建立现有 `Decoder` 索引。它不寻找文件、不解密、
不学习、不写磁盘，也不联网。

TSF 类工厂现在会检查 DLL 同目录下固定的 `candidate-data`。目录不存在时，
它使用编译进 DLL 的 50 词公开演示包；目录一旦存在，就只沿 `slots.zcs` 的
`current` 引用加载一个通过预检的公开包，不扫描目录。每个新类工厂取得一个
不可变快照，随后创建的文本服务共享该 `Arc`；已有类工厂和活动组合不会在
数据槽切换时偷换快照。

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

## 确定性生成

`candidatectl build` 只接受显式 `--public`、一个普通 UTF-8 TSV 和一个尚不存在
的输出目录。它先严格解析词典，再计算字节数、词条数和 FNV-1a 损坏检测值，
写入原样载荷与规范清单，最后从新目录完整回读一次：

```powershell
cargo run --release --bin candidatectl -- build `
  --source tests/fixtures/public/demo_lexicon.tsv `
  --output .local/candidate-demo-v1 `
  --revision tsf-public-demo-v1 `
  --public
```

输出目录已存在时一律拒绝覆盖。载荷先写、清单后写，因此中途失败最多留下
一个无法通过完整加载的不完整目录，不会冒充有效包。当前命令刻意不提供私人
明文生成；私人候选必须另行设计 DPAPI 封装和授权边界。

## 开发槽位

`candidatectl` 的 `adopt`、`stage`、`promote`、`rollback` 和 `status` 接收显式
`--root`。`adopt` / `stage` 还要求显式 `--package`，只从固定的
`manifest.zcm` 与 `lexicon.tsv` 加载，不扫描相邻目录。

槽库把验证后的公开包复制到内容寻址、只增不改的内部目录。四行
`ziranma-candidate-slots-v1` 状态只保存 current/candidate/previous 三个内部
标识；提升把旧 current 留作 previous，回退交换 current 与 previous。失败的
状态转换不改变内存状态，磁盘指针通过同步临时文件原子替换。`status` 不创建
目录，也不显示内部标识、文件路径、指纹或候选正文。

`adopt` 和 `stage` 会在安装副本上运行真实 Windows TSF 合成 Context 预检。
预检从词典首个合法词条的完整码开始，以快照实际首选作为目标，经过同一个类
工厂、文本服务、逐键预编辑、空格确认和文档读回。它验证 TSF 传输闭环，不把
该词条冒充整体候选质量。独立只读检查可显式运行：

```powershell
cargo run --release --bin candidatectl -- preflight `
  --package .local/candidate-demo-v1
```

成功后，槽库在包目录之外写一个不含按键和正文的
`ziranma-candidate-preflight-v1` 凭据。凭据绑定由清单和载荷共同计算的内部
内容标识。`promote` / `rollback` 会重新加载包、重算标识并复核凭据；包被
手工改写、凭据缺失或不匹配时，状态文件保持不变。

这个凭据只是本地生命周期证据，不是数字签名，也不抵抗主动伪造。它用于防止
把未预检包、旧凭据或普通文件漂移误当成已验证版本；公开来源认证仍需要未来的
固定 SHA-256、许可证和发布流程。

槽库不删除废弃包，也不接受 `contains_private_text=true` 的明文包。提升和
回退只原子改变数据指针：已有类工厂继续使用取得时的快照，提升之后创建的
新类工厂才读取新的 `current`。

## DLL 相邻运行时目录

运行时目录名固定为 `candidate-data`，位置是实际承载 `DllGetClassObject` 的模块
旁边，而不是当前工作目录、用户目录或环境变量。以本仓库 release 构建为例，
可以把同一个槽工具明确指向：

```powershell
cargo run --release --bin candidatectl -- adopt `
  --root .\target\release\candidate-data `
  --package .local\candidate-demo-v1
```

运行时只打开以下确定路径：`slots.zcs`、`packages/<current>/manifest.zcm`、
`packages/<current>/lexicon.tsv` 与 `preflights/<current>.zpf`。根目录、固定子目录、
包目录和文件都必须是普通对象；文件有固定大小上限并需为 UTF-8。包内容标识、
清单、载荷、公开标记和预检凭据全部复核后才建立类工厂。

“目录不存在”是唯一使用内嵌开发包的状态。目录存在但未配置、损坏、缺少凭据、
内容标识漂移或出现明文私人包时，`DllGetClassObject` 返回失败，不静默回退。
这样部署失误不会伪装成一次成功启动，也不会让操作者误以为新候选已经生效。

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

## 尚未接通的发布认证层

包清单、确定性公开生成、完整加载、TSF 合成宿主预检、三槽状态以及新类工厂
读取 `current` 已经完成。接下来仍有两道门：

1. 固定外部公开来源时，另行记录上游 SHA-256、许可证与解码兼容指纹；当前
   FNV-1a 只能发现普通损坏，不能认证来源；
2. 当前仍没有注册、安装、签名或跨进程升级协调。实际宿主中的版本观察、启动
   延迟、内存重复量和回退操作必须在用户另行授权注册之后测量。

私人模型仍沿用独立的 DPAPI、因果评测和显式授权流程，不因为公开候选包可
换代就自动启用学习。

是否把完整公开词典编译进 DLL、映射只读数据文件，或使用独立引擎进程，需要
在真实宿主权限、启动延迟、内存重复量和升级可靠性都有测量后再决定。当前快照
核心刻意不提前锁死这一选择。
