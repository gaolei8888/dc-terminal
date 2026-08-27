#!/bin/sh
#
# 装 dct。默认下现成的二进制，下不到才回头编译。
#
# 有两种用法，而且必须两种都成立：
#
#   curl -fsSL https://raw.githubusercontent.com/gaolei8888/dc-terminal/main/scripts/install.sh | sh
#   ./scripts/install.sh          # 在 clone 好的仓库里
#
# 第一种是给学生的。以前这里只有第二种，于是装 dct 的第一步是装 Rust、
# 装 git、外加 mac/Linux 上一套 C 工具链——几个 GB，而学生要的只是一个
# 几 MB 的可执行文件。
#
# 写的是 POSIX sh 不是 bash：`curl | sh` 里那个 `sh` 在 Debian 系上是 dash，
# 它没有 `pipefail`，也没有 `[[`。用了就是一句 syntax error，而且报在
# 一个跟 dct 毫无关系的行号上。
#
# 整段代码全在函数里，最后一行才 `main "$@"`。这不是风格：`curl | sh`
# 是**边下边执行**的，网断在半路时，shell 已经跑掉的那部分不会退回去。
# 把调用放在最后一行，等于「没下完就等于没开始」。
#
# ── 最后那一步为什么是 mv 不是 cp ──────────────────────────────────
#
# macOS 上 Rust 产出的二进制是 ad-hoc 签名的，内核把校验过的代码签名页哈希
# 缓存在 inode 上。**在有进程正在执行这个 inode 的时候**用 cp 原地覆盖它，
# 写是能写进去的（macOS 这里不报 ETXTBSY），但那份缓存就此对不上新内容，
# 下一次 exec 这个 inode 会被 code signing monitor 直接 SIGKILL——终端里
# 只看得到一行 "zsh: killed"，一行代码都没跑到。更绕的是 codesign -v 还会说
# 签名 "valid on disk"：磁盘上那份确实自洽，对不上的是内核手里那份。
#
# 光是 cp 覆盖不会触发，内容一样也不会——得凑齐「有活进程占着这个 inode」
# 这一条。而 dct 恰好总是凑得齐：它留着一个常驻守护进程，跑的就是装好的
# 这个文件。所以重新编译再 cp 装一次，几乎每次都会中招。
#
# rename(2) 换的是目录项指向的 inode，老 inode 连同那份缓存一起留给还在跑
# 的老进程，新名字指向一个干干净净的新 inode。所以这里先在目标目录里写一个
# 临时文件，再 mv 覆盖过去。临时文件必须跟目标同一个目录（同一个文件系统），
# 否则 mv 退化成拷贝，白折腾一场。

set -eu

# 换成自己的镜像用 DCT_RELEASE_BASE。这个口子是为国内的课堂留的：
# GitHub 在教室的网络里经常慢到不可用，老师可以把三个包和 SHA256SUMS
# 原样放到任何一个能下的地方，学生那条命令一个字都不用改。
DEFAULT_RELEASE_BASE=https://github.com/gaolei8888/dc-terminal/releases/latest/download

usage() {
	cat <<'EOF'
用法：install.sh [选项]

  --dir <目录>   装到哪里（默认 ~/.local/bin，也可以用 DCT_INSTALL_DIR 环境变量）
  --build        不下现成的，从源码编译（要 Rust，且要在 clone 好的仓库里）
  --no-build     既不下也不编，装 target/release/dct 里现成的那个
  -h, --help     看这段

环境变量：
  DCT_INSTALL_DIR   同 --dir
  DCT_RELEASE_BASE  从别处下预编译包（默认是 GitHub releases）
EOF
}

say() { printf '%s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
die() { printf '%s\n' "$*" >&2; exit 1; }

have() { command -v "$1" >/dev/null 2>&1; }

# 这台机器对应哪个预编译包。认不出来不是错误，只是意味着得回头编译，
# 所以这里返回空字符串而不是退出——决定权在 main 手里。
detect_target() {
	os=$(uname -s 2>/dev/null || echo unknown)
	arch=$(uname -m 2>/dev/null || echo unknown)
	case "$os" in
		Linux) os_part=unknown-linux-gnu ;;
		Darwin) os_part=apple-darwin ;;
		*) return 0 ;;
	esac
	case "$arch" in
		x86_64 | amd64) arch_part=x86_64 ;;
		arm64 | aarch64) arch_part=aarch64 ;;
		*) return 0 ;;
	esac
	# ARM 的 Linux 现在没有预编译包（教室里没见过），认出来了也没得下。
	# 与其下一个 404 回来，不如在这里就承认没有。
	if [ "$os_part" = unknown-linux-gnu ] && [ "$arch_part" = aarch64 ]; then
		return 0
	fi
	printf '%s-%s' "$arch_part" "$os_part"
}

# curl 和 wget 有一个就行。两个都没有的机器，后面那句话会说清楚。
fetch_to() {
	url=$1
	out=$2
	if have curl; then
		# -f：HTTP 错误码要变成非零退出，否则 404 那页 HTML 会被当成包存下来。
		# -L：GitHub 的 latest/download 是一串跳转。
		curl -fsSL "$url" -o "$out"
	elif have wget; then
		wget -qO "$out" "$url"
	else
		die "install.sh：这台机器上既没有 curl 也没有 wget，下不了东西。
装一个再来，或者用 --build 从源码编译。"
	fi
}

# 算 sha256。不同系统上这条命令的名字不一样，两个都试。
sha256_of() {
	if have sha256sum; then
		sha256sum "$1" | cut -d' ' -f1
	elif have shasum; then
		shasum -a 256 "$1" | cut -d' ' -f1
	else
		printf ''
	fi
}

# 下预编译包，解出二进制，把路径写进 $fetched。下不到就返回非零，
# 由调用方决定要不要回头编译。
#
# 校验和是**不能跳过**的一步：下到一半断了，和下到一个被人换过的文件，
# 在解压之前长得一模一样，而这个文件接下来会被放进 PATH 天天执行。
download_release() {
	target=$1
	workdir=$2
	base=${DCT_RELEASE_BASE:-$DEFAULT_RELEASE_BASE}
	asset=dct-$target.tar.gz

	say "下预编译包：$asset"
	if ! fetch_to "$base/$asset" "$workdir/$asset" 2>/dev/null; then
		note "没下到（$base/$asset）"
		return 1
	fi

	if ! fetch_to "$base/SHA256SUMS" "$workdir/SHA256SUMS" 2>/dev/null; then
		note '没下到 SHA256SUMS，这个包没法验，不装它。'
		return 1
	fi

	# 用 awk 按字段比对文件名，不要 grep 那一行。两个原因：sha256sum 在
	# 二进制模式下印的是「哈希 *文件名」而不是「哈希  文件名」，认死空格
	# 会漏；而 grep 一个子串的话，`dct-x86_64-apple-darwin.tar.gz` 这种
	# 名字互相是前缀的包会挑错行——挑错了就是拿另一个平台的哈希去验。
	want=$(awk -v n="$asset" '{ f = $2; sub(/^\*/, "", f); if (f == n) { print $1; exit } }' "$workdir/SHA256SUMS" 2>/dev/null || true)
	got=$(sha256_of "$workdir/$asset")
	if [ -z "$want" ]; then
		note "SHA256SUMS 里没有 $asset 这一条，没法验，不装它。"
		return 1
	fi
	if [ -z "$got" ]; then
		note '这台机器上没有 sha256sum 也没有 shasum，验不了，不装它。'
		return 1
	fi
	if [ "$want" != "$got" ]; then
		note '下回来的文件跟官方校验和对不上，不装它。'
		note '多半是下到一半断了，重跑一次；一直对不上就换个网络。'
		return 1
	fi

	if ! tar -xzf "$workdir/$asset" -C "$workdir"; then
		note '包解不开。'
		return 1
	fi
	if [ ! -f "$workdir/dct" ]; then
		note '包里没有 dct。'
		return 1
	fi
	fetched=$workdir/dct
	return 0
}

build_from_source() {
	repo_root=$1
	[ -n "$repo_root" ] || die "install.sh：不在 dct 的仓库里，编不了。
先把仓库拿下来：
  git clone https://github.com/gaolei8888/dc-terminal
  cd dc-terminal
  ./scripts/install.sh --build"

	# rustup 装完只往 ~/.cargo/env 里写 PATH，要 rc 文件 source 一下才生效。
	# 少了那一行，cargo 明明在盘上却不在 PATH 上——脚本自己找一遍，别让人以为
	# 没装 Rust。rustc 之类的其余工具都是同目录下的 rustup shim，一并带上。
	if ! have cargo; then
		cargo_bin=${CARGO_HOME:-$HOME/.cargo}/bin
		if [ -x "$cargo_bin/cargo" ]; then
			PATH=$cargo_bin:$PATH
			export PATH
			say "cargo 不在 PATH 上，用 $cargo_bin 里的这个。"
			note "想一劳永逸，把这行加进 ~/.zshrc：. \"\$HOME/.cargo/env\""
		else
			die "install.sh：找不到 cargo。装 Rust 1.80 以上：https://rustup.rs"
		fi
	fi
	say '编译中（release，第一次会比较久）……'
	cargo build --release --manifest-path "$repo_root/Cargo.toml"
	fetched=$repo_root/target/release/dct
}

# 把 $1 那个文件装到 $2/dct。整个脚本的重点在这几行，理由见文件头。
install_binary() {
	src=$1
	install_dir=$2

	mkdir -p "$install_dir"
	dest=$install_dir/dct

	staged=$(mktemp "$install_dir/.dct.XXXXXX")
	trap 'rm -f "$staged"' EXIT

	cat "$src" >"$staged"
	chmod 755 "$staged"
	mv -f "$staged" "$dest"
	trap - EXIT

	# 装完真的跑一下。上面说的那个 SIGKILL 是在 exec 阶段发生的，跑一次
	# 就能当场发现，总好过下次用的时候只看到一行 "killed" 摸不着头脑。
	# 这里不能写 `if ! "$dest" --help; then status=$?`——那样 $? 拿到的是 `!`
	# 取反之后的 0，报出来的退出码永远是 0，正好把最该看的那个数字吃掉。
	status=0
	"$dest" --help >/dev/null 2>&1 || status=$?
	if [ "$status" -ne 0 ]; then
		printf '%s\n' "install.sh：装是装上了，但 $dest 跑不起来（退出码 $status）。" >&2
		if [ "$status" -eq 137 ]; then
			printf '%s\n' "  137 是被 SIGKILL 了。看一眼 ~/Library/Logs/DiagnosticReports/dct-*.ips，" >&2
			printf '%s\n' "  如果写着 CODESIGNING，说明覆盖没换成新 inode——把 $dest 删掉再跑一次。" >&2
		fi
		exit 1
	fi
}

# git 不是编译依赖，是运行时依赖：每一轮对话之前的那次隐藏快照是 shell 出去
# 调 git 做的（src/git.rs）。没有它，撤销就是死的——而撤销正是 dct 敢让
# agent 关掉所有权限确认的全部理由。
#
# 这里真的跑一次 `git --version`，不是 `command -v git`。macOS 上
# /usr/bin/git 是个占位的壳，Xcode 命令行工具没装时它照样在 PATH 上、
# `command -v` 照样说有——真跑起来才会弹一个安装窗口出来。只查名字的话，
# 这条检查在最需要它的那台机器上恰好是失灵的。
check_git() {
	if git --version >/dev/null 2>&1; then
		return 0
	fi
	say ''
	say '还差一样：这台电脑上没有 git。'
	note 'dct 每轮对话前会给你的项目拍一张隐藏快照，靠的就是 git。'
	note '没有它，agent 闯了祸就退不回去了。'
	case "$(uname -s 2>/dev/null || echo unknown)" in
		Darwin) note '装一个：xcode-select --install' ;;
		Linux) note '装一个：sudo apt install git（或者你的发行版对应的那条）' ;;
	esac
}

main() {
	install_dir=${DCT_INSTALL_DIR:-$HOME/.local/bin}
	mode=auto

	while [ $# -gt 0 ]; do
		case "$1" in
			--dir)
				[ $# -ge 2 ] || die "install.sh：--dir 后面要跟一个目录"
				install_dir=$2
				shift 2
				;;
			--build)
				mode=build
				shift
				;;
			--no-build)
				mode=prebuilt-local
				shift
				;;
			-h | --help)
				usage
				exit 0
				;;
			*)
				printf '%s\n' "install.sh：不认识的选项 $1" >&2
				usage >&2
				exit 2
				;;
		esac
	done

	# 在仓库里跑，还是从管道里跑？`$0` 在 `curl | sh` 下不是一个能读的文件，
	# 那时候就没有仓库，也就没有源码可编——这不是错误，是最常见的那条路。
	repo_root=''
	if [ -f "$0" ]; then
		maybe=$(cd -- "$(dirname -- "$0")/.." 2>/dev/null && pwd) || maybe=''
		if [ -n "$maybe" ] && [ -f "$maybe/Cargo.toml" ]; then
			repo_root=$maybe
		fi
	fi

	fetched=''
	workdir=''

	case "$mode" in
		prebuilt-local)
			[ -n "$repo_root" ] || die "install.sh：--no-build 是装 target/release/dct 里现成的那个，
但这里不是 dct 的仓库，没有那个文件。"
			fetched=$repo_root/target/release/dct
			[ -f "$fetched" ] || die "install.sh：$fetched 不存在。去掉 --no-build 让它先装。"
			;;
		build)
			build_from_source "$repo_root"
			;;
		auto)
			target=$(detect_target)
			if [ -n "$target" ]; then
				workdir=$(mktemp -d)
				trap 'rm -rf "$workdir"' EXIT
				download_release "$target" "$workdir" || fetched=''
			else
				say "这个系统（$(uname -s 2>/dev/null) $(uname -m 2>/dev/null)）没有现成的包。"
			fi
			if [ -z "$fetched" ]; then
				if [ -n "$repo_root" ]; then
					say '回头从源码编译。'
					build_from_source "$repo_root"
				else
					die "
装不上：没下到预编译包，这里也不是 dct 的仓库所以编不了。

能试的两条路：
  1. 网络的问题居多，过一会儿重跑一次同一条命令。
  2. 自己编：
       git clone https://github.com/gaolei8888/dc-terminal
       cd dc-terminal
       ./scripts/install.sh --build
     （这条路要先装 Rust：https://rustup.rs）"
				fi
			fi
			;;
	esac

	say "装到 $install_dir"
	install_binary "$fetched" "$install_dir"
	# install_binary 结尾要 `trap - EXIT` 把它自己那份清理摘掉，而那一句
	# 连带把上面解包用的临时目录也摘了——所以这里补一次，别在 /tmp 里
	# 留下几 MB 谁也不会去删的垃圾。
	if [ -n "$workdir" ]; then
		rm -rf "$workdir"
		workdir=''
	fi
	say "装好了：$install_dir/dct（$("$install_dir/dct" --version 2>/dev/null || echo '版本未知')）"

	case ":$PATH:" in
		*":$install_dir:"*) ;;
		*)
			say ''
			say "注意：$install_dir 不在 PATH 上，直接敲 dct 会找不到。"
			note "把这行加进 ~/.zshrc（或者你在用的那个 rc 文件）："
			note "export PATH=\"$install_dir:\$PATH\""
			;;
	esac

	check_git

	if have pgrep && pgrep -f 'dct daemon' >/dev/null 2>&1; then
		say ''
		say '旧版本的守护进程还在跑。下次 dct 启动会发现这件事，跟你说清楚重启'
		say '会让正在跑的会话全部结束，问过你才动手——这里不替你做决定。'
	fi

	say ''
	say '进到任何一个文件夹里敲 dct 就能开始。'
}

main "$@"
