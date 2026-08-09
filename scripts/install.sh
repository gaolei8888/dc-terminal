#!/usr/bin/env bash
#
# 把 dct 编译好装到 PATH 上。
#
# 这个脚本存在的理由是最后那一步 mv。
#
# macOS 上 Rust 产出的二进制是 ad-hoc 签名的，内核把校验过的代码签名页哈希
# 缓存在 inode 上。**在有进程正在执行这个 inode 的时候**用 cp 原地覆盖它，
# 写是能写进去的（macOS 这里不报 ETXTBSY），但那份缓存就此对不上新内容，
# 下一次 exec 这个 inode 会被 code signing monitor 直接 SIGKILL——终端里
# 只看得到一行 "zsh: killed"，一行代码都没跑到。更绕的是 codesign -v 还会说
# 签名 "valid on disk"：磁盘上那份确实自洽，对不上的是内核手里那份。
#
# 光是 cp 覆盖不会触发，内容一样也不会——得凑齐"有活进程占着这个 inode"
# 这一条。而 dct 恰好总是凑得齐：它留着一个常驻守护进程，跑的就是装好的
# 这个文件。所以重新编译再 cp 装一次，几乎每次都会中招。
#
# rename(2) 换的是目录项指向的 inode，老 inode 连同那份缓存一起留给还在跑
# 的老进程，新名字指向一个干干净净的新 inode。所以这里先在目标目录里写一个
# 临时文件，再 mv 覆盖过去。临时文件必须跟目标同一个目录（同一个文件系统），
# 否则 mv 退化成拷贝，白折腾一场。

set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
install_dir=${DCT_INSTALL_DIR:-$HOME/.local/bin}
do_build=1

usage() {
	cat <<'EOF'
用法：scripts/install.sh [选项]

  --dir <目录>   装到哪里（默认 ~/.local/bin，也可以用 DCT_INSTALL_DIR 环境变量）
  --no-build     不编译，直接装 target/release/dct 里现成的那个
  -h, --help     看这段
EOF
}

while [ $# -gt 0 ]; do
	case "$1" in
		--dir)
			[ $# -ge 2 ] || { echo "install.sh：--dir 后面要跟一个目录" >&2; exit 2; }
			install_dir=$2
			shift 2
			;;
		--no-build)
			do_build=0
			shift
			;;
		-h|--help)
			usage
			exit 0
			;;
		*)
			echo "install.sh：不认识的选项 $1" >&2
			usage >&2
			exit 2
			;;
	esac
done

built=$repo_root/target/release/dct

if [ "$do_build" -eq 1 ]; then
	# rustup 装完只往 ~/.cargo/env 里写 PATH，要 rc 文件 source 一下才生效。
	# 少了那一行，cargo 明明在盘上却不在 PATH 上——脚本自己找一遍，别让人以为
	# 没装 Rust。rustc 之类的其余工具都是同目录下的 rustup shim，一并带上。
	if ! command -v cargo >/dev/null 2>&1; then
		cargo_bin=${CARGO_HOME:-$HOME/.cargo}/bin
		if [ -x "$cargo_bin/cargo" ]; then
			PATH=$cargo_bin:$PATH
			export PATH
			echo "cargo 不在 PATH 上，用 $cargo_bin 里的这个。"
			echo "想一劳永逸，把这行加进 ~/.zshrc：. \"\$HOME/.cargo/env\""
		else
			echo "install.sh：找不到 cargo。装 Rust 1.80 以上：https://rustup.rs" >&2
			exit 1
		fi
	fi
	echo "编译中（release，第一次会比较久）……"
	cargo build --release --manifest-path "$repo_root/Cargo.toml"
fi

[ -f "$built" ] || {
	echo "install.sh：$built 不存在。去掉 --no-build 让它先编译。" >&2
	exit 1
}

mkdir -p "$install_dir"
dest=$install_dir/dct

# 临时文件跟 dest 同目录，mv 才是 rename 而不是拷贝。
staged=$(mktemp "$install_dir/.dct.XXXXXX")
trap 'rm -f "$staged"' EXIT

cat "$built" >"$staged"
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
	echo "install.sh：装是装上了，但 $dest 跑不起来（退出码 $status）。" >&2
	if [ "$status" -eq 137 ]; then
		echo "  137 是被 SIGKILL 了。看一眼 ~/Library/Logs/DiagnosticReports/dct-*.ips，" >&2
		echo "  如果写着 CODESIGNING，说明覆盖没换成新 inode——把 $dest 删掉再跑一次。" >&2
	fi
	exit 1
fi

echo "装好了：$dest"

case ":$PATH:" in
	*":$install_dir:"*)
		;;
	*)
		echo
		echo "注意：$install_dir 不在 PATH 上，直接敲 dct 会找不到。"
		echo "把这行加进 ~/.zshrc（或者你在用的那个 rc 文件）："
		echo "  export PATH=\"$install_dir:\$PATH\""
		;;
esac

if pgrep -f 'dct daemon' >/dev/null 2>&1; then
	echo
	echo "旧版本的守护进程还在跑。下次 dct 启动会发现这件事，跟你说清楚重启"
	echo "会让正在跑的会话全部结束，问过你才动手——这里不替你做决定。"
fi
