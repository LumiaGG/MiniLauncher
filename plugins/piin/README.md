# Piin 备注

**Piin = Pin Ideas, Images & Notes** —— 把想法、图片和备注固定下来，随时取用。

手动收藏文本和图片的原生插件。所有备注长期保留，置顶只改变排序；不监听剪贴板，不提供定时提醒。

| 输入／操作 | 效果 |
| --- | --- |
| `piin` / `note` / `备注` | 显示保存的备注，置顶优先 |
| `note 内容` | 检索已有备注；列表中的“保存备注”可新增当前内容 |
| `note /add 内容` | 直接显示新增操作，Enter 保存 |
| 输入 `note ` 后 Ctrl+V 粘贴图片 | 输入框显示图片标记，Enter 保存图片备注；可同时填写说明 |
| `Ctrl+Alt+N` | 打开备注范围，再次按下关闭 |
| 备注 Enter | 复制并向之前的窗口粘贴 |
| 右键 | 置顶／取消置顶、仅复制、删除（需确认）；带说明的图片可单独复制文字 |
| 图片 Space | 查看原图；Space／Esc 关闭 |
| 拖动备注 | 拖出原始文本或图片 |

已有匹配项优先显示；明确新增时使用 `/add`。输入框支持 Ctrl+V、Shift+Insert 粘贴 PNG、DIB／DIBV5 图片。图片标记可选中删除，支持文字撤销／重做；完整标记被删掉后不再提交该图片。清空输入释放附件。一条备注支持一张图片，图片说明用于检索，复制与拖出图片时输出图片本体。

设置位于 `[plugins.piin]`，默认：

```toml
enabled = true
keywords = ["piin", "note", "备注"]
direct_display = false
hotkey_open = "Ctrl+Alt+N"
```

数据位于主配置文件同级的 `piin/`：`notes.json` 为明文元数据及文字，`images/` 为原图与缩略图，`notes.lock` 防止并发写入。最多 500 条、单条文字 16 KiB、文字总量 2 MiB、图片像素总量 128 MiB，单张图片沿用宿主 40 MiB 像素、单边 16384 的限制。达到上限时拒绝新增，不自动删除任何旧备注。保存失败保留原有元数据；损坏的存储文件会报告错误，不覆盖旧数据。插件退出或输入被替换后，未提交的保存操作需重新查询确认。

构建安装：

```powershell
.\dev.ps1 build -Release
.\scripts\reinstall-plugins.ps1 -InstallDirectory .\target\release -Plugins piin -SkipBuild
```

安装后重新启动新版宿主。发行脚本同时提供独立插件 ZIP。通过 JSON-RPC 2.0 协议 v2 的通用图片输入、预览、菜单、拖拽及执行后粘贴扩展与宿主交互。
