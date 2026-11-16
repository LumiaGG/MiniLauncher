# 剪贴板历史（clipboard-history）

MiniLauncher 自带的 Windows 剪贴板历史插件，记录文本、图片与文件，支持检索、置顶、预览和拖出。

## 功能

- 常驻监听系统剪贴板，记录文本、图片及文件列表并去重，重启后保留历史。
- 文本、图片和文件使用独立类型图标；标题优先显示内容，时间位于副标题末尾。
- 支持文件名、路径及“文件”“文件夹”“file”等关键词检索。
- 支持文本多词筛选，以及通过“图片”“截图”“image”等词筛选图片记录。
- 支持置顶、删除、清空、暂停与恢复记录；图片显示缩略图并支持原图预览。
- 支持全局快捷键直接打开，以及综合范围空输入时直接显示历史。

## 使用

| 输入 / 操作 | 效果 |
| :--- | :--- |
| `clip` / `cb` | 显示历史，置顶项优先 |
| `clip 会议 记录` | 筛选同时包含这些词的文本 |
| `clip 图片` | 筛选图片历史 |
| `clip /pause` / `clip /resume` | 显示暂停 / 恢复记录操作，按 Enter 执行 |
| `clip /clear` | 显示清空全部历史（含置顶）的操作，按 Enter 确认 |
| `Ctrl+Alt+V` | 直接打开剪贴板历史，再次按下关闭 |
| 选中历史项按 Enter | 复制内容并向先前窗口尝试粘贴 |
| 右键历史项 | 置顶、取消置顶、仅复制、删除或确认清空 |
| 选中图片按 Space | 预览原图，再按 Space 或 Esc 退出 |
| 拖动历史项 | 向支持的目标程序拖出文本、图片或文件 |

默认在综合范围唤出启动器或清空输入时显示历史；开始输入后恢复普通搜索。清空历史不会清空当前系统剪贴板。

## 配置

在“设置 → 插件 → 剪贴板历史”修改，或编辑主配置文件的以下配置段。这里列出的是默认值：

```toml
[plugins.clipboard_history]
enabled = true
keywords = ["clip", "cb"]
direct_display = true
max_history = 100
hotkey_open = "Ctrl+Alt+V"
```

| 字段 | 说明 |
| :--- | :--- |
| `enabled` | 启用或停用插件；停用后不再监听 |
| `keywords` | 显式触发关键词 |
| `direct_display` | 是否在综合范围空输入时直接显示历史 |
| `max_history` | 保留条数上限，范围 1–500，默认 100 |
| `hotkey_open` | 全局唤出快捷键，需含修饰键；留空禁用 |

## 数据与限制

数据位于当前数据目录的 `clipboard-history/`：`history.dat` 保存历史元数据和文本，`images/` 保存原图及缩略图，`history.lock` 用于防止多实例同时写入。历史文本和文件路径以 JSON 明文保存，图片以 PNG 文件保存。文件历史只保存路径引用，不备份文件内容。

单条文件记录支持 1–64 个文件或文件夹，单个路径最多 32768 字节，路径总量最多 64 KiB。源文件删除、移动、改名或无法访问时，整组复制与拖出会报错。恢复文件历史统一按复制处理，包括原先剪切的文件；仅支持真实文件系统路径，不支持邮件附件等虚拟文件。

单条文本上限 16 KiB，文本与文件路径总量上限 2 MiB，原图像素数据总量上限 128 MiB；历史同时受条数限制。置顶不提供永久保存保证。复制敏感内容前可用 `clip /pause` 暂停记录。自动粘贴效果取决于目标窗口是否接受粘贴。

## 构建与安装

发行包已包含此插件。源码开发需要 Windows 10/11 x64、Rust MSVC 工具链及 Visual Studio C++ Build Tools。以下命令在仓库根目录执行：

```powershell
.\dev.ps1 build -Release -Bin clipboard-history
.\scripts\reinstall-plugins.ps1 -InstallDirectory .\target\release -Plugins clipboard-history -SkipBuild
```

重装前需已构建同版本的宿主和设置程序（可运行 `.\dev.ps1 build -Release`）。脚本会退出目标目录的旧宿主，将插件安装到其当前数据目录并登记配置；完成后重新启动宿主。插件包包含 `clipboard-history.exe`、`plugin.json`、`settings.default.toml`、`settings.schema.json`、`Images/` 类型图标和本说明。图标可通过 `scripts/generate-clipboard-icons.ps1` 从矢量几何重新生成。

## 开发参考

`main.rs` 负责初始化与 RPC，`monitor.rs` 监听剪贴板，`history.rs` 管理存储，`plugin.rs` 提供查询、执行和拖拽数据。插件以常驻独立进程运行，通过 JSON-RPC 2.0 插件协议 v2 及通用预览、拖拽、执行后粘贴扩展与宿主交互。

仓库内协议说明：[外部插件协议 v2](../../docs/plugins.md)。
