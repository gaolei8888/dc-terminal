@echo off
rem Install dct from a cmd.exe prompt.
rem
rem The real work is in install.ps1 next to this file; this exists so that
rem running the installer from cmd is one word, and so that PowerShell's
rem execution policy cannot stop it. On a default Windows install the policy is
rem Restricted, which means double-clicking or dot-sourcing a .ps1 fails with a
rem security error that has nothing to do with dct and sends people looking in
rem the wrong direction. -ExecutionPolicy Bypass applies to this one process
rem only -- nothing about the machine's policy is changed.
rem
rem Comments here are English on purpose: cmd.exe reads batch files in the
rem console code page, so non-ASCII in this file renders as garbage on any
rem machine whose code page differs from the one it was saved in. Every other
rem script in this directory is read by bash or PowerShell, which have no such
rem problem, and is commented in Chinese like the rest of the codebase.

setlocal

set "DCT_PS1=%~dp0install.ps1"

if not exist "%DCT_PS1%" (
	echo install.cmd: cannot find "%DCT_PS1%".
	echo Run this from the scripts\ directory of a dct checkout.
	exit /b 1
)

rem Read the script as UTF-8 and run it as a script block, rather than handing
rem the path to -File.
rem
rem install.ps1 carries no byte order mark, because it must survive being piped
rem through `irm ... | iex`, where a BOM becomes part of the first command name
rem and the failure names nothing you could act on. Without a BOM, though,
rem Windows PowerShell 5.1 falls back to the machine's ANSI code page when it
rem reads a .ps1 from disk, and every Chinese line the installer prints comes
rem out as garbage. Reading the bytes ourselves and naming the encoding settles
rem it for both 5.1 and 7, BOM or no BOM.
rem
rem -Command must come last: everything after it on the command line is joined
rem into one command string, which is how %* reaches the script block as its
rem own arguments (-InstallDir D:\bin and friends still work).
rem
rem -NoProfile: the user's PowerShell profile has no business running here, and
rem a slow or broken one would look like the installer hanging or failing.
rem
rem Prefer PowerShell 7 (pwsh) when it is installed, fall back to the Windows
rem PowerShell that every machine has.
rem A script block has no idea which file it came from, so $PSScriptRoot is
rem empty even here, in a real checkout. Hand the repo root over explicitly, or
rem -Build would claim it cannot find a repo while standing inside one.
rem setlocal above keeps this out of the caller's environment.
set "DCT_REPO_ROOT=%~dp0.."

set "DCT_RUN=& ([scriptblock]::Create([IO.File]::ReadAllText('%DCT_PS1%',[Text.Encoding]::UTF8)))"

where /q pwsh.exe
if %ERRORLEVEL%==0 goto :pwsh

powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "%DCT_RUN%" %*
exit /b %ERRORLEVEL%

:pwsh
pwsh.exe -NoProfile -ExecutionPolicy Bypass -Command "%DCT_RUN%" %*
exit /b %ERRORLEVEL%
