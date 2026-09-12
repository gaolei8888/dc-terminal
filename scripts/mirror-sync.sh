#!/bin/sh
#
# 把 GitHub 上某一版的产物拉下来、验一遍、摆成镜像该有的样子。
#
#   ./scripts/mirror-sync.sh                      # 拉 latest，放进 ./mirror
#   ./scripts/mirror-sync.sh --out /srv/dct       # 放到别处
#   ./scripts/mirror-sync.sh --tag v0.2.14        # 拉指定的一版
#
# 这个脚本是给**镜像维护者**跑的，不是给学生跑的。学生跑的是 install.sh。
#
# 为什么需要它：GitHub 的 `releases/latest/download/<固定名字>` 这条路由，
# 别家基本都没有。Gitee 的发行版附件地址绑定具体 tag，没有 latest 别名
# （实测 `/releases/latest/download/x.zip` 返回的是通用 API 的 JSON 404，
# 说明根本没这条路由）。所以镜像端只能退回到「一个目录，一堆固定名字的
# 文件」这种最朴素的形状——而这恰好是 install.sh 唯一的要求：它拼的就是
# `$base/$asset` 和 `$base/SHA256SUMS`，两个 curl，没有任何 GitHub 特有的
# 假设。把这五个文件平铺在任何一个能 HTTP GET 到的目录下就够了。
#
# ── 为什么这里一定要验校验和 ─────────────────────────────────────
#
# 学生那端当然也验（install.sh 下完包会比对 SHA256SUMS）。但那道检查只能
# 告诉学生「这个包坏了，不装」——它发生在几十台机器上、在课堂中间、在老师
# 没法调试的时候。镜像端多验这一遍，是把同一个错误提前到一台机器上、一个
# 人面前、一个能重跑的时刻。坏包不该有机会上线。
#
# 写的是 POSIX sh，理由同 install.sh：不能假设跑它的是 bash。

set -eu

DEFAULT_BASE=https://github.com/gaolei8888/dc-terminal/releases
OUT=./mirror
TAG=
BASE=

# 四个平台包 + 校验和文件。名字里**故意不带版本号**，这是 release.yml 的
# 设计：镜像端发新版 = 覆盖同名文件，学生那条命令永远不变。
ASSETS='dct-x86_64-unknown-linux-gnu.tar.gz
dct-x86_64-apple-darwin.tar.gz
dct-aarch64-apple-darwin.tar.gz
dct-x86_64-pc-windows-msvc.zip'

usage() {
	cat <<'EOF'
用法：mirror-sync.sh [选项]

  --out <目录>   放哪里（默认 ./mirror）
  --tag <tag>    拉指定的一版（默认 latest）
  --base <url>   换个上游（默认 GitHub releases）
  -h, --help     看这段

跑完之后，把 --out 那个目录整个传到你的 HTTP 服务上，学生设：
  export DCT_RELEASE_BASE=https://你的地址/<那个目录>
EOF
}

say()  { printf '%s\n' "$*"; }
note() { printf '  %s\n' "$*"; }
die()  { printf '%s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

while [ $# -gt 0 ]; do
	case $1 in
		--out)  OUT=${2:?--out 后面要跟目录}; shift 2 ;;
		--tag)  TAG=${2:?--tag 后面要跟 tag}; shift 2 ;;
		--base) BASE=${2:?--base 后面要跟地址}; shift 2 ;;
		-h|--help) usage; exit 0 ;;
		*) die "不认识的选项：$1（--help 看用法）" ;;
	esac
done

if [ -z "$BASE" ]; then
	if [ -n "$TAG" ]; then
		BASE=$DEFAULT_BASE/download/$TAG
	else
		BASE=$DEFAULT_BASE/latest/download
	fi
fi

have curl || die "mirror-sync.sh：没有 curl，下不了东西。"

if have sha256sum; then
	sha256_of() { sha256sum "$1" | cut -d' ' -f1; }
elif have shasum; then
	sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
	die "mirror-sync.sh：sha256sum 和 shasum 都没有，验不了包。
验不了就不该往镜像上传——坏包在学生那端只会变成一句他们看不懂的错。"
fi

# 先下到一个临时目录，全部验过了再整体搬进 $OUT。
#
# 中途失败时，$OUT 里应当还是**上一版完好的文件**，而不是新旧混在一起。
# 半新半旧的镜像比过时的镜像坏得多：学生下到的四个包可能来自两个版本，
# 而每一个都能通过自己的校验和。
TMP=$(mktemp -d) || die "建不了临时目录"
cleanup() { [ -n "${TMP:-}" ] && rm -rf "$TMP"; }
trap cleanup EXIT INT TERM

say "上游：$BASE"

say "下 SHA256SUMS"
curl -fsSL "$BASE/SHA256SUMS" -o "$TMP/SHA256SUMS" \
	|| die "没下到 SHA256SUMS（$BASE/SHA256SUMS）
这一版可能还在编，或者 tag 写错了。"

failed=
for asset in $ASSETS; do
	say "下 $asset"
	if ! curl -fsSL "$BASE/$asset" -o "$TMP/$asset"; then
		note "没下到，跳过"
		failed="$failed $asset"
		continue
	fi

	# 按字段比对文件名，不要 grep 那一行——理由同 install.sh：
	# 包名互为前缀（dct-x86_64-apple-darwin 和 dct-aarch64-apple-darwin
	# 不是，但 dct-x86_64-apple-darwin.tar.gz 和将来任何以它开头的名字是），
	# grep 子串会挑错行，挑错了就是拿另一个平台的哈希去验。
	want=$(awk -v n="$asset" '{ f = $2; sub(/^\*/, "", f); if (f == n) { print $1; exit } }' "$TMP/SHA256SUMS")
	got=$(sha256_of "$TMP/$asset")

	if [ -z "$want" ]; then
		die "SHA256SUMS 里没有 $asset 这一行，不敢上线。"
	fi
	if [ "$want" != "$got" ]; then
		die "$asset 校验和对不上，不敢上线。
  期望 $want
  实际 $got"
	fi
	note "校验和 OK"
done

[ -n "$failed" ] && die "下面这些没下到，镜像不完整，不搬：$failed"

# install.sh / install.ps1 也要放进镜像——学生连 raw.githubusercontent.com
# 都连不上，不然第一条命令就卡死了。优先用仓库里这一份（跑这个脚本的人
# 多半就在 clone 里），没有再回头去网上下。
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
for s in install.sh install.ps1; do
	if [ -f "$here/$s" ]; then
		cp "$here/$s" "$TMP/$s"
		say "带上 ${s}（仓库里这份）"
	else
		say "下 $s"
		curl -fsSL "https://raw.githubusercontent.com/gaolei8888/dc-terminal/main/scripts/$s" -o "$TMP/$s" \
			|| die "没下到 $s"
	fi
done

mkdir -p "$OUT"
for f in $ASSETS SHA256SUMS install.sh install.ps1; do
	cp "$TMP/$f" "$OUT/$f"
done

say ''
say "好了：$OUT"
say ''
say '把这个目录传上去之后，学生那两条命令是：'
say ''
say '  export DCT_RELEASE_BASE=https://你的地址'
say '  curl -fsSL https://你的地址/install.sh | sh'
say ''
say '装完第一次跑 dct 之前，国内还要再设两个（Node 运行时和 npm 包）：'
say ''
say '  export DCT_NODE_BASE=https://npmmirror.com/mirrors/node'
say '  export DCT_NPM_REGISTRY=https://registry.npmmirror.com'
