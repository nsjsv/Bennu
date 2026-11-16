//! 预览窗的七个滚动区域（目录树/归档树/分页文档/纯文本/Markdown/SQLite 表列表/
//! SQLite 数据网格）。滚轮惯性在滚动控件里自己推进，滚动条由 `bennu_theme::scrollbar`
//! 按当帧布局常驻绘制，这里只定区域与 widget id。id 用 portal-preview-* 命名空间，
//! 与选择窗的 portal-file-list#/portal-breadcrumb# 前缀互不串窗。

/// 预览窗的滚动区域（主软件 ScrollbarRegion 预览七区的 portal 对应物）。
/// Copy：接线闭包与探针回信按值携带。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PreviewScrollRegion {
    Directory,
    Archive,
    Document,
    Text,
    Markdown,
    SqliteTables,
    SqliteData,
}

/// 滚动容器的 widget Id：widget 操作跨全部窗口执行，portal-preview-*
/// 前缀保证只命中预览窗的滚动容器（选择窗用请求路径命名空间，前缀
/// 不同不会串窗；进程至多一个预览窗，固定 id 即唯一）。
pub(crate) fn preview_scroll_id(region: PreviewScrollRegion) -> iced::widget::Id {
    iced::widget::Id::from(format!(
        "portal-preview-{}",
        match region {
            PreviewScrollRegion::Directory => "directory",
            PreviewScrollRegion::Archive => "archive",
            PreviewScrollRegion::Document => "document",
            PreviewScrollRegion::Text => "text",
            PreviewScrollRegion::Markdown => "markdown",
            PreviewScrollRegion::SqliteTables => "sqlite-tables",
            PreviewScrollRegion::SqliteData => "sqlite-data",
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_scroll_ids_are_namespaced_away_from_picker_ids() {
        // portal spec 红线：iced operate 遍历所有窗口的控件树，预览
        // id 不得与选择窗（请求路径命名空间）或文本查看器固定 id 相撞。
        let picker_list = crate::picker_session::scrollbar::scroll_id(
            "/req/a",
            crate::picker_session::SessionScrollRegion::List,
        );
        let picker_breadcrumb = crate::picker_session::scrollbar::scroll_id(
            "/req/a",
            crate::picker_session::SessionScrollRegion::Breadcrumb,
        );
        for region in [
            PreviewScrollRegion::Directory,
            PreviewScrollRegion::Archive,
            PreviewScrollRegion::Document,
            PreviewScrollRegion::Text,
            PreviewScrollRegion::Markdown,
            PreviewScrollRegion::SqliteTables,
            PreviewScrollRegion::SqliteData,
        ] {
            let id = preview_scroll_id(region);
            assert_ne!(id, picker_list);
            assert_ne!(id, picker_breadcrumb);
            assert_ne!(id, crate::view::address_input_id("/req/a"));
        }
        // 七区域彼此也不相同。
        let all: Vec<_> = [
            PreviewScrollRegion::Directory,
            PreviewScrollRegion::Archive,
            PreviewScrollRegion::Document,
            PreviewScrollRegion::Text,
            PreviewScrollRegion::Markdown,
            PreviewScrollRegion::SqliteTables,
            PreviewScrollRegion::SqliteData,
        ]
        .iter()
        .map(|region| preview_scroll_id(*region))
        .collect();
        for (index, id) in all.iter().enumerate() {
            assert_eq!(
                all.iter().filter(|other| *other == id).count(),
                1,
                "第 {index} 个区域 id 重复"
            );
        }
    }
}
