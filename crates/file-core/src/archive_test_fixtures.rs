//! 测试用压缩包：用系统 tar 现场造（各发行版都有）。包内浏览、提取和格式无关，单元测试都用它；
//! zip 走 7z 的部分见集成测试。

use std::path::Path;
use std::process::Command;

/// `entries` 里以 `/` 结尾的是目录，其余是文件和内容；按给出的顺序进包，不自动带上父目录。
fn stage(entries: &[(&str, &[u8])]) -> tempfile::TempDir {
    let staging = tempfile::tempdir().expect("staging dir");
    for (name, contents) in entries {
        let path = staging.path().join(name);
        if name.ends_with('/') {
            std::fs::create_dir_all(&path).expect("create staged dir");
        } else {
            std::fs::create_dir_all(path.parent().expect("staged parent")).expect("create parent");
            std::fs::write(&path, contents).expect("write staged file");
        }
    }
    staging
}

fn run(command: &mut Command) {
    let output = command.output().expect("run archiver");
    assert!(
        output.status.success(),
        "{command:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn write_tar(archive: &Path, entries: &[(&str, &[u8])]) {
    let staging = stage(entries);
    run(Command::new("tar")
        .arg("-c")
        .arg("--no-recursion")
        .arg("-f")
        .arg(archive)
        .arg("-C")
        .arg(staging.path())
        .arg("--")
        .args(entries.iter().map(|(name, _)| name.trim_end_matches('/'))));
}
