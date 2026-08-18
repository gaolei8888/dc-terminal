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

rem -NoProfile: the user's PowerShell profile has no business running here, and
rem a slow or broken one would look like the installer hanging or failing.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%DCT_PS1%" %*

exit /b %ERRORLEVEL%
