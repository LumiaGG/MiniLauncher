# 防休眠（caffeine）

MiniLauncher 自带的 Windows 防休眠插件，可保持屏幕常亮或仅阻止系统自动睡眠，并支持定时关闭。

## 功能

- 两种模式：保持系统唤醒并让屏幕常亮；仅保持系统唤醒，允许自动息屏。
- 按默认配置一键开启，或指定 1–1440 分钟的倒计时；切换模式时保留剩余时间。
- 在启动器顶栏显示当前模式与剩余时间；可通过独立托盘图标调整模式、时长或关闭。

## 使用

唤出 MiniLauncher，输入以下内容，选择结果后按 Enter 执行：

| 输入 / 操作 | 效果 |
| :--- | :--- |
| `caf` / `caffeine` / `awake` | 显示开启、关闭及模式切换操作 |
| `caf 30` | 按当前模式（未开启时按默认模式）开启 30 分钟倒计时 |
| `caf 1.5h` / `caf 1h30m` / `caf 定时 90分钟` | 开启 90 分钟倒计时 |
| `caf off` | 显示关闭防休眠操作 |
| 右键顶栏状态胶囊 | 立即关闭防休眠 |
| 右键独立托盘图标 | 调整模式、时长或关闭 |
| 双击独立托盘图标 | 立即关闭防休眠 |

输入时长只会显示候选操作，按 Enter 后才开始计时。插件加载后默认关闭，不会自动开启防休眠。

## 配置

在“设置 → 插件 → 防休眠”修改，或编辑主配置文件的以下配置段。这里列出的是默认值：

```toml
[plugins.caffeine]
enabled = true
keywords = ["caf", "caffeine", "awake"]
keep_display_on = true
default_duration_mins = 0
show_tray_icon = true
```

| 字段 | 说明 |
| :--- | :--- |
| `enabled` | 启用或停用插件 |
| `keywords` | 显式触发关键词 |
| `keep_display_on` | 默认开启时是否同时保持屏幕常亮 |
| `default_duration_mins` | 默认持续分钟数，范围 0–1440；0 表示持续到手动关闭或退出宿主 |
| `show_tray_icon` | 开启期间是否显示独立托盘图标 |

## 数据与限制

运行状态暂存于当前数据目录的 `caffeine-runtime.json`，仅用于同一宿主进程内恢复插件会话，不会在重启宿主后自动续开。倒计时结束、关闭插件或退出宿主后，系统恢复按自身电源策略运行。

插件使用 Windows 电源请求阻止自动睡眠，不阻止用户主动睡眠、关机或系统强制执行的电源操作。

## 构建与安装

发行包已包含此插件。源码开发需要 Windows 10/11 x64、Rust MSVC 工具链及 Visual Studio C++ Build Tools。以下命令在仓库根目录执行：

```powershell
.\dev.ps1 build -Release -Bin caffeine
.\scripts\reinstall-plugins.ps1 -InstallDirectory .\target\release -Plugins caffeine -SkipBuild
```

重装前需已构建同版本的宿主和设置程序（可运行 `.\dev.ps1 build -Release`）。脚本会退出目标目录的旧宿主，将插件安装到其当前数据目录并登记配置；完成后重新启动宿主。插件包包含 `caffeine.exe`、`plugin.json`、`settings.default.toml`、`settings.schema.json`、`Images/` 和本说明。

## 开发参考

`main.rs` 负责电源请求、查询及执行，`runtime.rs` 和 `tray.rs` 负责托盘运行环境与交互。插件以常驻独立进程运行，通过 JSON-RPC 2.0 插件协议 v2 通信，顶栏状态使用通用 `WM_COPYDATA` 状态胶囊扩展。

仓库内协议说明：[外部插件协议 v2](../../docs/plugins.md)。
