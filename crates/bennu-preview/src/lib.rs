//! 预览子系统共享 crate：模型/状态机/面板在此单源实现，app-ui 与
//! portal-backend 都是消费方（依赖方向见任务 design.md：宿主 →
//! bennu-preview，禁止反向依赖 app-ui）。本批只下沉模型层，视图层与
//! 渲染器相关的 feature 选择留在宿主。

pub mod image_preview_viewport;
// 纯搬移：预览状态聚合模型（PreviewState/PreviewContent/播放运行时/chrome
// 动画/预览树）自 app-ui 的 model/preview.rs 下沉，作为本 crate 顶层
// preview 模块；app-ui 的 model.rs re-export 维持 crate::model::* 路径。
pub mod preview;
// 缓动单源在 bennu-theme，preview 模块经 crate::animation:: 路径消费。
pub mod animation;
// 字节进度分数随 RemotePreviewDownload 下沉；app-ui 侧 re-export 单源消费。
pub mod operation_progress;
// 纯搬移：rodio 音频预览运行时自 app-ui 下沉；app-ui 经 main.rs 的
// re-export 维持 crate::audio_preview::* 调用路径不变。
pub mod audio_preview;
// 纯搬移：文本预览模型（分块/行索引/Markdown 模式）自 app-ui 下沉；
// 纯 std 实现，无宿主耦合。
pub mod text_preview;
// 纯搬移：文档预览模型自 app-ui 下沉；命令层（外部进程调用）仍留 app-ui。
pub mod document_preview;
// 纯搬移：GIF 动图预览的类型/解码管线自 app-ui 下沉；返回 app-ui
// Message 的订阅与首帧加载命令（含文件大小文案）仍留 app-ui。
pub mod animated_image_preview;
// 纯搬移：视频预览的元数据探测/PPM 帧解码自 app-ui 下沉；返回 app-ui
// Message 的 ffmpeg 流式订阅仍留 app-ui。
pub mod sqlite_preview;
pub mod video_preview;
// 预览域配置（分类型大小上限/后缀规则/展开层级）自 app-ui config.rs
// 下沉；TOML 键解析/写出留 app-ui 存储域，app-ui config.rs re-export。
pub mod preview_config;
// 预览子系统自有消息类型：宿主 Message 体系不同，共享命令层/状态机以
// 它为输出契约（Task::map 包装进宿主）。
pub mod preview_message;
// 容量格式化纯函数自 app-ui formatting.rs 下沉（预览超限文案引用），
// app-ui formatting.rs re-export 维持 35 处调用路径。
pub mod formatting;
// UI 节拍常量自 app-ui 下沉（预览命令层引用 PROGRESS_UI_INTERVAL），
// app-ui ui_pacing.rs re-export 维持非预览消费者路径。
pub mod ui_pacing;
// 空格预览的路径分类与内容加载管线自 app-ui preview.rs 下沉。
pub mod preview_loading;
// 纯搬移：原图预览解码管线（光栅/SVG）自 app-ui 下沉。
pub mod original_image_preview;
// 纯搬移：远程预览缓存（下载/容量预算/LRU 清理）自 app-ui 下沉。
pub mod remote_preview_cache;
// 纯搬移：文本预览分块加载自 app-ui 下沉。
pub mod text_preview_loading;
// 纯搬移：文本预览自绘查看器部件（滚动几何/自管光标选择/行号 gutter）
// 自 app-ui 下沉。渲染器保持泛型（R: text::Renderer<Paragraph=…>）：
// 本 crate 的 iced 声明不带渲染器 feature（禁止把 wgpu/tiny-skia 强加
// 给宿主），宿主（app-ui=wgpu、portal-backend=tiny-skia）实例化时各自的
// Paragraph/Font 关联类型与 iced_graphics 具体段落类型一致，均可用；
// 滚动几何宿主（smooth_scroll/ScrollbarRegion）仍留 app-ui，部件经
// 调用方注入的闭包输出事件（见 spec text-preview-scrolling-guidelines）。
pub mod text_preview_viewer;
// 面板文本构造辅助（readable_text/localized_text 单源，翻译经
// bennu-localization），app-ui typography re-export 维持调用路径。
pub mod panel_text;
// Markdown 预览渲染面板（pulldown-cmark 收集器 + 块视图），滚动接线经
// scroll_wiring 由宿主注入。
pub mod markdown_preview;
// 文档（PDF/Office 分页）预览面板，滚动接线经 scroll_wiring 由宿主注入。
pub mod document_preview_panel;
// SQLite 预览面板（表列表/数据网格/SQL 查询），标签行与滚动接线由宿主注入。
pub mod sqlite_preview_panel;
// 文本/Markdown 预览面板；查看器部件与模式切换行由宿主注入（渲染器
// 泛型与共用词汇边界，见模块注释）。
pub mod text_preview_panel;
// 音视频/动图预览共用的底部控制件词汇（迷你进度条/淡出样式/位置文案）。
pub mod media_controls;
// 预览面板外层 surface（背景容器与滚动区高度下限）。
pub mod preview_surface;
// 静态图片预览面板（缩略图/光栅/SVG）与缩放平移媒体区。
pub mod image_preview_panel;
// 音频预览面板（标题摘要/时间线控制/音量控制）。
pub mod audio_preview_panel;
// 视频预览面板（帧视图/底部控件/音量/迷你进度条）。
pub mod video_preview_panel;
// 动图（GIF）预览面板（双帧叠加/底部 seek 控件）。
pub mod animated_image_preview_panel;
// 目录/归档预览树面板（缩进条目行/展开切换/状态行）。
pub mod preview_tree_panel;
// 预览主面板：按 PreviewState 内容分派到各分类型子面板。
pub mod preview_panel;
// 预览浮动窗口 chrome（悬浮控制层/固定按钮/窗口控制组机件）。
pub mod preview_window_chrome;
// 宿主滚动管线接线参数（smooth-scroll 包装/id/视口回传闭包），
// 视图面板经它消费宿主滚动几何，见 scrollbar-guidelines 分工。
pub mod scroll_wiring;
// 预览命令层（返回 Task<PreviewMessage>）。
pub mod commands;
// PreviewEngine 状态机聚合体（预览字段自 app-ui FileBrowser 下沉的
// 组合宿主），Message::Preview 路由的引擎侧入口。
pub mod engine;
