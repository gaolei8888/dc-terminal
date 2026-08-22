<#
.SYNOPSIS
    在 Windows 上装 dct。原生版，不需要 WSL。

.DESCRIPTION
    这个脚本的上一版把整件事转给 WSL——那时候 dct 编不出 Windows 版本，
    它整个骨架架在 Unix 上：界面和守护进程之间走 Unix domain socket，
    守护进程的生死靠 kill 和 setsid，密钥文件靠 0600 这个位。

    现在这些都按平台分好了（src/sys/），Windows 上各有各的说法：AF_UNIX
    （Windows 10 1803 起自带）、DETACHED_PROCESS 加 TerminateProcess、
    一条只有当前用户的 ACL。所以这个脚本回到它本来该做的事：编译，装到
    PATH 上，跑一下试试。

    不需要 Visual Studio Build Tools。整棵依赖树里一行 C 都没有（TLS 在
    Windows 上走系统自带的 schannel，绕开了那个要 lib.exe 的 ring），
    rustup 自带的链接器就够。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\install.ps1

.EXAMPLE
    .\scripts\install.ps1 -InstallDir D:\bin
#>
[CmdletBinding()]
param(
	# dct.exe 装到哪。这个目录会被加进用户 PATH。
	[string]$InstallDir = "$env:LOCALAPPDATA\Programs\dct",

	# 不编译，直接装 target\release\dct.exe 里现成的那个。
	[switch]$NoBuild,

	# 不动 PATH。
	[switch]$NoPath
)

$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$Text) Write-Host "==> $Text" -ForegroundColor Cyan }
function Write-Note { param([string]$Text) Write-Host "    $Text" -ForegroundColor DarkGray }
function Write-Warn { param([string]$Text) Write-Host "    $Text" -ForegroundColor Yellow }

function Add-ToUserPath {
	param([string]$Dir)

	# 这里不能用 setx。setx 会把 PATH 截断在 1024 个字符上，多出来的直接
	# 丢掉；而且它写的是展开后的值，%USERPROFILE% 这种会被固化成绝对路径。
	# 两件事都是不可逆的破坏，且要等到用户下次发现某个命令找不到了才暴露。
	# 直接写注册表，并且用 DoNotExpandEnvironmentNames 读、按原来的类型写，
	# REG_EXPAND_SZ 就还是 REG_EXPAND_SZ。
	$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
	if (-not $key) { throw '打不开 HKCU:\Environment，PATH 没法改。' }
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

# ---------------------------------------------------------------- 开始干活

$repoRoot = Split-Path -Parent $PSScriptRoot
Write-Step "仓库：$repoRoot"

if (-not $NoBuild) {
	if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
		Write-Error @'
找不到 cargo。dct 是从源码装的，得先有 Rust。

    winget install --id Rustlang.Rustup -e
    winget install --id BrechtSanders.WinLibs.POSIX.UCRT -e

第二条是链接工具链。装 Rust 的时候如果它问你要不要装 Visual Studio
Build Tools，可以不装——那是几个 GB 加一次管理员提权，而 WinLibs 是
一份解压即用的 mingw，装在用户目录里。用它的话把 Rust 也切成 gnu：

    rustup default stable-x86_64-pc-windows-gnu

两条都装完新开一个窗口再跑一遍这个脚本（PATH 要新窗口才生效）。
'@
		exit 1
	}

	# gnu 工具链缺 as.exe 的话，编译会死在一句谁也看不懂的话上：
	# 「error calling dlltool 'dlltool.exe': program not found」，或者更糟的
	# 「dlltool.exe: CreateProcess」——后者是 dlltool 找到了、但它自己要调的
	# as 没找到。rustup 的 rust-mingw 组件带了 gcc、ld、dlltool，**唯独没带
	# as**，而 windows-sys / getrandom 这些都要靠 dlltool 生成 import 库。
	# 与其让人对着那句话去搜，不如在动手之前就说清楚缺什么。
	$isGnu = ((& cargo.exe -vV 2>&1) -join "`n") -match 'host:\s*\S*windows-gnu'
	if ($isGnu -and -not (Get-Command as.exe -ErrorAction SilentlyContinue)) {
		Write-Error @'
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
		exit 1
	}

	Write-Step '编译（第一次要几分钟）'
	Push-Location $repoRoot
	try {
		& cargo.exe build --release
		if ($LASTEXITCODE -ne 0) { throw "cargo build 失败（退出码 $LASTEXITCODE）。" }
	} finally {
		Pop-Location
	}
}

$built = Join-Path $repoRoot 'target\release\dct.exe'
if (-not (Test-Path $built)) {
	throw "$built 不在那儿。$(if ($NoBuild) { '带了 -NoBuild，但没有现成的二进制可装。' } else { '编译说成功了，但产物不在预期位置。' })"
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
& $dest --help | Out-Null
if ($LASTEXITCODE -ne 0) {
	Write-Error "装是装上了，但 $dest 跑不起来（退出码 $LASTEXITCODE）。"
	exit 1
}

# git 不是编译依赖，是运行时依赖：每一轮对话之前的那次隐藏快照是 shell 出去
# 调 git 做的（src/git.rs）。没有它，撤销就是死的——而撤销正是 dct 敢让
# agent 关掉所有权限确认的全部理由。
if (-not (Get-Command git.exe -ErrorAction SilentlyContinue)) {
	Write-Warn '没找到 git。dct 每轮对话前的快照要靠它，没有 git 就没有撤销。'
	Write-Warn '装一个：winget install --id Git.Git -e'
}

Write-Host ''
Write-Host '装好了。' -ForegroundColor Green
Write-Host '新开一个 PowerShell 或 cmd，进到某个 git 项目里，敲 dct。'
Write-Host ''
Write-Host '板子是全屏 TUI，用 Windows Terminal 开效果最好——老的 conhost' -ForegroundColor DarkGray
Write-Host '窗口画得出来，但配色和边框会难看一些。' -ForegroundColor DarkGray
