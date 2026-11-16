# 构建与分发

环境：Windows 10/11 x64、Rust MSVC、Visual Studio C++ Build Tools 和 Windows SDK。打包另需 PowerShell 7、Python 3.11+ 和 Git。

## 构建

解压源码包并进入 `MiniLauncher-source`。工具链版本见 `SOURCE_MANIFEST.json`；依赖已包含在 `vendor/` 中。

```powershell
cargo build --release --workspace --bins --locked --offline
```

生成完整发行包需运行下列脚本；从源码 ZIP 构建时，先建立 Git 仓库：

```powershell
git init
git add .
git -c user.name=LocalBuilder -c user.email=local@example.invalid commit -m "Import corresponding source"
.\scripts\build-release.ps1
```

普通 `cargo build` 生成的安装器仅含占位载荷。发行脚本生成实际安装器、便携包、插件包、对应源码和校验文件。

发行脚本通过 Rust 的 `--remap-path-prefix` 将项目、用户、Cargo 和工具链目录
映射为 `/build/` 下的固定路径，覆盖宿主、设置程序、插件、依赖和安装器。
打包前扫描各 EXE 中的 UTF-8 / UTF-16 本机目录字符串；发现残留时停止发行。
设置程序的复用校验包含路径映射参数，避免复用旧的未处理程序。
普通 Cargo 开发构建不应用这套发行路径映射。

## 分发

按 [GPL-3.0-or-later](LICENSE) 分发时，将二进制和对应 `Source.zip` 放在同一下载位置，免费提供源码，并保留 `LICENSE`、`COPYRIGHT`、`THIRD_PARTY_NOTICES.html` 和 `SOURCE.md`。修改版须提供修改后的对应源码。

发行脚本自动汇编第三方许可；缺少正文或校验失败会停止打包。依赖升级后的许可维护见 [legal/README.md](legal/README.md)。
