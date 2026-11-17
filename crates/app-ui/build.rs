//! 发布构建的链接设置，只管主程序：
//!
//! - 按 `link-order/functions.txt` 把常用场景里执行过的函数排进单独的输出段。整体优化后常用的
//!   和用不到的函数交错排列，内核按 64 KB 一块映射代码页，静置时用到的十来 MB 代码会撑出 25 MB 的映射。
//!   清单怎么来见 `link-order/train.sh`。
//! - 相对重定位用压缩格式存：启动时动态链接器要整张读一遍，原来的格式有 1.5 MB，压缩后几十 KB
//!   （glibc 2.36 起支持）。
//!
//! 开发构建不用：七千多条通配规则让每次链接多花半分多钟。设了 `BENNU_LINK_ORDER_PROBE` 时
//! 改成每个函数单独占一页，在虚拟机里看哪些页被碰过就知道哪些函数执行过，用来重新生成清单。

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

const ORDER_LIST: &str = "link-order/functions.txt";
const PROBE_ENV: &str = "BENNU_LINK_ORDER_PROBE";

fn main() {
    println!("cargo:rerun-if-changed={ORDER_LIST}");
    println!("cargo:rerun-if-env-changed={PROBE_ENV}");
    if env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }
    let script = if env::var_os(PROBE_ENV).is_some() {
        PROBE_SCRIPT.to_owned()
    } else {
        ordered_script(&fs::read_to_string(ORDER_LIST).expect("read link-order/functions.txt"))
    };
    let path = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("link.ld");
    fs::write(&path, script).expect("write linker script");
    println!("cargo:rustc-link-arg-bins=-Wl,-T,{}", path.display());
    println!("cargo:rustc-link-arg-bins=-Wl,-z,pack-relative-relocs");
}

/// 每个函数单独一段（`.text.<符号>`）；编译器认为很少执行的放在 `.text.unlikely.<符号>`，
/// 两种都要写。脚本只补充默认布局（`INSERT`），没列出的照常放进 `.text`。
fn ordered_script(list: &str) -> String {
    let mut script = String::from("SECTIONS\n{\n  .text.used :\n  {\n");
    for pattern in list
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        writeln!(script, "    *(.text.{pattern} .text.unlikely.{pattern})")
            .expect("write to String");
    }
    script.push_str("  }\n}\nINSERT BEFORE .text;\n");
    script
}

/// 每个函数段按页对齐，一页里只有一个函数。不带函数名的 `.text`（C 运行时的启动代码等）
/// 留在原处：`.text` 空了就没法插在它前面。
const PROBE_SCRIPT: &str =
    "SECTIONS\n{\n  .text.probe : SUBALIGN(4096)\n  {\n    *(.text.*)\n  }\n}\nINSERT BEFORE .text;\n";
