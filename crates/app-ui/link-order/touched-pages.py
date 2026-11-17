#!/usr/bin/env python3
"""列出进程映射的程序代码里哪些页在内存里（/proc/PID/pagemap 第 63 位），每行一个文件内页号。

用法：touched-pages.py PID BINARY >> pages.txt

内核默认按 64 KB 一块映射代码页，没执行过的邻居页也会在内存里；量之前 train.sh 会把
/sys/kernel/debug/fault_around_bytes 调成一页。
"""

from __future__ import annotations

import pathlib
import struct
import sys

PAGE = 4096


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__.strip().split("\n\n")[1], file=sys.stderr)
        return 2
    pid, binary = sys.argv[1], str(pathlib.Path(sys.argv[2]).resolve())
    with open(f"/proc/{pid}/pagemap", "rb") as pagemap:
        for line in open(f"/proc/{pid}/maps"):
            fields = line.split()
            if len(fields) < 6 or fields[5] != binary or "x" not in fields[1]:
                continue
            start, end = (int(value, 16) for value in fields[0].split("-"))
            first_page = int(fields[2], 16) // PAGE
            pagemap.seek(start // PAGE * 8)
            entries = pagemap.read((end - start) // PAGE * 8)
            for index in range(len(entries) // 8):
                (entry,) = struct.unpack_from("<Q", entries, index * 8)
                if entry >> 63:
                    print(first_page + index)
    return 0


if __name__ == "__main__":
    sys.exit(main())
