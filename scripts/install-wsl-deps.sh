#!/usr/bin/env bash
#
# 在 WSL 里备齐编译和运行 dct 需要的东西。
#
# 拆成单独一个文件，是因为 install.ps1 是从 Windows 那边过来的，它对
# 发行版里有什么一无所知。一个刚 `wsl --install` 出来的 Ubuntu 里没有
# cc，没有 cargo，git 装了但不一定。install.sh 只管编译和安装：缺 cargo
# 的时候它指一句 rustup.rs 就退出了——那句话在终端里看是清楚的，但从
# PowerShell 隔着 wsl.exe 传出来，用户看到的就是一行没头没尾的中文加
# 一个非零退出码，然后不知道该干什么。这个脚本负责把那一步真的做掉。
#
# 三样东西，缺一不可，而且缺的理由各不相同：
#
#   cc    ureq 的 TLS 走 rustls，rustls 底下是 ring，ring 里有 C 和汇编。
#         没有 C 编译器，cargo build 会在依赖树深处炸开，报错里一个字都
#         不会提到 dct。
#   git   不是编译需要，是运行需要。每轮对话前的那个隐藏快照走的就是 git
#         （src/git.rs 直接 Command::new("git")）。没有 git，dct 编得出来
#         也装得上，但撤销这个功能是哑的——而撤销正是它敢关掉权限询问的
#         全部理由。所以宁可在这里拦住。
#   cargo Rust 本体。
#
# 装完 rustup 别指望 PATH 会自己变：rustup 只往 ~/.cargo/env 里写一行，
# 当前这个 shell 不会回头去读 rc 文件。所以下面装完立刻把 ~/.cargo/bin
# 挂上去，后面 install.sh 才找得到 cargo。

set -euo pipefail

# 编译 dct 需要的最低版本，README 里写的那个。
min_rust=1.80

say() { printf '%s\n' "$*"; }
die() { printf '%s\n' "$*" >&2; exit 1; }

# apt 要 sudo。密码是能问的——wsl.exe 从控制台起来的时候 stdin 是通的，
# sudo 的提示会直接出现在 PowerShell 窗口里。但得先说一声，不然用户看到
# 一个光标停在那儿不动，只会以为卡住了。
run_apt() {
	if ! command -v apt-get >/dev/null 2>&1; then
		die "install-wsl-deps.sh：这个发行版不是 apt 系的，缺的东西请自己装：$*"
	fi
	if ! sudo -n true >/dev/null 2>&1; then
		say "接下来要 sudo 装系统包，下面这行提示要的是你在 WSL 里的密码。"
	fi
	sudo apt-get update -qq
	sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "$@"
}

missing=()
command -v cc >/dev/null 2>&1 || missing+=(build-essential)
command -v curl >/dev/null 2>&1 || missing+=(curl)
command -v git >/dev/null 2>&1 || missing+=(git)

if [ ${#missing[@]} -gt 0 ]; then
	say "缺这些，装一下：${missing[*]}"
	run_apt "${missing[@]}"
fi

# install.sh 里也有一段找 cargo 的逻辑，但它找不到就只是退出。这里要的是
# 找不到就装，所以两段不重复：先按同样的地方找一遍，真没有才下 rustup。
if ! command -v cargo >/dev/null 2>&1; then
	cargo_bin=${CARGO_HOME:-$HOME/.cargo}/bin
	if [ -x "$cargo_bin/cargo" ]; then
		PATH=$cargo_bin:$PATH
		export PATH
		say "cargo 装了但不在 PATH 上，用 $cargo_bin 里的这个。"
	else
		say "没有 Rust，装 rustup（几百 MB，第一次会等一会儿）……"
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
			| sh -s -- -y --no-modify-path --default-toolchain stable
		# --no-modify-path：不去动用户的 rc 文件。是不是要把 Rust 常驻到
		# PATH 上是用户自己的事，装 dct 不该顺手替他决定。这里只管当前这
		# 个 shell 够用，install.sh 在同一个进程链里跑，继承得到。
		PATH=${CARGO_HOME:-$HOME/.cargo}/bin:$PATH
		export PATH
		say
		say "Rust 装好了，但只对这次安装生效。想在 WSL 里平时也能用 cargo，"
		say "把这行加进 ~/.bashrc：. \"\$HOME/.cargo/env\""
	fi
fi

command -v cargo >/dev/null 2>&1 || die "install-wsl-deps.sh：装完还是找不到 cargo，装崩了。"

# 版本卡一下。老 Rust 编不过的报错同样出现在依赖树里，跟"版本不够"这四个
# 字毫无相似之处，当场说清楚比让人去猜强。
have=$(rustc --version | awk '{print $2}')
lowest=$(printf '%s\n%s\n' "$min_rust" "$have" | sort -V | head -n1)
if [ "$lowest" != "$min_rust" ]; then
	die "install-wsl-deps.sh：dct 要 Rust $min_rust 以上，现在是 $have。跑 rustup update 升一下。"
fi

say "工具链齐了：$(rustc --version)，$(git --version)"

# PATH 是导出的，但导出只对子进程有效，回不到调用方那个 shell 里去。
# install.ps1 是用 `bash -lc "这个脚本 && install.sh"` 串起来的，两句在
# 同一个 bash 进程里，所以上面 export 的 PATH 对 install.sh 是有效的。
# 换成两次 wsl.exe 调用就不成立了——别这么改。
