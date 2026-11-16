# 构建与分发

环境：Windows 10/11 x64、Rust MSVC、Visual Studio C++ Build Tools 和 Windows SDK。打包另需 PowerShell 7、Python 3.11+ 和 Git。

## 构建

克隆仓库并切换到发行说明中的提交或对应标签。首次构建需联网下载 Cargo.lock 锁定的依赖。

```powershell
cargo build --release --workspace --bins --locked
```

生成完整发行包运行：

```powershell
.\scripts\build-release.ps1
```

普通 `cargo build` 生成的安装器仅含占位载荷。发行脚本生成实际安装器、便携包和插件包。
`release-manifest.json` 记录构建提交和产物文件名，不包含哈希；不再复制全部依赖或制作离线源码包。

发行脚本通过 Rust 的 `--remap-path-prefix` 将项目、用户、Cargo 和工具链目录
映射为 `/build/` 下的固定路径，覆盖宿主、设置程序、插件、依赖和安装器。
打包前扫描各 EXE 中的 UTF-8 / UTF-16 本机目录字符串；发现残留时停止发行。
设置程序的复用校验包含路径映射参数，避免复用旧的未处理程序。
普通 Cargo 开发构建不应用这套发行路径映射。

## 分发

发行说明和包内 `SOURCE.md` 链接对应提交的公开源码仓库。软件包内部保留
`LICENSE`、`COPYRIGHT`、`THIRD_PARTY_NOTICES.html` 和 `SOURCE.md`。项目采用
[GPL-3.0-or-later](LICENSE)，分发修改版时应同时提供对应修改源码及构建说明。

发行脚本自动汇编第三方许可；缺少正文或校验失败会停止打包。依赖升级后的许可维护见 [legal/README.md](legal/README.md)。

## 发布到 GitHub Releases

更新版本、提交并推送源码后，在仓库根目录执行：

```powershell
.\scripts\publish-release.ps1
# 可选：提供发布说明；上传中断后跳过已有构建继续上传
.\scripts\publish-release.ps1 -NotesFile .\release-notes.md -SkipBuild
```

默认使用 `origin` 对应的公开 GitHub 仓库和 `v<版本>` 标签，产物位于
`target/packages/<标签>-<提交号>/`。可用 `-Repository owner/repo`、`-Tag v0.2.0`
显式指定仓库或标签。通过系统 Git 凭据管理器登录，或设置 `GH_TOKEN` / `GITHUB_TOKEN`；
脚本不输出凭据。需要 PowerShell 7、Python 3.11+ 及现有构建环境。

发行页只上传安装器、便携包和插件 ZIP，并链接对应提交的源码仓库；许可材料已包含在软件包内部。
脚本按本次构建清单选择文件，不收集目录中的其他旧包；上传完成后才公开草稿，
不覆盖已有正式发行版。不生成或核对发行文件 SHA256，也不下载远端文件复验。
