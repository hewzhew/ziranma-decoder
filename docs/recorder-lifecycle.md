# 持续记录器的本地换代协议

## 为什么不追求双实例零停机

这是单人本地输入记录器，不是承接远程请求的服务器。两个版本同时监听会
争用同一组全局热键、重复捕获同一按键，并可能把一个真实动作保存两次。
因此这里的“高可用”不是双实例重叠，而是：

- 当前会话能完整排空；
- 换代间隙短、可见且不回填；
- 候选版先只读预检；
- 新版失败时能回到上一已知良好版本；
- 每段数据能追溯到实际生产者版本和采集口径。

几十秒的明确维护窗口比两套监听器并行更安全。

## 五个角色

1. **源码状态**：通过格式、严格静态检查和完整测试的代码。
2. **候选版**：从该源码构建、尚未接管记录的不可变二进制。
3. **当前版**：下一次 `--run` 使用的二进制。
4. **上一良好版**：最近一次已通过真实会话验证、可用于回滚的二进制。
5. **会话数据**：只追加的 DPAPI 加密段；换二进制不迁移、不重写旧段。

`producer_version` 区分实现代次；只有捕获语义变化时才提升
`capture_profile`。普通性能优化不能伪造新的 profile。

## 一次安全换代

```text
构建候选版
  → 候选版 --check（只读、无监听、无写盘）
  → 当前版继续记录
  → 用户选择边界并按 F12
  → 当前版刷新最后非空段并打印 STOPPED
  → 提升候选版，保留上一良好版
  → 正式版再次 --check
  → 启动新会话
  → 用首个完成段做健康观察
```

当前版没有出现 `STOPPED flushed=true` 时，不应覆盖或提升。候选版预检失
败时，旧版继续运行，不进入维护窗口。正式版预检失败时，不启动采集，并
把当前指针退回上一良好版。

## 会话边界就是部署边界

换代一定产生新的会话号。旧会话保留旧 `producer_version`；新会话写入
新版本。两者可以共享 `capture_profile`，但报告必须保留版本分组。这样
能把“程序变了”和“宝宝这次输入内容不同”分开解释。

暂停不是部署边界：`Ctrl+Shift+F10` 仍属于同一进程和同一会话，只用于
临时不采集。换代使用 `Ctrl+Shift+F12`，因为它会解除监听、刷新和关闭
进程。

`codex-uia-v1` → `codex-uia-v2` 是一次真正的采集口径升级，因此必须按
上述会话边界换代：先让含 v1/v2 双读能力的回放器通过测试，再 stage v2
记录器；旧记录器继续写完自己的 v1 会话，`drain` 后才能 `promote` 并
启动全新 v2 会话。历史 `.zcs` 不迁移、不重写。若 v2 真实会话异常，
排空后可 `rollback` 到上一记录器；新版回放器仍能同时解释已经完成的
v1 和 v2 段，但完整性报告会分开显示可用覆盖度。

通过源码检查并选定会话边界后，v1 → v2 的完整 PowerShell 顺序如下。
`stage` 和第一次 `status` 不会打断当前记录器；从 `drain` 开始才进入短暂
维护窗口：

```powershell
Set-Location -LiteralPath 'C:\path\to\ziranma-decoder'
cargo build --release --bin codex-recorder --bin recorderctl --bin capsule-replay
.\target\release\recorderctl.exe stage .\target\release\codex-recorder.exe
.\target\release\recorderctl.exe status

# 只在准备结束旧会话时继续执行以下四行
.\target\release\recorderctl.exe drain
.\target\release\recorderctl.exe promote
.\target\release\recorderctl.exe run --session-kind daily --background
.\target\release\recorderctl.exe status
```

若 `drain` 没有报告正常刷新退出，就停在原地排查，不运行 `promote`。
`promote` 报错时先运行 `status`，不要盲目 `rollback`：只有状态确认
v2 已成为 current 且没有记录器进程，或新版在成功提升后启动失败，才用
`recorderctl rollback` 恢复上一槽位，再启动新的回滚会话。任何失败都
不应通过启动第二个监听器来掩盖。

## 已实现的 `recorderctl`

控制器是仓库中的独立小程序，不是系统服务。先构建：

```powershell
cargo build --release --bin recorderctl
```

它提供以下显式操作：

- `recorderctl status`：用中文报告当前、候选、上一良好版本、进程和下一步；
- `recorderctl status --machine`：输出稳定的逐行机器字段；槽位只显示
  已验证的安全文件名，进程路径固定为 `redacted`；
- `recorderctl adopt <codex-recorder.exe>`：仅用于第一次收编已知良好版；
- `recorderctl stage <candidate.exe>`：复制为不可变候选并执行 `--check`；
- `recorderctl promote`：仅在所有记录器退出后把 candidate 提升为 current；
- `recorderctl rollback`：仅在所有记录器退出后交换 current/previous；
- `recorderctl run --session-kind daily`：从 current 槽前台运行；
- `recorderctl run --session-kind daily --background`：从 current 槽静默后台运行；
- `recorderctl drain`：请求唯一受管理进程正常刷新退出，不强杀。

本地槽位保存在 Git 忽略的 `.local/recorder/`：

```text
.local/recorder/
  slots-v1.txt
  active-v1.txt
  builds/
    codex-recorder-build-<时间>-<控制器进程>.exe
    codex-recorder-build-<时间>-<控制器进程>.exe.meta
```

构建文件一经安装不再覆盖；`slots-v1.txt` 只保存三个安全文件名。提升和
回滚用同目录临时文件、落盘同步和 Windows replace-existing/write-through
替换这一个状态指针。构建元数据只含 `producer_version`、
`capture_profile`、可选控制状态 schema 和二进制字节数，不含输入文字
或按键。

声明支持 `ziranma-recorder-active-v1` 的 current 版由控制器显式附加
`--control-state`。记录器把会话号、PID、类别、版本、开始时间、运行/
暂停/停止状态、Codex 目标是否连接、已完成分段/事件数及最近刷新时间
原子发布到 `active-v1.txt`。它不含正文、拼音或按键；只在启动、目标连接
变化、暂停/恢复、非空加密分段完成和停止时更新，不做定时心跳。旧版本
没有该能力时，`status` 明确显示“尚未启用”，不会猜测或扫描私有目录。

`status` 默认把内部槽位翻译为“当前版本、待升级版本、可回退版本”，并
根据是否正在运行及有无候选给出下一条安全操作；有 `active-v1` 时还显示
当前会话、时长、连接、保存计数和刷新时间。只有仍在运行或暂停的当前
会话才显示输入框连接状态；已经结束的历史会话不再把停止前的
`connected`/`waiting` 冒充当前状态，其时间也显示为“开始于多久前”，
而不是继续增长的运行时长。`--machine` 仍保留原始
schema/slot/session/process 字段，供本地脚本解析，但不公开绝对路径；这些
会话、时长和数量仍是行为元数据，不宜原样分享。两种模式在未配置时都不会创建目录；
它们不初始化 UIA，不读取
`data/private/`，也不写文件。`stage` 会在复制完成后对复制品运行只读
`--check`，确认至少一个精确 Codex 目标、版本和采集口径，再更新 candidate
指针。候选预检失败时临时文件会清理，原槽位保持不变。

`promote`、`rollback` 和 `run` 只要发现任何 `codex-recorder.exe` 就拒绝，
避免热键争用和重复采集。`drain` 更保守：必须恰有一个进程，而且可执行
路径必须是仓库正式版或三个受管理槽位之一；它只向该 PID 的消息线程投递
记录器自己的 F12 停止标识，15 秒内未自行退出就报错，不调用强制终止。

从 `continuous.6` 起，运行状态由生命周期守卫兜底。正常收尾发布
`stopped`；记录器自己返回的错误以及能够展开栈的 Rust panic 尽力发布
`failed`，即使最后一次刷新、计数或状态回写中的较早步骤已经失败，也不会
主动跳过后续状态标记。若进程已经消失而状态仍为 `running`/`paused`，
`status` 只报告“没有正常退出回执（外部结束或不可观测中止）”，不把强杀、
进程 abort、系统终止或状态存储失效中的任何一种猜成既定原因。活动状态
仍只含固定的脱敏计数与生命周期字段，不记录错误正文、输入内容或按键。

控制器不下载、不联网、不自动提升、不安装启动项，也不在宝宝没有发出
`drain` 或热键时结束会话。

## 自动化的边界

可以自动化候选构建、只读预检、原子提升和失败回滚。不能自动决定“现在
正好适合结束宝宝的会话”，也不能为追求零间隙而启动第二个捕获器。最终
排空由宝宝按 F12，或明确运行 `recorderctl drain`；两条路径都会让记录器
自己完成解绑、加密刷新和退出。
