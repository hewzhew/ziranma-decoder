# 开源边界审计：MPL-2.0 已应用

审计日期：2026-07-29（初始公开边界建立于 2026-07-27）。

## 当前结论

维护者已经确认采用 MPL-2.0。仓库根 `LICENSE` 保存 Mozilla 官方完整
文本，`Cargo.toml` 使用 SPDX 标识 `MPL-2.0`，README 附有 Exhibit A
通知。许可声明范围是：

> 除文件或相邻目录另有说明外，本项目原创的源代码、测试、文档和配置采用
> MPL-2.0。`data/public/` 中的第三方快照继续采用各自上游许可证；构建
> 输出与私人运行数据不属于公开发行内容。

这是工程边界审计，不代替律师针对具体发行方式给出的法律意见。

维护者在 2026-07-27 确认：自己有权以 MPL-2.0 许可准备纳入公开仓库的
项目原创材料。该确认不改变第三方快照的原许可证，也不把任何私人运行
数据纳入发行范围。

## 四类材料

| 类别 | 当前内容 | 建议处理 |
| --- | --- | --- |
| 项目原创材料 | `src/`、`tests/`、`docs/`、README、Cargo 配置和仓库规则 | MPL-2.0 |
| 第三方依赖 | crates.io 的 `windows*`、RustCrypto `sha2`、`ed25519-dalek`、过程宏及其传递依赖 | 不复制其源码；保留 Cargo 锁定与上游 MIT/Apache/BSD/Unicode 许可 |
| 第三方公开数据 | Rime 字典、UD GSDSimp、Conway Stroke Data | 逐目录保留原许可证、署名、固定提交、来源、校验和与转换账目 |
| 私人和本地材料 | `data/private/`、`data/raw/`、`logs/`、`models/private/`、`.local/` | 永不纳入项目许可证、Git 历史或公开发行包 |

选择 MPL-2.0 不会把第三方数据重新许可为 MPL。以后新增 GPL、LGPL、APL
或其他资料时，仍须按具体文件、组合方式和发行方式单独审计。

## Git 与隐私事实

在 2026-07-27 开始边界审计时：

- 没有配置 Git 远端；
- 当时已有的历史为 26 个提交和一个唯一提交者身份；
- 历史对象路径中没有
  `data/private/`、`data/raw/`、`logs/`、`models/private/` 或 `.local/`；
- 当前 Git 忽略状态确认 `.local/` 与 `data/private/` 被排除；
- 已跟踪文件名中没有环境文件、私钥、数据库或上述私人目录；
- 对已跟踪内容的常见私钥头和 AWS 访问密钥形状扫描没有命中。

前两项是可追溯的审计起点，不声称公开后的克隆仍然没有远端或仍然只有
同一组提交元数据。首次公开前的新增提交继续接受相同的路径、内容、身份
和干净工作树检查。

这些检查降低误发风险，但不是对任意秘密格式的数学证明。首次公开前仍应
从准备发布的精确提交重新执行完整历史、文件名、内容和打包清单检查。

现有 Git 提交元数据包含维护者昵称和旧邮箱。维护者决定保留这 26 个提交
及其哈希，不重写历史，并接受旧邮箱仍可从公开提交对象中读取。当前仓库
已使用仅作用于本仓库的 Git 配置，让未来提交沿用公开昵称并改用 GitHub
提供的 ID 型 `users.noreply.github.com` 地址；全局 Git 身份没有改变。

## 依赖边界

`cargo metadata` 在审计时报告：

- `windows`、`windows-core` 及其传递依赖：MIT OR Apache-2.0；
- RustCrypto `sha2`、`digest`、`block-buffer`、`crypto-common`、
  `cpufeatures` 及相关支持依赖：MIT OR Apache-2.0；`generic-array`：MIT；
- `ed25519-dalek` 与 `curve25519-dalek`：BSD-3-Clause；`ed25519` 和
  `signature`：Apache-2.0 OR MIT；`subtle`：BSD-3-Clause；
  `fiat-crypto`：MIT OR Apache-2.0 OR BSD-1-Clause；
- `curve25519-dalek-derive`、构建期 `rustc_version` 与 `semver`：
  MIT OR Apache-2.0；
- `proc-macro2`、`quote`、`syn`：MIT OR Apache-2.0；
- `unicode-ident`：`(MIT OR Apache-2.0) AND Unicode-3.0`。

`ed25519-dalek` 关闭默认功能；运行时代码只验签，不启用随机密钥生成，也没有
私钥文件接口。上述依赖源码没有复制进仓库。该组合没有显示出阻止项目原创
Rust 代码采用 MPL-2.0 的许可冲突，但发行二进制时仍需生成依赖许可清单并随包
提供必要通知。

## 数据包边界

### Rime pinyin-simp

- 固定提交：`0c6861ef7420ee780270ca6d993d18d4101049d0`
- 许可：Apache-2.0
- 本地保留：完整许可、作者、来源、SHA-256 与导入规则
- 结论：独立第三方数据，不纳入 MPL

### UD Chinese GSDSimp

- 固定提交：`4231dfd59866fa5999ad4a6bc1fdecd7985b3b59`
- 许可：CC BY-SA 4.0
- 本地保留：上游声明、README、完整 CC BY-SA 4.0 法律文本、来源、
  SHA-256 与行数账目
- 限定：上游只明确许可 UD 标注，并声明不主张底层内容的所有权
- 结论：独立 ShareAlike 数据，不纳入 MPL

### Conway Stroke Data

- 固定提交：`4449c63198292fd36d68d8068d39641bb6bbf86d`
- 许可：CC BY 4.0
- 本地保留：完整许可、上游 README、来源、SHA-256、转换和导入账目
- 结论：独立署名数据，不纳入 MPL

三个快照目录均由 `.gitattributes` 标为 `-text`，避免跨平台检出时自动
更改换行并破坏“逐字节固定”与校验和。

## 自动发布审计

仓库已经加入 `scripts/release-audit.ps1`。它审查已跟踪文件与未被忽略的
未跟踪候选，核对历史对象路径、禁止发布的目录和文件类型、常见秘密形状、
本机路径、个人邮箱、必要许可证文件与固定公开快照的 SHA-256。它不访问
网络，也不扫描 Git 已忽略的私人目录；历史检查只读取路径名，不读取历史
文件内容。

准备阶段可以在有未提交改动时运行；最终发布候选必须在干净工作树上使用
`-RequireClean`。当前准备工作树已经通过非干净模式审计，最终模式会按
设计拒绝尚未整理成提交的改动。

## 公开发布门

本机示例路径已经参数化，`CONTRIBUTING.md`、隐私边界和发布检查脚本也已
加入。任何公开远端或发行包都必须依次通过：

1. 审阅准备发布的改动，形成一个精确、可重复测试的提交；
2. 对该精确提交重新运行格式、Clippy、测试、许可与隐私扫描，并以
   `-RequireClean` 通过发布审计；
3. 只有前两项通过后，才创建公开远端或上传发行包。

`publish = false` 可以继续保留：它只阻止误发 crates.io，不阻止以源码仓库
形式开源。
