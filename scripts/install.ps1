<#
.SYNOPSIS
    在 Windows 上装 dct。

.DESCRIPTION
    dct 现在编不出 Windows 版本，这不是"还没顾上"，是它整个骨架就架在
    Unix 上：界面和守护进程之间走的是 Unix domain socket（src/client.rs、
    src/daemon.rs），守护进程的生死靠 libc::kill 和 setsid（src/client.rs、
    src/pty.rs），密钥文件靠 0600 这个位（src/secrets.rs）。src 底下一个
    #[cfg(windows)] 都没有。所以在 Windows 上 cargo build 是过不去的，
    而没有二进制，再漂亮的安装包也没有东西可装。

    能跑的地方是 WSL。所以这个脚本做的是 Windows 这一半的活：挑一个发行版、
    把仓库路径翻译过去、把真正的编译安装交给 Linux 那边的 install.sh，
    最后在 Windows 的 PATH 上留一个 dct.cmd。留这个 cmd 的意思是，装完
    之后在 PowerShell 里敲 dct，和在 cmd 里敲 dct，和在 WSL 里敲 dct，
    是同一件事——中间那层 wsl.exe 不该需要用户自己记得。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\install.ps1

.EXAMPLE
    .\scripts\install.ps1 -Distro Ubuntu-22.04
#>
[CmdletBinding()]
param(
	# 装到哪个发行版里。不给就用 WSL 的默认发行版。
	[string]$Distro,

	# WSL 里二进制装到哪。不给就是 install.sh 的默认值 ~/.local/bin。
	[string]$InstallDir,

	# dct.cmd 放在哪。这个目录会被加进用户 PATH。
	[string]$ShimDir = "$env:LOCALAPPDATA\Programs\dct",

	# 不编译，直接装 target/release/dct 里现成的那个。
	[switch]$NoBuild,

	# 不动 PATH。
	[switch]$NoPath,

	# 不生成 dct.cmd，只在 WSL 里装。
	[switch]$NoShim
)

$ErrorActionPreference = 'Stop'

# wsl.exe 默认吐 UTF-16LE，PowerShell 5.1 按当前控制台代码页读，读出来
# 每个字符中间夹一个 NUL，正则全废。WSL_UTF8=1 让它改吐 UTF-8（新一点的
# WSL 都认），OutputEncoding 这边跟着改成 UTF-8 才对得上。老 WSL 不认这个
# 变量，所以下面解析的时候还留了一道去 NUL 的兜底——两手都要有。
$env:WSL_UTF8 = '1'
$script:PrevEncoding = [Console]::OutputEncoding

function Write-Step { param([string]$Text) Write-Host "==> $Text" -ForegroundColor Cyan }
function Write-Note { param([string]$Text) Write-Host "    $Text" -ForegroundColor DarkGray }

function Get-WslDistros {
	[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
	try {
		$raw = & wsl.exe -l -v
	} finally {
		[Console]::OutputEncoding = $script:PrevEncoding
	}
	if ($LASTEXITCODE -ne 0) { throw "wsl -l -v 失败（退出码 $LASTEXITCODE），WSL 可能没装好。" }

	$list = @()
	foreach ($line in $raw) {
		$clean = ($line -replace "`0", '').Trim()
		if ($clean -eq '') { continue }

		$isDefault = $clean.StartsWith('*')
		$fields = ($clean.TrimStart('*').Trim()) -split '\s+'
		if ($fields.Count -lt 3) { continue }

		# 表头那行（NAME STATE VERSION，或者中文的）字段数一样，靠最后
		# 一列是不是数字来甩掉——版本号永远是 1 或 2。
		$version = 0
		if (-not [int]::TryParse($fields[-1], [ref]$version)) { continue }

		$list += [pscustomobject]@{
			Name      = $fields[0]
			State     = $fields[-2]
			Version   = $version
			IsDefault = $isDefault
		}
	}
	return $list
}

function Select-Distro {
	param([object[]]$All, [string]$Wanted)

	# docker-desktop 是 Docker Desktop 自己的后端，不是给人用的发行版：
	# 里头没有包管理器，装什么都装不进去。默认挑发行版的时候必须绕开它，
	# 否则一台装了 Docker 的机器上，"默认发行版"很可能就是它。
	$usable = @($All | Where-Object { $_.Name -notlike 'docker-desktop*' })

	if ($Wanted) {
		$hit = @($All | Where-Object { $_.Name -ieq $Wanted })
		if ($hit.Count -eq 0) {
			$names = ($All | ForEach-Object { $_.Name }) -join ', '
			throw "找不到发行版 '$Wanted'。这台机器上有：$names"
		}
		return $hit[0]
	}

	if ($usable.Count -eq 0) {
		throw "WSL 里没有能用的 Linux 发行版。先跑 wsl --install -d Ubuntu，装完再回来。"
	}

	$default = @($usable | Where-Object { $_.IsDefault })
	if ($default.Count -gt 0) { return $default[0] }
	return $usable[0]
}

function Invoke-WslCapture {
	param([string]$DistroName, [string[]]$Arguments)
	[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
	try {
		$out = & wsl.exe -d $DistroName -- @Arguments
	} finally {
		[Console]::OutputEncoding = $script:PrevEncoding
	}
	if ($LASTEXITCODE -ne 0) {
		throw "在 $DistroName 里跑 '$($Arguments -join ' ')' 失败（退出码 $LASTEXITCODE）。"
	}
	return (($out -join "`n") -replace "`0", '').Trim()
}

function ConvertTo-BashSingleQuoted {
	param([string]$Text)
	return "'" + ($Text -replace "'", "'\''") + "'"
}

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

if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) {
	Write-Error @'
找不到 wsl.exe。

dct 跑在 Linux 上（原因见这个脚本开头的说明），Windows 这边要靠 WSL。
用管理员权限开一个 PowerShell，跑：

    wsl --install -d Ubuntu

装完重启，再回来跑这个脚本。
'@
	exit 1
}

Write-Step '找 WSL 发行版'
$distros = Get-WslDistros
$target = Select-Distro -All $distros -Wanted $Distro
Write-Note "用 $($target.Name)（WSL $($target.Version)，$($target.State)）"

if ($target.Version -lt 2) {
	Write-Note "提醒：这是 WSL 1。dct 能跑，但文件操作和 pty 都比 WSL 2 慢不少。"
	Write-Note "想升：wsl --set-version $($target.Name) 2"
}

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path (Join-Path $repoRoot 'Cargo.toml'))) {
	throw "$repoRoot 底下没有 Cargo.toml，这个脚本得放在 dct 仓库的 scripts\ 里跑。"
}

Write-Step '把仓库路径翻译成 WSL 路径'

# 反斜杠必须先换成正斜杠，不能原样递给 wslpath。
#
# 经 wsl.exe 传到 Linux 那边的参数里，反斜杠会被吃掉：C:\Users\gaole 到了
# wslpath 手里是 C:Usersgaole，于是它报"路径不存在"，而错误信息里的路径
# 看着跟你敲的几乎一样，只是少了几个符号——这种错最难认。
#
# wslpath 本来就收正斜杠形式的 Windows 路径（C:/Users/...），带空格也照样
# 认。所以在 Windows 这边换完再递过去，那一层吃字符的逻辑就没东西可吃。
$repoForWsl = $repoRoot -replace '\\', '/'
$repoWsl = Invoke-WslCapture -DistroName $target.Name -Arguments @('wslpath', '-a', $repoForWsl)
Write-Note $repoWsl

if ($repoWsl.StartsWith('/mnt/')) {
	Write-Note '仓库在 Windows 盘上，走的是 9p，cargo 编译会明显慢（几分钟起）。'
	Write-Note '嫌慢就把仓库 clone 到 WSL 自己的文件系统里（比如 ~/src/dc-terminal）再装。'
}

Write-Step "在 $($target.Name) 里装工具链并编译"
Write-Note '第一次会久，可能要 sudo 密码。'

$installArgs = ''
if ($NoBuild) { $installArgs += ' --no-build' }
if ($InstallDir) { $installArgs += ' --dir ' + (ConvertTo-BashSingleQuoted $InstallDir) }

# 两句串在同一个 bash 里跑，不是两次 wsl.exe 调用：install-wsl-deps.sh 装完
# rustup 之后是靠 export PATH 把 ~/.cargo/bin 交给 install.sh 的，而 export
# 只在同一个进程链里有效。拆开就断了。
# -l 是为了读 ~/.profile，之前装过 Rust 的机器上 cargo 是从那儿上 PATH 的。
$bash = 'cd ' + (ConvertTo-BashSingleQuoted $repoWsl) + ' && bash scripts/install-wsl-deps.sh && bash scripts/install.sh' + $installArgs
& wsl.exe -d $target.Name -- bash -lc $bash
if ($LASTEXITCODE -ne 0) {
	Write-Error "WSL 里的安装失败了（退出码 $LASTEXITCODE）。上面那几行是原因。"
	exit 1
}

Write-Step '找装好的二进制'
if ($InstallDir) {
	$binDirWsl = $InstallDir
} else {
	# 问 HOME 用 printenv，不用 sh -c 'printf %s "$HOME"'。参数里只要出现
	# 双引号，PowerShell 5.1 给原生程序拼命令行时就会自作主张地加反斜杠转义，
	# 而反斜杠又活不过 wsl.exe 那一层，两下一凑参数就变了形。printenv 是个
	# 真程序，不经过 shell，也就没有任何需要引号的东西。
	$binDirWsl = (Invoke-WslCapture -DistroName $target.Name -Arguments @('printenv', 'HOME')) + '/.local/bin'
}
$binWsl = "$binDirWsl/dct"

& wsl.exe -d $target.Name -- sh -c ('test -x ' + (ConvertTo-BashSingleQuoted $binWsl))
if ($LASTEXITCODE -ne 0) {
	throw "$binWsl 不在那儿，或者不可执行。install.sh 说装好了，但东西不在预期位置。"
}
Write-Note $binWsl

if (-not $NoShim) {
	Write-Step "写 dct.cmd 到 $ShimDir"
	if (-not (Test-Path $ShimDir)) {
		New-Item -ItemType Directory -Path $ShimDir -Force | Out-Null
	}

	# 只写 .cmd，不写 .ps1。PowerShell 和 cmd 都会执行 PATH 上的 .cmd，
	# 一个就够；两个同名不同后缀摆在同一个目录里，只会让"到底跑的是哪个"
	# 变成一个需要查的问题，而 dct 没有任何参数补全值得为此付这个代价。
	$shim = @"
@echo off
rem dct is installed inside WSL ($($target.Name)); this file just hands the
rem command over. Generated by scripts\install.ps1 -- do not edit, a reinstall
rem overwrites it.
setlocal

rem Carry the current directory across. dct takes cwd as its starting project
rem (src/main.rs), so the board should open on whatever project you typed in.
set "DCT_CD=%CD%"

rem A UNC path (\\server\share) has no place in WSL. wsl.exe does survive an
rem untranslatable --cd -- it prints "Failed to translate ..." and starts in the
rem home directory anyway -- so this is not about avoiding a crash. It is about
rem not printing an alarming line above the board every single time.
if "%DCT_CD:~0,2%"=="\\" set "DCT_CD=%USERPROFILE%"

wsl.exe -d $($target.Name) --cd "%DCT_CD%" -- $binWsl %*
exit /b %ERRORLEVEL%
"@

	# 纯 ASCII 写出去。cmd.exe 按当前控制台代码页读批处理文件，UTF-8 的
	# BOM 会被当成一条命令，第一行就报错；中文注释在 GBK 和 UTF-8 之间
	# 也来回错位。这个文件是唯一一个不由 PowerShell 解释、而由 cmd 解释
	# 的产物，所以它里面的注释写英文——不是风格问题，是编码问题。
	# 顺手把行尾统一成 CRLF。这个脚本本身在仓库里可能是 LF 的（跨平台仓库
	# 常见），here-string 会原样带出来；批处理文件用 LF 大多数时候没事，
	# 但 cmd 解析块结构时对行尾是敏感的，赌不值得。
	$shim = (($shim -replace "`r`n", "`n") -replace "`n", "`r`n")

	$shimPath = Join-Path $ShimDir 'dct.cmd'
	[System.IO.File]::WriteAllText($shimPath, $shim, (New-Object System.Text.ASCIIEncoding))
	Write-Note $shimPath

	if (-not $NoPath) {
		Write-Step '把它加进 PATH'
		if (Add-ToUserPath -Dir $ShimDir) {
			Publish-EnvChange
			Write-Note '加好了。已经开着的窗口读不到，新开一个 PowerShell 或 cmd 才有。'
			$env:Path = $env:Path + ';' + $ShimDir
		} else {
			Write-Note "$ShimDir 本来就在 PATH 上，没动。"
		}
	}
}

Write-Step '跑一下试试'
& wsl.exe -d $target.Name -- $binWsl --help | Out-Null
if ($LASTEXITCODE -ne 0) {
	Write-Error "装是装上了，但 $binWsl 跑不起来（退出码 $LASTEXITCODE）。"
	exit 1
}

Write-Host ''
Write-Host '装好了。' -ForegroundColor Green
if ($NoShim) {
	Write-Host '在 WSL 里敲 dct。Windows 这边没有入口（-NoShim）。'
} else {
	Write-Host '新开一个 PowerShell 或 cmd，敲 dct。'
	Write-Host ''
	Write-Host '板子是全屏 TUI，用 Windows Terminal 开效果最好——老的 conhost' -ForegroundColor DarkGray
	Write-Host '窗口画得出来，但配色和边框会难看一些。' -ForegroundColor DarkGray
}
