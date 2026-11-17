//! 超大 JPEG 逐行解码用系统的 libjpeg-turbo（为什么见 `src/image_decode/jpeg_rows.c`）。
//! 头文件和库由 pkg-config 找；打包流水线要装它的开发文件。

fn main() {
    println!("cargo:rerun-if-changed=src/image_decode/jpeg_rows.c");
    let libjpeg = pkg_config::Config::new()
        .atleast_version("1.2.1")
        .probe("libjpeg")
        .expect("libjpeg-turbo development files are needed (pkg-config libjpeg)");
    let mut build = cc::Build::new();
    build.file("src/image_decode/jpeg_rows.c");
    for include in &libjpeg.include_paths {
        build.include(include);
    }
    build.compile("bennu_jpeg_rows");
}
