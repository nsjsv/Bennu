use file_core::{
    create_archive_with_progress, ArchiveCompressionLevel, ArchiveCreationRequest, ArchiveFormat,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use crate::model::{PreviewTreeDirectoryChildren, TextPreviewFormat};
use crate::text_preview_loading::PREVIEW_TEXT_LIMIT;

use super::*;

fn default_rules() -> crate::config::PreviewExtensionRules {
    crate::config::PreviewExtensionRules::default_rules()
}

const TEST_MAX_PREVIEW_FILE_BYTES: u64 = 8 * 1024 * 1024;
const TEST_LARGE_MAX_PREVIEW_FILE_BYTES: u64 = 32 * 1024 * 1024;

#[tokio::test]
async fn load_preview_reads_zip_archive_tree() {
    let temp_dir = tempdir().expect("temp dir");
    let archive_path = temp_dir.path().join("sample.zip");
    write_test_archive(&archive_path, ArchiveFormat::Zip).await;

    let preview_content = load_preview(
        archive_path.clone(),
        FileKind::File,
        &default_rules(),
        ScanOptions::default(),
        TEST_MAX_PREVIEW_FILE_BYTES,
    )
    .await
    .expect("zip archive preview");

    let PreviewContent::Archive { entries } = preview_content else {
        panic!("expected archive preview");
    };

    assert_preview_tree_entry(&entries[0], "src", FileKind::Directory, 0, None);
    assert_preview_tree_entry(&entries[1], "main.rs", FileKind::File, 1, Some(0));
    assert_preview_tree_entry(&entries[2], "README.md", FileKind::File, 0, None);
    assert!(entries[0].is_expanded);
    assert_eq!(entries[0].toggle_rotation_progress, 1.0);
}

#[tokio::test]
async fn load_preview_reports_recognized_unsupported_archive_format() {
    let temp_dir = tempdir().expect("temp dir");
    let archive_path = temp_dir.path().join("sample.XZ");
    std::fs::write(&archive_path, b"not an archive").expect("write archive fixture");

    let error = load_preview(
        archive_path,
        FileKind::File,
        &default_rules(),
        ScanOptions::default(),
        TEST_MAX_PREVIEW_FILE_BYTES,
    )
    .await
    .expect_err("unsupported archive should be identified");

    assert!(error.contains("This archive format is not supported yet"));
    assert!(error.contains(SUPPORTED_ARCHIVE_FORMAT_MESSAGE));
}

#[test]
fn unsupported_archive_classifier_does_not_overlap_supported_formats() {
    for path in [
        "sample.xz",
        "sample.BZ2",
        "sample.zst",
        "sample.deb",
        "sample.rpm",
    ] {
        assert!(
            is_recognized_unsupported_archive_path(Path::new(path)),
            "{path}"
        );
    }
    for path in [
        "sample.zip",
        "sample.tar",
        "sample.tar.gz",
        "sample.tgz",
        "sample.7z",
        "sample.rar",
        "sample.txt",
    ] {
        assert!(
            !is_recognized_unsupported_archive_path(Path::new(path)),
            "{path}"
        );
    }
}

#[tokio::test]
async fn load_preview_reads_directory_top_layer_only() {
    let temp_dir = tempdir().expect("temp dir");
    let nested_dir = temp_dir.path().join("src");
    std::fs::create_dir(&nested_dir).expect("create nested dir");
    std::fs::write(nested_dir.join("main.rs"), "fn main() {}\n").expect("write nested file");
    std::fs::write(temp_dir.path().join("README.md"), "# sample\n").expect("write readme");

    let preview_content = load_preview(
        temp_dir.path().to_path_buf(),
        FileKind::Directory,
        &default_rules(),
        ScanOptions::default(),
        TEST_MAX_PREVIEW_FILE_BYTES,
    )
    .await
    .expect("directory preview");

    let PreviewContent::Directory { entries } = preview_content else {
        panic!("expected directory preview");
    };

    assert_eq!(entries.len(), 2);
    assert_preview_tree_entry(&entries[0], "src", FileKind::Directory, 0, None);
    assert_preview_tree_entry(&entries[1], "README.md", FileKind::File, 0, None);
    assert_eq!(
        entries[0].filesystem_path.as_deref(),
        Some(nested_dir.as_path())
    );
    assert_eq!(
        entries[0].directory_children.as_ref(),
        Some(&PreviewTreeDirectoryChildren::Pending)
    );
    assert!(!entries[0].is_expanded);
    assert_eq!(entries[0].toggle_rotation_progress, 0.0);
}

#[tokio::test]
async fn load_directory_preview_children_reads_expanded_layer() {
    let temp_dir = tempdir().expect("temp dir");
    let nested_dir = temp_dir.path().join("src");
    std::fs::create_dir(&nested_dir).expect("create nested dir");
    std::fs::create_dir(nested_dir.join("deeper")).expect("create deeper dir");
    std::fs::write(nested_dir.join("main.rs"), "fn main() {}\n").expect("write nested file");

    let children = load_directory_preview_children(nested_dir, ScanOptions::default())
        .await
        .expect("directory preview children");
    let child_names = children
        .iter()
        .map(|entry| entry.name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(child_names, vec!["deeper".to_owned(), "main.rs".to_owned()]);
}

#[tokio::test]
async fn load_preview_reads_gzip_tar_archive_tree() {
    let temp_dir = tempdir().expect("temp dir");
    let archive_path = temp_dir.path().join("sample.tar.gz");
    write_test_archive(&archive_path, ArchiveFormat::TarGz).await;

    let preview_content = load_preview(
        archive_path,
        FileKind::File,
        &default_rules(),
        ScanOptions::default(),
        TEST_MAX_PREVIEW_FILE_BYTES,
    )
    .await
    .expect("tar.gz archive preview");

    let PreviewContent::Archive { entries, .. } = preview_content else {
        panic!("expected archive preview");
    };

    assert_preview_tree_entry(&entries[0], "src", FileKind::Directory, 0, None);
    assert_preview_tree_entry(&entries[1], "main.rs", FileKind::File, 1, Some(0));
    assert_preview_tree_entry(&entries[2], "README.md", FileKind::File, 0, None);
}

#[tokio::test]
async fn load_preview_keeps_utf8_text_preview() {
    let temp_dir = tempdir().expect("temp dir");
    let text_path = temp_dir.path().join("note.txt");
    std::fs::write(&text_path, "hello\n").expect("write text file");

    let preview_content = load_preview(
        text_path,
        FileKind::File,
        &default_rules(),
        ScanOptions::default(),
        TEST_MAX_PREVIEW_FILE_BYTES,
    )
    .await
    .expect("text preview");

    let PreviewContent::Text {
        rendered, format, ..
    } = preview_content
    else {
        panic!("expected text preview");
    };
    assert_eq!(rendered.as_ref(), "hello\n");
    assert_eq!(format, TextPreviewFormat::Plain);
}

#[tokio::test]
async fn load_preview_renders_markdown_text_preview() {
    let temp_dir = tempdir().expect("temp dir");
    let text_path = temp_dir.path().join("README.md");
    std::fs::write(&text_path, "# Title\n\nHello **world**.\n").expect("write markdown file");

    let preview_content = load_preview(
        text_path,
        FileKind::File,
        &default_rules(),
        ScanOptions::default(),
        TEST_MAX_PREVIEW_FILE_BYTES,
    )
    .await
    .expect("markdown preview");

    let PreviewContent::Text {
        rendered, format, ..
    } = preview_content
    else {
        panic!("expected text preview");
    };
    assert_eq!(format, TextPreviewFormat::Markdown);
    assert_eq!(rendered.as_ref(), "# Title\n\nHello **world**.\n");
}

#[tokio::test]
async fn load_preview_chunks_plain_text_preview() {
    let temp_dir = tempdir().expect("temp dir");
    let text_path = temp_dir.path().join("large.txt");
    let content = numbered_line_range(0, 150);
    std::fs::write(&text_path, &content).expect("write text file");

    let preview_content = load_preview(
        text_path,
        FileKind::File,
        &default_rules(),
        ScanOptions::default(),
        TEST_MAX_PREVIEW_FILE_BYTES,
    )
    .await
    .expect("text preview");

    let PreviewContent::Text {
        rendered,
        next_offset,
        loaded_line_count,
        line_limit_notice,
        ..
    } = preview_content
    else {
        panic!("expected text preview");
    };
    assert!(rendered.contains("line 49"));
    assert!(!rendered.contains("line 50"));
    assert_eq!(loaded_line_count, 50);
    assert!(next_offset.is_some());
    assert_eq!(line_limit_notice, None);
}

#[tokio::test]
async fn load_preview_rejects_file_over_configured_limit() {
    let temp_dir = tempdir().expect("temp dir");
    let text_path = temp_dir.path().join("single-line.txt");
    let content = "a".repeat(1025);
    std::fs::write(&text_path, content).expect("write text file");

    let error = load_preview(
        text_path,
        FileKind::File,
        &default_rules(),
        ScanOptions::default(),
        1024,
    )
    .await
    .expect_err("file should be rejected");

    assert!(error.contains("File is too large to preview"));
}

#[tokio::test]
async fn load_preview_allows_any_size_when_limit_is_zero() {
    let temp_dir = tempdir().expect("temp dir");
    let text_path = temp_dir.path().join("single-line.txt");
    let content = "a".repeat(4096);
    std::fs::write(&text_path, content).expect("write text file");

    let preview_content = load_preview(
        text_path,
        FileKind::File,
        &default_rules(),
        ScanOptions::default(),
        0,
    )
    .await
    .expect("zero limit must not reject");

    assert!(matches!(preview_content, PreviewContent::Text { .. }));
}

#[test]
fn classify_preview_path_matches_preview_dispatch() {
    let rules = default_rules();

    assert_eq!(
        classify_preview_path(Path::new("report.pdf"), &rules),
        Some(PreviewPathKind::Document)
    );
    assert_eq!(
        classify_preview_path(Path::new("report.docx"), &rules),
        Some(PreviewPathKind::Document)
    );
    assert_eq!(
        classify_preview_path(Path::new("bundle.zip"), &rules),
        Some(PreviewPathKind::Archive)
    );
    assert_eq!(
        classify_preview_path(Path::new("bundle.tar.gz"), &rules),
        Some(PreviewPathKind::Archive)
    );
    assert_eq!(
        classify_preview_path(Path::new("backup.db3"), &rules),
        Some(PreviewPathKind::Sqlite)
    );
    assert_eq!(
        classify_preview_path(Path::new("clip.mp4"), &rules),
        Some(PreviewPathKind::Video)
    );
    assert_eq!(
        classify_preview_path(Path::new("song.flac"), &rules),
        Some(PreviewPathKind::Audio)
    );
    assert_eq!(
        classify_preview_path(Path::new("animation.gif"), &rules),
        Some(PreviewPathKind::AnimatedImage)
    );
    assert_eq!(
        classify_preview_path(Path::new("photo.png"), &rules),
        Some(PreviewPathKind::Image)
    );
    assert_eq!(
        classify_preview_path(Path::new("notes.txt"), &rules),
        Some(PreviewPathKind::Text)
    );
    // 不在任何类型列表里的后缀不可预览。
    assert_eq!(
        classify_preview_path(Path::new("data.unknownext"), &rules),
        None
    );
}

#[test]
fn classify_preview_path_follows_user_rules_replacing_builtins() {
    use crate::config::PreviewExtensionRules;

    // 替换式语义：从图片列表删掉 png 后，png 不再按图片预览。
    let mut rules = default_rules();
    rules.image.retain(|candidate| candidate != "png");
    assert_eq!(classify_preview_path(Path::new("photo.png"), &rules), None);

    // 自定义后缀加入视频列表后按视频预览。
    let mut rules = default_rules();
    rules.video.push("dat".to_owned());
    assert_eq!(
        classify_preview_path(Path::new("media.DAT"), &rules),
        Some(PreviewPathKind::Video)
    );

    // 同一后缀命中多个列表时，dispatch 顺序决定类型（文档优先）。
    let mut rules = PreviewExtensionRules::default_rules();
    rules.document.push("dat".to_owned());
    rules.text.push("dat".to_owned());
    assert_eq!(
        classify_preview_path(Path::new("a.dat"), &rules),
        Some(PreviewPathKind::Document)
    );

    // 清空全部列表后没有文件可预览。
    let empty = PreviewExtensionRules::default();
    assert_eq!(classify_preview_path(Path::new("notes.txt"), &empty), None);
}

#[tokio::test]
async fn load_preview_sniffs_custom_archive_extension_by_content() {
    let temp_dir = tempdir().expect("temp dir");
    let archive_path = temp_dir.path().join("sample.datpack");
    write_test_archive(&archive_path, ArchiveFormat::Zip).await;

    let mut rules = default_rules();
    rules.archive.push("datpack".to_owned());
    let preview_content = load_preview(
        archive_path,
        FileKind::File,
        &rules,
        ScanOptions::default(),
        TEST_MAX_PREVIEW_FILE_BYTES,
    )
    .await
    .expect("custom archive extension preview");

    assert!(matches!(preview_content, PreviewContent::Archive { .. }));
}
#[tokio::test]
async fn load_preview_truncates_long_markdown_on_utf8_boundary() {
    let temp_dir = tempdir().expect("temp dir");
    let text_path = temp_dir.path().join("unicode.md");
    let content = "€".repeat(PREVIEW_TEXT_LIMIT);
    std::fs::write(&text_path, content).expect("write text file");

    let preview_content = load_preview(
        text_path,
        FileKind::File,
        &default_rules(),
        ScanOptions::default(),
        TEST_LARGE_MAX_PREVIEW_FILE_BYTES,
    )
    .await
    .expect("text preview");

    let PreviewContent::Text { rendered, .. } = preview_content else {
        panic!("expected text preview");
    };
    assert_eq!(rendered.len(), PREVIEW_TEXT_LIMIT - 1);
    assert!(rendered.is_char_boundary(rendered.len()));
}

fn numbered_line_range(start: usize, end: usize) -> String {
    let mut content = String::new();
    for index in start..end {
        content.push_str(&format!("line {index}\n"));
    }
    content
}

async fn write_test_archive(path: &Path, format: ArchiveFormat) {
    let source_dir = path
        .parent()
        .expect("archive parent")
        .join("archive-source");
    std::fs::create_dir_all(source_dir.join("src")).expect("create source dir");
    std::fs::write(source_dir.join("src").join("main.rs"), "fn main() {}\n")
        .expect("write nested source");
    std::fs::write(source_dir.join("README.md"), "# sample\n").expect("write readme source");

    create_archive_with_progress(
        ArchiveCreationRequest {
            sources: vec![source_dir.join("src"), source_dir.join("README.md")],
            target: path.to_path_buf(),
            format,
            compression_level: ArchiveCompressionLevel::Balanced,
            password: None,
        },
        CancellationToken::new(),
        |_| {},
    )
    .await
    .expect("create test archive");
}

fn assert_preview_tree_entry(
    entry: &PreviewTreeEntry,
    name: &str,
    kind: FileKind,
    depth: usize,
    parent: Option<usize>,
) {
    assert_eq!(entry.name, name);
    assert_eq!(entry.kind, kind);
    assert_eq!(entry.depth, depth);
    assert_eq!(entry.parent, parent);
}
