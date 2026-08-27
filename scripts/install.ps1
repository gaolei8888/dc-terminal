<#
.SYNOPSIS
    在 Windows 上装 dct。默认下现成的二进制，下不到才回头编译。

.DESCRIPTION
    有两种用法，而且必须两种都成立：

        irm https://raw.githubusercontent.com/gaolei8888/dc-terminal/main/scripts/install.ps1 | iex
        scripts\install.cmd          # 在 clone 好的仓库里

    第一种是给学生的。以前这里只有第二种，于是装 dct 的第一步是装 rustup、
    装 WinLibs、装 git——几个 GB 加一堆能装错的地方，而学生要的只是一个几
    MB 的 exe。

    这个文件**故意不带 BOM**。`irm | iex` 拿到的是一个字符串，开头那三个
    字节会被当成命令名的一部分，报一句「无法将﻿param 项识别为 cmdlet」——
    跟真实原因对不上号。代价是 Windows PowerShell 5.1 直接 `.\install.ps1`
    时会按 ANSI 读它，中文变乱码；所以仓库里那条路走 `install.cmd`，
    它显式按 UTF-8 读这个文件（见那边的注释）。

    不需要 Visual Studio Build Tools。整棵依赖树里一行 C 都没有（TLS 在
    Windows 上走系统自带的 schannel，绕开了那个要 lib.exe 的 ring）。
    而走预编译那条路的话，连 Rust 都不需要。

.EXAMPLE
    irm https://raw.githubusercontent.com/gaolei8888/dc-terminal/main/scripts/install.ps1 | iex

.EXAMPLE
    .\scripts\install.ps1 -InstallDir D:\bin
#>
[CmdletBinding()]
param(
	# dct.exe 装到哪。这个目录会被加进用户 PATH。
	[string]$InstallDir = "$env:LOCALAPPDATA\Programs\dct",

	# 不下现成的，从源码编译（要 Rust，且要在 clone 好的仓库里）。
	[switch]$Build,

	# 既不下也不编，装 target\release\dct.exe 里现成的那个。
	[switch]$NoBuild,

	# 不动 PATH。
	[switch]$NoPath,

	# 缺 git 时不要自动装那份便携版，只提醒一句。
	[switch]$NoGit,

	# 换成自己的镜像。这个口子是为国内的课堂留的：GitHub 在教室的网络里
	# 经常慢到不可用，老师可以把包和 SHA256SUMS 原样放到任何一个能下的
	# 地方，学生那条命令一个字都不用改。
	[string]$ReleaseBase,

	# 同上，给那份便携 git 用。
	[string]$GitZipUrl
)

$ErrorActionPreference = 'Stop'

# Windows PowerShell 5.1 默认还在用 TLS 1.0 谈握手，而 GitHub 早就不收了。
# 不设这一行，下载会失败在一句「基础连接已经关闭」上——那句话不会提到 TLS
# 一个字，是这台机器上最难查的一类报错。PowerShell 7 不需要，设了也无害。
try {
	[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.ServicePointManager]::SecurityProtocol
} catch { }

# Invoke-WebRequest 画进度条这件事在 5.1 上会让下载慢一个数量级（每收一块
# 就重绘一次控制台）。那份便携 git 有 45 MB，差别是一分钟和十几秒。
$ProgressPreference = 'SilentlyContinue'

$DefaultReleaseBase = 'https://github.com/gaolei8888/dc-terminal/releases/latest/download'
# 钉死版本，不去查「最新的 git 是哪个」：那要么查 API（有速率限制），
# 要么每次都可能下到一个没人试过的版本。dct 用到 git 的地方只有
# 「拍快照、回退」，这份 2.47.1 够用到天荒地老。下不到会退回去只提醒一句，
# 所以这个 URL 哪天失效了，最坏的结果也只是回到今天的行为。
$DefaultGitZipUrl = 'https://github.com/git-for-windows/git/releases/download/v2.47.1.windows.1/MinGit-2.47.1-64-bit.zip'

if (-not $ReleaseBase) {
	$ReleaseBase = if ($env:DCT_RELEASE_BASE) { $env:DCT_RELEASE_BASE } else { $DefaultReleaseBase }
}
if (-not $GitZipUrl) {
	$GitZipUrl = if ($env:DCT_MINGIT_URL) { $env:DCT_MINGIT_URL } else { $DefaultGitZipUrl }
}

function Write-Step { param([string]$Text) Write-Host "==> $Text" -ForegroundColor Cyan }
function Write-Note { param([string]$Text) Write-Host "    $Text" -ForegroundColor DarkGray }
function Write-Warn { param([string]$Text) Write-Host "    $Text" -ForegroundColor Yellow }

# 走不下去时用这个，不要用 throw。throw 出来的东西 PowerShell 会连着
# 「Exception:」、行号、和一段指着 install.ps1 源码的箭头一起印出来——
# 对一个刚想装个东西的人来说，那三行的作用只有一个：让他以为自己把
# 什么弄坏了。这里印的是给人看的话，退出码留给脚本看。
function Stop-WithMessage {
	param([string]$Text)
	Write-Host ''
	Write-Host $Text -ForegroundColor Yellow
	exit 1
}

function Add-ToUserPath {
	param([string]$Dir)

	# 这里不能用 setx。setx 会把 PATH 截断在 1024 个字符上，多出来的直接
	# 丢掉；而且它写的是展开后的值，%USERPROFILE% 这种会被固化成绝对路径。
	# 两件事都是不可逆的破坏，且要等到用户下次发现某个命令找不到了才暴露。
	# 直接写注册表，并且用 DoNotExpandEnvironmentNames 读、按原来的类型写，
	# REG_EXPAND_SZ 就还是 REG_EXPAND_SZ。
	$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
	if (-not $key) { Stop-WithMessage '改不了 PATH（打不开当前用户的环境变量）。加上 -NoPath 可以跳过这一步，但那样敲 dct 会找不到，得用完整路径。' }
	try {
		$current = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
		try { $kind = $key.GetValueKind('Path') }
		catch { $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString }

		foreach ($part in ($current -split ';')) {
			if ($part.Trim() -eq '') { continue }
			if ($part.Trim().TrimEnd('\') -ieq $Dir.TrimEnd('\')) { return $false }
		}

		if ($current -eq '') { $updated = $Dir }
		elseif ($current.EndsWith(';')) { $updated = $current + $Dir }
		else { $updated = $current + ';' + $Dir }

		$key.SetValue('Path', $updated, $kind)
		return $true
	} finally {
		$key.Close()
	}
}

function Publish-EnvChange {
	# 光写注册表，已经开着的进程（资源管理器、别的终端）不会知道。广播一下
	# WM_SETTINGCHANGE，新开的窗口就能拿到新 PATH，不用注销重登。
	if (-not ('DctEnv.Native' -as [type])) {
		Add-Type -Namespace DctEnv -Name Native -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
'@
	}
	$result = [System.UIntPtr]::Zero
	# HWND_BROADCAST = 0xffff, WM_SETTINGCHANGE = 0x1A, SMTO_ABORTIFHUNG = 2
	[void][DctEnv.Native]::SendMessageTimeout([System.IntPtr]0xffff, 0x1A, [System.UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
}

# 下预编译包，验校验和，解出 dct.exe，返回它的路径。下不到就返回 $null——
# 由调用方决定要不要回头编译，这不是异常，是最常见的那条岔路。
#
# 校验和是**不能跳过**的一步：下到一半断了，和下到一个被人换过的文件，
# 在解压之前长得一模一样，而这个文件接下来会被放进 PATH 天天执行。
function Get-Prebuilt {
	param([string]$Workdir)

	# ARM64 的 Windows 跑 x64 是靠系统自带的模拟，透明且够快，所以不必
	# 单独出一个 ARM 包——多一个包就多一个没人验过的构建。
	$asset = 'dct-x86_64-pc-windows-msvc.zip'
	$zip = Join-Path $Workdir $asset
	$sums = Join-Path $Workdir 'SHA256SUMS'

	Write-Step "下预编译包：$asset"
	try {
		Invoke-WebRequest -Uri "$ReleaseBase/$asset" -OutFile $zip -UseBasicParsing
		Invoke-WebRequest -Uri "$ReleaseBase/SHA256SUMS" -OutFile $sums -UseBasicParsing
	} catch {
		# 原始报错是一句英文的 socket 错误（「目标计算机积极拒绝」之类），
		# 对着它没有任何人能采取下一步行动。真正有用的只有「从哪儿没下到」。
		# 技术细节留给 -Verbose：需要的人问得到，不需要的人看不见。
		Write-Note "没下到：$ReleaseBase"
		Write-Verbose $_.Exception.Message
		return $null
	}

	$want = $null
	foreach ($line in (Get-Content -LiteralPath $sums)) {
		# 一行是「<哈希>  <文件名>」。按空白切，取头尾两段。
		$parts = $line -split '\s+', 2
		if ($parts.Count -eq 2 -and $parts[1].Trim() -eq $asset) { $want = $parts[0].Trim(); break }
	}
	if (-not $want) {
		Write-Note "SHA256SUMS 里没有 $asset 这一条，没法验，不装它。"
		return $null
	}

	$got = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
	if ($got -ine $want) {
		Write-Note '下回来的文件跟官方校验和对不上，不装它。'
		Write-Note '多半是下到一半断了，重跑一次；一直对不上就换个网络。'
		return $null
	}

	$unpacked = Join-Path $Workdir 'unpacked'
	Expand-Archive -LiteralPath $zip -DestinationPath $unpacked -Force
	$exe = Join-Path $unpacked 'dct.exe'
	if (-not (Test-Path $exe)) {
		Write-Note '包里没有 dct.exe。'
		return $null
	}
	return $exe
}

function Invoke-SourceBuild {
	param([string]$RepoRoot)

	if (-not $RepoRoot) {
		Stop-WithMessage @'
这里不是 dct 的仓库，编不了。

先把仓库拿下来：

    git clone https://github.com/gaolei8888/dc-terminal
    cd dc-terminal
    scripts\install.cmd -Build
'@
	}

	if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
		Stop-WithMessage @'
找不到 cargo。要从源码装 dct，得先有 Rust。

    winget install --id Rustlang.Rustup -e
    winget install --id BrechtSanders.WinLibs.POSIX.UCRT -e

第二条是链接工具链。装 Rust 的时候如果它问你要不要装 Visual Studio
Build Tools，可以不装——那是几个 GB 加一次管理员提权，而 WinLibs 是
一份解压即用的 mingw，装在用户目录里。用它的话把 Rust 也切成 gnu：

    rustup default stable-x86_64-pc-windows-gnu

两条都装完新开一个窗口再跑一遍这个脚本（PATH 要新窗口才生效）。

顺带一提：不带 -Build 的话根本不用编译，直接下现成的就行。
'@
	}

	# gnu 工具链缺 as.exe 的话，编译会死在一句谁也看不懂的话上：
	# 「error calling dlltool 'dlltool.exe': program not found」，或者更糟的
	# 「dlltool.exe: CreateProcess」——后者是 dlltool 找到了、但它自己要调的
	# as 没找到。rustup 的 rust-mingw 组件带了 gcc、ld、dlltool，**唯独没带
	# as**，而 windows-sys / getrandom 这些都要靠 dlltool 生成 import 库。
	# 与其让人对着那句话去搜，不如在动手之前就说清楚缺什么。
	$isGnu = ((& cargo.exe -vV 2>&1) -join "`n") -match 'host:\s*\S*windows-gnu'
	if ($isGnu -and -not (Get-Command as.exe -ErrorAction SilentlyContinue)) {
		Stop-WithMessage @'
Rust 用的是 gnu 工具链，但 PATH 上没有 as.exe（GNU 汇编器）。

rustup 自带的那份 mingw 是精简过的：有 gcc、ld、dlltool，唯独没有 as，
而 dlltool 生成 import 库时要调它。缺了它，编译会停在一句
「dlltool.exe: CreateProcess」上，那句话跟真实原因对不上号。

装一份完整的 mingw（解压即用，装在用户目录，不需要管理员）：

    winget install --id BrechtSanders.WinLibs.POSIX.UCRT -e

装完新开一个窗口再跑一遍这个脚本。

已经有 Visual Studio Build Tools 的话，切回 msvc 也行，那条路不需要 as：

    rustup default stable-x86_64-pc-windows-msvc
'@
	}

	Write-Step '编译（第一次要几分钟）'
	Push-Location $RepoRoot
	try {
		& cargo.exe build --release
		if ($LASTEXITCODE -ne 0) { Stop-WithMessage "编译失败了（退出码 $LASTEXITCODE）。上面 cargo 自己印的那几行才是原因。" }
	} finally {
		Pop-Location
	}
	return (Join-Path $RepoRoot 'target\release\dct.exe')
}

# git 不是编译依赖，是运行时依赖：每一轮对话之前的那次隐藏快照是 shell 出去
# 调 git 做的（src/git.rs）。没有它，撤销就是死的——而撤销正是 dct 敢让
# agent 关掉所有权限确认的全部理由。
#
# 所以这里不是提醒一句就算，而是真的装一份。装的是 MinGit：一个解压即用的
# 便携版，不写注册表、不要管理员、不碰系统里已有的任何东西，整个躺在 dct
# 自己的安装目录下面。用户哪天想删 dct，连它一起删掉就干净了。
function Install-PortableGit {
	param([string]$InstallDir)

	$gitRoot = Join-Path $InstallDir 'git'
	$gitCmd = Join-Path $gitRoot 'cmd'

	# 上次装过就不必再来一遍。认的是 git.exe 在不在，不是目录在不在——
	# 上次解压到一半断电的话，目录是在的，而里面没有能跑的东西。
	if (-not (Test-Path (Join-Path $gitCmd 'git.exe'))) {
		Write-Step '这台电脑上没有 git，装一份便携版给 dct 用（45 MB 左右）'
		Write-Note 'dct 每轮对话前会给你的项目拍一张隐藏快照，靠的就是它。'
		Write-Note "装在 $gitRoot，不碰系统里别的东西。"

		$tmpZip = Join-Path ([IO.Path]::GetTempPath()) "MinGit-$PID.zip"
		try {
			Invoke-WebRequest -Uri $GitZipUrl -OutFile $tmpZip -UseBasicParsing
			# 解到一个新目录再挪过去，中途断了不会留下一个半拉的 git
			# 骗过上面那个 Test-Path。
			$staging = Join-Path ([IO.Path]::GetTempPath()) "MinGit-$PID"
			if (Test-Path $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
			Expand-Archive -LiteralPath $tmpZip -DestinationPath $staging -Force
			if (Test-Path $gitRoot) { Remove-Item -LiteralPath $gitRoot -Recurse -Force }
			Move-Item -LiteralPath $staging -Destination $gitRoot
		} catch {
			Write-Warn '那份便携 git 没下来（多半是网络不通）。'
			Write-Verbose $_.Exception.Message
			Write-Warn '没有 git 的话，dct 里的撤销是死的。自己装一个也行：'
			Write-Warn '  winget install --id Git.Git -e'
			return $false
		} finally {
			if (Test-Path $tmpZip) { Remove-Item -LiteralPath $tmpZip -Force -ErrorAction SilentlyContinue }
		}
	}

	if (-not $NoPath) {
		if (Add-ToUserPath -Dir $gitCmd) { Publish-EnvChange }
		# 这个进程自己也要能看见它，下面那句「跑一下试试」才验得到。
		$env:Path = $env:Path + ';' + $gitCmd
	}
	return $true
}

# ---------------------------------------------------------------- 开始干活

# 在仓库里跑，还是从管道里跑？`irm | iex` 下 $PSScriptRoot 是空的，那时候
# 就没有仓库，也就没有源码可编——这不是错误，是最常见的那条路。
#
# $PSScriptRoot 不是唯一的来源，因为它在**仓库里那条路上也是空的**：
# install.cmd 为了指定编码，是把这个文件读成字符串再当 scriptblock 跑的
# （见那边的注释），而 scriptblock 没有「自己在哪个文件里」这回事。所以
# install.cmd 顺手把仓库根写进 DCT_REPO_ROOT。两个来源都认，谁先命中算谁。
$repoRoot = $null
foreach ($maybe in @($(if ($PSScriptRoot) { Split-Path -Parent $PSScriptRoot }), $env:DCT_REPO_ROOT)) {
	if ($maybe -and (Test-Path (Join-Path $maybe 'Cargo.toml'))) {
		$repoRoot = (Resolve-Path -LiteralPath $maybe).Path
		break
	}
}
if ($repoRoot) { Write-Step "仓库：$repoRoot" }

if ($Build -and $NoBuild) {
	Stop-WithMessage '-Build 和 -NoBuild 是相反的两件事，一次只能给一个。'
}

$workdir = $null
$built = $null

if ($NoBuild) {
	if (-not $repoRoot) { Stop-WithMessage '-NoBuild 是装 target\release\dct.exe 里现成的那个，但这里不是 dct 的仓库，没有那个文件。' }
	$built = Join-Path $repoRoot 'target\release\dct.exe'
	if (-not (Test-Path $built)) { Stop-WithMessage "$built 不在那儿。去掉 -NoBuild 让它先装。" }
} elseif ($Build) {
	$built = Invoke-SourceBuild -RepoRoot $repoRoot
} else {
	$workdir = Join-Path ([IO.Path]::GetTempPath()) "dct-install-$PID"
	New-Item -ItemType Directory -Path $workdir -Force | Out-Null
	$built = Get-Prebuilt -Workdir $workdir
	if (-not $built) {
		if ($repoRoot -and (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
			Write-Note '回头从源码编译。'
			$built = Invoke-SourceBuild -RepoRoot $repoRoot
		} else {
			Stop-WithMessage @'
装不上：没下到预编译包，这台机器也没有从源码编译的条件。

能试的两条路：
  1. 网络的问题居多，过一会儿重跑一次同一条命令。
  2. 老师给了内网地址的话，这样用：
       $env:DCT_RELEASE_BASE = '老师给的地址'
     再重跑一遍上面那条命令。
'@
		}
	}
}

if (-not (Test-Path $built)) {
	Stop-WithMessage "$built 不在那儿。"
}

Write-Step "装到 $InstallDir"
if (-not (Test-Path $InstallDir)) {
	New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$dest = Join-Path $InstallDir 'dct.exe'
$stage = Join-Path $InstallDir "dct-new-$PID.exe"
$retired = Join-Path $InstallDir "dct-old-$PID.exe"

# 不能直接覆盖：dct 留着一个常驻守护进程，跑的很可能就是这个文件，而
# Windows 不让你写一个正在执行的映像（拿到的是 sharing violation，报错
# 文字还完全看不出是这个原因）。**但改名是允许的**——正在跑的进程认的是
# 文件本身，不是它叫什么。所以先把老的挪开，再把新的搬进来：跑着的那个
# 守护进程继续用它手里那份，下一次启动才会碰到新的。
Copy-Item -LiteralPath $built -Destination $stage -Force
if (Test-Path $dest) {
	Move-Item -LiteralPath $dest -Destination $retired -Force
}
Move-Item -LiteralPath $stage -Destination $dest -Force

# 老的那份等它彻底没人用了才删得掉。删不掉不是错误，下次装的时候顺手再
# 试一次——所以这里连同上几次留下的一起扫。
Get-ChildItem -LiteralPath $InstallDir -Filter 'dct-old-*.exe' -ErrorAction SilentlyContinue | ForEach-Object {
	try { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction Stop } catch { }
}
Write-Note $dest

if ($workdir -and (Test-Path $workdir)) {
	Remove-Item -LiteralPath $workdir -Recurse -Force -ErrorAction SilentlyContinue
}

# 上一版的安装器往这个目录里放过一个 dct.cmd，作用是把命令转发进 WSL。
# 它和新装的 dct.exe 同名同目录，只差扩展名：PATHEXT 里 .EXE 排在 .CMD
# 前面，所以敲 dct 跑的是新的那个——但「到底跑的是哪个」不该是一个需要
# 查 PATHEXT 才答得上来的问题，而且那个 shim 现在指向的东西已经没人维护了。
#
# 只删我们自己生成的那一个：认那行 wsl.exe。用户自己放的同名文件不动。
$legacyShim = Join-Path $InstallDir 'dct.cmd'
if (Test-Path $legacyShim) {
	$body = Get-Content -LiteralPath $legacyShim -Raw -ErrorAction SilentlyContinue
	if ($body -and $body -match 'wsl\.exe') {
		Remove-Item -LiteralPath $legacyShim -Force
		Write-Note '顺手删掉了上一版留下的 dct.cmd（那是转发进 WSL 的 shim，现在用不上了）。'
	}
}

if (-not $NoPath) {
	Write-Step '把它加进 PATH'
	if (Add-ToUserPath -Dir $InstallDir) {
		Publish-EnvChange
		Write-Note '加好了。已经开着的窗口读不到，新开一个 PowerShell 或 cmd 才有。'
		$env:Path = $env:Path + ';' + $InstallDir
	} else {
		Write-Note "$InstallDir 本来就在 PATH 上，没动。"
	}
}

Write-Step '跑一下试试'
$version = (& $dest --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
	Write-Error "装是装上了，但 $dest 跑不起来（退出码 $LASTEXITCODE）。"
	exit 1
}
Write-Note $version

# 真的跑一次 `git --version`，不是只看 PATH 上有没有这个名字：装了别的
# 工具链（比如某些 IDE 自带的壳）时，名字在而跑不起来是有可能的，而
# dct 要的是**跑得起来**的那一个。
$hasGit = $false
try {
	& git --version *> $null
	$hasGit = ($LASTEXITCODE -eq 0)
} catch { $hasGit = $false }

if (-not $hasGit) {
	if ($NoGit) {
		Write-Warn '没找到 git。dct 每轮对话前的快照要靠它，没有 git 就没有撤销。'
		Write-Warn '装一个：winget install --id Git.Git -e'
	} else {
		[void](Install-PortableGit -InstallDir $InstallDir)
	}
}

Write-Host ''
Write-Host '装好了。' -ForegroundColor Green
Write-Host '新开一个 PowerShell 或 cmd，进到任何一个文件夹里，敲 dct。'
Write-Host ''
Write-Host '板子是全屏 TUI，用 Windows Terminal 开效果最好——老的 conhost' -ForegroundColor DarkGray
Write-Host '窗口画得出来，但配色和边框会难看一些。' -ForegroundColor DarkGray
