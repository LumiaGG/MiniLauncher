# 文本面板 Demo

独立进程示例插件，通过协议 v2 的通用 `text_panel` 和 `copy` 动作展示宿主能力。关键词为 `textdemo`。不访问网络、不读写业务数据，也不修改剪贴板；用户执行复制或拖出时由宿主完成相应操作。宿主和设置程序无需识别本插件。

## 安装

可在设置 → 插件中安装生成的 `text-panel-demo-windows-x64.zip`，也可安装包含 `plugin.json` 的 `text-panel-demo` 目录。安装后从托盘重新加载配置。

从源码构建并安装到默认测试发行目录（需要该目录已有宿主）：

```powershell
.\scripts\install-text-panel-demo.ps1

# 指定另一个宿主目录
.\scripts\install-text-panel-demo.ps1 -InstallDirectory .\target\release

# 仅构建并打包，不关闭或修改现有宿主
.\scripts\install-text-panel-demo.ps1 -PackageOnly
```

源码构建需要 Windows x64 Rust MSVC 工具链及 Visual Studio C++ Build Tools。脚本仅构建 `text-panel-demo` 示例，生成 `target/packages/text-panel-demo/` 和相邻 ZIP；安装时退出指定目录的宿主，复制插件并通过通用 CLI 登记，完成后自行重新启动宿主。`-SkipBuild` 可复用已编译的示例。本插件不加入默认发行插件列表。

## 场景

每条命令只返回一个面板，避免其他演示占用正文显示空间。

| 输入 | 展示内容 |
| --- | --- |
| `textdemo` / `textdemo help` | 场景导航与操作说明 |
| `textdemo single` | 单区块，选择、复制与键盘操作 |
| `textdemo compare` | 横向双栏对照；窄窗口自动单列 |
| `textdemo four` | 四个区块，auto 布局与焦点顺序 |
| `textdemo vertical` | 四个区块，始终纵向排列 |
| `textdemo unequal` | 长短正文的宽度分配 |
| `textdemo long` | 两个长文区，独立滚动、自动换行与跨行拖选 |
| `textdemo unicode` | 多语言、Emoji、组合字符、空行、缩进、CRLF 与纯文本标记 |
| `textdemo static` | 可选正文与不可选静态区块混排，Tab 跳过静态区块 |
| `textdemo blank` | 真正的空正文；占位文字不进入复制内容 |
| `textdemo empty` | 没有正文区块的 empty 状态 |
| `textdemo loading` | 首次 loading，点击重试 / Ctrl+R 后恢复 ready |
| `textdemo error` | 首次 error，重试后恢复 ready |
| `textdemo retained` | 保留正文的 error，重试仍返回演示错误 |
| `textdemo append` | 稳定 ID，重试向长文尾部追加日志 |
| `textdemo replace` | 稳定 ID，重试交替替换 A/B 正文 |
| `textdemo actions` | 搜索框 Enter 复制整项、标题右键分别复制区块、标题拖出整项 |
| `textdemo limit` | 65,520 UTF-8 字节单区块，接近 64 KiB 上限 |

所有场景均提供复制整项及分别复制各区块的扩展菜单。整项复制只拼接真实正文，不包含区块标题、占位和状态消息；区块之间以两个换行分隔。`limit` 复制的也是完整正文。

## 手动验收

1. 输入 `textdemo compare`，按 Tab 进入正文，拖选后 Ctrl+C；使用 Tab / Shift+Tab 在区块间切换，越过边界返回搜索框。正文 Enter 不执行复制，Esc / Ctrl+F 返回搜索框；直接输入或粘贴继续搜索。
2. 输入 `textdemo long`，分别滚动两个区块，跨行选择并复制，检查换行是否保留。输入 `textdemo unicode`，检查所选文字及 Emoji 的复制结果；具体字形取决于系统字体。
3. 输入 `textdemo static`，确认静态区块不参与正文选择与 Tab 导航。输入 `textdemo blank`，使用标题菜单复制空字符串，确认未复制“暂无内容”。
4. 输入 `textdemo loading` / `textdemo error`，点击重试或按 Ctrl+R，观察恢复后的结果；重试不执行默认复制动作。`retained` 用于检查错误状态下正文仍可阅读和选择。
5. 输入 `textdemo append`，滚动并选中已有文字，松开鼠标后按 Ctrl+R，检查有效选区与滚动位置。输入 `textdemo replace`，选择开头的版本文字并重试，检查被替换的选区清除；说明区块内容始终不变。
6. 输入 `textdemo actions`，在面板总标题上右键复制整项或单块；从总标题拖到支持文本拖放的程序。正文区域拖动用于选字。

刷新场景使用每进程内存计数，不进行后台流式推送或轮询。`loading` / `error` 在本进程第一次查询时展示初始状态，之后为成功；清空输入再回来不会重置。`append` / `replace` 最多推进 60 次。插件空闲 300 秒、宿主重载或重启导致进程重新初始化后，计数清零。查询文本或搜索范围改变会让宿主清理旧面板；观察状态保留时请保持查询文字不变，使用重试。

正文是只读纯文本。Markdown、HTML 与 URL 仅作为普通字符显示。主题、字体、DPI、焦点和选择交互由宿主管理。

## 开发入口

- 源码：`examples/text-panel-demo.rs`
- 清单：`examples/text-panel-demo.json`
- 构建安装：`scripts/install-text-panel-demo.ps1`
- 通用协议：仓库 `docs/plugins.md`

针对性验证：`.\dev.ps1 test -Example text-panel-demo`。测试检查所有场景的宿主文本面板校验、复制动作与协议帧大小、重试状态变化、稳定 ID、追加前缀及 NDJSON 生命周期，不启动 UI。

用户明确要求界面测试后，可运行 `.\scripts\test-text-panel-demo.ps1`。该脚本关闭已有启动器，以隔离配置打开当前 Release 宿主，验证 UIA 结果、真实剪贴板操作和刷新行为，保存实际窗口截图及 HTML / JSON 报告到 `artifacts/text-panel-demo-ui/`，结束时退出测试实例并恢复剪贴板。需要先构建宿主与本示例。失败时脚本返回非零退出码，不把未通过的功能标为通过。

用 `-CaseIds 14-append-selection,15-replace-selection` 可只复核指定用例，并在同一输出目录合并旧结果；每项记录各自的隔离配置目录。截图需人工检查实际可见区块、占位和可读性，UIA 含有文字不能代替看图验证。不自动执行压力、跨应用拖放或主题切换回归。
