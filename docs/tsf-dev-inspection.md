# TSF 开发检查：注册前先核对什么

`tsf-devctl inspect` 是 Windows TSF Alpha 的只读检查器。它只读取一个明确
指定的 DLL，并通过系统 TSF 枚举简体中文语言配置；没有注册、注销、启用、
激活、文件写入或网络请求。

```powershell
Set-Location -LiteralPath 'C:\path\to\ziranma-decoder'

cargo build --release --lib --bin tsf-devctl
.\target\release\tsf-devctl.exe inspect `
  --dll .\target\release\ziranma_core.dll
```

默认输出只保留六类事实：DLL 路径、PE 格式、COM 入口、注册入口、证书目录和
系统语言配置。它不会在报告末尾替研究者安排下一步。

## 固定身份

开发 Alpha 为后续注册预留了三项固定身份：

| 项目 | 值 |
|---|---|
| 文本服务 CLSID | `{4CC8427B-D0F5-439E-B6AF-D45EACD7E577}` |
| 语言配置 GUID | `{8099D3F8-9F40-4DA5-9B01-C12DE0CD6370}` |
| 语言 | 简体中文 `zh-CN`，LANGID `0x0804` |

固定 GUID 只是身份，不会自行注册任何东西。当前 DLL 仍只导出
`DllGetClassObject` 和 `DllCanUnloadNow`，没有 `DllRegisterServer` 或
`DllUnregisterServer`。

## 检查口径

- PE 解析器有 64 MiB 文件上限、节表与 RVA 边界检查，只接受 x86-64、PE32+
  且带 DLL 标志的产物；
- 必须存在两个 COM 加载入口；发现注册/注销入口会使检查失败；
- “证书目录存在”只表示 PE 安全目录非空，不代表证书可信、未过期或签名验证
  成功；
- “系统语言配置”来自 `ITfInputProcessorProfileMgr::EnumProfiles(0x0804)`，只
  查找上表中的确切 CLSID 与配置 GUID；它不扫描其他语言，也不读取私人数据；
- 此检查尚未读取标准 COM 类注册位置。因此“语言配置未发现”不能证明系统里
  绝对没有同 CLSID 的孤立 COM 残留；真正安装工具出现前还要补这一项只读核对。

## 为什么现在仍不注册

微软把可用 TSF 输入服务分成多道边界：标准进程内 COM 服务器注册、TSF 语言
配置、TIP 类别，以及现代第三方 IME 所需的数字签名与兼容能力。Vista 以后
推荐使用 `ITfInputProcessorProfileMgr::RegisterProfile` 管理配置；键盘输入服务
还要通过 `ITfCategoryMgr` 声明 `GUID_TFCAT_TIP_KEYBOARD`。

当前 Alpha 尚有明确缺口：

- DLL 同目录没有 `candidate-data` 时，默认类工厂接入经过只读快照校验的 50 词
  公开开发候选源；它用于合成 Context 闭环，不是日用词典。目录存在时，新类
  工厂严格加载来源、许可、SHA-256、解码兼容性及 TSF 预检凭据全部有效的
  外部 current；
- 没有候选 UI、品牌图标或经验证的数字签名；
- 没有安装、释放已加载实例、反向注销和失败回滚实现；
- 尚未在记事本或 Codex 中验证真实焦点与异步编辑时序。

因此本阶段只保留检查器。以后若实现开发安装，注册、启用、激活和设为默认必须
继续分成不同操作；工具不得直接把自己设为默认输入法，卸载也必须先释放加载中
的实例并逐项反向清理。

官方依据：

- <https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration>
- <https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfinputprocessorprofilemgr>
- <https://learn.microsoft.com/en-us/windows/win32/tsf/predefined-category-values>
- <https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editors>
