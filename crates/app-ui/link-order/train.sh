#!/usr/bin/env bash
# 用探针版把常用操作走一遍，记下碰过的代码页，供 generate.py 生成 functions.txt。
#
# 用法：train.sh PROBE_BINARY PAGES_FILE VIEW:DIR [VIEW:DIR ...]
#   VIEW 是 icons、list、columns；每组：进目录 → 静置 → 滚动 → 连续预览 → 搜索 → 关窗口。
#
# 重新生成清单（在 sway 会话里，装好 wtype、ydotool 并开着 ydotoold；最好在虚拟机里，
# 要 sudo 调内核参数）：
#   CARGO_PROFILE_RELEASE_STRIP=none BENNU_LINK_ORDER_PROBE=1 \
#       cargo build --release -p app-ui --target-dir target/link-order-probe
#   crates/app-ui/link-order/train.sh target/link-order-probe/release/app-ui vulkan.pages icons:DIR1 list:DIR2 columns:DIR2 ...
#   （每种渲染路径各跑一遍：有 Vulkan 的显卡、只有 OpenGL 的显卡、没有显卡——后者设
#    LIBGL_ALWAYS_SOFTWARE=1 VK_DRIVER_FILES=/dev/null 再跑）
#   crates/app-ui/link-order/generate.py crates/app-ui/link-order/functions.txt \
#       --binary target/link-order-probe/release/app-ui vulkan=vulkan.pages gl=gl.pages software=software.pages
#
# 目录里要有各种图片、文档、压缩包、视频，和一个上万个文件的目录：清单只收这些场景里执行过的函数。
set -euo pipefail
[ $# -ge 3 ] || { sed -n '4,5p' "$0" >&2; exit 2; }
binary=$(realpath "$1") pages=$2
shift 2
here=$(dirname "$(realpath "$0")")
command -v swaymsg wtype ydotool >/dev/null
[ -S "${YDOTOOL_SOCKET:-/tmp/.ydotool_socket}" ] || { echo "ydotoold is not running" >&2; exit 1; }

# 内核默认把用到的那页前后 64 KB 一起映射进来，看不出到底执行了哪个函数：量的时候只映射一页。
fault_around=/sys/kernel/debug/fault_around_bytes
sudo mount -t debugfs none /sys/kernel/debug 2>/dev/null || true
previous=$(sudo cat "$fault_around")
trap 'echo "$previous" | sudo tee "$fault_around" >/dev/null' EXIT
echo 4096 | sudo tee "$fault_around" >/dev/null

window_center() {  # 被测程序窗口的中心
    swaymsg -t get_tree | python3 -c "
import json, sys
def walk(node):
    if node.get('pid') == $1:
        rect = node['rect']; print(rect['x'] + rect['width'] // 2, rect['y'] + rect['height'] // 2); sys.exit()
    for child in node.get('nodes', []) + node.get('floating_nodes', []): walk(child)
walk(json.load(sys.stdin))"
}

scroll() {  # 在窗口中心往下滚 $2 秒，再往回滚
    read -r x y <<<"$(window_center "$1")"
    swaymsg seat seat0 cursor set "$x" "$y" >/dev/null
    local end=$((SECONDS + $2))
    while ((SECONDS < end)); do ydotool mousemove -w -- 0 -3; sleep 0.03; done
    end=$((SECONDS + $2 / 2))
    while ((SECONDS < end)); do ydotool mousemove -w -- 0 6; sleep 0.03; done
}

for scenario in "$@"; do
    view=${scenario%%:*} directory=${scenario#*:}
    work=$(mktemp -d)
    export XDG_CONFIG_HOME=$work/config XDG_DATA_HOME=$work/data XDG_CACHE_HOME=$work/cache XDG_STATE_HOME=$work/state
    # 第一次启动要探测显卡、建设置库，那些只跑一次的代码不进清单：先启动一次再关掉。
    "$binary" "$HOME" >/dev/null 2>&1 & pid=$!
    sleep 8; kill "$pid"; wait "$pid" 2>/dev/null || true
    python3 - "$XDG_DATA_HOME/bennu/state.sqlite" "$view" <<'PY'
import json, sqlite3, sys
db = sqlite3.connect(sys.argv[1])
for key, payload in db.execute("select preference_key, payload_json from user_preferences").fetchall():
    data = json.loads(payload)
    if isinstance(data, dict) and "browser_view_mode" in data:
        data["browser_view_mode"] = sys.argv[2]
        db.execute("update user_preferences set payload_json=? where preference_key=?", (json.dumps(data), key))
db.commit()
PY
    sync; echo 3 | sudo tee /proc/sys/vm/drop_caches >/dev/null
    "$binary" "$directory" >/dev/null 2>&1 & pid=$!
    sleep 15
    scroll "$pid" 20
    sleep 3
    step=$([ "$view" = icons ] && echo Right || echo Down)
    wtype -k Down; sleep 0.5; wtype -k space; sleep 2
    for _ in $(seq 20); do wtype -k "$step"; sleep 0.6; done
    sleep 2; wtype -k Escape; sleep 1
    wtype photo; sleep 5
    for _ in 1 2 3 4 5; do wtype -k BackSpace; sleep 0.2; done
    sleep 2; wtype -k Escape; sleep 10
    python3 "$here/touched-pages.py" "$pid" "$binary" >>"$pages"
    kill "$pid"; wait "$pid" 2>/dev/null || true
    rm -rf "$work"
    echo "$scenario: $(sort -u "$pages" | wc -l) pages so far"
done
