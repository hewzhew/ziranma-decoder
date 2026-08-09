# 用户态工具独立刷新

## 目的

TSF DLL 被 Codex、Typora 等宿主加载后，Windows 不允许安全地原地覆盖或强制
卸载它。许愿管理、反馈分析、别名管理等独立程序却不需要跟着 DLL 一起换代。
`refresh-ime.cmd` 因此只更新这些用户态工具，不注册输入法、不改变当前用户
TSF 状态，也不要求关闭正在输入的宿主。

```powershell
.\refresh-ime.cmd
.\refresh-ime.cmd status
.\refresh-ime.cmd rollback
```

默认命令使用 `Cargo.lock` 和 `--offline`，在
`.local/tsf-alpha/user-tools/cargo-target` 隔离构建：

- `aliasctl`、`aliaspad`；
- `candidatectl`；
- `personalctl`；
- `researchctl`；
- `wishctl`、`wishpad`。

这里没有 `--lib`，因此不会生成、替换或加载 `ziranma_core.dll`。依赖没有预先
缓存时构建会直接失败，不回退联网。

## 不可变版本槽

七个 EXE 的 SHA-256 写入一个规范清单；清单自身的 SHA-256 是包标识。工具包
只增不改地进入 `user-tools/builds/<sha256>`，`slots.zut` 只保存 `current` 和
`previous`。状态文件使用同目录临时文件原子替换：

- 首次成功构建建立 `current`；
- 后续不同构建把旧 `current` 留作 `previous`；
- 相同构建重复刷新不改指针；
- `rollback` 完整验证两个包后交换指针；
- 构建、复制、摘要或状态校验失败时，原 current 不变。

`status` 不创建目录、不构建、不读取别名、个人排序、研究批次或许愿正文，只
复核工具槽、清单和 EXE 摘要。旧包暂不自动删除，避免清理动作误伤仍在运行的
管理器；磁盘回收要等有明确的进程与保留策略后再设计。

`alias-ime.cmd`、`candidate-data.cmd`、`personal-ime.cmd`、`research-ime.cmd`
和 `wish-ime.cmd` 通过固定白名单解析 current。还没有工具槽时，它们兼容回退
到 `target/release`；一旦槽状态存在但损坏，则失败并提示运行只读状态，不静默
执行不明版本。已经打开的 `aliaspad` 或 `wishpad` 不会被关闭；退出后重新打开
才使用新的 current。

## 哪些能力会立即变化

| 能力 | 用户态刷新后的生效边界 |
|---|---|
| 许愿查看、整理和反馈分析界面 | 关闭旧管理器后再次打开 |
| `researchctl review` 等离线分析 | 下一次命令立即使用 current |
| 显式别名的内容指针 | 文本服务在空组合的下一次首键前检查，不改变活动组合 |
| 持续研究开关 | 已加载文本服务按既有的一秒授权轮询发现 |
| 个人排序与忘记动作 | 仍按既有焦点/输入边界刷新；这里只更新管理工具程序 |
| 核心及公开补充候选快照 | 当前类工厂已冻结，宿主重新加载 DLL 后才读取 |
| 按键、候选窗 UI、新采集字段 | 必须经过明确的 `update-ime.cmd` DLL 换代 |

因此，这个入口解决的是“分析和管理程序不必等待所有宿主退出”，不把 Windows
不支持的 DLL 热卸载伪装成已经完成，也不声称当前公开候选数据已经跨宿主热换。

## 隐私与权限

工具槽只含程序、无正文摘要和版本指针，位于 Git 忽略的 `.local`。刷新过程
不访问 `aliases`、`personal-ranking`、`research-inbox` 或 `wishes`，不连接
网络，不写模型，不请求管理员权限，也不改变微软拼音或默认输入法。
