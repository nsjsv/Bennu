use std::path::Path;

use iced::futures::StreamExt;
use tempfile::tempdir;

use super::*;
use crate::engine::{
    default_preview_size, PreviewEngine, PreviewEngineConfig, PreviewWindowIdentity,
};
use crate::preview::PreviewWindowProfile;
use crate::preview_config::{PreviewExtensionRules, PreviewFileSizeLimits};
use crate::text_document::{LineCountStatus, TextViewport};
use crate::text_preview_loading::load_initial_text_preview;
use file_core::ScanOptions;

fn test_engine() -> PreviewEngine {
    PreviewEngine::new(
        PreviewEngineConfig {
            directory_expand_levels: 1,
            extension_rules: Default::default(),
            size_limits: PreviewFileSizeLimits::with_default_limits(),
        },
        default_preview_size(PreviewWindowProfile::Regular),
        PreviewWindowIdentity {
            app_id: "bennu-preview-tests".to_owned(),
            icon: None,
        },
    )
}

/// 执行任务，把产出的消息交回引擎，直到没有新任务（替代宿主的运行时）。
async fn drive(engine: &mut PreviewEngine, task: Task<PreviewMessage>) {
    let mut pending = vec![task];
    let mut rounds = 0;
    while let Some(task) = pending.pop() {
        rounds += 1;
        assert!(rounds < 10_000, "tasks never settled");
        let Some(mut stream) = iced_runtime::task::into_stream(task) else {
            continue;
        };
        while let Some(action) = stream.next().await {
            if let iced_runtime::Action::Output(message) = action {
                if let Ok(next) = engine.handle(message) {
                    pending.push(next);
                }
            }
        }
    }
}

async fn open(engine: &mut PreviewEngine, path: &Path) {
    engine.preview = Some(PreviewState::Loading(path.to_path_buf()));
    let content =
        load_initial_text_preview(path.to_path_buf(), &PreviewExtensionRules::default_rules())
            .await
            .expect("open");
    let (_, task) =
        engine.accept_preview(path.to_path_buf(), Ok(content), ScanOptions::default(), 1);
    drive(engine, task).await;
}

fn document(engine: &PreviewEngine) -> &TextPreviewDocument {
    engine
        .active_text_preview_document()
        .expect("text document")
}

async fn report_viewport(engine: &mut PreviewEngine, first: usize, rows: usize, at_bottom: bool) {
    let window = document(engine).rows();
    let viewport = TextViewport {
        first_row: window[first].start,
        last_row: window[(first + rows - 1).min(window.len() - 1)].start,
        at_bottom,
        cursor: None,
    };
    let task = engine.handle_text_viewer_event(TextViewerEvent::ViewportChanged(viewport));
    drive(engine, task).await;
}

fn numbered(count: usize) -> String {
    (0..count).map(|index| format!("line {index}\n")).collect()
}

#[tokio::test]
async fn opening_counts_all_lines_in_the_background() {
    let dir = tempdir().expect("dir");
    let path = dir.path().join("big.log");
    std::fs::write(&path, numbered(300_000)).expect("write");
    let mut engine = test_engine();
    open(&mut engine, &path).await;
    assert_eq!(
        document(&engine).line_count_status(),
        LineCountStatus::Done { total: 300_000 }
    );
    // 第一个窗口读的是开头，不是整个文件。
    assert!(!document(&engine).at_end());
}

#[tokio::test]
async fn appended_lines_appear_and_are_followed_from_the_bottom() {
    let dir = tempdir().expect("dir");
    let path = dir.path().join("app.log");
    std::fs::write(&path, numbered(30)).expect("write");
    let mut engine = test_engine();
    open(&mut engine, &path).await;
    report_viewport(&mut engine, 20, 10, true).await;

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append");
    std::io::Write::write_all(&mut file, b"line 30\nline 31\n").expect("write more");
    let id = document(&engine).id();
    let task = engine.handle_text_file_changed(id);
    drive(&mut engine, task).await;

    let document = document(&engine);
    assert_eq!(document.id(), id);
    assert_eq!(document.rows().len(), 32);
    let jump = document.jump().expect("follow");
    assert_eq!(jump.row, document.rows().back().expect("last").start);
    assert_eq!(
        document.line_count_status(),
        LineCountStatus::Done { total: 32 }
    );
}

#[tokio::test]
async fn a_rewritten_file_is_reopened_near_the_same_place() {
    let dir = tempdir().expect("dir");
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, numbered(100)).expect("write");
    let mut engine = test_engine();
    open(&mut engine, &path).await;
    report_viewport(&mut engine, 50, 10, false).await;
    let old_id = document(&engine).id();

    // 编辑器的写法：写到临时文件再改名替换。
    let replacement = dir.path().join("notes.txt.tmp");
    std::fs::write(&replacement, numbered(100).replace("line", "LINE")).expect("write new");
    std::fs::rename(&replacement, &path).expect("replace");
    let task = engine.handle_text_file_changed(old_id);
    drive(&mut engine, task).await;

    let document = document(&engine);
    assert_ne!(document.id(), old_id);
    let jump = document.jump().expect("focus");
    let row = &document.rows()[document.row_index(jump.row).expect("row")];
    assert_eq!(row.text, "LINE 50");
}

#[tokio::test]
async fn a_deleted_file_keeps_showing_what_was_open() {
    let dir = tempdir().expect("dir");
    let path = dir.path().join("gone.txt");
    std::fs::write(&path, numbered(10)).expect("write");
    let mut engine = test_engine();
    open(&mut engine, &path).await;
    let id = document(&engine).id();
    std::fs::remove_file(&path).expect("remove");
    let task = engine.handle_text_file_changed(id);
    drive(&mut engine, task).await;
    assert_eq!(document(&engine).id(), id);
    assert_eq!(document(&engine).rows().len(), 10);
}

#[tokio::test]
async fn choosing_an_encoding_keeps_the_position_and_the_markdown_mode() {
    let dir = tempdir().expect("dir");
    let path = dir.path().join("notes.md");
    std::fs::write(&path, numbered(80_000)).expect("write");
    let mut engine = test_engine();
    open(&mut engine, &path).await;
    let middle = document(&engine).len() / 2;
    let task =
        engine.handle_text_viewer_event(TextViewerEvent::JumpRequested(TextJump::Offset(middle)));
    drive(&mut engine, task).await;
    let focus = document(&engine).jump().expect("jump").row;
    let focus_index = document(&engine).row_index(focus).expect("focus");
    let focus_text = document(&engine).rows()[focus_index].text.clone();
    report_viewport(&mut engine, focus_index, 10, false).await;
    let task = engine.handle(PreviewMessage::MarkdownPreviewModeSelected(
        crate::text_preview::MarkdownPreviewMode::Raw,
    ));
    drive(&mut engine, task.expect("engine message")).await;

    let task = engine.select_text_preview_encoding(encoding_rs::WINDOWS_1252);
    drive(&mut engine, task).await;

    let document = document(&engine);
    assert_eq!(document.encoding(), encoding_rs::WINDOWS_1252);
    assert!(document.encoding_chosen());
    assert_eq!(
        document.markdown_preview_mode(),
        crate::text_preview::MarkdownPreviewMode::Raw
    );
    let jump = document.jump().expect("focus");
    let row = &document.rows()[document.row_index(jump.row).expect("row")];
    assert_eq!(row.text, focus_text);
}

fn resident_memory_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

/// 手动跑的测量：`cargo test -p bennu-preview --release measure_large_text -- --ignored --nocapture`。
/// 生成 2 GB / 2000 万行日志和一行 50 MB 的 JSON，记录打开、跳转、统计行数的耗时与常驻内存。
#[tokio::test]
#[ignore]
async fn measure_large_text_files() {
    use std::io::Write;
    use std::time::Instant;

    let dir = tempdir().expect("dir");
    let log = dir.path().join("huge.log");
    {
        let mut writer = std::io::BufWriter::with_capacity(
            1 << 20,
            std::fs::File::create(&log).expect("create"),
        );
        for index in 0..20_000_000_u64 {
            writeln!(
                writer,
                "2026-11-13 12:34:56.{:03} INFO [worker-{:02}] request id={index:010} took {}ms path=/api/v1/items/{}",
                index % 1000,
                index % 16,
                index % 997,
                index % 100_000
            )
            .expect("write");
        }
        writer.flush().expect("flush");
    }
    let size = std::fs::metadata(&log).expect("metadata").len();
    let rss_before = resident_memory_kib();

    let mut engine = test_engine();
    engine.preview = Some(PreviewState::Loading(log.clone()));
    let started = Instant::now();
    let content = load_initial_text_preview(log.clone(), &PreviewExtensionRules::default_rules())
        .await
        .expect("open");
    let (_, count_task) =
        engine.accept_preview(log.clone(), Ok(content), ScanOptions::default(), 1);
    let open_time = started.elapsed();

    let middle = document(&engine).len() / 2;
    let started = Instant::now();
    let task =
        engine.handle_text_viewer_event(TextViewerEvent::JumpRequested(TextJump::Offset(middle)));
    drive(&mut engine, task).await;
    let middle_time = started.elapsed();

    let started = Instant::now();
    let task = engine.handle_text_viewer_event(TextViewerEvent::JumpRequested(TextJump::End));
    drive(&mut engine, task).await;
    let end_time = started.elapsed();
    let rss_after_jumps = resident_memory_kib();

    let started = Instant::now();
    drive(&mut engine, count_task).await;
    let count_time = started.elapsed();
    let rss_after_count = resident_memory_kib();
    assert_eq!(
        document(&engine).line_count_status(),
        LineCountStatus::Done { total: 20_000_000 }
    );

    let json = dir.path().join("minified.json");
    {
        let mut writer = std::io::BufWriter::new(std::fs::File::create(&json).expect("create"));
        write!(writer, "[").expect("write");
        let mut written = 1;
        let mut index = 0;
        while written < 50 * 1024 * 1024 {
            let item =
                format!("{{\"id\":{index},\"名字\":\"条目{index}\",\"tags\":[\"a\",\"b\"]}},");
            written += item.len();
            writer.write_all(item.as_bytes()).expect("write");
            index += 1;
        }
        write!(writer, "0]").expect("write");
        writer.flush().expect("flush");
    }
    let mut json_engine = test_engine();
    json_engine.preview = Some(PreviewState::Loading(json.clone()));
    let started = Instant::now();
    let content = load_initial_text_preview(json.clone(), &PreviewExtensionRules::default_rules())
        .await
        .expect("open json");
    let (_, json_count) =
        json_engine.accept_preview(json.clone(), Ok(content), ScanOptions::default(), 1);
    let json_open_time = started.elapsed();
    let json_middle = document(&json_engine).len() / 2;
    let started = Instant::now();
    let task = json_engine.handle_text_viewer_event(TextViewerEvent::JumpRequested(
        TextJump::Offset(json_middle),
    ));
    drive(&mut json_engine, task).await;
    let json_middle_time = started.elapsed();
    drop(json_count);
    let rows = document(&json_engine).rows();
    assert!(rows.iter().all(|row| !row.text.contains('\u{fffd}')));

    println!("log: {size} bytes, 20000000 lines");
    println!("open to first window: {open_time:?}");
    println!("jump to 50%: {middle_time:?}");
    println!("jump to end: {end_time:?}");
    println!("count all lines: {count_time:?}");
    println!("RSS before {rss_before} KiB, after jumps {rss_after_jumps} KiB, after count {rss_after_count} KiB");
    println!("json: open {json_open_time:?}, jump to middle of the single line {json_middle_time:?}, window rows {}", rows.len());
}

fn token_kinds(
    document: &TextPreviewDocument,
    row_text: &str,
) -> Vec<crate::text_highlight::TokenKind> {
    let row = document
        .rows()
        .iter()
        .find(|row| row.text == row_text)
        .unwrap_or_else(|| panic!("row {row_text:?} not in window"));
    document
        .row_tokens(row.start)
        .map(|tokens| tokens.iter().map(|token| token.kind).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn code_is_highlighted_consistently_after_a_jump_into_a_block_comment() {
    use crate::text_document::HighlightStatus;
    use crate::text_highlight::TokenKind;

    let dir = tempdir().expect("dir");
    let path = dir.path().join("main.c");
    // 一个跨越很多检查点的块注释，跳进中间看颜色是否仍是注释。
    let mut source = String::from("int before;\n/* long comment starts\n");
    for index in 0..5_000 {
        source.push_str(&format!("comment body {index}\n"));
    }
    source.push_str("comment ends */\nint after;\n");
    for index in 0..20_000 {
        source.push_str(&format!("int value_{index} = {index};\n"));
    }
    std::fs::write(&path, &source).expect("write");
    let mut engine = test_engine();
    open(&mut engine, &path).await;
    assert!(matches!(
        document(&engine).highlight_status(),
        HighlightStatus::Ready(_)
    ));
    report_viewport(&mut engine, 0, 20, false).await;
    assert!(token_kinds(document(&engine), "int before;").contains(&TokenKind::Keyword));

    let target = source.find("comment body 4000").expect("line") as u64;
    let task =
        engine.handle_text_viewer_event(TextViewerEvent::JumpRequested(TextJump::Offset(target)));
    drive(&mut engine, task).await;
    let focus = document(&engine).jump().expect("jump").row;
    let index = document(&engine).row_index(focus).expect("focus");
    report_viewport(&mut engine, index, 20, false).await;
    assert_eq!(
        token_kinds(document(&engine), "comment body 4000"),
        vec![TokenKind::Comment]
    );
}

#[tokio::test]
async fn plain_text_and_huge_files_are_not_highlighted() {
    use crate::text_document::HighlightStatus;

    let dir = tempdir().expect("dir");
    let plain = dir.path().join("notes.txt");
    std::fs::write(&plain, numbered(100)).expect("write");
    let mut engine = test_engine();
    open(&mut engine, &plain).await;
    assert_eq!(
        *document(&engine).highlight_status(),
        HighlightStatus::Unavailable
    );

    let huge = dir.path().join("huge.rs");
    let line = "fn f() { let x = 1; }\n";
    let count = (crate::text_highlight::HIGHLIGHT_LIMIT_BYTES as usize / line.len()) + 10;
    std::fs::write(&huge, line.repeat(count)).expect("write");
    let mut engine = test_engine();
    open(&mut engine, &huge).await;
    report_viewport(&mut engine, 0, 20, false).await;
    assert_eq!(
        *document(&engine).highlight_status(),
        HighlightStatus::TooLarge
    );
    assert!(document(&engine).row_tokens(0).is_none());
}

/// 手动跑的测量：`... measure_highlighting -- --ignored --nocapture`。一个约 10 MB 的 Rust
/// 源文件：打开到首屏上色、跳到末尾到上色（要等后台从头解析到末尾）、解析完后跳到中间
/// 到上色的耗时，以及前后的常驻内存。
#[tokio::test]
#[ignore]
async fn measure_highlighting() {
    use std::time::Instant;

    let dir = tempdir().expect("dir");
    let path = dir.path().join("big.rs");
    let unit = "/// 文档注释\npub fn compute(value: u64) -> Result<u64, String> {\n    let doubled = value * 2; // 注释\n    if doubled > 100 { return Err(\"too big\".to_owned()); }\n    Ok(doubled)\n}\n";
    let count = (crate::text_highlight::HIGHLIGHT_LIMIT_BYTES as usize - 4096) / unit.len();
    std::fs::write(&path, unit.repeat(count)).expect("write");
    let rss_before = resident_memory_kib();

    let mut engine = test_engine();
    let started = Instant::now();
    open(&mut engine, &path).await;
    let first_screen = started.elapsed();
    assert!(document(&engine)
        .row_tokens(document(&engine).rows()[0].start)
        .is_some());

    let end_time = jump_and_colour(&mut engine, TextJump::End).await;
    let rss_after_pass = resident_memory_kib();
    let middle = document(&engine).len() / 2;
    let middle_time = jump_and_colour(&mut engine, TextJump::Offset(middle)).await;

    println!("file: {} bytes", document(&engine).len());
    println!("open + colour first screen: {first_screen:?}");
    println!("jump to end + colour (waits for the whole pass): {end_time:?}");
    println!("jump to middle + colour after the pass: {middle_time:?}");
    println!("RSS before {rss_before} KiB, after highlighting {rss_after_pass} KiB");
}

async fn jump_and_colour(engine: &mut PreviewEngine, jump: TextJump) -> std::time::Duration {
    let started = std::time::Instant::now();
    let task = engine.handle_text_viewer_event(TextViewerEvent::JumpRequested(jump));
    drive(engine, task).await;
    let focus = document(engine).jump().expect("jump").row;
    let index = document(engine).row_index(focus).expect("focus");
    report_viewport(engine, index.saturating_sub(40), 40, false).await;
    assert!(document(engine).row_tokens(focus).is_some());
    started.elapsed()
}
