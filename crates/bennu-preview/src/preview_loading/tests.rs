use file_core::{
    create_archive_with_progress, ArchiveCompressionLevel, ArchiveCreationRequest, ArchiveFormat,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use crate::preview::PreviewTreeDirectoryChildren;
use crate::text_preview::TextPreviewFormat;

use super::*;

fn default_rules() -> crate::preview_config::PreviewExtensionRules {
    crate::preview_config::PreviewExtensionRules::default_rules()
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

    let PreviewContent::Text { opened, .. } = preview_content else {
        panic!("expected text preview");
    };
    let rows: Vec<_> = opened
        .batch
        .rows
        .iter()
        .map(|row| row.text.as_str())
        .collect();
    assert_eq!(rows, ["hello"]);
    assert_eq!(opened.format(), TextPreviewFormat::Plain);
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

    let PreviewContent::Text { opened, .. } = preview_content else {
        panic!("expected text preview");
    };
    assert_eq!(opened.format(), TextPreviewFormat::Markdown);
    assert_eq!(
        opened.markdown.as_deref(),
        Some(&crate::markdown_render::RenderedMarkdown::parse(
            "# Title\n\nHello **world**.\n"
        ))
    );
}

#[tokio::test]
async fn load_preview_reads_a_first_window_of_rows() {
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

    let PreviewContent::Text { opened, .. } = preview_content else {
        panic!("expected text preview");
    };
    // 小文件第一个窗口就读完：150 行全在，最后一行行号 149。
    assert_eq!(opened.batch.rows.len(), 150);
    assert!(opened.batch.at_start && opened.batch.at_end);
    assert_eq!(opened.batch.rows[149].text, "line 149");
    assert_eq!(opened.batch.rows[149].line_number, Some(149));
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
    // 名字认不出的先当文本，打开时看内容定论。
    assert_eq!(
        classify_preview_path(Path::new("data.unknownext"), &rules),
        Some(PreviewPathKind::Text)
    );
}

#[test]
fn classify_preview_path_asks_the_type_library_for_text() {
    let rules = default_rules();
    for name in [
        "Makefile",
        "Dockerfile",
        "README",
        "main.go",
        "init.lua",
        "build-script",
        // TypeScript 与 MPEG-TS 同名，名字定不下来，交给内容。
        "index.ts",
    ] {
        assert_eq!(
            classify_preview_path(Path::new(name), &rules),
            Some(PreviewPathKind::Text),
            "{name}"
        );
    }
    // 名字已确定是非文本类型的不预览。
    assert_eq!(classify_preview_path(Path::new("libfoo.so"), &rules), None);
}

#[test]
fn classify_preview_path_asks_the_type_library_for_audio() {
    let rules = default_rules();
    let classify = |name: &str| classify_preview_path(Path::new(name), &rules);
    for name in [
        "a.mp3", "a.flac", "a.m4a", "a.opus", "a.wma", "a.ape", "a.aiff", "a.mka", "a.wv",
    ] {
        assert_eq!(classify(name), Some(PreviewPathKind::Audio), "{name}");
    }
    // 播放列表不是音频数据：m3u 声明继承 text/plain，走文本；pls 不预览。
    assert_eq!(classify("list.m3u"), Some(PreviewPathKind::Text));
    assert_eq!(classify("list.pls"), None);
    // 视频后缀表排在音频前面。
    assert_eq!(classify("clip.mp4"), Some(PreviewPathKind::Video));
    // 没后缀不为音频读内容，仍归文本兜底。
    assert_eq!(classify("track01"), Some(PreviewPathKind::Text));
}

#[tokio::test]
async fn load_preview_dispatches_type_library_audio_to_the_audio_loader() {
    let temp_dir = tempdir().expect("temp dir");
    let audio_path = temp_dir.path().join("broken.opus");
    std::fs::write(&audio_path, b"not really opus").expect("write audio fixture");

    let outcome = load_preview(
        audio_path,
        FileKind::File,
        &default_rules(),
        ScanOptions::default(),
        TEST_MAX_PREVIEW_FILE_BYTES,
    )
    .await;

    // 走的是音频加载（读不出音频信息；没装 ffmpeg 时是安装提示），不是文本加载（“二进制数据”）。
    match outcome {
        Err(error) => assert!(error.contains("audio file"), "{error}"),
        Ok(content) => assert!(
            matches!(content, PreviewContent::FfmpegRequired { .. }),
            "{content:?}"
        ),
    }
}

#[test]
fn classify_preview_path_follows_user_rules_replacing_builtins() {
    use crate::preview_config::PreviewExtensionRules;

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

    // 额外音频后缀压过类型库的结论（xyz 在类型库里是化学结构文件）。
    let mut rules = default_rules();
    rules.audio.push("xyz".to_owned());
    assert_eq!(
        classify_preview_path(Path::new("a.xyz"), &rules),
        Some(PreviewPathKind::Audio)
    );

    // 额外文本后缀压过类型库的非文本结论。
    let mut rules = default_rules();
    rules.text.push("so".to_owned());
    assert_eq!(
        classify_preview_path(Path::new("libfoo.so"), &rules),
        Some(PreviewPathKind::Text)
    );

    // 清空全部列表后，文本、音频仍由类型库认，其他类型不再预览。
    let empty = PreviewExtensionRules::default();
    assert_eq!(classify_preview_path(Path::new("photo.png"), &empty), None);
    assert_eq!(
        classify_preview_path(Path::new("song.flac"), &empty),
        Some(PreviewPathKind::Audio)
    );
    assert_eq!(
        classify_preview_path(Path::new("notes.txt"), &empty),
        Some(PreviewPathKind::Text)
    );
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
async fn load_preview_splits_a_long_markdown_line_on_character_boundaries() {
    let temp_dir = tempdir().expect("temp dir");
    let text_path = temp_dir.path().join("unicode.md");
    let content = "€".repeat(4 * 1024 * 1024);
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

    let PreviewContent::Text { opened, .. } = preview_content else {
        panic!("expected text preview");
    };
    // 12 MB 的一行：超过 2 MB 不给渲染全文；按约 1 万字节切段，每段都是完整的「€」。
    assert!(opened.markdown.is_none());
    assert!(opened.batch.rows.len() > 1);
    for row in &opened.batch.rows {
        assert!(row.text.chars().all(|character| character == '€'));
        assert!(row.end - row.start <= 10_002);
    }
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

#[test]
fn text_and_audio_read_on_demand_without_a_size_limit_or_a_local_copy() {
    // 文本按需读取、音频边读边放：没有大小上限，网络挂载上也不先下载。
    for kind in [PreviewPathKind::Text, PreviewPathKind::Audio] {
        assert_eq!(kind.size_limit_kind(), None, "{kind:?}");
        assert!(!kind.needs_local_copy(), "{kind:?}");
    }
    for kind in [
        PreviewPathKind::Document,
        PreviewPathKind::Archive,
        PreviewPathKind::Sqlite,
        PreviewPathKind::AnimatedImage,
        PreviewPathKind::Image,
        PreviewPathKind::Video,
    ] {
        assert!(kind.size_limit_kind().is_some(), "{kind:?}");
        assert!(kind.needs_local_copy(), "{kind:?}");
    }
}
