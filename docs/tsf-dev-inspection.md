# TSF Alpha：检查、开发注册与撤回

`tsf-devctl` 管理固定身份的 Windows TSF Alpha。检查是只读操作；注册和注销
必须在管理员 PowerShell 中使用各自的显式命令，并附上
`--confirm-machine-wide-development-alpha`。

```powershell
Set-Location -LiteralPath 'C:\path\to\ziranma-decoder'

cargo build --release --lib --bin tsf-devctl

.\target\release\tsf-devctl.exe inspect `
  --dll .\target\release\ziranma_core.dll

.\target\release\tsf-devctl.exe register-machine `
  --dll .\target\release\ziranma_core.dll `
  --confirm-machine-wide-development-alpha
```

本轮开发注册固定为：

- 本机范围，需要管理员权限；
- 64 位进程内 COM 服务器；
- 简体中文 `zh-CN` 配置；
- 键盘 TIP 类别；
- 默认不启用、不激活，也不设为默认输入法。

微软拼音和现有默认输入法不会被修改。注册、启用、激活和设为默认仍是四个
不同边界；当前工具只实现第一项。

## 固定身份

| 项目 | 值 |
|---|---|
| 文本服务 CLSID | `{4CC8427B-D0F5-439E-B6AF-D45EACD7E577}` |
| 语言配置 GUID | `{8099D3F8-9F40-4DA5-9B01-C12DE0CD6370}` |
| 语言 | 简体中文 `zh-CN`，LANGID `0x0804` |

DLL 只导出 `DllGetClassObject` 和 `DllCanUnloadNow`，没有自注册入口。
`tsf-devctl` 负责标准 COM、TSF 文本服务身份、语言配置和键盘类别四层注册，避免
`regsvr32` 把这些边界藏进 DLL 回调。

## 不可变开发副本

注册命令先检查 DLL 的 x86-64、PE32+、导出和大小边界，再计算 SHA-256，
复制到：

```text
.local/tsf-alpha/builds/<sha256>/ziranma_core.dll
```

注册表只指向这份按内容寻址的副本，不指向可能被下一次 `cargo build`
覆盖的 `target\release\ziranma_core.dll`。安装记录位于
`.local/tsf-alpha/install-v1.txt`；两者都在 Git 忽略的本地目录中。
安装记录只保存版本、摘要和相对路径，不含输入数据。

注册前，固定 CLSID 在四个标准 COM 视图、TSF 文本服务列表、固定 zh-CN 配置
和键盘类别中都必须不存在；工具不会接管或覆盖来历不明的同名注册。四层写入
完成后还会重新枚举并确认：只存在本机 64 位 COM 注册，配置未启用、未活动，
键盘类别存在，服务器路径和注册表形状与安装记录完全一致。

普通权限进程无法完成 Windows 的持久 TSF 文本服务登记；本机实测正式接口返回
`E_FAIL`，事务会移除先前创建的 COM 键。因此工具不把 HKCU 直接注册表拼装
冒充受支持安装，也不混合“当前用户 COM + 本机 TSF”两种不完整作用域。

任一步失败都会按“类别、配置、文本服务身份、COM”的相反顺序撤回。若撤回
本身失败，命令会明确报告 `rollback incomplete`，不会把半完成状态说成成功。

## 只读检查

```powershell
.\target\release\tsf-devctl.exe inspect `
  --dll .\.local\tsf-alpha\builds\<sha256>\ziranma_core.dll
```

检查报告包括：

- DLL 路径、PE 格式、COM 导出和证书目录；
- 固定 CLSID 在当前用户/本机、64/32 位 COM 视图中的存在情况；
- 固定 TSF 文本服务身份是否存在；
- 固定 zh-CN 配置是否注册、启用或活动；
- 固定 CLSID 是否属于键盘 TIP 类别。

“证书目录存在”只表示 PE 安全目录非空，不验证证书可信、有效或未过期。
候选数据包的 Ed25519 验签也不能代替 DLL 的 PE/Authenticode 签名。

## 注销

开发注册可以用下列命令完整撤回：

```powershell
.\target\release\tsf-devctl.exe unregister-machine `
  --confirm-machine-wide-development-alpha
```

注销不依赖 `target\release` 中的旧构建，而是读取严格的本地安装记录，复核
不可变 DLL 摘要和系统状态，再按“类别、配置、文本服务身份、COM”的顺序
移除。若中途失败，已经移除的层会尽量恢复；只有全部移除并复查通过后，安装
记录才会删除。
按摘要保存的无引用 DLL 可以留在 `.local` 供审计或以后清理，不会再被系统
加载。

当前 Alpha 仍没有候选窗口、品牌图标或经验证的 Authenticode 签名。注册成功
只表示 Windows 能识别这个默认关闭的开发文本服务，不表示它适合日用或分发。

官方依据：

- <https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration>
- <https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfinputprocessorprofilemgr-registerprofile>
- <https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfcategorymgr-registercategory>
- <https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editors>
