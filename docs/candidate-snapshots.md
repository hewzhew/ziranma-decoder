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

清单不含候选正文，载荷单独保存为 UTF-8 TSV。外部公开包还必须有严格九行的
`ziranma-candidate-provenance-v1`：

```text
schema=ziranma-candidate-provenance-v1
package_schema=ziranma-candidate-package-v1
decoder_compatibility=ziranma-candidate-decoder-v1
source_id=<公开来源标识>
source_license=<单一 SPDX 风格许可证标识>
source_url=<HTTPS 来源>
source_sha256=<源文件 SHA-256>
manifest_sha256=<清单 SHA-256>
payload_sha256=<载荷 SHA-256>
```

核心快照仍只接收调用者明确提供的清单和载荷内存，不解析路径；来源层独立校验
侧车及其材料绑定。`candidatectl inspect` 只读打开用户分别点名的三个普通文件；
它拒绝符号链接、空文件、非 UTF-8 和超过固定上限的文件，不扫描相邻目录。

```powershell
cargo run --release --bin candidatectl -- inspect `
  --manifest tests/fixtures/public/demo_candidate_manifest.zcm `
  --payload tests/fixtures/public/demo_lexicon.tsv `
  --provenance tests/fixtures/public/demo_candidate_provenance.zcp
```

报告只显示版本、公开/私人标记、来源 ID、许可证、词条数、载荷字节数和校验
结果，不显示文件路径、摘要值、来源 URL 或候选文字。检查器不写文件、不学习、
不修改 TSF 配置、不联网，也不会替操作者安排下一步。

## 确定性生成

`candidatectl build` 只接受显式 `--public`、一个普通 UTF-8 TSV、完整公开来源
声明和一个尚不存在的输出目录。`build-rime` 接受同样的来源声明，但把一个
固定版本的 Rime 词典 YAML 先经仓库内的严格解析器转换成规范 TSV。两条命令都
先检查源文件精确字节是否等于操作者显式提供的 SHA-256，再生成清单与来源
侧车，最后从新目录完整回读三份材料：

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

Rime 导入不会扫描配方、用户目录或相邻词典，也不会执行 YAML 指令；输入必须
是操作者明确点名的普通文件。输出顺序与规范频率由同一个解析器确定，因此同一
源字节、版本和来源声明会得到同一候选载荷：

当来源身份与 SHA-256 同时匹配仓库固定的 `rime-pinyin-simp` 快照时，
`build-rime` 还会应用同目录下的可审计简体清单。它只省略已有同拼音、
等高或更高权重简体对应项的繁体单字读音，并在构建摘要中报告数量；多字词、
其他读音和任何非固定来源继续走通用 Rime 解析器。

```powershell
cargo run --release --bin candidatectl -- build-rime `
  --source .local/sources/pinyin_simp.dict.yaml `
  --output .local/candidate-rime-v1 `
  --revision rime-pinyin-simp-<PIN>-tsf-v1 `
  --source-id rime-pinyin-simp `
  --source-license Apache-2.0 `
  --source-url https://github.com/rime/rime-pinyin-simp `
  --source-sha256 <固定源文件的 SHA-256> `
  --public
```

大于普通快照输入上限、且使用 Unicode 声调的公开 Rime 词表只能走显式的
`build-rime-slice`。它仍先核对完整 64 MB 以内源文件的 SHA-256，再逐行去调、
交给中央 codec 编码，并按调用者明确给出的词条数和文字长度上限构造有界前沿：

```powershell
cargo run --release --bin candidatectl -- build-rime-slice `
  --source .local/public-audit/wanxiang-fdda7afb/jichu.dict.yaml `
  --output .local/public-audit/wanxiang-fdda7afb/package-top100k-v1 `
  --revision wanxiang-jichu-fdda7afb-top100k-v1 `
  --source-id wanxiang-jichu `
  --source-license CC-BY-4.0 `
  --source-url https://github.com/amzxyz/rime_wanxiang `
  --source-sha256 9d14c0c49588d63b16c554df4711bed5da822c63de9d50f4759c53542138ac00 `
  --max-entries 100000 `
  --max-text-characters 8 `
  --public
```

该命令只生成一个单来源实验包，不把它 stage、promote 或安装。异常字段、
非正权重、文字范围、拼音、字音数量、音节长度、上限外词条和所选重复均分项
计数。两个已经生成的公开 TSV 可进一步做不显示候选文字的只读对照：

```powershell
cargo run --release --bin candidatectl -- compare `
  --base-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --challenger-payload .local/public-audit/wanxiang-fdda7afb/package-top100k-v1/lexicon.tsv
```

对照只报告词形、文字/规范码身份、共同规范码、同码首选变化，以及“对照首选
本身也被基线同码确认”的可校准数量；不显示候选文字，也不声称两个来源的
原始权重可以直接合并。

两个独立公开载荷还可以运行纯完整词层审计。它不调用 TSF，不写槽位，也不
比较跨来源权重；核心已有完整码时固定保留核心首选，补充层只允许明确数量的
新完整词进入，其自由简拼和句子候选完全不参与。实际补进新词时，交互页保留
核心/补充完整词及受限的四键双字组合，不再以核心自由简拼句子补齐空位：

```powershell
cargo run --release --bin candidatectl -- layer-audit `
  --core-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --supplemental-payload .local/public-audit/wanxiang-fdda7afb/package-top100k-v1/lexicon.tsv `
  --frontier-limit 6 `
  --exact-promotions 1
```

报告只给出规范码、可用/进入前沿的新完整词、核心首选是否保持和单码实际
影响上限等聚合计数，不显示候选文字。`frontier-limit` 固定在 1～50；
`exact-promotions` 固定在 0～50。该审计只证明合并规则和静态完整词覆盖，
不代表交互延迟、句子排序或真实选择成本已经通过。

同一组固定公开完整码和音节边界前缀可以做 release 热路径对照。命令会先
预热，再分别计时核心候选和启用补充后的前六候选；不显示文字、不写文件：

```powershell
cargo run --release --bin candidatectl -- layer-benchmark `
  --core-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --supplemental-payload .local/public-audit/wanxiang-fdda7afb/package-top100k-v1/lexicon.tsv `
  --repetitions 5 `
  --exact-promotions 1
```

本机一次 120 样本结果为核心 median 4.924 ms、启用补充 5.083 ms，median
增量 0.159 ms；核心/补充索引构建分别约 174/352 ms。绝对耗时只是同机诊断。
TSF 在一个宿主进程中只加载一次不可变候选蓝图，避免重复建立 100k 补充索引；
新包或启停状态由新打开的宿主观察。

## 独立公开补充根

日用 TSF 的补充根固定在跨 DLL 版本共享的
`.local/tsf-alpha/user-data/public-supplement`。它使用现有公开候选包、来源、
SHA-256、预检和三槽机制，但另有严格四行的 `supplemental.zcl` 显式开关。
根或开关不存在时默认关闭；状态损坏、所绑定包不再是 current、包或预检失效
时，补充层失败关闭，核心候选快照继续工作。

首次准备仍需可信发布摘要：

```powershell
$SupplementRoot = '.local\tsf-alpha\user-data\public-supplement'

.\target\release\candidatectl.exe adopt `
  --root $SupplementRoot `
  --package .local\public-audit\wanxiang-fdda7afb\package-top100k-v1 `
  --expected-sha256 ae268a35f8e0125598a98205a3ce1c057f7567d08bec84e148804b70e7330eb7

.\target\release\candidatectl.exe supplement-enable `
  --root $SupplementRoot `
  --exact-promotions 1

.\target\release\candidatectl.exe supplement-status --root $SupplementRoot
```

关闭只原子改写小状态文件，保留已验证候选包，之后可再次启用：

```powershell
.\target\release\candidatectl.exe supplement-disable --root $SupplementRoot
```

启用、关闭、提升或回退后，需要重新打开待验证的应用，让它建立新的 TSF
宿主；已经运行的宿主持有原来的不可变内存快照。补充根中的 current 提升后，
旧 `supplemental.zcl` 与新 current 不一致，运行时会先回退到仅核心候选，必须
再次显式运行 `supplement-enable` 才接受新包。

输出目录已存在时一律拒绝覆盖。载荷、清单先写，来源侧车最后写；中途失败最多
留下一个无法通过完整加载的不完整目录，不会冒充有效包。当前命令刻意不提供
私人明文生成；私人候选必须另行设计 DPAPI 封装和授权边界。

生成报告会显示绑定三份精确材料的“发布 SHA-256”。它适合由发布者复制到
GitHub Release、签名公告或另一个受信渠道，不应与待验证包放在同一目录后由包
自行证明。这个值是对来源侧车、清单和载荷按固定域与长度编码计算的材料摘要，
不是目录或 ZIP 的文件摘要。使用者拿到独立摘要后可先运行：

```powershell
cargo run --release --bin candidatectl -- verify `
  --package .local/candidate-demo-v1 `
  --expected-sha256 1f2f3c81280641d9963b0ea0fac1fcdaf749d76bae778034037f015f8b8434c2
```

`verify` 只读三份固定文件，摘要不符时不写状态；报告不回显摘要或候选正文。

## 脱离签名验证

`candidatectl verify-signature` 为公开候选包增加一个只读的 Ed25519 验证边界。
调用者必须分别点名包目录、脱离签名声明文件，以及从独立可信渠道取得的 32
字节公钥。工具不会在包目录中寻找公钥或签名，也不会把声明内的密钥指纹当成
信任根。

签名声明采用严格的五行 LF 结尾格式：

```text
schema=ziranma-candidate-release-signature-v1
algorithm=ed25519
key_sha256=<公钥的 64 位小写 SHA-256>
package_sha256=<发布 SHA-256>
signature=<128 位小写 Ed25519 签名>
```

签名消息有固定域分隔，并同时绑定原始公钥 SHA-256 与候选包发布 SHA-256。
验证会重新加载包内三份规范材料、计算实际发布摘要、核对公钥指纹，再使用
Ed25519 严格验证。声明损坏、公钥不符、包被替换或签名无效都会失败；失败和
成功都不写文件、不预检、不改变槽位，也不回显公钥或签名正文。

成功报告给出的发布 SHA-256 可由操作者继续传给 `adopt` / `stage` 的
`--expected-sha256`，保留“先只读审查、再单独写入”的流程。显式
`adopt-signed` / `stage-signed` 则同时要求槽根、包目录、签名文件和可信公钥：
它们在任何槽位写入前完成相同验签，并把同一份已加载材料直接交给安装和预检，
不在验签后重新读取外部包。错误公钥、错误包或无效签名不会创建槽根、复制包、
运行预检或改变状态。

两条流程都不会自行发现签名、选择或保存信任密钥。项目尚未发布真实发布公钥，
也没有签名命令、私钥存储、密钥轮换或吊销策略；测试只使用显然为合成数据的
内存密钥字节，仓库和运行时都不生成或保存私钥。

候选包签名只认证候选包材料，不是 Windows PE/Authenticode 签名，不能证明
TSF DLL 或工具 EXE 的发布者身份，也不会把本机范围、默认关闭的开发注册变成
可分发安装包。

### Alpha 格式迁移

早期实验生成的双文件包和 `ziranma-candidate-preflight-v1` 凭据不再接受，也
不会被原地补写或静默升级。新包必须由 `candidatectl build` 写入全新的输出
目录，再由 `adopt` 写入全新的 `candidate-data` 槽根；旧凭据不能复用。开发
注册指向按 DLL 摘要固定的不可变目录，不能在其中手工修改旧包或内部槽位；
迁移应先在新的测试根完成，再通过后续独立升级流程采用新的不可变构建。这样
旧格式、半迁移状态与新三文件包不会共用一个内部标识。

## 开发槽位

`candidatectl` 的 `adopt`、`stage`、`adopt-signed`、`stage-signed`、`promote`、
`rollback` 和 `status` 接收显式 `--root`。`adopt` / `stage` 还要求显式
`--package` 和从独立可信渠道取得的 `--expected-sha256`；签名变体改为要求
显式 `--package`、`--signature` 与 `--trusted-public-key`。它们只从固定的
`manifest.zcm`、`lexicon.tsv` 与 `provenance.zcp` 加载，不扫描相邻目录。摘要
或签名验证失败时，它们在创建槽根、复制包或运行预检之前失败。

槽库把验证后的公开包复制到内容寻址、只增不改的内部目录。四行
`ziranma-candidate-slots-v1` 状态只保存 current/candidate/previous 三个内部
标识；提升把旧 current 留作 previous，回退交换 current 与 previous。失败的
状态转换不改变内存状态，磁盘指针通过同步临时文件原子替换。`status` 不创建
目录，也不显示内部标识、文件路径、指纹或候选正文。

四个 adopt / stage 命令都会在安装副本上运行真实 Windows TSF 合成 Context
预检。预检从词典首个合法词条的完整码开始，以快照实际首选作为目标，经过同
一个类工厂、文本服务、逐键预编辑、空格确认和文档读回。它验证 TSF 传输闭环，
不把该词条冒充整体候选质量。独立只读检查可显式运行：

```powershell
cargo run --release --bin candidatectl -- preflight `
  --package .local/candidate-demo-v1
```

成功后，槽库在包目录之外写一个不含按键和正文的
`ziranma-candidate-preflight-v2` 凭据。凭据同时绑定预检宿主、解码兼容标识、
内部包标识，以及由来源侧车、清单和载荷精确字节计算的完整 SHA-256。
`promote` / `rollback` 会重新加载三份材料并复核凭据；任一材料被手工改写、
凭据缺失或不匹配时，状态文件保持不变。

这个凭据仍只是本地生命周期证据，不是数字签名。来源 URL、许可证和源摘要是
操作者声明；SHA-256 可以绑定精确材料，不能独自证明声明真实，也不能阻止有权
改写槽库的人重新预检另一份材料。发布者认证可先经过上述独立验签或可信摘要
清单，但槽库本身不保存信任根，也不把预检冒充发布者认证。

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
  --package .local\candidate-demo-v1 `
  --expected-sha256 1f2f3c81280641d9963b0ea0fac1fcdaf749d76bae778034037f015f8b8434c2
```

只读核对当前公开运行管线可使用：

```powershell
candidatectl runtime-query `
  --root .\target\release\candidate-data `
  --supplemental-root .\.local\tsf-alpha\user-data\public-supplement `
  --code dago `
  --limit 10
```

它包含公开核心/补充分层和 TSF 冷启动共有词校准，但不伪装成完整实机现场：
显式别名、项目覆盖、会话记忆、个人学习和左侧上下文均明确排除。

运行时只打开以下确定路径：`slots.zcs`、`packages/<current>/manifest.zcm`、
`packages/<current>/provenance.zcp`、`packages/<current>/lexicon.tsv` 与
`preflights/<current>.zpf`。根目录、固定子目录、包目录和文件都必须是普通
对象；文件有固定大小上限并需为 UTF-8。包内容标识、来源与许可、解码兼容性、
三份材料 SHA-256、公开标记和预检凭据全部复核后才建立类工厂。

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
1～131,072。字节数、指纹或实际词条数任一不符都会拒绝；候选接口最多接受
第 1～50 名。交互宿主应先请求两页所需的浅层结果，再随用户翻页逐步扩大，
不应在每次组合更新时固定计算 Top-50。交互快照先列出任意长度的完整、零纠错、
不简写词典项。恰为四键、两个完整音节时，还会从每个音节的前 24 个单字中按
词频分组成最多 50 个临时双字候选，并随宿主翻页深度按需暴露，再追加连续句子
解码结果并按文字去重；它让
“只动”一类无需入词的组合保持可见，但不会扩散到长句。准确完整码不会被大量
高频自由简写路径挤出可见边界。例如 `cj` 同时可以解释为完整的 `can` 与
`c + j` 简拼，前者先显示，后者仍保留。无歧义的 `ju` / `qu` / `xu` / `yu`
拼音写法还允许第二键 `u` 沿用规范自然码的 `v` 词条，不占用纠错预算。
这个交互合并不改变研究解码器的原始句子评分。交互宿主还可以显式请求单独的
相邻换序恢复视图；该视图
不并入普通候选顺序。它在最多 16 键内逐一尝试一处相邻交换，只保留不需要
第二次纠错、完全由词典解释且使用完整双拼或锚定尾简的候选。

清单内的 FNV-1a 只用于普通损坏检测，不承担安全含义。外部包的内部目录标识
截取自三份精确材料的 SHA-256，完整 SHA-256 同时写入预检凭据；这可以绑定
来源侧车、清单和载荷，却仍不是数字签名。`contains_private_text` 也是调用者
声明，不可能从文字本身推断隐私归属。私人包必须先经过既定的 DPAPI 外层校验
与解密，再把内存明文交给快照解析器。

## 输入失败时怎样处理

- 小词典不能完整解释首选时，确认操作提交原始组合串，不把研究报告里的
  “未解析”标记写进宿主；
- 数字指定的候选不存在时不伪造结果；
- 快照自身无法加载时，类工厂拒绝创建文本服务，让宿主保留原输入路径；
- 文本服务内部若没有候选源，也会拒绝处理按键，作为第二道防线；
- 解析错误只报告结构类别，不回显词典行、拼音或候选文字。

这些规则首先保证不吞字。它们不声称未知双拼已经被正确转换，也不把按键直通
包装成候选质量。

## 签名与尚未完成的分发边界

包清单、显式来源与许可、SHA-256 材料绑定、解码兼容边界、TSF 合成宿主预检、
三槽状态、新类工厂读取 `current`、独立公钥驱动的 Ed25519 只读验签，以及
显式签名驱动的 adopt / stage 已经完成。正式分发仍有两道门：

1. 当前没有真实发布公钥、签名命令、私钥保管、轮换或吊销策略；验签与槽位
   写入可以分步，也可以由显式签名命令组合，但不能把测试能力称为已建立发布
   体系；
2. 当前仍没有注册、安装、Windows PE/Authenticode 签名或跨进程升级协调。
   实际宿主中的版本观察、启动延迟、内存重复量和回退操作必须在用户另行授权
   注册之后测量。

私人模型仍沿用独立的 DPAPI、因果评测和显式授权流程，不因为公开候选包可
换代就自动启用学习。

## 显式别名的独立热刷新层

`aliasctl` 管理的显式别名不是候选词典，也不进入上述公开包。它使用严格的
`ziranma-explicit-aliases-v1` 明文结构，但明文只在当前进程内短暂构造；磁盘上
的 `aliases.zap` 始终由 Windows 当前用户 DPAPI 保护。加密包以精确密文字节的
SHA-256 内容寻址，`ziranma-explicit-alias-slots-v1` 只保存 current、candidate
和 previous 三个不含正文的标识。包不原地覆盖，槽状态用同步临时文件原子
替换，因此暂存、提升、放弃暂存与回退都不修改正在读取的文件。

安装布局中的所有新 DLL 版本共同使用
`.local/tsf-alpha/user-data/aliases`，而不是把私人配置复制进每个
`.local/tsf-alpha/builds/<dll-sha256>`。每个文本服务保留自己的最后已验证别名
快照，只在空组合收到第一个字母之前检查固定的 `slots.zas`。current 未改变时
不解密包；改变时完整验证路径类型、包大小、内容标识、DPAPI 和内部格式后再
一次替换内存 `Arc`。失败时沿用最后已知可用版本。这样一个服务不会因为另一个
输入框刚刚刷新而在活动组合中途改变候选。

别名只做完整实际码的精确首选召回；它不接受通配符，不参与自由简拼、纠错或
自动学习。当前格式上限为 1,024 条，每个码 1～64 个小写 ASCII 字母，每段文字
最多 64 个 Unicode 标量值和 256 个 UTF-8 字节。`status` 只显示各槽条目数和
校验结果；显示正文需要 `list --confirm-show-private-text`。

是否把完整公开词典编译进 DLL、映射只读数据文件，或使用独立引擎进程，需要
在真实宿主权限、启动延迟、内存重复量和升级可靠性都有测量后再决定。当前快照
核心刻意不提前锁死这一选择。
