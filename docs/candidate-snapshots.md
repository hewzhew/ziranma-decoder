# 候选快照：把数据换代与 TSF 生命周期分开

## 当前状态

`CandidateSnapshot` 是纯内存、只读的候选数据边界。调用者显式提供词典文字和
描述符，快照完成校验后才建立现有 `Decoder` 索引。它不寻找文件、不解密、
不学习、不写磁盘，也不联网。

TSF 类工厂现在会检查 DLL 同目录下固定的 `candidate-data`。目录不存在时，
它使用编译进 DLL 的 50 词公开演示包；目录一旦存在，就只沿 `slots.zcs` 的
`current` 引用加载一个通过预检的公开包，不扫描目录。核心快照随类工厂保持
不可变；可选公开补充层则由每个文本服务保留最后已知可用快照，只在空组合的
下一次字母首键前检查小型状态和槽指针。活动组合不会在数据槽切换时偷换快照。

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

需要两份或更多公开材料时使用严格的
`ziranma-candidate-provenance-v2`。它保留相同的包格式与解码兼容字段，增加
`source_count` 和 `source_1_*` 至 `source_N_*` 材料块；材料数量固定在 2～8，
按稳定 `source_id` 升序排列，重复 ID、缺行、调换顺序、非规范计数和未知字段
都会被拒绝。构造器可以直接校验每份显式来源字节的 SHA-256，运行时则继续把
完整 provenance、manifest 和 payload 字节共同纳入现有存储 ID、预检摘要与
Ed25519 签名消息。旧的单来源 v1 文件仍按原九行格式解析和渲染，不会被改写：

```text
schema=ziranma-candidate-provenance-v2
package_schema=ziranma-candidate-package-v1
decoder_compatibility=ziranma-candidate-decoder-v1
source_count=2
source_1_id=<按 ID 排序的第一份材料>
source_1_license=<许可证>
source_1_url=<HTTPS 来源>
source_1_sha256=<源文件 SHA-256>
source_2_id=<第二份材料>
source_2_license=<许可证>
source_2_url=<HTTPS 来源>
source_2_sha256=<源文件 SHA-256>
manifest_sha256=<manifest.zcm 的 SHA-256>
payload_sha256=<lexicon.tsv 的 SHA-256>
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

默认不写 `--frequency-frontier-entries` 时，频率前沿等于总上限，行为仍是纯
Top-N。需要研究全局频率裁剪遗漏的正常双字词时，可以显式缩小频率前沿；剩余
容量只补入“该规范完整码尚未被前沿覆盖”的最高权重双字整词，总条数仍受
`--max-entries` 硬上限约束：

```powershell
cargo run --release --bin candidatectl -- build-rime-slice `
  --source .local/public-audit/wanxiang-fdda7afb/jichu.dict.yaml `
  --output .local/public-audit/wanxiang-fdda7afb/package-top76300-plus-bigram-cover-v2 `
  --revision wanxiang-jichu-fdda7afb-top76300-plus-bigram-cover-v2 `
  --source-id wanxiang-jichu `
  --source-license CC-BY-4.0 `
  --source-url https://github.com/amzxyz/rime_wanxiang `
  --source-sha256 9d14c0c49588d63b16c554df4711bed5da822c63de9d50f4759c53542138ac00 `
  --max-entries 120000 `
  --frequency-frontier-entries 76300 `
  --max-text-characters 8 `
  --public
```

固定万象修订上的本机确定性导入结果为 120,000 条：76,300 条全局频率前沿
覆盖 24,311 个双字码，随后补入 43,677 个缺码代表，并用 23 条后续全局高频项
填满剩余容量；67,988 个合格双字码全部获得至少一个公开整词，覆盖候选上限外
为 0。最低权重降到 1 是覆盖长尾的预期结果，不表示低频词会跨来源抢占核心
首选。与日用核心做前七、每码补一个的层审计仍保持共有完整码的核心首选不变。

“一个码已有代表”仍不等于同码深层正常词可查询。离线实验可显式添加
`--two-character-coverage-depth 2..8`：先保持每码第一个代表，再让更深身份按
同一来源内部权重占用剩余容量；默认值 1 完全保持上述旧行为。固定万象中每码
最多 4 项共有 136,586 个双字身份，超过 120k 切片上限，所以这个开关只用于量化
容量矛盾，不能被当作新的默认包策略。

独立公开词表可进一步只做二值词面确认。下面的只读审计不写包，也不把两份来源
的数值频率混合；读音和自然码始终来自万象，jieba 只回答“是否也收录这个双字
词面”，随后排除基础载荷并按完整码限制深度：

```powershell
target\release\candidatectl.exe short-consensus-audit `
  --source .local\public-audit\wanxiang-fdda7afb\jichu.dict.yaml `
  --confirmation .local\research\upstreams\jieba\jieba\dict.txt `
  --base-payload .local\public-audit\wanxiang-fdda7afb\package-top120k-v1\lexicon.tsv `
  --per-code-depth 2 `
  --entry-limit 50000
```

命令逐字节报告三个输入的 SHA-256，并只输出聚合计数。固定结果为 55,950 个
基础外双来源确认身份、34,894 个规范码；每码 1 项需 34,894 条，每码 2 项需
46,832 条。该规模不能塞进基础包到当前快照上限之间的余量，因此现在使用独立、
默认关闭的精确短词包，而不是扩大通用 Decoder：

```powershell
target\release\candidatectl.exe build-short-consensus-layer `
  --source .local\public-audit\wanxiang-fdda7afb\jichu.dict.yaml `
  --confirmation .local\research\upstreams\jieba\jieba\dict.txt `
  --base-payload .local\public-audit\wanxiang-fdda7afb\package-top120k-v1\lexicon.tsv `
  --output .local\public-audit\wanxiang-fdda7afb\package-exact-short-consensus-depth2-v1 `
  --revision wanxiang-jieba-exact-short-depth2-v1 `
  --per-code-depth 2 --entry-limit 50000 `
  --source-id wanxiang-jichu-fdda7afb --source-license CC-BY-4.0 `
  --source-url https://github.com/amzxyz/rime_wanxiang `
  --source-sha256 9d14c0c49588d63b16c554df4711bed5da822c63de9d50f4759c53542138ac00 `
  --confirmation-id jieba-dict-67fa2e36 --confirmation-license MIT `
  --confirmation-url https://github.com/fxsjy/jieba `
  --confirmation-sha256 7197c3211ddd98962b036cdf40324d1ea2bfaa12bd028e68faa70111a88e12a8 `
  --base-id wanxiang-top120k-v1 --base-license CC-BY-4.0 `
  --base-url https://github.com/hewzhew/ziranma-decoder `
  --base-sha256 849aae039dfe503eb4dadfcb8529ddb74992a05b202e7cf6a69be932c73c8717 `
  --public
```

构建命令在创建输出目录前认证三份普通公开材料，并把三者写入 v2 provenance；
基础载荷只参与去重，不被复制进新包。输出仍使用标准 `manifest.zcm`、
`provenance.zcp` 和 `lexicon.tsv`，但额外要求所有行是双汉字、双音节、按四键完整
码排序，同码内按万象权重递减，且每码不超过 8 项。加载器只保留约
`code -> TSV 字节范围` 的数组，不构造 trie、纠错图或句子 lattice。

固定 depth-2 包有 46,832 条、34,894 个完整码，载荷 899,671 字节，紧凑索引
558,304 字节，认证摘要为
`2cd80edd03f2c420e8b54b37db32576dc73c7f63e787df1c82ba99980c0ddec3`。
本机 release 的一次观测中，首次文件认证与建索引约 36.846 ms，固定码热查询
十万次平均约 0.105 µs；这只证明没有构造第二个通用 Decoder，不是跨机器性能承诺。
可用 `exact-short-benchmark` 复测当前机器与文件缓存状态。

`exact-short-query` 只读查询一个四键码。纯预览合并器可选择固定已有 Top-1，
也可固定完整第一页；它只插入既有 50 项中不存在的新身份，重复身份不消耗插入
名额，候选总数仍受 50 项边界约束，剩余旧候选相对顺序不变。分页保护版本还会
检查同码已有精确短词：若一次插入会让其中任何一个跨页或掉出总范围，就缩减插入
数，必要时完全跳过该码。浅于一页的候选不会被它擅自扩成第一页。

公开分页安全门可用下列只读命令复现；它同时比较首选后补 1/2 项、固定第一页后
补 1/2 项，以及分页保护后补 1/2 项，不写包、不改槽位：

```powershell
cargo run --release --bin candidatectl -- exact-short-layer-audit `
  --core-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --supplemental-payload .local/public-audit/wanxiang-fdda7afb/package-top120k-v1/lexicon.tsv `
  --exact-package .local/public-audit/wanxiang-fdda7afb/package-exact-short-consensus-depth2-v1 `
  --held-out-corpus data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu `
  --frontier-limit 6 --supplemental-promotions 1
```

固定 UD test 留出有 334 个精确层匹配词面（421 次）。机械插在首选后会改变
113 个第一页，并让 36 个目标名次后移，其中 4 个跨页，故被否决。仅把插入点移到
第二页开头虽使第一页零变化，仍有 2 个目标跨页，也被否决。分页保护后，补 1 项
让 59 个目标新进入前两页，补 2 项让 64 个目标新进入；两者均保持第一页逐项零
变化、目标零跨页、零掉出 50 项总范围。补 2 项额外挤出 22 个第 50 名附近的旧尾项，
却多覆盖 5 个公开目标，且没有增加目标跨页，因此它是当前离线胜出配置。尾端位移
仍单独报告，不等同于正确性结论。

后来又验证了一个更贴近候选语义的反事实：先保留核心和补充层已有的连续完整词
通道，再把精确短词放在逐字组合之前。它把名次后移目标从 36 个降到 22 个，但仍
改变 106 个第一页，并有 2 个目标跨到下一页，安全门同样未通过。这说明“完整词
身份”是有用特征，却不能脱离上下文和独立排序证据直接决定第一页；运行时继续
采用分页保护的第二页入口。

`exact-short-layer-benchmark` 把精确层增量与既有深解码分开计时，并要求显式给出
候选请求深度。现行 TSF 页宽是 6，翻到第二页会请求前 12 项；对应复现命令为：

```powershell
cargo run --release --bin candidatectl -- exact-short-layer-benchmark `
  --core-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --supplemental-payload .local/public-audit/wanxiang-fdda7afb/package-top120k-v1/lexicon.tsv `
  --exact-package .local/public-audit/wanxiang-fdda7afb/package-exact-short-consensus-depth2-v1 `
  --frontier-limit 6 --supplemental-promotions 1 --exact-promotions 2 `
  --candidate-limit 12 --sample-limit 128 --repetitions 5
```

本机 640 个 release 样本中，核心＋补充前 12 项 median 8.276 ms、P95 10.463 ms；
单独的精确查询与分页保护合并 median 0.003 ms、P95 0.004 ms、P99 0.005 ms。
精确包认证建索引约 33.990 ms。把同一工作负载故意扩大到前 50 项时，既有深解码
median 达 95.898 ms，而精确合并 P95 仍只有 0.008 ms；所以 Top-50 只用于离线
全局分页审计，绝不能被每次按键或首帧同步调用。两个约百毫秒解码分布的 median
相减受运行噪声支配，命令只报告该差值，不再把它误作精确层增量安全门。

启用前的真实 TSF 状态路径可用另一条 release-only、只读命令复现。它认证完整核心、
可选补充和精确短词包，等距寻找能保持第一页且把公开精确项放在索引 6 的目标；探针
码与候选正文只在内存中用于合成 Context，不进入报告：

```powershell
cargo run --release --bin candidatectl -- exact-short-tsf-preflight `
  --core-package .local/candidate-rime-pinyin-simp-0c6861ef-v1 `
  --supplemental-package .local/public-audit/wanxiang-fdda7afb/package-top120k-v1 `
  --supplemental-promotions 1 `
  --exact-package .local/public-audit/wanxiang-fdda7afb/package-exact-short-consensus-depth2-v1 `
  --exact-promotions 2 --sample-limit 16 --repetitions 5
```

固定本机 80 个 release 样本全部保持第一页并由空格提交带
`PublicConsensusExact` 来源的第二页首项。第一页状态 median 12.013 ms、p95
15.347 ms；PageDown 增量 median 7.453 ms、p95 9.810 ms；首键至第二页状态
median 19.615 ms、p95 25.264 ms、max 26.463 ms；提交 p95 0.035 ms。核心、补充、
精确层认证与索引分别约 88.857、233.781、79.631 ms。命令每个目标先作一次不计时
预热，工作量上限为 32 码 × 20 次且总计不超过 640 个样本；它不写文件、不生成组合
凭据、不改变槽位。同步返回只证明候选状态已就绪，不证明候选窗完成绘制，也不包含
桌面合成、真实应用宿主和运行时换包延迟。

真正准备日用根必须改用 release-only `exact-short-prepare`。它从核心槽和显式补充槽
解析当前已认证包，先对“核心＋补充及上限＋精确包及上限＋固定页宽 6”执行同一真实
TSF 第二页工作负载，全部通过后才安装精确包并写 `exact-short-preflight.zep`。凭据
绑定三层认证 SHA-256、两个注入上限、TSF host 和页宽；任一项变化都会失效。准备
结束仍把 `exact-short.zcl` 保持为关闭，故不会改变日用候选。普通候选包的 `.zpf`
只证明单包可由 TSF 上屏，不能替代组合凭据。

仓库根的收口入口固定了当前已审计的三层组合、可信摘要与工作量。无参数调用会运行
`exact-short-readiness` 只读体检：它认证固定公开包和当前核心/补充槽，并把独立精确根
区分为尚未准备、组合凭据匹配且可启用、已启用、凭据漂移或已有另一版本；不会只凭
几个文件存在就宣称就绪。只有显式 `prepare` 才会运行上述准备，而且成功后再次运行
同一体检并保持关闭：

```powershell
.\prepare-exact-short.cmd
.\prepare-exact-short.cmd prepare
```

体检对公开材料或组合损坏仍然失败关闭；尚未准备时不会创建独立根，组合凭据偏离时
只报告需要重新专项预检，不覆盖旧包或旧凭据。入口通过不可变用户工具包解析
`candidatectl`，不会自动刷新工具、安装或换代 DLL，
也不包含启用动作。若当前用户工具包早于本命令，它只提示操作者另行显式运行
`refresh-ime.cmd refresh`；失败不会退而调用仓库里另一条启用路径。这个固定入口只适用
于当前审计组合，公开包或组合参数改变时必须随新证据更新，不能把旧摘要继续复用。

准备完成后，启用与关闭使用另一条独立入口；无参数仍只运行同一准备度体检，两个写入
动作都必须显式给出：

```powershell
.\exact-short-ime.cmd
.\exact-short-ime.cmd enable
.\exact-short-ime.cmd disable
```

启用动作重新绑定固定公开包与可信摘要，写入小状态文件后立即通过生产运行时加载器
复读核心、补充和精确三层；只有精确包标识、认证摘要、上限完全一致，且补充层与精确
层均未失败回退，才返回成功。复读失败会立即写回关闭；若回退写入本身也失败，错误会
明确说明无法确认安全状态。入口不包含准备、工具刷新、DLL 换代或管理员提升。

TSF 尚未加载、安装或启用这个包，因此当前日用候选位移严格为零。候选缓存已经
按 6、12、18……惰性扩页。`ExactShortPageSession` 现在提供一个不落盘的纯候选文字
边界：没有同码个人精确偏好时，第一页不查询精确层，第二页只作一次保护插入决定，后续扩深冻结全部已返回
前缀并按基础层原顺序补回尚未展示的身份；基础层若重写旧前缀则失败关闭。合成测试
已覆盖 6→12→18→50，并特意构造了“18 项独立重算会撤回 12 项决定”的反例。
会话同时公开每项对应的基础候选索引与独立穷尽状态；可选的 TSF `CandidateProvider`
钩子已经由 `CandidateCache` 消费，用该映射同步来源标签，并让后续会话个性化继续
通过原有并行数组镜像。合成宿主覆盖了层缺失/中途消失、会话个性化和自动换序共存：
旧前缀保持不变，精确插入标为 `PublicConsensusExact`，底层已穷尽时不会因插入项刚好
填满页面而虚报下一页。若同码会话记忆或已确认个人偏好存在，首次展示会有界预取
前 12 项，让原本位于第二页的已知身份在第一帧就完成个人重排；公开精确层自身仍不能
无证据改变第一页。这样翻页不会才突然激活已经存在的个人记忆，也不会扩成 Top-50
常态查询。真实 `SnapshotCandidateProvider` 现可消费一个独立、默认关闭
的认证运行时根：`user-data/public-exact-short` 不存在、`exact-short.zcl` 缺失或显式
关闭时仍返回“无精确层”。启用选择必须与 current 包一致，并通过既有内容寻址、来源、
单包预检、组合专项凭据及专用双字目录校验；只在新组合首键前轮询小指针，同组合不换
目录，损坏更新保留最后有效目录。若核心、补充包或补充上限改变，旧组合凭据会立即
失败关闭精确注入，不能借“保留最后有效”跨组合复用。同包上限变化也必须重新专项
预检。独立根以 `exact-short-prepare/status/enable/disable` 管理；prepare 只准备并保持
关闭，enable 再次核对 current、完整公开运行时组合和专项凭据。
换代脚本仍不创建或开启该根，所以加入 CLI 不等于自动换代；V17 许愿身份已记录精确层
revision。
一个不发布状态的专用预检 API 会在进入宿主前严格确认第一页为 6 项、目标不在其中、
并以 `PublicConsensusExact` 来源稳定位于索引 6；随后通过真实系统 Thread Manager、
合成 Context 和同一按键接收器输入完整码，发送 PageDown，再以空格提交第二页首项。
正反例、可选补充层和连续精确 revision 替换均有 Windows 合成测试。返回报告只含
核心/可选补充/精确 revision、输入长度与高分辨率耗时，不含正文。真实 release
状态路径的重复测量已完成上述第一批固定基线；视觉呈现及运行时换包门通过前仍不启用。

导入器同时聚合报告三字、四字规范码，而不显示词条文字。上述固定切片中，
536,722 个合格三字码有 26,897 个进入最终切片，635,180 个合格四字码有
14,797 个进入最终切片；因此不能把双字“每码补一个”机械扩大到更长词。可选的
`--three-character-coverage-entries` 与 `--four-character-coverage-entries` 只在
全局前沿之后各保留指定数量的最高来源权重缺码代表，默认均为 0，且两者之和
必须放进现有 `--max-entries`，不会增大运行时快照。

固定来源上的 2,000 + 2,000 配额实验仍保持 120,000 条和全部共有码首选，新增
3,977 个长词规范码，同时放弃 3,977 个最低权重双字码。独立 UD train 词面
负对照只新增 4 个三字词面（4 次）、2 个四字词面（4 次），却丢失 34 个双字
词面（44 次）；UD test 新增 0 个三字、1 个四字词面（1 次），丢失 7 个双字
词面（8 次）。该 token 口径不能代表连续短语或个人输入，但已经足以拒绝自动
换代：配额功能只保留为离线实验工具，当前日用包继续使用三字、四字配额为 0
的完整双字覆盖方案。

上述数字可用只读、聚合的留出审计复现。命令要求训练侧参考与留出语料具有不同
SHA-256，同时报告独立词面数和实际出现次数，不显示文字、不推断 UD 中没有的
读音，也不比较候选包之间的原始权重：

```powershell
cargo run --release --bin candidatectl -- length-coverage-audit `
  --base-payload .local/public-audit/wanxiang-fdda7afb/package-top76300-plus-bigram-cover-v2/lexicon.tsv `
  --challenger-payload .local/public-audit/wanxiang-fdda7afb/package-top76300-plus-length2k-v1/lexicon.tsv `
  --fit-corpus data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu `
  --held-out-corpus data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu
```

普通万象切片包的 provenance 只绑定一个词典来源。若将 UD train 真正用于挑选长词，
输出包还依赖核心、补充和训练语料，不能继续冒充单来源切片。通用切片器仍不提供这种
模式；下述专用三字构建器会把四份材料一起认证，留出审计本身则始终不能绕过来源绑定。

### 三字精确短语层审计

容量内替换实验不能回答另一个问题：不删除双字、另开一个有界精确通道，能否让
来源确认的三字整词越过机械单字组合。`exact-phrase-layer-audit` 因此把公开 UD 中
一至三个相邻、不可跨标点的 token 合并成恰好三字的 span；完整码优先由现有核心
token 取得，缺词时逐字取得。训练侧 span 只负责向固定万象请求相同文字/完整码，
并排除多音文字、万象中同码对应多个三字词面以及核心/补充已经存在的身份。每码
最终最多保留一个词，留出语料不参与选层。

预览规则也比“总是插到第一”窄：已有完整词通道存在时，新词只能跟在该通道后；
没有已有完整词时，来源确认的三字整词才可越过自由组合。安全对照分成两类：既有
完整词必须零位移；任意相邻片段仍须保留在原分页，不能跨页或掉出 Top-10。后者不能
被误标为金标准整词——例如公开 dev 中的“和 + 工业”让位给“核工业”，但仍由第 1
项留在第 2 项，这属于整词与自由组合的预期取舍，不是候选消失。

冻结命令如下；它只构建进程内快照并打印至多八个公开反例，不写包或槽位：

```powershell
cargo run --release --bin candidatectl -- exact-phrase-layer-audit `
  --source .local/public-audit/wanxiang-fdda7afb/jichu.dict.yaml `
  --core-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --supplemental-payload .local/public-audit/wanxiang-fdda7afb/package-top100k-v1/lexicon.tsv `
  --fit-corpus data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu `
  --held-out-corpus data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu `
  --entry-limit 5000 --repetitions 5
```

训练侧 39,456 个可编码三字 span 与固定万象相交后，排除来源同码歧义和既有身份，
得到 2,799 条、每码唯一的实验层。独立 GSDSimp test 有 51 个目标身份（57 次）：
基线 Top-1 为 21/51、Top-6 为 37/51；预览后 Top-1 和 Top-6 均为 51/51，新增
30 个正确首选，没有目标变差。7 个同码自由组合对照均未跨页或掉出 Top-10，既有
整词对照零变化，严格门通过。另一次 PUD 跨域压力测试在 17 个可比目标上新增 8 个
正确首选，同样没有跨页或可见性损失；其中含繁体表面，只作外部压力证据。

三字层载荷为 2,799 条、72,909 字节，索引首次构建约 6.8 ms。48 个公开完整码的
同次预热查询中，既有两层与预览路径的 median 分别约 11.7 ms 和 11.2 ms、p95
约 13.7 ms 和 13.2 ms；差异落在同机噪声内，不能声称新层更快。当前结论只是
“独立层假设值得进入多来源认证和宿主预检”，不是自动发布许可；日用包、槽位和
TSF 均未改变，像“再进来”这类未被训练侧公开 span 选中的个人目标仍交给新版精确
记忆验证，不能借本审计逐条硬编码。

证据门通过后，`build-exact-phrase-layer` 才允许生成一个仍不接入运行时的实验包。
它必须在创建输出目录前逐一核对四份公开材料，任一哈希错误、材料摘要重复、解析失败
或选层为空都不留下部分包：

```powershell
cargo run --release --bin candidatectl -- build-exact-phrase-layer `
  --source .local/public-audit/wanxiang-fdda7afb/jichu.dict.yaml `
  --core-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --supplemental-payload .local/public-audit/wanxiang-fdda7afb/package-top100k-v1/lexicon.tsv `
  --fit-corpus data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu `
  --output .local/public-audit/wanxiang-fdda7afb/package-exact-phrase-train-span-v1 `
  --revision wanxiang-gsdsimp-exact-phrase-span-v1 --entry-limit 5000 `
  --source-id wanxiang-jichu-fdda7afb --source-license CC-BY-4.0 `
  --source-url https://github.com/amzxyz/rime_wanxiang `
  --source-sha256 9d14c0c49588d63b16c554df4711bed5da822c63de9d50f4759c53542138ac00 `
  --core-id rime-pinyin-simp-0c6861ef-core-payload --core-license Apache-2.0 `
  --core-url https://raw.githubusercontent.com/rime/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml `
  --core-sha256 fec5d5173127d568a047655b2f92a94c4e546c91565d7fd14808fbf71266b834 `
  --supplemental-id wanxiang-jichu-fdda7afb-top100k-payload `
  --supplemental-license CC-BY-4.0 `
  --supplemental-url https://github.com/amzxyz/rime_wanxiang `
  --supplemental-sha256 96b36073b0386ef84bd6347fe9c91a118bfcfa79c13eb6b73b18c1c9bc98f382 `
  --fit-id ud-chinese-gsdsimp-4231dfd-train --fit-license CC-BY-SA-4.0 `
  --fit-url https://raw.githubusercontent.com/UniversalDependencies/UD_Chinese-GSDSimp/4231dfd59866fa5999ad4a6bc1fdecd7985b3b59/zh_gsdsimp-ud-train.conllu `
  --fit-sha256 956636fe612a1166e8b19e7413fee2e73d68231aca2f0455be2c616b947d629d `
  --public
```

固定材料生成的包版本为 `wanxiang-gsdsimp-exact-phrase-span-v1`，发布 SHA-256 为
`570abd32582e695e5a4d042a2c4cb1a67a6bb137cf52c1f72c6e9f41b19a181e`。包级
`verify`、通用 TSF 候选 `preflight` 和四源 `inspect` 均通过；输出只保存在 Git
忽略的公开审计目录。通用预检只证明包本身可加载和提交一个三字词，不证明三层合并
顺序、真实首帧或热切换，因此仍不能安装或启用。

`exact-phrase-layer-preflight` 随后补上三包组合专项门。它同时认证现行核心包、
Top-100k 补充包和四源三字包，并要求三字包 provenance 精确包含前两包的载荷
SHA-256；即使三个包各自合法，换成另一份核心或补充载荷也会在候选查询前失败。
三字载荷还必须全部为三汉字、三音节、六键完整码且每码唯一。每一个目标码都经过
完整候选合并：聚合核对有无既有完整词前缀、目标第 1–6 项分布、目标缺失或重复、
跨出第一页、守护位次、前缀变化、候选重复与结果上限；任何异常都只以计数报告并
失败关闭，不输出码或候选正文。等距样本只承担热路径性能计时，等距负对照只核对
没有三字层身份的码逐项等于原两层，二者均不再冒充全目录正确性证明。命令全程只读，
不创建运行时根、槽位或开关，也不调用 TSF：

```powershell
target\release\candidatectl.exe exact-phrase-layer-preflight `
  --core-package .local\candidate-rime-pinyin-simp-0c6861ef-v1 `
  --supplemental-package .local\public-audit\wanxiang-fdda7afb\package-top100k-v1 `
  --phrase-package .local\public-audit\wanxiang-fdda7afb\package-exact-phrase-train-span-v1 `
  --sample-limit 16 --repetitions 5
```

固定 release 全目录运行认证了核心
`38c4697bdea55857bbe03ee970528d4658e80ce4c258ebbe2d074550ab852c1d`、补充
`ae268a35f8e0125598a98205a3ce1c057f7567d08bec84e148804b70e7330eb7` 和三字层
`570abd32582e695e5a4d042a2c4cb1a67a6bb137cf52c1f72c6e9f41b19a181e`。2,799 个码
全部通过，其中 2,797 个没有既有完整词前缀并位于第 1 项，2 个跟随一个既有完整词
并位于第 2 项；第 3–6 项均为 0，跨页、缺失、重复、守护位次不符、前缀变化、候选
重复和越界也全部为 0。这个结果同时证明旧的 16 个等距目标样本不足：它们恰好全在
第 1 项，漏掉了两个真实的第 2 项分支。最初实现对每个码重复生成同一两层基线并重复
读取既有精确集合，全目录离线审计耗时 70,735.689 ms；复用一次查询结果后，同一
2,799 条范围、同一 `2797/2` 位次分布、同一零异常门与同一校验和耗时 37,442.021 ms，
减少约 47.1%，没有以缩小范围换速度。独立的 16 码 × 5 次性能抽样中，优化后两层
查询 median/p95 为 12.292/19.771 ms，三包预览为 11.872/18.153 ms。这些差异只
视作同机噪声，不声称新层更快；37 秒全量离线扫描也不是日常按键热路径。该门仍没有
创建真实 Context、候选窗或桌面帧，因此它本身不能替代 TSF 三层宿主预检，也不构成
启用许可。

`exact-phrase-tsf-preflight` 随后把相同三包送入真实 Windows TSF 合成 Context。命令
在创建 Context 前重新执行四源绑定、六键唯一形状和上述全目录前缀、去重、第一页
门，并执行有界负对照；随后先为全目录实际存在的每个目标位次桶安排至少一个探针，
剩余名额才按全码序分散取样。请求探针数若少于非空位次桶数，会在创建 Context 前
失败；每个探针最多检查 64 个候选码，只保留能在第一页稳定提交者。首项用空格，
后续项用普通数字键；正式计时总量固定不超过 640 次。发现与报告均不输出探针码或
候选正文：

```powershell
target\release\candidatectl.exe exact-phrase-tsf-preflight `
  --core-package .local\candidate-rime-pinyin-simp-0c6861ef-v1 `
  --supplemental-package .local\public-audit\wanxiang-fdda7afb\package-top100k-v1 `
  --phrase-package .local\public-audit\wanxiang-fdda7afb\package-exact-phrase-train-span-v1 `
  --sample-limit 16 --repetitions 5
```

固定 release 分层运行重新认证上述三份 SHA-256，检查 16 个码并完成 16 个发现预热与
80 个计时提交；位次分布为第 1 项 75 次、第 2 项 5 次，其余为 0。由此，当前公开
实包里的一个第 2 项分支已在五次重复中通过普通数字键真实提交，不再只由聚焦合成
回归代替。第一页状态 median/p95 为 27.718/39.783 ms，提交为 0.014/0.020 ms，
首键至提交为 27.730/39.799 ms。更早的等距策略曾连续选出 16 个第 1 项探针，现已
由全目录位次分层策略取代；旧结果只保留为发现抽样盲点的历史证据，不再代表当前门。
该计时截止同步候选状态与提交，不包含候选窗绘制、桌面合成、屏幕首帧或实际应用
呈现。命令不写槽位、状态或
`exact-phrase-preflight.zep`，通过仍不构成 prepare、enable、安装或换代许可。

下一条 `exact-phrase-popup-preflight` 已把可见候选窗验收入口准备好，但默认绝不运行。
它先复用上面的真实 Context 稳定探针，再以同一核心、补充和三字快照重建该探针的
第一页；目标必须继续携带 `PublicConsensusExact` 来源。页面随后经过生产
`CandidatePopup` 的创建、布局、`WM_PAINT`、双缓冲、`BitBlt`、`EndPaint`、
`DwmFlush`、隐藏和销毁路径，并固定覆盖 96/120/144/192 DPI。为了不制造大量闪窗，
参数收紧为最多 4 个页面、每页最多 5 次，即最多 80 个短暂窗口；帮助文本和最终报告
都会明确说明它是可见操作：

```powershell
target\release\candidatectl.exe exact-phrase-popup-preflight `
  --core-package .local\candidate-rime-pinyin-simp-0c6861ef-v1 `
  --supplemental-package .local\public-audit\wanxiang-fdda7afb\package-top100k-v1 `
  --phrase-package .local\public-audit\wanxiang-fdda7afb\package-exact-phrase-train-span-v1 `
  --sample-limit 2 --repetitions 1
```

当前阶段完成了共享页面、来源标记、参数上限、release guard 和窗口阶段完整性实现；
当前固定三包有两个非空目标位次桶，因此可见门至少请求 2 个页面；更少会在创建窗口前
失败关闭。参数的通用解析上限仍为 1–4，供只有一个位次桶的其他认证包使用。
报告类型本身不持有探针码或候选正文，自动测试覆盖共享页面、来源和失败关闭。猫猫
没有在宝宝桌面上擅自运行上述命令，因此这里不虚构任何三字实包绘制耗时。即使未来
显式运行通过，它仍只是“真实 TSF Context 提交”与“同页生产 popup 绘制”两条串行
组件路径，不是已安装编辑器里的一次真实回调；`DwmFlush` 也不等于屏幕已经扫描显示。

三包专项门之后新增了一个仍不可启用的运行时骨架。独立根内预留严格三行
`exact-phrase.zcl`，并继续复用不可变 `slots.zcs`、`packages` 与普通包 `.zpf`；
`load_candidate_runtime_snapshots_with_all_layers` 只在调用方显式给出该根时读取。启用
选择必须与 current 包一致，补充层必须同时有效，载荷必须逐条满足三汉字、三音节、
六键且每码唯一，四源 provenance 还须包含当前核心与补充载荷摘要。

即便上述检查全部通过，加载器仍要求独立 `exact-phrase-preflight.zep` 同时绑定三包
认证摘要、经过公开审计的补充层影响上限 1、页宽 6 和未来的真实
`tsf-exact-phrase-first-page-context-v1` 宿主。当前没有任何命令会生成该凭据，也没有
prepare/enable、安装脚本或 TSF 类工厂根发现逻辑；已有日用加载入口仍不传入三字根。
因此这一步只冻结了默认关闭、失败回退的状态与认证契约，为下一阶段真实 Context
预检提供目标，不能通过手工写一个开关绕过缺失的宿主证据。

### 同修订固定短语表审计

万象固定修订 `fdda7afb` 还包含一份独立的
[`lua/data/chengyu.txt`](https://github.com/amzxyz/rime_wanxiang/blob/fdda7afb/lua/data/chengyu.txt)，
SHA-256 为
`0cda817d29d312d46458f884bbb9f32b8048b92fe91e99288b4804df4629cc20`。
它不是拼音词典，而是小写索引到一个或多个四字固定短语的辅助表；内容也不全是
成语，包含普通短语和专名。因此实验只把它当作允许研究的词面集合，拼音、规范码
和来源权重仍必须由同一修订的 `jichu.dict.yaml` 独立确认。

`phrase-coverage-audit` 只做这两个公开文件的交集与公开 UD 训练/留出词面审计，
不输出短语文字，不运行候选排序，也不构建候选包：

```powershell
target\release\candidatectl.exe phrase-coverage-audit `
  --source .local\public-audit\wanxiang-fdda7afb\jichu.dict.yaml `
  --allowlist .local\public-audit\wanxiang-fdda7afb\chengyu.txt `
  --base-payload .local\public-audit\wanxiang-fdda7afb\package-top76300-plus-bigram-cover-v2\lexicon.tsv `
  --fit-corpus data\public\ud-chinese-gsdsimp\zh_gsdsimp-ud-train.conllu `
  --held-out-corpus data\public\ud-chinese-gsdsimp\zh_gsdsimp-ud-test.conllu `
  --entry-limit 5000
```

固定材料解析出 26,040 个唯一四字词面，其中 21,651 个能从基础词典取得合格的
四音节读音，2,483 个已在完整双字覆盖包内；剩余 19,168 个按同一来源权重排序。
在不删除基础载荷条目的纯加法口径下：

| 新增长词配额 | UD train 新增词面 / 实例 | UD test 新增词面 / 实例 |
| ---: | ---: | ---: |
| 2,000 | 19 / 29 | 2 / 2 |
| 5,000 | 54 / 72 | 5 / 5 |
| 10,000 | 71 / 90 | 7 / 7 |

三档的公开 token 审计均未丢失原覆盖，但这还没有测量完整候选排序、内存、索引
建立和首帧成本；5,000 到 10,000 条的留出增益也已经变缓。实际短语层不仅依赖
基础词典与固定短语表，还依赖用于排除既有词面的基础 `lexicon.tsv`；即使最终
新增载荷恰好相同，换一份基础载荷也必须改变包的认证身份。

`build-phrase-layer` 因此要求三份普通 UTF-8 材料及三组独立声明。命令先完整
读取并逐字节核对三个 SHA-256，随后才解析基础载荷、按词面去重，并沿用来源
权重顺序精确选取 `--entry-limit` 条；可用新增词不足时失败。三份材料共同写入
v2 provenance，输出仍只包含新增短语层，不复制基础载荷，也不安装或切换槽位：

```powershell
target\release\candidatectl.exe build-phrase-layer `
  --source .local\public-audit\wanxiang-fdda7afb\jichu.dict.yaml `
  --allowlist .local\public-audit\wanxiang-fdda7afb\chengyu.txt `
  --base-payload .local\public-audit\wanxiang-fdda7afb\package-top76300-plus-bigram-cover-v2\lexicon.tsv `
  --output .local\public-audit\wanxiang-fdda7afb\package-phrase-layer-top10k-v1 `
  --revision wanxiang-fdda7afb-phrase-layer-top10k-v1 `
  --entry-limit 10000 `
  --source-id wanxiang-jichu-fdda7afb `
  --source-license CC-BY-4.0 `
  --source-url https://github.com/amzxyz/rime_wanxiang `
  --source-sha256 9d14c0c49588d63b16c554df4711bed5da822c63de9d50f4759c53542138ac00 `
  --allowlist-id wanxiang-chengyu-fdda7afb `
  --allowlist-license CC-BY-4.0 `
  --allowlist-url https://github.com/amzxyz/rime_wanxiang `
  --allowlist-sha256 0cda817d29d312d46458f884bbb9f32b8048b92fe91e99288b4804df4629cc20 `
  --base-id ziranma-wanxiang-base-top76300-bigram-v2 `
  --base-license CC-BY-4.0 `
  --base-url https://github.com/hewzhew/ziranma-decoder `
  --base-sha256 d4090b731f0aee40a06f2bbc102a447b64273caef9d775529e9e721447aa843c `
  --public
```

输出目录必须尚不存在。哈希漂移、重复来源 ID、格式错误、配额不足或回读失败都
不会留下一个可被误认作完整包的新目录。旧单来源 v1 包仍保持逐字节兼容。
固定材料的 10,000 条实包为 331,550 字节，发布 SHA-256 为
`7a868ecc3004db512a1af94dc778657c3ab447986a798b37a2883eaa866d59c6`；现有
`inspect`、独立摘要 `verify` 与 Windows TSF 合成 `preflight` 均已通过。该验证
没有采用、暂存、启用或安装短语层。

双字覆盖包与固定短语层可以在采用前合成为一个新的不可变公开包，而不占用第二个
运行时补充槽。`merge-public-packages` 会先完整加载两边的清单、provenance 和
载荷，拒绝明文私人包、认证漂移及格式损坏；随后保持基础包顺序不变，只把叠加包
中尚不存在的 `(文字, 规范双拼码)` 身份按原顺序追加。相同来源 ID 的四项声明
必须完全一致，否则失败；所有不同来源会去重后写入新的 v2 provenance。合并结果
仍受 131,072 条和 16 MiB 的固定快照边界约束：

```powershell
target\release\candidatectl.exe merge-public-packages `
  --base .local\public-audit\wanxiang-fdda7afb\package-top76300-plus-bigram-cover-v2 `
  --overlay .local\public-audit\wanxiang-fdda7afb\package-phrase-layer-top10k-v1 `
  --output .local\public-audit\wanxiang-fdda7afb\package-bigram-cover-plus-phrase-top10k-v1 `
  --revision wanxiang-fdda7afb-bigram-cover-plus-phrase-top10k-v1 `
  --public
```

输出目录必须尚不存在，命令不会覆盖、安装、暂存或切换候选槽。完整校验和合并均
在创建目录之前完成；写入期间任一步失败时会清理未完成的新目录。叠加包不能覆盖
基础包已有身份的拼音、频率或顺序，因此该操作不是跨来源权重混合。生成后可用
`package-query --package <PACKAGE_DIR> --code <KEYS> --limit <N>` 直接做只读候选
检查，无需先复制进候选槽；它拒绝私人包，也不加载个人数据或运行时重排。

固定两包的实际合并结果为 130,000 条、3,113,973 字节，包认证 SHA-256 为
`285dd1cb7b87cb86b310eba5b4b80053324a91703d14f681c15e869cc981988c`。
`inspect`、独立摘要 `verify` 和 TSF 合成 `preflight` 均通过；四组既有完整码
控制的前七项与基础包逐项相同。目标查询也揭示了选择边界：双字覆盖层让
`bgdr` 的“绷断”成为首选，但 `blklrbsv` 的“掰开揉碎”并不在固定短语
allowlist，因而也不在 10,000 条短语层；`wutijc` 的“误提交”同样没有被
该层解决。`jichu.dict.yaml` 中出现某个词面，不等于它已通过固定短语集合与
配额两道选择。合并包因此继续保持未采用，不能把生成成功写成三个目标都已修复。

以日用核心和该 130,000 条合并包运行 release 热路径时，本机一次索引建立约
210/561 ms（核心/补充）；140 个预热查询的核心与启用补充 median 约
5.86/5.55 ms、p95 约 13.77/13.68 ms，核心完整码首选变化为 0。固定 UD
组合审计的 128 条正样本由基础 Top-7 32 提高到 89，但 128 条全核心负对照有
1 条原可见目标掉出 Top-7。样本集合会随补充载荷改变，这些数字不能与旧 120k
包直接作逐项因果比较；它们足以否定“合并无代价即可换代”，后续需先补来源覆盖
审计和负对照门槛。

### 公开漏词分诊

`diagnose-public-miss` 把一个明确声明为公开的完整双拼目标同时放到四个可核验
层次中：固定 Rime 来源是否含相同文字与规范码、日用核心是否有完整词、补充包
是否有完整词、两层合在一起最少需要几个完整分段，以及目标在与 TSF 相同的公开
分层候选前 50 中实际位于哪里。它要求明确的 `--public`，拒绝私人候选包，只
输出调用者显式给出的目标，不读取许愿、会话、个人排序或上下文：

```powershell
target\release\candidatectl.exe diagnose-public-miss `
  --source .local\public-audit\wanxiang-fdda7afb\jichu.dict.yaml `
  --core-package .local\candidate-rime-pinyin-simp-0c6861ef-v1 `
  --supplemental-package .local\public-audit\wanxiang-fdda7afb\package-bigram-cover-plus-phrase-top10k-v1 `
  --code wutijc `
  --text 误提交 `
  --public
```

固定来源 SHA-256 为
`9d14c0c49588d63b16c554df4711bed5da822c63de9d50f4759c53542138ac00`。
三条公开案例给出三个不同原因：

- `bgdr → 绷断`：来源与补充包都有完整同码词，公开分层候选第 1；
- `blklrbsv → 掰开揉碎`：来源有完整词，两层包都没有；现有三段组合仅第 22；
- `wutijc → 误提交`：来源和两层包都没有完整词，但可由两段完整词组成，仅第 14。

因此前者已经解决，第二条是构建选择缺口，第三条是组合召回/排序问题。分诊只
陈述固定材料下的证据，不把“来源出现”当成收录建议，也不把 Top-50 外误报成
“无法生成”。

随后新增的 `phrase-layer-audit` 会在内存中建立基础、5,000 条和 10,000 条三个
真实 `CandidateSnapshot`，让新增目标走现有交互候选合并路径。它把新增目标与
基础稳定性对照分开，完整检查所有 10,000 条配额命中的 UD 四字目标；基础对照
则在训练和留出侧各使用最多 128 个、按稳定词面哈希固定选择的公开样本，避免
审计工具无界重复完整解码。计时另取最多 48 个公开完整码并先预热：

```powershell
target\release\candidatectl.exe phrase-layer-audit `
  --source .local\public-audit\wanxiang-fdda7afb\jichu.dict.yaml `
  --allowlist .local\public-audit\wanxiang-fdda7afb\chengyu.txt `
  --base-payload .local\public-audit\wanxiang-fdda7afb\package-top76300-plus-bigram-cover-v2\lexicon.tsv `
  --fit-corpus data\public\ud-chinese-gsdsimp\zh_gsdsimp-ud-train.conllu `
  --held-out-corpus data\public\ud-chinese-gsdsimp\zh_gsdsimp-ud-test.conllu `
  --small-limit 5000 `
  --large-limit 10000 `
  --repetitions 5
```

同机 release 结果中，5,000 条层让训练侧 71 个目标中的 57 个进入 Top-10，
10,000 条层为 71/71；独立 UD test 分别为 5/7 和 7/7，而且七个 10,000 条目标
都成为首选。训练侧 96 个、留出侧 17 个基础包四字对照没有候选顺序变化、首选
变化、目标降名次或掉出 Top-10。

5,000/10,000 条短语层载荷分别为 165,602/331,550 字节，紧凑索引节点为
14,083/26,948，隐式拼写为 80,000/160,000；本机构建约 17.5/40.8 ms。
48 个预热公开码各重复五次时，基础、5,000 和 10,000 条路径的 median 分别约
34.17/33.88/33.68 ms，P95 约 42.84/44.07/42.58 ms，分布没有显示稳定的层间
延迟差异。这些结构计数不是实际堆内存，候选查询也不是 TSF 窗口首帧；宿主加载
后的进程增量、首次绘制和具体实验包的三材料逐字节验证仍是发布前门槛。因此本轮只能说明
10,000 条在该公开留出上有额外召回且未触发固定负对照，不能据此直接换代。

```powershell
cargo run --release --bin candidatectl -- compare `
  --base-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --challenger-payload .local/public-audit/wanxiang-fdda7afb/package-top100k-v1/lexicon.tsv
```

对照只报告词形、文字/规范码身份、共同规范码、同码首选变化，以及“对照首选
本身也被基线同码确认”的可校准数量；后者继续按它在基线真实完整码候选顺序中
原本位于第 2、第 3～6 或第 7 名以后分桶。并列频率使用与完整码解码一致的稳定
文字次序，重复文字/规范码身份只计一次。报告不显示候选文字，也不声称两个来源
的原始权重可以直接合并。

真实记录中的首次非首选压力按键长集中在 1～4 键，但 `consensus-audit` 按输出
文字长度汇总，不能区分“规范码还没输入完”和“完整码核心同码次序”。先用独立
公开 UD 留出做只读的短输入审计：

```powershell
cargo run --release --bin candidatectl -- short-rank-audit `
  --core-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --held-out-corpus data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu `
  --frontier-limit 6
```

命令只选择核心词典可确认、且词面只有一个规范码的公开 1～2 字 token。1～4
个逐键前缀分别报告“已经完成规范码”和“仍在输入”的目标首选/可见数量；后者
明确称为预览，不把尚未表达完的目标冒充用户当前意图。完整码另按“按键数 ×
文字数”报告目标在核心精确词中的名次，并按同码精确候选宽度 `1 / 2～6 / ≥7`
分桶。报告不显示词面，不读取个人记忆，不写文件或候选槽位。

固定 65,116 条核心包和同一 UD test 实际留下 2,850 个无多音歧义目标。两键
单字中，165/500 是首选、423/500 在前六；494/500 个目标所在的规范码至少有
七个精确候选。四键双字中，1,978/2,350 是首选、2,343/2,350 在前六；同码
唯一、2～6 个及至少 7 个精确候选的目标分别为 1,209、1,076 和 65 个。尚未
完成的两键双字在第二键只有 1/2,350 个目标进入前六，到第三键则有
1,950/2,350 个进入前六、1,058 个成为首选。因此这份描述性证据把两类压力
分开了：单字完整码主要面对宽同码集合，双字在第二键主要只是目标尚未完成，
而四键完整双字的核心覆盖和前六可见性已经很高。UD token 频次并不等于独立
输入时的真实意图，而且这份 test 已经被观察过；结果只能定位下一轮公开协议，
不能直接拿来重排日用候选。

### 公开连续短语少分段反事实

`segment-penalty-audit` 检查一种比逐词修补更一般、但仍很弱的假设：在现行
句子解码 Top-50 已经生成以后，每多一个词界统一扣少量分，能否让较完整的词段
越过同码的多段单字组合。它冻结原候选池，只比较
`0 / .010 / .025 / .050 / .100 / .250 / .500` 七档，不创建新候选、不读取
私人记录，也不按目标词面逐例调参：

```powershell
target\release\candidatectl.exe segment-penalty-audit `
  --core-payload .local\candidate-rime-pinyin-simp-0c6861ef-v1\lexicon.tsv `
  --fit-corpus data\public\ud-chinese-gsdsimp\zh_gsdsimp-ud-train.conllu `
  --held-out-corpus data\public\ud-chinese-gsdsimp\zh_gsdsimp-ud-dev.conllu `
  --frontier-limit 6 `
  --sample-limit 128
```

固定核心 SHA-256 为
`fec5d5173127d568a047655b2f92a94c4e546c91565d7fd14808fbf71266b834`；
train 与 dev 的 SHA-256 分别为
`956636fe612a1166e8b19e7413fee2e73d68231aca2f0455be2c616b947d629d` 和
`d03f1eeb93b16071bfbbe6c76b971554be87c9a2307b3f3a820dd7c07f73fb63`。
两侧各按公开相邻双词选取 128 个句代表。train 基线首选为 83/128、Top-6
为 114/128；`.025 / .050 / .100` 只各改善一个目标的池内名次，没有新增
正确首选。`.250 / .500` 各新增一个正确首选并让一个目标进入 Top-6，但也各
改变了一个仍不正确的非目标首选。严格拟合规则因此选择 `0.000`。dev 基线为
首选 78/128、Top-6 114/128；因为拟合侧没有安全的非零档位，最终安全门明确
未通过，运行时排序保持不变。

这个结果说明类似“捕获到”被三段碎片轻微压住的局部现象真实存在，却否定了
全局固定少分段惩罚。下一轮若继续，应要求更有条件的公开词界证据或完整词结构，
并继续冻结候选池、保留非目标首选零变化的门槛；不能把一次局部改善写成通用
排序规则。

静态冲突数量不能证明校准有益，因此还要在不参与词典构建和规则选择的独立公开
UD test 上比较普通分层与共识校准。审计只保留核心词典能够确认、且词面只有一个
规范码的 1～4 字 token，避免把语料没有标注的多音读法冒充排序错误：

```powershell
cargo run --release --bin candidatectl -- consensus-audit `
  --core-payload .local/candidate-rime-pinyin-simp-0c6861ef-v1/lexicon.tsv `
  --supplemental-payload .local/public-audit/wanxiang-fdda7afb/package-top100k-v1/lexicon.tsv `
  --held-out-corpus data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu `
  --frontier-limit 6
```

报告分别统计校准前后正确首选、正确首选新增/丢失、非目标首选变化和目标名次移动，
不显示词面。只有至少新增一个正确首选、正确首选零损失且非目标首选零变化时，安全
门才通过；实例权重同时报告但不能掩盖任何一个唯一词面损失。命令不写文件、不改
候选槽位，也不把一次公开留出结果等同于真实用户意图。

固定 65,116 条核心包、100,000 条万象补充包和从未参与规则选择的 UD test
（500 句、12,010 个句法 token）实际评测了 2,909 个无多音歧义词面。校准让
51 个词面新增正确首选，却让 57 个原本正确的首选丢失，另有 27 个非目标首选
变化；按语料实例计分别为 62、109 和 43。正确首选从 2,202/2,909 降到
2,196/2,909，因此安全门明确失败。当前 TSF、`runtime-query` 和公开漏词分诊
继续使用保守分层；共识重排只留在离线审计函数中，不会随普通换代进入日用。
这份 test 结果不再用于反复调参；若继续研究，必须先冻结新的 fit 协议并保留另一份
独立材料作一次性安全门。

按核心精确同码宽度交叉后，同码唯一的 1,268 个目标完全没有首选变化；2～6 个
精确候选的 1,082 个目标承载了 126 次变化，其中正确首选新增 51、丢失 55、
非目标首选变化 20；至少 7 个精确候选的 559 个目标有 9 次变化，却只有 2 次
正确首选丢失和 7 次非目标变化，没有新增正确首选。结合上面的文字长度分桶，
公开共识既没有触及单字宽同码次序，在中等宽度双字组也仍是净损失，而进入最宽
同码组时只观察到伤害。因此不能靠简单限制文字长度或同码宽度挽救这条规则。

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
TSF 首次建立候选蓝图后复用已验证的核心和初始补充索引。补充状态未变化时，
新组合只读取小型状态和槽指针，不重新打开 100k 词载荷；只有指向新包时才建立
新索引，同包仅改变影响上限时复用原索引。

同一命令还对 256 个按规范码去重的公开四字词分别合成邻键替换、相邻换序、
少按一键和多按一键。固定 120k 包上，四类分别有 250、254、240、254 个唯一
恢复提示；其余均因原码已有完整词或多规范码歧义而保守拒绝，错误目标码和原码
保护失败均为 0。少键审计曾发现“精确简拼先覆盖同词条完整码少键证据”的筛选
顺序问题；修复后 256 个样本由 112 个目标可见、4 个错误目标码变为 240 个目标
可见、0 个错误目标码。四类结果分开报告，不再用换序样本代表整个错误通道。

## 独立公开补充根

日用 TSF 的补充根固定在跨 DLL 版本共享的
`.local/tsf-alpha/user-data/public-supplement`。它使用现有公开候选包、来源、
SHA-256、预检和三槽机制，但另有严格四行的 `supplemental.zcl` 显式开关。
根或开关不存在时默认关闭。首次载入若状态损坏、所绑定包不再是 current、包或
预检失效，则只使用核心候选；已有文本服务遇到损坏更新时保留最后已知可用的
补充快照，等待后续有效状态，不让坏包中断输入。

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

装有热刷新支持的文本服务会在空组合的下一次字母首键前观察启用、关闭或重新
确认的包；已经开始的组合始终用原快照。指针不变时不会重新读取大词典。新状态
和包必须完整通过既有来源、摘要、预检与 current 绑定校验后才替换内存快照；
失败时保留最后有效版本。补充根中的 current 提升或回退后，旧
`supplemental.zcl` 与新 current 不一致，必须再次显式运行 `supplement-enable`
确认目标包。尚未装入该 DLL 能力的旧宿主仍需重新打开。

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
  --exact-short-root .\.local\tsf-alpha\user-data\public-exact-short `
  --code dago `
  --limit 10
```

它包含公开核心/补充分层、公开整词纠错和显式提供的精确短词层。精确短词会按
TSF 固定六项页宽重放 `6 → 12 → 18 …` 的惰性扩页，新增身份带
`〔公开精确短词〕` 标记；因此不会再把“主层第 12 名”误报成启用精确层后的真实
展示位次。它仍不伪装成完整实机现场：显式别名、项目覆盖、会话记忆、个人学习
和左侧上下文均明确排除。

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

快照还提供一个更窄的四字完整词单次纠错证据接口。它只接受目标为四个汉字、
四个完整双拼音节的公开词典项，并沿用解码器唯一一次的全局错误预算；简拼、
普通句子分段、未解析输入和第二次纠错都被排除。接口返回目标文字、拼音、八键
规范码、具体纠错操作和评分拆解，也不把所有四字词宣称为“成语”。TSF 只在
原码没有四字完整词、核心与补充层的一次编辑结果共同指向唯一规范码时，才把该
码的首个公开整词放在普通首选之后；多个目标码一律保持原候选。它不越过显式
别名或项目词，不自动提交，也不抢首选。

五键和七键还启用一个独立、更加保守的短词多按恢复门：目标只能是两个或三个
完整双拼音节，唯一编辑只能删除一枚与前后至少一键互为 QWERTY 邻键的多余键。
核心包和补充包分别按各自内部顺序选择最佳规范码，只有两边一致时才把该码的
首个公开整词放到第二项；缺少补充层、任一层无证据、两层意见不同或多按键不邻接
时都保持普通候选。之所以暂不抢首选，是因为奇数长度也可能真实表达“完整词 +
下一音节首字母”，例如“辛苦吃”；后续若要升首位，必须另加因果按键时序证据。
许愿快照沿用历史 tag 10，但面向用户统一显示为“公开整词纠错”，以免把这条来源
错误地继续描述成只有四字。

固定万象覆盖包的公开 release 审计从高频四字完整词中按规范码去重选取 256 条，
对每条合成一次音节内相邻换序：254 条获得唯一恢复，目标文字在 254 条中均为
恢复首项；另外 2 条因同时指向多个规范码而安全拒绝。错误目标码为 0，原码整词
保护失败为 0。同机预热的 15 次固定纠错查询 median 4.790 ms、p95 9.466 ms；
这是一次实验机诊断，不是跨设备延迟保证。

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
