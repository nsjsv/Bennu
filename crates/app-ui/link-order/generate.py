#!/usr/bin/env python3
"""由探针版在常用场景里碰过的代码页，生成 crates/app-ui/link-order/functions.txt。

用法：generate.py OUTPUT --binary PROBE_BINARY LABEL=PAGES [LABEL=PAGES ...]

探针版（BENNU_LINK_ORDER_PROBE=1、不去符号的发布构建）里每个函数单独占一页，哪页在内存里
就是哪个函数执行过。每个 PAGES 来自一种渲染路径（vulkan、gl、software），是 train.sh
用 touched-pages.py 记下的页号，按重要程度排（用户最多的路径在前）。用户只走一种路径：
先排第一种路径用到的全部函数，让它连成一片，其中别的路径也用到的靠前；后面的路径依次类推。
组内按名字排序，重新生成后 diff 只显示真正变了的函数。
"""

from __future__ import annotations

import argparse
import bisect
import pathlib
import re
import subprocess

PAGE = 4096
PROBE_SECTION = ".text.probe"
# 符号名里随构建变的部分换成通配符，链接时对上以后构建的名字：
# - `_R` 开头：每个包名前的哈希 `Cs<哈希>_` 随编译器版本、包版本变；`B<数字>_` 是指向
#   名字前面某段的偏移，哈希长度一变它也跟着变。
# - `_ZN` 开头（旧式）：末尾 `17h<16 位哈希>E`。
# LTO 改名的局部函数末尾带 `.llvm.<数字>`，由结尾的通配符接住。C 函数名不变，原样写。
V0_CRATE_HASH = re.compile(r"Cs[0-9a-zA-Z]*_")
V0_BACKREF = re.compile(r"B[0-9a-zA-Z]*_")
LEGACY_HASH = re.compile(r"17h[0-9a-f]{16}E(?:\..*)?$")
LLVM_SUFFIX = re.compile(r"\.llvm\.\d+$")
# 链接脚本里有特殊含义的字符；带它们的名字（极少）不写进清单。
SCRIPT_SPECIAL = re.compile(r"[\s(){};,=\"*?\[\]]")


def symbol_pattern(symbol: str) -> str | None:
    name = LLVM_SUFFIX.sub("", symbol)
    if SCRIPT_SPECIAL.search(name):
        return None
    if name.startswith("_R"):
        return V0_BACKREF.sub("B*_", V0_CRATE_HASH.sub("Cs*_", name)) + "*"
    if name.startswith("_ZN"):
        return LEGACY_HASH.sub("17h", name) + "*"
    return name


def run(*command: str) -> str:
    return subprocess.run(command, capture_output=True, text=True, check=True).stdout


class ProbeBinary:
    def __init__(self, path: pathlib.Path):
        # 文件内偏移 → 地址：可执行的 LOAD 段
        self.segments = []
        for line in run("readelf", "-lW", str(path)).splitlines():
            fields = line.split()
            if fields[:1] == ["LOAD"] and "E" in fields[6:-1]:
                offset, address, size = int(fields[1], 16), int(fields[2], 16), int(fields[4], 16)
                self.segments.append((offset, address, size))
        probe = None
        for line in run("readelf", "-SW", str(path)).splitlines():
            match = re.match(r"\s*\[\s*\d+\]\s+(\S+)\s+\S+\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)", line)
            if match and match.group(1) == PROBE_SECTION:
                probe = (int(match.group(2), 16), int(match.group(3), 16))
        if probe is None:
            raise SystemExit(f"{path}: no {PROBE_SECTION} section, build with BENNU_LINK_ORDER_PROBE=1")
        start, size = probe
        self.functions = []
        for line in run("nm", "-S", "--defined-only", str(path)).splitlines():
            fields = line.split()
            if len(fields) == 4 and fields[2] in "tTwW":
                address = int(fields[0], 16)
                if start <= address < start + size:
                    self.functions.append((address, address + int(fields[1], 16), fields[3]))
        if not self.functions:
            raise SystemExit(f"{path}: no function symbols, build with CARGO_PROFILE_RELEASE_STRIP=none")
        self.functions.sort()
        self.starts = [function[0] for function in self.functions]

    def executed(self, pages: pathlib.Path) -> set[str]:
        patterns = set()
        for page in {int(line) for line in pages.read_text().split()}:
            offset = page * PAGE
            for segment_offset, address, size in self.segments:
                if segment_offset <= offset < segment_offset + size:
                    low = offset - segment_offset + address
                    break
            else:
                continue
            index = max(bisect.bisect_right(self.starts, low) - 1, 0)
            while index < len(self.functions) and self.functions[index][0] < low + PAGE:
                start, end, symbol = self.functions[index]
                if end > low:
                    pattern = symbol_pattern(symbol)
                    if pattern:
                        patterns.add(pattern)
                index += 1
        return patterns


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("pages", nargs="+", metavar="LABEL=PAGES")
    args = parser.parse_args()
    binary = ProbeBinary(args.binary)
    labels = [item.partition("=")[0] for item in args.pages]
    executed_in: dict[str, frozenset[str]] = {}
    for item in args.pages:
        label, _, path = item.partition("=")
        for pattern in binary.executed(pathlib.Path(path)):
            executed_in[pattern] = executed_in.get(pattern, frozenset()) | {label}
    groups: dict[frozenset[str], list[str]] = {}
    for pattern, paths in executed_in.items():
        groups.setdefault(paths, []).append(pattern)
    order = sorted(groups, key=lambda paths: tuple(label not in paths for label in labels))
    with args.output.open("w") as listing:
        listing.write(
            "# 常用场景里执行过的函数，发布构建时排在一起（crates/app-ui/build.rs）。\n"
            "# 由 link-order/generate.py 生成，别手改；怎么重新生成见 link-order/train.sh。\n"
        )
        for paths in order:
            listing.write(f"# {'+'.join(sorted(paths, key=labels.index))}\n")
            listing.writelines(f"{pattern}\n" for pattern in sorted(groups[paths]))
    print(f"{len(executed_in)} functions -> {args.output}")


if __name__ == "__main__":
    main()
