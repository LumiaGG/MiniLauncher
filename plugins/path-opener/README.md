# 路径打开（path-opener）

MiniLauncher 自带的 Windows 路径浏览插件，识别文件或文件夹路径，列出目录内容并交给系统打开。

## 功能

- 支持本地绝对路径、UNC 网络路径、环境变量、用户主目录及 `file://` URI。
- 兼容粘贴时附带的中英文引号、正反斜杠及重复路径分隔符。
- 文件夹结果按文件夹优先、名称排序；输入文件路径时优先提供该文件的打开操作。
- 支持直接输入路径触发，也可用关键词显式调用。

## 使用

唤出 MiniLauncher，输入以下内容，选择结果后按 Enter 打开：

| 输入 | 效果 |
| :--- | :--- |
| `C:\Windows` | 全局识别路径，列出文件夹内容 |
| `path C:\Windows` / `p C:\Windows` | 显式浏览文件夹 |
| `path "C:\Windows\win.ini"` | 首项打开所在文件夹，随后提供打开该文件及其他同级项 |
| `path %USERPROFILE%\Desktop` | 展开环境变量后浏览 |
| `path ~\Documents` | 相对于当前 Windows 用户主目录浏览 |
| `path Documents` | 将相对路径按用户主目录解析 |
| `path file:///C:/Windows` | 解析文件 URI 后浏览 |
| `path \\server\share` | 浏览可访问的网络共享 |

结果右键菜单提供“复制路径”。仅输入 `path` 或 `p` 不列出内容，需要补充路径。插件只浏览现有路径，不提供不存在路径的补全。

## 配置

在“设置 → 插件 → 路径打开”修改，或编辑主配置文件的以下配置段。这里列出的是默认值：

```toml
[plugins.path_opener]
enabled = true
keywords = ["path", "p", "*"]
```

| 字段 | 说明 |
| :--- | :--- |
| `enabled` | 启用或停用插件 |
| `keywords` | `path`、`p` 为显式关键词；`*` 表示允许全局查询，不是需要输入的字符 |

全局查询还需满足清单的 `global_trigger.type = "path"` 规则。保留 `*` 才能直接输入路径触发；相对路径建议使用 `path` 或 `p` 显式调用。

## 数据与限制

插件不保存独立业务数据。相对路径和 `~` 以 `USERPROFILE` 为基准，不以插件工作目录为基准。

每次最多返回 100 项，并受宿主结果数量和协议帧大小限制。目录不可枚举但路径可访问时，仍保留“打开当前文件夹”；不存在或无法访问的路径在显式查询时返回错误，全局查询时返回空结果。网络共享的响应速度取决于网络和访问权限。

## 构建与安装

发行包已包含此插件。源码开发需要 Windows 10/11 x64、Rust MSVC 工具链及 Visual Studio C++ Build Tools。以下命令在仓库根目录执行：

```powershell
.\dev.ps1 build -Release -Bin path-opener
.\scripts\reinstall-plugins.ps1 -InstallDirectory .\target\release -Plugins path-opener -SkipBuild
```

重装前需已构建同版本的宿主和设置程序（可运行 `.\dev.ps1 build -Release`）。脚本会退出目标目录的旧宿主，将插件安装到其当前数据目录并登记配置；完成后重新启动宿主。插件包包含 `path-opener.exe`、`plugin.json`、`settings.default.toml` 和本说明。

## 开发参考

`main.rs` 负责路径规范化、目录枚举及 RPC。插件按需启动，空闲 30 秒后由宿主回收，通过 JSON-RPC 2.0 插件协议 v2 返回通用打开、复制动作。

仓库内协议说明：[外部插件协议 v2](../../docs/plugins.md)。
