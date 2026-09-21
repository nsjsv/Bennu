use crate::text_preview::TextPreviewDocument;
use crate::text_preview::TextPreviewFormat;
use iced::advanced::widget::operation::scrollable::AbsoluteOffset;
use iced::advanced::widget::operation::Scrollable as _;
use iced::Point;

use super::*;

fn viewer_state_for(document: &TextPreviewDocument) -> TextPreviewViewerState {
    let mut state = TextPreviewViewerState::default();
    state.line_heights = vec![None; document.line_count()];
    state.cached_content_height = document.line_count() as f32 * base_line_height();
    state.content_revision = Some(document.content_revision());
    state
}

fn document_for_text(content: &str) -> TextPreviewDocument {
    TextPreviewDocument::new_initial(
        PathBuf::from("/tmp/text-preview-test.txt"),
        content,
        TextPreviewFormat::Plain,
        1,
        Some(100),
        0,
        None,
    )
}

fn lines(count: usize) -> String {
    (0..count)
        .map(|line_number| format!("line {line_number}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn scroll_viewer_by_advances_anchor_by_pixel_distance() {
    let document = document_for_text(&lines(1_000));
    let mut state = viewer_state_for(&document);
    let line_height = base_line_height();
    let applied = scroll_viewer_by(&mut state, line_height * 10.0 + 5.0, 384.0);

    assert_eq!(applied, 10);
    assert_eq!(state.scroll_line, 10);
    assert!((state.scroll_pixel - 5.0).abs() < 1e-3);
    assert!((viewer_scroll_absolute(&state) - (line_height * 10.0 + 5.0)).abs() < 1e-3);
    // 前缀缓存与锚定行保持一致。
    assert!((state.anchor_prefix - line_height * 10.0).abs() < 1e-3);
}

#[test]
fn scroll_viewer_by_clamps_at_content_edges() {
    let document = document_for_text(&lines(100));
    let mut state = viewer_state_for(&document);
    let line_height = base_line_height();
    let viewport = 384.0;
    let total = 100.0 * line_height;

    scroll_viewer_by(&mut state, -50.0, viewport);
    assert_eq!(state.scroll_line, 0);
    assert_eq!(state.scroll_pixel, 0.0);

    scroll_viewer_by(&mut state, total, viewport);
    let bottom = total - viewport;
    assert!((viewer_scroll_absolute(&state) - bottom).abs() < 1e-2);

    // 到底后继续向下滚，锚点与绝对位置都不再变化。
    assert_eq!(scroll_viewer_by(&mut state, 1_000.0, viewport), 0);
    assert!((viewer_scroll_absolute(&state) - bottom).abs() < 1e-2);

    // 向上回滚脱离底部边界。
    scroll_viewer_by(&mut state, -line_height, viewport);
    assert!(viewer_scroll_absolute(&state) < bottom - line_height * 0.5);
}

#[test]
fn visible_lines_start_partial_at_anchor_offset() {
    let document = document_for_text(&lines(200));
    let mut state = viewer_state_for(&document);
    let line_height = base_line_height();
    state.scroll_line = 5;
    state.scroll_pixel = line_height * 0.5;

    let visible = visible_logical_lines(&state, 384.0);

    // 锚定行是首个部分可见行，y 为行内偏移的负值。
    assert_eq!(visible.first().map(|line| line.line_index), Some(5));
    assert!((visible[0].visible_y - -line_height * 0.5).abs() < 1e-3);
    assert!(visible.len() > 1);
    assert!(visible
        .windows(2)
        .all(|pair| pair[1].line_index == pair[0].line_index + 1));
    assert!(visible
        .iter()
        .all(|line| line.visible_y < 384.0 + line_height));
}

#[test]
fn measured_line_heights_only_grow() {
    let document = document_for_text(&lines(1_000));
    let mut state = viewer_state_for(&document);
    let line_height = base_line_height();
    let initial_content_height = state.cached_content_height;

    note_measured_line_height(&mut state, 3, line_height * 5.0);
    assert_eq!(line_height_of(&state, 3), line_height * 5.0);
    assert!(
        (state.cached_content_height - (initial_content_height + line_height * 4.0)).abs() < 1e-2
    );

    // 编辑器内部裁剪 layout 后观测值缩小时，保留旧值。
    note_measured_line_height(&mut state, 3, line_height * 2.0);
    assert_eq!(line_height_of(&state, 3), line_height * 5.0);
    assert!(
        (state.cached_content_height - (initial_content_height + line_height * 4.0)).abs() < 1e-2
    );
}

#[test]
fn scroll_by_reports_line_delta_and_absolute_offset() {
    let content = lines(1_000);
    let document = document_for_text(&content);
    let mut state = viewer_state_for(&document);
    let line_height = base_line_height();
    let bounds = Rectangle::new(Point::ORIGIN, Size::new(400.0, 400.0));

    let mut bridge = ScrollBridge {
        state: &mut state,
        document: &document,
        text_width: 384.0,
    };
    bridge.scroll_by(
        AbsoluteOffset {
            x: 0.0,
            y: line_height * 2.0,
        },
        bounds,
        bounds,
    );

    assert_eq!(bridge.state.scroll_line, 2);
    assert_eq!(bridge.state.scroll_pixel, 0.0);
    assert_eq!(
        bridge.state.pending_scroll_report,
        Some((2, line_height * 2.0, 384.0))
    );

    // 不足一行的增量进入锚定行内偏移，总位置保持连续。
    bridge.scroll_by(
        AbsoluteOffset {
            x: 0.0,
            y: line_height * 0.5,
        },
        bounds,
        bounds,
    );
    assert!((bridge.state.scroll_pixel - line_height * 0.5).abs() < 1e-3);
    assert_eq!(bridge.state.scroll_line, 2);

    bridge.scroll_by(
        AbsoluteOffset {
            x: 0.0,
            y: line_height * 0.5,
        },
        bounds,
        bounds,
    );
    assert!(bridge.state.scroll_pixel.abs() < 1e-3);
    assert_eq!(bridge.state.scroll_line, 3);
}

#[test]
fn single_wrapped_line_scroll_keeps_total_contiguous_and_paragraph_cached() {
    let content = "word ".repeat(4000);
    let document = document_for_text(&content);
    let mut state = viewer_state_for(&document);
    let bounds = Rectangle::new(Point::ORIGIN, Size::new(400.0, 400.0));
    let viewport = 384.0;
    let base = base_line_height();

    // 首次渲染生成段落并量测换行行真实高度，滚动几何随之确立。
    update_retained_text_lines_for_size(&mut state, &document, 384.0, viewport);
    let measured_height = line_height_of(&state, 0);
    assert!(measured_height > base, "wrapped line must measure taller");
    assert!((state.cached_content_height - measured_height).abs() < 1e-2);

    let mut bridge = ScrollBridge {
        state: &mut state,
        document: &document,
        text_width: 384.0,
    };
    let steps = ((measured_height - viewport) / 25.0).ceil() as i32;
    let mut previous_total = 0.0;
    for _ in 0..=steps {
        bridge.scroll_by(AbsoluteOffset { x: 0.0, y: 25.0 }, bounds, bounds);
        let total = viewer_scroll_absolute(bridge.state);
        // 总位置单调不减且不越过内容底。
        assert!(total >= previous_total - 0.01, "total regressed: {total}");
        assert!(
            total <= measured_height - viewport + 1.0,
            "total overscrolled: {total}"
        );
        previous_total = total;
        // 超长换行行始终以单个缓存段落呈现，滚动不触发重建清空。
        assert_eq!(bridge.state.retained_text_lines.len(), 1);
    }
    assert!(line_height_of(bridge.state, 0) > base);
}

#[test]
fn chunk_append_preserves_scroll_geometry_and_measured_prefix() {
    use crate::text_preview::{TextPreviewChunk, TEXT_PREVIEW_CHUNK_LINE_LIMIT};

    let content = lines(50);
    let mut document = document_for_text(&content);
    let mut state = viewer_state_for(&document);
    let line_height = base_line_height();
    let viewport = 384.0;

    // 首次 layout 建立查看器与文档的源关联。
    sync_with_document(&mut state, &document, 392.0, 400.0);
    // 量测第 3 行并抬高其行高，再滚动到第 10 行。
    note_measured_line_height(&mut state, 3, line_height * 4.0);
    state.scroll_line = 10;
    let anchor_prefix = state.anchor_prefix;
    let measured_line = line_height_of(&state, 3);

    // 触发分块请求进入 loading 态，随后追加一个 chunk 并让查看器同步。
    let request = document.scroll_by(45, viewport).expect("chunk request");
    assert!(document.append_chunk(
        TextPreviewChunk {
            start_offset: request.start_offset,
            content: (0..TEXT_PREVIEW_CHUNK_LINE_LIMIT)
                .map(|line| format!("extra {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            line_count: TEXT_PREVIEW_CHUNK_LINE_LIMIT,
            next_offset: None,
            line_limit_notice: None,
        },
        viewport,
    ));
    sync_with_document(&mut state, &document, 392.0, 400.0);

    // 行高表前缀保留、滚动锚点不动、内容高度只增出新行的估算值。
    assert_eq!(state.line_heights.len(), 50 + TEXT_PREVIEW_CHUNK_LINE_LIMIT);
    assert_eq!(line_height_of(&state, 3), measured_line);
    assert_eq!(state.scroll_line, 10);
    assert!((state.anchor_prefix - anchor_prefix).abs() < 1e-3);
    let expected_growth = TEXT_PREVIEW_CHUNK_LINE_LIMIT as f32 * line_height;
    let base_content = 50.0 * line_height + line_height * 3.0;
    let expected = base_content + expected_growth;
    assert!((state.cached_content_height - expected).abs() < 1e-2);
}

#[test]
fn word_range_covers_same_class_run() {
    // "foo bar-baz  qux"：0-2 foo，3 空格，4-10 bar-baz，11-12 空格，13-15 qux
    let text = "foo bar-baz  qux";
    // 词中双击选整个词。
    assert_eq!(selection::word_range(text, 5), (4, 7));
    // 标点单字符成段。
    assert_eq!(selection::word_range(text, 7), (7, 8));
    // 空白连续段。
    assert_eq!(selection::word_range(text, 11), (11, 13));
}
#[test]
fn selection_text_joins_lines_with_newlines() {
    let document = document_for_text("alpha\nbeta\ngamma");
    let anchor = TextPos { line: 0, column: 2 };
    let cursor = TextPos { line: 2, column: 3 };

    let text = selection::selection_text(&document, anchor, cursor).expect("selection");
    assert_eq!(text, "pha\nbeta\ngam");

    // 端点顺序无关。
    let reversed = selection::selection_text(&document, cursor, anchor).expect("selection");
    assert_eq!(reversed, text);
}

#[test]
fn move_position_steps_across_lines_and_clamps() {
    let document = document_for_text("ab\ncdef");

    let start = TextPos { line: 0, column: 0 };
    let visible = 30;
    // Left 在文档开头原地不动。
    assert_eq!(
        selection::move_position(&document, start, text_editor::Motion::Left, visible),
        start
    );
    // Right 跨行：行尾 → 下一行行首。
    let end_of_first = TextPos { line: 0, column: 2 };
    assert_eq!(
        selection::move_position(&document, end_of_first, text_editor::Motion::Right, visible),
        TextPos { line: 1, column: 0 }
    );
    // Down 保持列并钳制到目标行长。
    assert_eq!(
        selection::move_position(&document, end_of_first, text_editor::Motion::Down, visible),
        TextPos { line: 1, column: 2 }
    );
    // DocumentEnd 落在末行末尾。
    assert_eq!(
        selection::move_position(&document, start, text_editor::Motion::DocumentEnd, visible),
        TextPos { line: 1, column: 4 }
    );
}

#[test]
fn sync_with_document_resets_only_on_source_change() {
    let content = lines(10);
    let document = document_for_text(&content);
    let mut state = TextPreviewViewerState::default();

    // 首次同步：完整初始化。
    sync_with_document(&mut state, &document, 392.0, 400.0);
    assert_eq!(state.line_heights.len(), 10);
    assert_eq!(state.content_revision, Some(document.content_revision()));

    // 同一内容重复同步为 no-op。
    let snapshot_cached = state.cached_content_height;
    sync_with_document(&mut state, &document, 392.0, 400.0);
    assert!((state.cached_content_height - snapshot_cached).abs() < 1e-6);
}

#[test]
fn hit_test_maps_click_to_line_and_column() {
    let document = document_for_text("hello\nworld");
    let mut state = TextPreviewViewerState::default();
    sync_with_document(&mut state, &document, 392.0, 400.0);
    assert_eq!(state.retained_text_lines.len(), 2);

    // 第二行中部的命中落在 line 1。
    let line_height = base_line_height();
    let hit = selection::hit_test(&state, &document, Point::new(60.0, line_height * 1.5))
        .expect("hit within text");
    assert_eq!(hit.line, 1);

    // 行右侧空白命中行尾。
    let end_hit = selection::hit_test(&state, &document, Point::new(1_000.0, line_height * 0.5))
        .expect("hit past right edge");
    assert_eq!(end_hit.line, 0);
    assert_eq!(end_hit.column, "hello".len());
}

#[test]
fn drag_threshold_rejects_click_micro_movements() {
    let press = Point::new(120.0, 65.0);
    let threshold = TEXT_PREVIEW_DRAG_THRESHOLD_PX;

    // 阈值内的微动（点击抖动）不激活拖动选择。
    for delta in [(2.0, 0.0), (0.0, -2.0), (1.5, 1.5), (-2.9, 0.0)] {
        let moved = Point::new(press.x + delta.0, press.y + delta.1);
        let dx = moved.x - press.x;
        let dy = moved.y - press.y;
        assert!(
            dx * dx + dy * dy < threshold * threshold,
            "movement {delta:?} must stay below threshold"
        );
    }

    // 超过阈值才算拖动选择。
    let moved = Point::new(press.x + 4.0, press.y);
    let dx = moved.x - press.x;
    let dy = moved.y - press.y;
    assert!(dx * dx + dy * dy >= threshold * threshold);
}

#[test]
fn hit_test_survives_missing_retained_paragraph() {
    let document = document_for_text(&lines(50));
    let mut state = TextPreviewViewerState::default();
    sync_with_document(&mut state, &document, 392.0, 400.0);

    // 模拟行高量测回填后可见集合先行变化：第二行的段落缓存缺失。
    state
        .retained_text_lines
        .retain(|line| line.line_index != 1);

    let line_height = base_line_height();
    let hit = selection::hit_test(&state, &document, Point::new(80.0, line_height * 1.5))
        .expect("click must resolve even without retained paragraph");

    // 行级退化定位：行内命中、列回落行首，选区清除语义不受影响。
    assert_eq!(hit.line, 1);
    assert_eq!(hit.column, 0);
}

#[test]
fn single_click_never_leaves_selection_even_after_wrap_measure_growth() {
    // 复现用户场景：wrap 长行 + 拖动留下跨行选区 + 单击取消。
    // 行高量测回填会改变可见行集合，历史上此时点击定位失败、
    // 选区清除被跳过，表现为"点一下还选着一堆内容"。
    let content = "testtest".repeat(120); // 单逻辑行 wrap 成多个可视行
    let document = document_for_text(&content);
    let mut state = TextPreviewViewerState::default();
    sync_with_document(&mut state, &document, 392.0, 400.0);

    let line_height = base_line_height();
    let start = Point::new(30.0, line_height * 0.5);

    // 拖动形成选区：按下 → 移过阈值 → 松手。
    selection::press(&mut state, &document, start);
    assert!(selection::drag(
        &mut state,
        &document,
        Point::new(start.x + 200.0, start.y + line_height * 2.0)
    ));
    selection::release(&mut state);
    assert_ne!(state.cursor, state.anchor, "drag should select");
    assert!(!selection::selection_quads(&state, &document).is_empty());

    // 行高量测回填使可见集合变化（点第二下时段落缓存缺失）。
    note_measured_line_height(&mut state, 0, line_height * 3.0);
    state.retained_text_lines.retain(|line| line.height != 0.0);

    // 单击取消：无论定位精度如何，选区必须被清除。
    let click_pos = Point::new(90.0, line_height * 2.5);
    selection::press(&mut state, &document, click_pos);
    selection::release(&mut state);

    assert_eq!(
        state.cursor, state.anchor,
        "single click must collapse selection"
    );
    assert!(selection::selection_quads(&state, &document).is_empty());
}

#[test]
fn press_then_release_without_motion_keeps_caret() {
    let document = document_for_text(&lines(30));
    let mut state = TextPreviewViewerState::default();
    sync_with_document(&mut state, &document, 392.0, 400.0);

    selection::press(&mut state, &document, Point::new(60.0, 10.0));
    // 阈值内的微动不产生选区。
    assert!(!selection::drag(
        &mut state,
        &document,
        Point::new(61.5, 10.5)
    ));
    assert!(!selection::drag(
        &mut state,
        &document,
        Point::new(62.0, 11.0)
    ));
    selection::release(&mut state);

    assert_eq!(state.cursor, state.anchor);
    assert!(selection::selection_quads(&state, &document).is_empty());
}
