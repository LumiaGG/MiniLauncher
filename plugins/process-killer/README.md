# 进程与端口管理（process-killer）

MiniLauncher 自带的 Windows 进程与端口查询插件，可查看内存占用、网络绑定和程序路径，并终止进程。

## 功能

- 按进程名称或 PID 搜索，默认按内存占用降序显示。
- 查询 TCP / UDP、IPv4 / IPv6 端口绑定；端口总览显示监听中的 TCP 及 UDP 绑定。
- 在详情面板显示进程属性、父进程 PID、程序路径和网络连接。
- 提供终止进程、复制 PID / 端口 / 路径和打开文件所在位置等操作。

## 使用

| 输入 / 操作 | 效果 |
| :--- | :--- |
| `ps` / `kill` | 显示内存占用靠前的进程 |
| `ps chrome` / `kill node` | 按进程名称筛选 |
| `ps 1234` / `kill 1234` | 按 PID 搜索，精确匹配的进程优先显示 |
| `port` | 显示端口总览 |
| `port :8080` | 明确查询本地端口 8080 的占用情况 |
| `:8080` | 通过全局前缀查询端口，需在关键词配置中保留 `*` |
| 选中普通进程或端口项按 Enter | 直接强制终止对应进程 |
| 选中受保护项按 Enter | 复制 PID |
| 右键结果项 | 查看可用的复制、打开位置及终止操作 |

**普通结果的 Enter 操作会直接终止进程，无二次确认，可能丢失未保存的数据。** `ps` 查询结果同样提供此操作，执行前请核对结果副标题。

关键词之后的纯数字会优先显示 PID 精确匹配的进程，并在其后显示同号端口的占用结果。需要明确只查端口时，可输入 `port :8080`；此写法不受宿主移除首个关键词的影响。

## 配置

在“设置 → 插件 → 进程与端口管理”修改，或编辑主配置文件的以下配置段。这里列出的是默认值：

```toml
[plugins.process_killer]
enabled = true
keywords = ["port", "kill", "ps"]
show_system_processes = false
sort_by = "memory"
max_port_results = 50
debounce_ms = 350
```

| 字段 | 说明 |
| :--- | :--- |
| `enabled` | 启用或停用插件 |
| `keywords` | 显式触发关键词；默认配置不含 `*`，要启用 `:8080` 全局查询需手动加入 `*` |
| `debounce_ms` | 非空查询等待输入停止的毫秒数，范围 0–5000；0 表示立即查询 |
| `show_system_processes` | 默认文件中已有此字段，当前查询实现尚未使用 |
| `sort_by` | 默认文件中已有此字段，当前固定按内存降序，尚未应用该设置 |
| `max_port_results` | 默认文件中已有此字段，当前结果上限取自宿主请求，尚未应用该设置 |

## 数据与限制

插件不保存独立业务数据，每次查询读取当前系统快照。受保护的核心进程不提供终止动作；其他进程仍可能因 Windows 权限限制而无法终止，插件不会自行提权。

每次最多返回 100 项；查询不持续自动刷新，进程和端口可能在操作前已退出或变化。当前插件不支持 `pid:` 搜索语法。

## 构建与安装

发行包已包含此插件。源码开发需要 Windows 10/11 x64、Rust MSVC 工具链及 Visual Studio C++ Build Tools。以下命令在仓库根目录执行：

```powershell
.\dev.ps1 build -Release -Bin process-killer
.\scripts\reinstall-plugins.ps1 -InstallDirectory .\target\release -Plugins process-killer -SkipBuild
```

重装前需已构建同版本的宿主和设置程序（可运行 `.\dev.ps1 build -Release`）。脚本会退出目标目录的旧宿主，将插件安装到其当前数据目录并登记配置；完成后重新启动宿主。插件包包含 `process-killer.exe`、`plugin.json`、`settings.default.toml`、`settings.schema.json` 和本说明。

## 开发参考

`model.rs` 解析查询意图，`processes.rs` 和 `ports.rs` 读取系统信息，`view.rs` 生成结果与通用文本面板，`runtime.rs` 处理 RPC、取消和输入防抖。插件按需启动，空闲 30 秒后由宿主回收，使用 JSON-RPC 2.0 插件协议 v2。

宿主显式关键词路由会移除首个关键词，而插件自身也识别 `ps`、`kill`、`port` 命令。移除关键词后的纯数字查询同时支持 PID 和端口，无需重复输入进程查询命令。

仓库内协议说明：[外部插件协议 v2](../../docs/plugins.md)。
