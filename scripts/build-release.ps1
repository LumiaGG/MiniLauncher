[CmdletBinding()]
param(
    [string]$OutputDirectory = "$PSScriptRoot\..\target\packages",
    [string]$SourceRepository = 'https://github.com/LumiaGG/MiniLauncher',
    # Test restarts may reuse settings even when only its shared host library changed.
    [switch]$ReuseSettings
)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$outputPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
$targetPath = Join-Path $projectRoot 'target'
$stagingRoot = Join-Path $targetPath 'package-staging'
$stagingPath = Join-Path $stagingRoot ([Guid]::NewGuid().ToString('N'))
$stagingPrefix = $stagingRoot + [IO.Path]::DirectorySeparatorChar
$cargoManifest = Get-Content -LiteralPath (Join-Path $projectRoot 'Cargo.toml') -Raw
$versionMatch = [regex]::Match($cargoManifest, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) { throw '无法读取 Cargo.toml 版本号' }
$version = $versionMatch.Groups[1].Value
$pluginIds = @('path-opener', 'two-factor', 'clipboard-history', 'caffeine', 'process-killer', 'piin')

function Copy-PackageFile([string]$Source, [string]$Destination) {
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
        throw "发行文件不存在：$Source"
    }
    if (Test-Path -LiteralPath $Destination -PathType Leaf) {
        # An unchanged executable can stay running during repeated packaging.
        if ((Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash -eq
            (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash) {
            return
        }
    }
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
}

function Copy-PackageDirectory([string]$Source, [string]$Destination) {
    # Copy individual files so repeated builds merge into the same directory,
    # without adding another directory level or deleting unrelated local files.
    foreach ($file in Get-ChildItem -LiteralPath $Source -File -Recurse) {
        $relativePath = $file.FullName.Substring($Source.Length + 1)
        $destinationFile = Join-Path $Destination $relativePath
        [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destinationFile))
        Copy-PackageFile $file.FullName $destinationFile
    }
}

function Copy-LicenseMaterials([string]$Destination) {
    foreach ($name in @('LICENSE', 'COPYRIGHT', 'THIRD_PARTY_NOTICES.html')) {
        Copy-PackageFile (Join-Path $projectRoot $name) $Destination
    }
    if ([IO.Path]::GetFullPath($Destination) -ne [IO.Path]::GetFullPath($archivesPath)) {
        Copy-PackageFile (Join-Path $archivesPath 'SOURCE.md') $Destination
    }
}

$savedCargoHome = $env:CARGO_HOME
$savedRustupHome = $env:RUSTUP_HOME
$savedPath = $env:PATH
$savedPayloadZip = $env:MINILAUNCHER_PAYLOAD_ZIP
$savedEncodedRustflags = $env:CARGO_ENCODED_RUSTFLAGS
Push-Location $projectRoot
try {
    if (Test-Path -LiteralPath (Join-Path $projectRoot '.tools\cargo\bin\cargo.exe')) {
        $env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
        $env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
        $env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
    }
    & python "$PSScriptRoot\collect-third-party-licenses.py"
    if ($LASTEXITCODE -ne 0) { throw '第三方许可材料不完整，停止发行构建。' }
    # Native builds share Cargo's default cache and executable layout.
    $compilerInfo = @(& rustc -vV)
    if ($LASTEXITCODE -ne 0 -or 'host: x86_64-pc-windows-msvc' -notin $compilerInfo -or $env:CARGO_BUILD_TARGET) {
        throw '发行构建需要原生 x86_64-pc-windows-msvc 工具链，且不能设置 CARGO_BUILD_TARGET。'
    }
    # Strip source locations from panic/file! strings as well as debug information.
    # Encoded arguments also support build directories containing spaces.
    $sysroot = & rustc --print sysroot
    if ($LASTEXITCODE -ne 0) { throw '无法读取 Rust 工具链目录' }
    $cargoHome = if ($env:CARGO_HOME) { [IO.Path]::GetFullPath($env:CARGO_HOME) } else { Join-Path $env:USERPROFILE '.cargo' }
    $rustupHome = if ($env:RUSTUP_HOME) { [IO.Path]::GetFullPath($env:RUSTUP_HOME) } else { Join-Path $env:USERPROFILE '.rustup' }
    $pathMappings = @(
        @{ Source = $env:USERPROFILE; Destination = '/build/user' }
        @{ Source = $projectRoot; Destination = '/build/mini-launcher' }
        @{ Source = $cargoHome; Destination = '/build/cargo' }
        @{ Source = $rustupHome; Destination = '/build/rustup' }
        @{ Source = $sysroot; Destination = '/build/toolchain' }
    ) | Where-Object { $_.Source } | Sort-Object { $_.Source.Length }
    $releaseRustflags = if ($savedEncodedRustflags) {
        @($savedEncodedRustflags.Split([char]0x1f))
    } elseif ($env:RUSTFLAGS) {
        @($env:RUSTFLAGS -split '\s+' | Where-Object { $_ })
    } else { @() }
    $releaseRustflags = @($releaseRustflags)
    $pathCheckArguments = @()
    # rustc uses the last matching mapping; more specific roots come last.
    foreach ($mapping in $pathMappings) {
        foreach ($source in @($mapping.Source.Replace('\', '/'), $mapping.Source.Replace('/', '\')) | Select-Object -Unique) {
            $releaseRustflags += "--remap-path-prefix=$source=$($mapping.Destination)"
        }
        $pathCheckArguments += @('--private-root', $mapping.Source)
    }
    $env:CARGO_ENCODED_RUSTFLAGS = $releaseRustflags -join [char]0x1f
    $binaryPath = Join-Path $targetPath 'release'
    # Release builds now overwrite the same binaries used by development scripts.
    foreach ($process in @(Get-Process -ErrorAction SilentlyContinue)) {
        if ($process.Path -and $process.Path.StartsWith($binaryPath + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            Stop-Process -Id $process.Id -Force
            if (-not $process.WaitForExit(3000)) { throw "构建程序仍在运行：$($process.Path)" }
        }
    }
    $settingsExecutable = Join-Path $binaryPath 'mini-launcher-config.exe'
    $settingsStamp = Join-Path $binaryPath 'settings-release-inputs.sha256'
    $settingsFingerprint = & "$PSScriptRoot\settings-build-fingerprint.ps1" -ProjectRoot $projectRoot
    $settingsFingerprint += "`n$env:CARGO_ENCODED_RUSTFLAGS"
    $reuse = $ReuseSettings -and (Test-Path -LiteralPath $settingsExecutable -PathType Leaf) -and
        (Test-Path -LiteralPath $settingsStamp -PathType Leaf) -and
        ((Get-Content -LiteralPath $settingsStamp -Raw).TrimEnd() -eq $settingsFingerprint)
    $buildArguments = @('build', '--release', '--bins', '--locked', '--target-dir', $targetPath)
    if ($reuse) {
        Write-Output '设置源码和依赖声明未变，复用设置管理器（共享库变更可用 -RebuildSettings 强制更新）。'
        $buildArguments += @('-p', 'mini-launcher')
    } else {
        $buildArguments += '--workspace'
    }
    & cargo @buildArguments
    if ($LASTEXITCODE -ne 0) { throw "发行编译失败，退出码 $LASTEXITCODE" }
    if (-not $reuse) {
        Set-Content -LiteralPath $settingsStamp -Value $settingsFingerprint -Encoding utf8
    }
    $releaseBinaries = @($settingsExecutable, (Join-Path $binaryPath 'mini-launcher.exe'))
    $releaseBinaries += $pluginIds | ForEach-Object { Join-Path $binaryPath "$_.exe" }
    & python -B "$PSScriptRoot\check-binary-paths.py" @pathCheckArguments --files @releaseBinaries
    if ($LASTEXITCODE -ne 0) { throw '发行程序包含本机构建路径，停止打包。' }
    $revision = & git rev-parse --short HEAD
    if ($LASTEXITCODE -ne 0) { throw '无法读取构建提交号' }
    $fullRevision = & git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw '无法读取完整构建提交号' }
    $dirty = @(& git status --porcelain --untracked-files=no).Count -gt 0
    $sourceDescription = "$revision" + $(if ($dirty) { '（含未提交改动）' } else { '' })

    [void][IO.Directory]::CreateDirectory($stagingPath)
    $archivesPath = Join-Path $stagingPath 'archives'
    [void][IO.Directory]::CreateDirectory($archivesPath)
    $sourceNotice = @"
# MiniLauncher 对应源码

构建来源：$sourceDescription
许可：GPL-3.0-or-later，见 LICENSE 和 COPYRIGHT。

对应版本源码：$SourceRepository/tree/$fullRevision
构建说明和脚本包含在仓库中，Cargo.lock 固定依赖版本；构建时通过 Cargo 下载依赖。
发行地址：$SourceRepository/releases
构建与分发要求见仓库 SOURCE_BUILD.md；第三方许可见 THIRD_PARTY_NOTICES.html。
"@
    Set-Content -LiteralPath (Join-Path $archivesPath 'SOURCE.md') -Value $sourceNotice -Encoding utf8
    Copy-LicenseMaterials $archivesPath

    $hostPath = Join-Path $stagingPath 'MiniLauncher'
    [void][IO.Directory]::CreateDirectory($hostPath)
    Copy-PackageFile (Join-Path $binaryPath 'mini-launcher.exe') $hostPath
    Copy-PackageFile (Join-Path $binaryPath 'mini-launcher-config.exe') $hostPath
    Copy-PackageFile (Join-Path $projectRoot 'resources\settings.default.toml') (Join-Path $hostPath 'settings.example.toml')
    Copy-LicenseMaterials $hostPath
    $hostReadme = @"
# MiniLauncher $version · Windows x64

构建来源：$sourceDescription

解压后运行 mini-launcher.exe；更新前先从托盘退出旧版。
Alt+Space 唤出 / 收起，Enter 打开，Esc 隐藏；--background 仅启动托盘。
右键托盘选择设置或输入 >settings，打开图形设置管理器；>plugins 直接进入插件管理。
mini-launcher-config.exe 需与主程序放在同一目录；Ctrl+S 保存全部并自动生效。
默认便携模式，配置、历史及插件数据位于安装目录的 data；settings.example.toml 仅供参考。
在基础设置中关闭便携模式后，重启迁移到 %APPDATA%\MiniLauncher；原数据保留。
更新时保留 data 和 portable.toml。
优先使用已安装的 Noto Sans SC，未安装时回退微软雅黑 UI；无需额外字体即可运行。
Everything 文件搜索需自行安装并运行 Everything 客户端。

自带插件在同批发行目录中单独提供。将插件 ZIP 中的整个插件目录解压到
安装目录的 data\plugins（关闭便携模式后为 %APPDATA%\MiniLauncher\plugins），然后从托盘“重新加载配置 / 扫描应用”。
也可在 GUI 插件管理页点击安装目录/ZIP，或卸载插件并保留备份。
首次安装会自动补齐主配置中的插件设置；已有使用记录和插件数据无需复制。
本包不包含用户配置、使用历史、验证码账号或字体文件。
GPL-3.0-or-later；许可正文见 LICENSE，第三方声明见 THIRD_PARTY_NOTICES.html，对应源码获取见 SOURCE.md。
"@
    Set-Content -LiteralPath (Join-Path $hostPath 'README.md') -Value $hostReadme -Encoding utf8
    Compress-Archive -LiteralPath $hostPath -DestinationPath (Join-Path $archivesPath 'MiniLauncher-Portable-windows-x64.zip') -CompressionLevel Optimal

    foreach ($id in $pluginIds) {
        $pluginSource = Join-Path $projectRoot "plugins\$id"
        $manifestPath = Join-Path $pluginSource 'plugin.json'
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        if ($manifest.id -ne $id -or $manifest.executable -ne "$id.exe") {
            throw "插件清单与发行目标不一致：$id"
        }
        $pluginPath = Join-Path $stagingPath $id
        [void][IO.Directory]::CreateDirectory($pluginPath)
        Copy-PackageFile (Join-Path $binaryPath $manifest.executable) $pluginPath
        foreach ($name in @('plugin.json', 'settings.default.toml', 'README.md')) {
            Copy-PackageFile (Join-Path $pluginSource $name) $pluginPath
        }
        $schemaPath = Join-Path $pluginSource 'settings.schema.json'
        if (Test-Path -LiteralPath $schemaPath -PathType Leaf) {
            Copy-PackageFile $schemaPath $pluginPath
        }
        if (Test-Path -LiteralPath (Join-Path $pluginSource 'Images') -PathType Container) {
            Copy-Item -LiteralPath (Join-Path $pluginSource 'Images') -Destination $pluginPath -Recurse
        }
        Copy-LicenseMaterials $pluginPath
        # The archive root is the plugin directory expected by the host scanner.
        Compress-Archive -LiteralPath $pluginPath -DestinationPath (Join-Path $archivesPath "$id-windows-x64.zip") -CompressionLevel Optimal
    }

    # 组装单文件安装包内置的完整载荷（包含宿主程序与全部预装插件）
    Write-Output '正在组装安装器载荷并编译单文件安装包...'
    $installerStaging = Join-Path $stagingPath 'installer-payload'
    [void][IO.Directory]::CreateDirectory($installerStaging)
    Copy-PackageDirectory $hostPath $installerStaging
    $installerPluginsDir = Join-Path $installerStaging 'data\plugins'
    [void][IO.Directory]::CreateDirectory($installerPluginsDir)
    foreach ($id in $pluginIds) {
        $pluginTarget = Join-Path $installerPluginsDir $id
        [void][IO.Directory]::CreateDirectory($pluginTarget)
        Copy-PackageDirectory (Join-Path $stagingPath $id) $pluginTarget
    }
    $payloadZip = Join-Path $stagingPath 'installer-payload.zip'
    $itemsToCompress = @(Get-ChildItem -LiteralPath $installerStaging | ForEach-Object { $_.FullName })
    Compress-Archive -LiteralPath $itemsToCompress -DestinationPath $payloadZip -CompressionLevel Optimal

    $env:MINILAUNCHER_PAYLOAD_ZIP = $payloadZip
    & cargo build --release --locked --bin mini-launcher-setup --target-dir $targetPath
    if ($LASTEXITCODE -ne 0) { throw "安装器编译失败，退出码 $LASTEXITCODE" }
    $setupExecutable = Join-Path $binaryPath 'mini-launcher-setup.exe'
    & python -B "$PSScriptRoot\check-binary-paths.py" @pathCheckArguments --files $setupExecutable
    if ($LASTEXITCODE -ne 0) { throw '安装器包含本机构建路径，停止打包。' }
    $setupOutputName = 'MiniLauncher-Setup-windows-x64.exe'
    Copy-PackageFile $setupExecutable (Join-Path $archivesPath $setupOutputName)

    $releaseReadme = @"
# MiniLauncher $version · Windows x64 发行文件

构建来源：$sourceDescription
构建时间（UTC）：$([DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss'))

- MiniLauncher-Setup-windows-x64.exe：单文件安装程序，支持自定义路径、快捷方式与便携配置。
- MiniLauncher-Portable-windows-x64.zip：宿主便携包，解压后运行 mini-launcher.exe。
- path-opener-windows-x64.zip：路径打开插件，关键词 path / p，可直接粘贴路径。
- two-factor-windows-x64.zip：两步验证码插件，关键词 2fa。
- clipboard-history-windows-x64.zip：剪贴板文本及图片历史插件，关键词 clip / cb，随宿主监听。
- piin-windows-x64.zip：文本及图片备注插件，关键词 piin / note，支持手动保存与置顶。
- caffeine-windows-x64.zip：防休眠与屏幕常亮插件，关键词 caf / caffeine / awake，支持定时关闭。
- process-killer-windows-x64.zip：进程与端口管理插件，关键词 port / kill / ps。
- LICENSE / COPYRIGHT / THIRD_PARTY_NOTICES.html / SOURCE.md：项目许可、第三方声明及源码获取说明。
- unpacked\MiniLauncher\：已解压的宿主程序、配置示例和说明。
- unpacked\path-opener\、unpacked\two-factor\、unpacked\clipboard-history\、unpacked\caffeine\、unpacked\process-killer\、unpacked\piin\：已解压的插件。

直接替换本地发行文件：先退出正在运行的宿主及相关插件，再从 unpacked 中
复制对应目录里的文件到现有安装目录，无需先解压 ZIP。保留已有用户数据；settings.example.toml 和 settings.default.toml 是模板。
宿主旁边的插件目录不会自动安装，仍需复制到下述插件目录或在主配置中登记。

安装插件：先退出宿主，将 ZIP 中的整个插件目录解压到
安装目录的 data\plugins（关闭便携模式后为 %APPDATA%\MiniLauncher\plugins），再启动宿主。也可安装后从托盘重新加载。
升级插件时仅替换 ZIP 内附带的程序、清单、默认模板与资源文件，保留现有数据文件。
主配置默认位于安装目录的 data\settings.toml，插件设置在 [plugins.<名称>]。
基础设置可关闭便携模式，退出设置和宿主再重启后迁移到 %APPDATA%\MiniLauncher。
更新时保留 data 和 portable.toml；原数据与冲突文件备份会保留。

宿主为 Windows 10/11 x64 原生程序。插件独立运行，剪贴板历史随宿主常驻，其余按需启动，无需 .NET 运行时。
开发示例 echo-plugin 不属于自带插件，本批不打包。各插件详细说明在各自 ZIP 的 README.md。
对应版本源码：$SourceRepository/tree/$fullRevision，详见 SOURCE.md。
"@
    Set-Content -LiteralPath (Join-Path $archivesPath 'README.md') -Value $releaseReadme -Encoding utf8
    $releaseManifest = @{
        revision = $fullRevision
        assets = @(Get-ChildItem -LiteralPath $archivesPath -File | Where-Object { $_.Extension -in @('.zip', '.exe') } | Sort-Object Name | ForEach-Object { $_.Name })
    }
    $releaseManifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $archivesPath 'release-manifest.json') -Encoding utf8

    # Publish archives and ready-to-copy directories without clearing the output directory.
    [void][IO.Directory]::CreateDirectory($outputPath)
    foreach ($file in Get-ChildItem -LiteralPath $archivesPath -File) {
        Copy-PackageFile $file.FullName (Join-Path $outputPath $file.Name)
    }
    $unpackedPath = Join-Path $outputPath 'unpacked'
    foreach ($name in @('MiniLauncher') + $pluginIds) {
        Copy-PackageDirectory (Join-Path $stagingPath $name) (Join-Path $unpackedPath $name)
    }
    Write-Output "发行文件已生成：$outputPath"
    Get-ChildItem -LiteralPath $archivesPath -File | Select-Object Name, Length
    Write-Output "已解压的宿主与插件：$unpackedPath"
}
finally {
    Pop-Location
    $env:CARGO_HOME = $savedCargoHome
    $env:RUSTUP_HOME = $savedRustupHome
    $env:PATH = $savedPath
    $env:MINILAUNCHER_PAYLOAD_ZIP = $savedPayloadZip
    $env:CARGO_ENCODED_RUSTFLAGS = $savedEncodedRustflags
    # Delete only this invocation's unique directory inside target/package-staging.
    $resolvedStaging = [IO.Path]::GetFullPath($stagingPath)
    if (-not $resolvedStaging.StartsWith($stagingPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw '发行暂存目录超出预期路径，停止清理'
    }
    if (Test-Path -LiteralPath $resolvedStaging) {
        if ((Get-Item -LiteralPath $resolvedStaging -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw '发行暂存目录是目录联接，停止清理'
        }
        Remove-Item -LiteralPath $resolvedStaging -Recurse -Force
    }
}
