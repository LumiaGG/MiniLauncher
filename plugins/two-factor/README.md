# 两步验证码（two-factor）

MiniLauncher 自带的 Windows TOTP 插件，在本地生成两步验证码，支持账号检索、复制及导入。

## 功能

- 根据本机时间离线生成验证码，支持 SHA1 / SHA256 / SHA512、6–8 位和 1–86400 秒周期。
- 支持按账号信息检索，包括中文拼音和首字母匹配。
- 执行复制时重新计算验证码，避免复制查询时已过期的值。
- 在设置页添加、编辑账号信息、删除或导入账号；支持 `otpauth://totp/` URI 和 Flow 导出 JSON / URI 文本文件。

## 使用

先打开“设置 → 插件 → 两步验证码”添加或导入账号，再唤出 MiniLauncher：

| 输入 / 操作 | 效果 |
| :--- | :--- |
| `2fa` | 列出账号、当前验证码和剩余有效秒数 |
| `2fa github` | 筛选匹配的账号 |
| 选中结果按 Enter | 复制执行时的当前验证码 |
| 右键结果项 | 复制当前验证码或账号标识 |

没有账号时会提示前往设置页添加。账号编辑支持修改名称、服务商和备注。

添加账号时填写 Base32 密钥，或粘贴标准 `otpauth://totp/` 链接。文件导入支持账号 JSON 数组、含 `OtpParams` 数组的配置对象，或每行一个 URI 的 UTF-8 文本；JSON 导入采用 30 秒周期，URI 按链接参数读取周期，未指定时默认 30 秒。账号改动立即保存，密钥不回显。

## 配置

在“设置 → 插件 → 两步验证码”修改，或编辑主配置文件的以下配置段。这里列出的是默认值：

```toml
[plugins.two_factor]
enabled = true
keywords = ["2fa"]
```

| 字段 | 说明 |
| :--- | :--- |
| `enabled` | 启用或停用插件 |
| `keywords` | 显式触发关键词 |

账号及密钥保存在独立数据文件中，不写入上述 TOML 配置段。

## 数据与限制

默认数据文件为当前数据目录的 `two-factor/accounts.dat`，使用 Windows DPAPI 加密，绑定当前 Windows 用户环境。便携目录复制到其他电脑或用户后，不保证可以解密；请保留原始密钥或可重新导入的备份。

插件生成验证码不访问网络，也不会自动校准系统时间。验证码无法通过验证时，请核对 Windows 时间、算法、位数及周期。导入文件可能包含明文密钥，应妥善保管。

最多保存 1000 个账号；不支持 HOTP、二维码图片识别或 Google `otpauth-migration://` 批量导入。列表不会每秒自动刷新，重新查询可更新显示。不要并行运行多个实例写入同一账号库。

可在命令行显式指定数据文件和导入文件，导入会合并并去重：

```powershell
.\target\release\two-factor.exe --data-file "D:\LauncherData\two-factor\accounts.dat" --import-file "D:\Backup\totp.txt"
```

## 构建与安装

发行包已包含此插件。源码开发需要 Windows 10/11 x64、Rust MSVC 工具链及 Visual Studio C++ Build Tools。以下命令在仓库根目录执行：

```powershell
.\dev.ps1 build -Release -Bin two-factor
.\scripts\reinstall-plugins.ps1 -InstallDirectory .\target\release -Plugins two-factor -SkipBuild
```

重装前需已构建同版本的宿主和设置程序（可运行 `.\dev.ps1 build -Release`）。脚本会退出目标目录的旧宿主，将插件安装到其当前数据目录并登记配置；完成后重新启动宿主。插件包包含 `two-factor.exe`、`plugin.json`、`settings.default.toml` 和本说明。

## 开发参考

`main.rs` 负责查询和执行，`accounts.rs` 管理账号、检索与导入，`crypto.rs` 提供 DPAPI 和 TOTP，`admin.rs` 提供账号管理入口。插件按需启动，空闲 30 秒后由宿主回收，搜索使用 JSON-RPC 2.0 插件协议 v2；账号管理入口由清单的 `settings_extension` 声明。

仓库内协议说明：[外部插件协议 v2](../../docs/plugins.md)。
