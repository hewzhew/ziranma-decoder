# 用户态工具独立刷新

## 目的

TSF DLL 被 Codex、Typora 等宿主加载后，Windows 不允许安全地原地覆盖或强制
卸载它。许愿管理、反馈分析、别名管理等独立程序却不需要跟着 DLL 一起换代。
`refresh-ime.cmd` 因此只更新这些用户态工具，不注册输入法、不改变当前用户
TSF 状态，也不要求关闭正在输入的宿主。

```powershell
.\refresh-ime.cmd
.\refresh-ime.cmd status
.\refresh-ime.cmd space
.\refresh-ime.cmd cleanup
.\refresh-ime.cmd rollback
```

默认命令使用 `Cargo.lock` 和 `--offline`，在
`.local/tsf-alpha/user-tools/cargo-target` 隔离构建：

- `aliasctl`、`aliaspad`；
- `candidatectl`；
- `personalctl`；
- `researchctl`；
- `wishctl`、`wishpad`；
- `ziranma-launcher`。

这里没有 `--lib`，因此不会生成、替换或加载 `ziranma_core.dll`。依赖没有预先
缓存时构建会直接失败，不回退联网。

## 不可变版本槽

八个 EXE 的 SHA-256 写入一个规范清单；清单自身的 SHA-256 是包标识。工具包
只增不改地进入 `user-tools/builds/<sha256>`，`slots.zut` 只保存 `current` 和
`previous`。状态文件使用同目录临时文件原子替换：

- 首次成功构建建立 `current`；
- 后续不同构建把旧 `current` 留作 `previous`；
- 相同构建重复刷新不改指针；
- `rollback` 完整验证两个包后交换指针；
- 构建、复制、摘要或状态校验失败时，原 current 不变。

`status` 不创建目录、不构建、不读取别名、个人排序、研究批次或许愿正文，只
复核工具槽、清单和 EXE 摘要。`space` 同样只读，逐项报告隔离 Cargo 缓存、
不可变工具包、current/previous、未引用工具包，以及不属于当前布局的根目录或
builds 条目的逻辑大小。“潜在可回收”只统计未引用包，但明确提示尚未检查进程
占用；仍用于回滚的两个包、会影响下次构建速度的 Cargo 缓存和来源不明的条目
都不算进去。它遇到重解析点会拒绝给出含糊结果，也不会删除任何文件。

旧包仍不自动删除，避免刷新动作误伤仍在运行的管理器。需要回收磁盘时先运行
`space` 留下可核对的基线。`cleanup` 是独立的显式删除操作，只处理规范摘要目录
中既非 current 也非 previous 的完整工具包。它先完整验证受保护槽和所有待删除
包，再检查八种受管工具的运行路径；仍被进程使用的包跳过，无法检查进程路径、
重解析点、损坏包或执行中状态漂移都会关闭失败。每个包在删除前再次核对身份和
进程，删除后再次验证 current/previous 未变。

`cleanup` 不处理 Cargo 缓存、来源不明的 builds 条目或其他根目录，也不关闭任
何程序。删除的旧包不再能直接恢复，但 current/previous 回滚能力保持不变，且
后续仍可从对应源码重新构建。正常 `refresh` 永远不会隐式调用清理。

`alias-ime.cmd`、`candidate-data.cmd`、`personal-ime.cmd`、`research-ime.cmd`
和 `wish-ime.cmd` 通过固定白名单解析 current。还没有工具槽时，它们兼容回退
到 `target/release`；一旦槽状态存在但损坏，则失败并提示运行只读状态，不静默
执行不明版本。已经打开的 `aliaspad` 或 `wishpad` 不会被关闭；退出后重新打开
才使用新的 current。

刷新成功后，还会先把已验证包里的 `ziranma-launcher.exe` 原子复制到固定位置
`.local/tsf-alpha/desktop-launcher`，再切换 current。固定文件使桌面快捷方式不必
追随内容寻址目录；复制失败时 current 保持不变。启动器的 `wish` 和 `alias` 模式
重新验证槽、完整清单、每个 EXE 摘要和包内文件集合，再直接打开 GUI，因此不会
闪出批处理窗口；`update` 只打开仓库固定的 `update-ime.cmd`，仍使用可见控制台。
它不接受任意程序或路径。回退只交换工具槽，稳定启动器保持新版并兼容旧的七工具
清单，因而桌面入口不会随回退失效。

## 哪些能力会立即变化

| 能力 | 用户态刷新后的生效边界 |
|---|---|
| 许愿查看、整理和反馈分析界面 | 关闭旧管理器后再次打开 |
| `researchctl review` 等离线分析 | 下一次命令立即使用 current |
| 显式别名的内容指针 | 文本服务在空组合的下一次首键前检查，不改变活动组合 |
| 持续研究开关 | 已加载文本服务按既有的一秒授权轮询发现 |
| 个人排序与忘记动作 | 仍按既有焦点/输入边界刷新；这里只更新管理工具程序 |
| 核心候选快照 | 当前类工厂已冻结，宿主重新加载 DLL 后才读取 |
| 公开补充候选快照 | 装入热刷新能力后，空组合的下一次首键前检查小指针；坏包保留最后有效版本 |
| 按键、候选窗 UI、新采集字段 | 必须经过明确的 `update-ime.cmd` DLL 换代 |

因此，这个入口解决的是“分析和管理程序不必等待所有宿主退出”，不把 Windows
不支持的 DLL 热卸载伪装成已经完成。公开补充层的数据指针可由已经装入对应能力
的宿主在组合之间观察；核心词典、代码和 UI 仍不能跨宿主热换。

## 隐私与权限

工具槽只含程序、无正文摘要和版本指针，位于 Git 忽略的 `.local`。刷新过程
不访问 `aliases`、`personal-ranking`、`research-inbox` 或 `wishes`，不连接
网络，不写模型，不请求管理员权限，也不改变微软拼音或默认输入法。
