//! 音频预览面板。够宽时左右分栏（左封面 + 歌曲信息，歌词在右），不够宽时上下排；
//! 底部整宽是进度区、时间和控件。外面包一层媒体键盘壳（回车、方向键）。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bennu_localization::tr;
use iced::alignment::Horizontal;
use iced::widget::{button, column, container, image, row, stack, Column, Space};
use iced::{Alignment, Color, Element, Length};

use bennu_theme::controls::{
    icon_button_style, normal_button, segmented_choice_row, slider, SegmentedChoice,
};
use bennu_theme::icons::{themed_icon, IconSymbol, IconTone};
use bennu_theme::styles::{muted_text_color, transparent_button_style};
use bennu_theme::typography::{BOLD, CAPTION_SIZE, TITLE_SIZE};

use crate::audio_lyrics_view::{lyrics_view, LYRIC_ROW_HEIGHT};
use crate::audio_preview::{AudioLyrics, AudioStreamFacts};
use crate::audio_timeline::{audio_timeline, TimelinePicture};
use crate::audio_track_text::{fact_line, stream_facts, track_subtitle, track_title};
use crate::formatting::{format_duration, format_file_size};
use crate::media_controls::{AUDIO_PROGRESS_SLIDER_STEP_SECONDS, AUDIO_VOLUME_SLIDER_STEP};
use crate::media_keys::media_keys;
use crate::panel_text::readable_text;
use crate::preview::{
    AudioAnalysis, AudioAnalysisState, AudioPreviewDetails, AudioPreviewPlayback,
    AudioPreviewPlaybackStatus, AudioTimelineView, PreviewSize,
};
use crate::preview_config::{AudioEndAction, MediaPlaybackSettings};
use crate::preview_message::PreviewMessage;

const PANEL_PADDING: f32 = 20.0;
/// 够放下「封面 + 信息」一行的宽度；再窄就上下排。
const SIDE_BY_SIDE_MIN_WIDTH: f32 = 560.0;
const COVER_MIN_EDGE: f32 = 96.0;
const COVER_MAX_EDGE: f32 = 280.0;
/// 进度区上方那一行（波形/声谱图切换、生成波形按钮、计算中提示）。
const TIMELINE_HEADER_HEIGHT: f32 = 28.0;
/// 波形、声谱图、还没算好时的普通滑杆共用一个高度：切换视图、算完替换时布局都不跳。
const TIMELINE_PICTURE_HEIGHT: f32 = 88.0;
/// 时间一行 + 控件行 + 间距。
const TIMES_AND_CONTROLS_HEIGHT: f32 = 68.0;
/// 进度区整块的高度，不随视图和计算状态变，用来算封面和歌词还剩多少地方。
const TRANSPORT_HEIGHT: f32 =
    TIMELINE_HEADER_HEIGHT + TIMELINE_PICTURE_HEIGHT + TIMES_AND_CONTROLS_HEIGHT;
const VIEW_SWITCH_WIDTH: f32 = 168.0;
/// 上下排时标题、歌手专辑、技术参数三行大约占的高度。
const STACKED_INFO_HEIGHT: f32 = 76.0;
const SECTION_GAP: f32 = 16.0;
/// 上下排时给歌词至少留的高度（3 句）。
const STACKED_LYRICS_MIN_HEIGHT: f32 = 3.0 * LYRIC_ROW_HEIGHT;
const PLAY_BUTTON_EDGE: f32 = 40.0;
const PLAY_ICON_EDGE: f32 = 18.0;
const VOLUME_ICON_EDGE: f32 = 16.0;
const VOLUME_SLIDER_WIDTH: f32 = 96.0;

/// 面板要显示的东西，避免一长串参数。
pub(crate) struct AudioPanelInput<'a> {
    pub path: &'a Path,
    pub details: &'a AudioPreviewDetails,
    pub cover: Option<&'a image::Handle>,
    pub playback: Option<&'a AudioPreviewPlayback>,
    pub analysis: Option<&'a AudioAnalysis>,
    pub lyrics: Option<&'a AudioLyrics>,
    pub media_settings: MediaPlaybackSettings,
    pub size: PreviewSize,
}

pub(crate) fn audio_preview_panel<Message>(input: AudioPanelInput<'_>) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let playback = input
        .playback
        .filter(|playback| playback.path.as_path() == input.path);
    let analysis = input
        .analysis
        .filter(|analysis| analysis.path.as_path() == input.path);
    let transport = transport(TransportInput {
        playback,
        duration: input.details.probe.duration,
        analysis,
        sample_rate: input.details.probe.stream.sample_rate,
        media_settings: input.media_settings,
    });
    let position = playback.map_or(Duration::ZERO, |playback| playback.position);
    let browse = playback
        .map(|playback| playback.lyrics_browse)
        .unwrap_or_default();
    let side_by_side = input.size.width >= SIDE_BY_SIDE_MIN_WIDTH;
    let body: Element<'static, Message> = if side_by_side {
        let top_height = input.size.height - 2.0 * PANEL_PADDING - TRANSPORT_HEIGHT - SECTION_GAP;
        let cover_edge = top_height.min(COVER_MAX_EDGE);
        let mut top = row![].spacing(24).align_y(Alignment::Center);
        if cover_edge >= COVER_MIN_EDGE {
            top = top.push(cover_art(input.cover, cover_edge));
        }
        top = top.push(info_column(&input, playback, Horizontal::Left));
        // 歌词在信息右边，和信息平分剩下的宽度；高度就是上半部分的高度。
        if let Some(lyrics) = input.lyrics.filter(|_| top_height >= LYRIC_ROW_HEIGHT) {
            top = top.push(
                container(lyrics_view(
                    lyrics,
                    position,
                    browse,
                    top_height,
                    Horizontal::Center,
                ))
                .width(Length::Fill),
            );
        }
        column![
            container(top).height(Length::Fill).center_y(Length::Fill),
            transport,
        ]
        .spacing(SECTION_GAP)
        .into()
    } else {
        let fixed_height = 2.0 * PANEL_PADDING + STACKED_INFO_HEIGHT + TRANSPORT_HEIGHT;
        // 有歌词时先给它留几行，再看封面还放不放得下。
        let lyrics_reserve = if input.lyrics.is_some() {
            STACKED_LYRICS_MIN_HEIGHT + SECTION_GAP
        } else {
            0.0
        };
        let cover_edge = (input.size.width - 2.0 * PANEL_PADDING)
            .min(input.size.height - fixed_height - lyrics_reserve - 2.0 * SECTION_GAP)
            .min(COVER_MAX_EDGE);
        let show_cover = cover_edge >= COVER_MIN_EDGE;
        let mut stacked = Column::new()
            .spacing(SECTION_GAP)
            .align_x(Alignment::Center)
            .width(Length::Fill);
        if show_cover {
            stacked = stacked.push(cover_art(input.cover, cover_edge));
        }
        stacked = stacked
            .push(info_column(&input, playback, Horizontal::Center))
            .push(transport);
        let cover_height = if show_cover {
            cover_edge + SECTION_GAP
        } else {
            0.0
        };
        let lyrics_height = input.size.height - fixed_height - cover_height - 2.0 * SECTION_GAP;
        if let Some(lyrics) = input
            .lyrics
            .filter(|_| lyrics_height >= 2.0 * LYRIC_ROW_HEIGHT)
        {
            stacked = stacked.push(lyrics_view(
                lyrics,
                position,
                browse,
                lyrics_height,
                Horizontal::Center,
            ));
        }
        stacked.into()
    };
    media_keys(
        container(body)
            .padding(PANEL_PADDING)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_y(Length::Fill)
            .into(),
    )
}

fn cover_art<Message: 'static>(
    cover: Option<&image::Handle>,
    edge: f32,
) -> Element<'static, Message> {
    let artwork: Element<'static, Message> = match cover {
        Some(handle) => image(handle.clone())
            .content_fit(iced::ContentFit::Cover)
            .width(Length::Fixed(edge))
            .height(Length::Fixed(edge))
            .into(),
        None => themed_icon(IconSymbol::Music, IconTone::Normal, edge * 0.4).into(),
    };
    container(artwork)
        .width(Length::Fixed(edge))
        .height(Length::Fixed(edge))
        .center_x(Length::Fixed(edge))
        .center_y(Length::Fixed(edge))
        .into()
}

fn info_column<Message: 'static>(
    input: &AudioPanelInput<'_>,
    playback: Option<&AudioPreviewPlayback>,
    align: Horizontal,
) -> Element<'static, Message> {
    let tags = &input.details.probe.tags;
    let mut info = Column::new()
        .spacing(6)
        .width(Length::Fill)
        .align_x(align)
        .push(
            readable_text(track_title(input.path, tags))
                .size(TITLE_SIZE)
                .font(BOLD)
                .align_x(align),
        );
    if let Some(subtitle) = track_subtitle(tags) {
        info = info.push(readable_text(subtitle).align_x(align));
    }
    info = info.push(
        readable_text(file_summary(&input.details.probe.stream, input.details.len))
            .style(muted_text)
            .align_x(align),
    );
    if let Some(status) = playback.and_then(status_line) {
        info = info.push(readable_text(status).style(muted_text).align_x(align));
    }
    info.into()
}

fn muted_text(theme: &iced::Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(muted_text_color(theme)),
    }
}

fn file_summary(stream: &AudioStreamFacts, len: u64) -> String {
    let mut facts = stream_facts(stream);
    facts.push(format_file_size(len));
    fact_line(facts)
}

/// 播放/暂停看按钮就知道；只有加载中和出错时多说一句。
fn status_line(playback: &AudioPreviewPlayback) -> Option<String> {
    match playback.status {
        AudioPreviewPlaybackStatus::Loading => Some(tr!("Opening audio output...")),
        AudioPreviewPlaybackStatus::Error => Some(
            playback
                .error
                .as_ref()
                .map(|error| tr!("Audio unavailable: %1", error))
                .unwrap_or_else(|| tr!("Could not start audio preview")),
        ),
        _ => None,
    }
}

struct TransportInput<'a> {
    playback: Option<&'a AudioPreviewPlayback>,
    duration: Option<Duration>,
    analysis: Option<&'a AudioAnalysis>,
    sample_rate: Option<u32>,
    media_settings: MediaPlaybackSettings,
}

fn transport<Message>(input: TransportInput<'_>) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let position = input
        .playback
        .map_or(Duration::ZERO, |playback| playback.position);
    // 时长未知时波形算出来的长度也能用来对齐跳转。
    let duration =
        input
            .duration
            .or_else(|| match input.analysis.map(|analysis| &analysis.waveform) {
                Some(AudioAnalysisState::Ready(waveform)) => Some(waveform.duration),
                _ => None,
            });
    let duration_seconds = duration
        .map(|duration| duration.as_secs_f32())
        .unwrap_or_else(|| position.as_secs_f32() + 1.0)
        .max(1.0);
    let position_seconds = position.as_secs_f32().min(duration_seconds);
    let times = row![
        readable_text(format_duration(position)).style(muted_text),
        Space::new().width(Length::Fill),
        readable_text(duration.map_or_else(|| "--:--".to_owned(), format_duration))
            .style(muted_text),
    ];
    let controls = row![
        // 左右两侧等宽，播放键才在正中。
        container(end_action_button(input.media_settings.audio_end_action))
            .width(Length::Fill)
            .align_x(Horizontal::Left),
        play_button(input.playback),
        container(volume_control(input.media_settings.volume))
            .width(Length::Fill)
            .align_x(Horizontal::Right),
    ]
    .align_y(Alignment::Center);
    column![
        timeline_header(input.analysis),
        timeline_picture(
            input.analysis,
            position_seconds,
            duration_seconds,
            input.sample_rate
        ),
        times,
        controls,
    ]
    .spacing(4)
    .width(Length::Fill)
    .into()
}

/// 左边是这张图现在的情况（要点「生成波形」、正在算、算不出），右边是波形/声谱图切换。
fn timeline_header<Message>(analysis: Option<&AudioAnalysis>) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let Some(analysis) = analysis else {
        return Space::new()
            .height(Length::Fixed(TIMELINE_HEADER_HEIGHT))
            .into();
    };
    let note: Element<'static, Message> = match analysis.view {
        AudioTimelineView::Waveform => match &analysis.waveform {
            AudioAnalysisState::NotRequested => {
                normal_button(readable_text(tr!("Generate Waveform")))
                    .on_press(Message::from(PreviewMessage::AudioWaveformRequested))
                    .into()
            }
            AudioAnalysisState::Computing => readable_text(tr!("Drawing the waveform..."))
                .style(muted_text)
                .into(),
            AudioAnalysisState::Failed(reason) => {
                readable_text(tr!("Waveform unavailable: %1", reason))
                    .style(muted_text)
                    .into()
            }
            AudioAnalysisState::Ready(_) => Space::new().into(),
        },
        AudioTimelineView::Spectrogram => match &analysis.spectrogram {
            AudioAnalysisState::Computing | AudioAnalysisState::NotRequested => {
                readable_text(tr!("Drawing the spectrogram..."))
                    .style(muted_text)
                    .into()
            }
            AudioAnalysisState::Failed(reason) => {
                readable_text(tr!("Spectrogram unavailable: %1", reason))
                    .style(muted_text)
                    .into()
            }
            AudioAnalysisState::Ready(_) => Space::new().into(),
        },
    };
    let choice = |label: String, view: AudioTimelineView| SegmentedChoice {
        label,
        selected: analysis.view == view,
        message: Some(Message::from(PreviewMessage::AudioTimelineViewSelected(
            view,
        ))),
        tooltip: None,
    };
    row![
        container(note).width(Length::Fill),
        container(segmented_choice_row(vec![
            choice(tr!("Waveform"), AudioTimelineView::Waveform),
            choice(tr!("Spectrogram"), AudioTimelineView::Spectrogram),
        ]))
        .width(Length::Fixed(VIEW_SWITCH_WIDTH)),
    ]
    .align_y(Alignment::Center)
    .height(Length::Fixed(TIMELINE_HEADER_HEIGHT))
    .into()
}

/// 波形或声谱图算好了就用它当进度条，没好就用普通滑杆。
fn timeline_picture<Message>(
    analysis: Option<&AudioAnalysis>,
    position_seconds: f32,
    duration_seconds: f32,
    sample_rate: Option<u32>,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let picture: Element<'static, Message> =
        match analysis.map(|analysis| (analysis.view, &analysis.waveform, &analysis.spectrogram)) {
            Some((AudioTimelineView::Waveform, AudioAnalysisState::Ready(waveform), _)) => {
                audio_timeline(
                    TimelinePicture::Waveform(Arc::clone(waveform)),
                    position_seconds,
                    duration_seconds,
                    TIMELINE_PICTURE_HEIGHT,
                )
            }
            // 刻度叠在声谱图里面（右侧），声谱图才能和波形、时间一样占满整宽、两端对齐。
            Some((AudioTimelineView::Spectrogram, _, AudioAnalysisState::Ready(spectrogram))) => {
                stack![
                    audio_timeline(
                        TimelinePicture::Spectrogram(spectrogram.clone()),
                        position_seconds,
                        duration_seconds,
                        TIMELINE_PICTURE_HEIGHT,
                    ),
                    frequency_scale(sample_rate),
                ]
                .into()
            }
            _ => slider(0.0..=duration_seconds, position_seconds, |value: f32| {
                Message::from(PreviewMessage::AudioPreviewSeekRequested(value))
            })
            .on_release(Message::from(PreviewMessage::AudioPreviewSeekCommitted))
            .step(AUDIO_PROGRESS_SLIDER_STEP_SECONDS)
            .width(Length::Fill)
            .into(),
        };
    container(picture)
        .width(Length::Fill)
        .height(Length::Fixed(TIMELINE_PICTURE_HEIGHT))
        .center_y(Length::Fixed(TIMELINE_PICTURE_HEIGHT))
        .into()
}

/// 声谱图是线性频率，从下往上 0 到奈奎斯特频率；标三处够看出截止线在哪。标签压在图上，
/// 底下垫一块半透明深色，亮色频谱上也读得清。
fn frequency_scale<Message: 'static>(sample_rate: Option<u32>) -> Element<'static, Message> {
    let nyquist = sample_rate.unwrap_or(44_100) / 2;
    let label = |text: String| {
        container(
            readable_text(text)
                .size(CAPTION_SIZE)
                .style(|_: &iced::Theme| iced::widget::text::Style {
                    color: Some(Color::WHITE),
                }),
        )
        .padding([0, 4])
        .style(|_: &iced::Theme| container::Style {
            background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.55).into()),
            border: iced::Border::default().rounded(3),
            ..container::Style::default()
        })
    };
    let kilohertz = |hertz: u32| format!("{} kHz", (hertz + 500) / 1000);
    container(
        column![
            label(kilohertz(nyquist)),
            Space::new().height(Length::Fill),
            label(kilohertz(nyquist / 2)),
            Space::new().height(Length::Fill),
            label("0".to_owned()),
        ]
        .align_x(Horizontal::Right),
    )
    .width(Length::Fill)
    .height(Length::Fixed(TIMELINE_PICTURE_HEIGHT))
    .padding(4)
    .align_x(Horizontal::Right)
    .into()
}

fn play_button<Message>(playback: Option<&AudioPreviewPlayback>) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let status = playback.map(|playback| playback.status);
    let icon = match status {
        Some(AudioPreviewPlaybackStatus::Playing) => IconSymbol::Pause,
        _ => IconSymbol::Play,
    };
    let play = button(themed_icon(icon, IconTone::Normal, PLAY_ICON_EDGE))
        .width(Length::Fixed(PLAY_BUTTON_EDGE))
        .height(Length::Fixed(PLAY_BUTTON_EDGE))
        .padding((PLAY_BUTTON_EDGE - PLAY_ICON_EDGE) / 2.0)
        .style(icon_button_style);
    if status == Some(AudioPreviewPlaybackStatus::Loading) {
        play.into()
    } else {
        play.on_press(Message::from(PreviewMessage::AudioPreviewPlaybackToggled))
            .into()
    }
}

/// 图标 + 小字：三种模式的图标不常见，光看图标认不出。
fn end_action_button<Message>(action: AudioEndAction) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let (icon, label) = match action {
        AudioEndAction::Stop => (IconSymbol::StopAfterTrack, tr!("Stop After Track")),
        AudioEndAction::Repeat => (IconSymbol::Repeat1, tr!("Repeat Track")),
        AudioEndAction::Next => (IconSymbol::SkipForward, tr!("Play Next Track")),
    };
    button(
        row![
            themed_icon(icon, IconTone::Normal, VOLUME_ICON_EDGE),
            readable_text(label).style(muted_text),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([4, 8])
    .style(transparent_button_style())
    .on_press(Message::from(PreviewMessage::AudioEndActionCycled))
    .into()
}

fn volume_control<Message>(media_volume: f32) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    row![
        themed_icon(IconSymbol::Volume2, IconTone::Normal, VOLUME_ICON_EDGE),
        slider(0.0..=1.0, media_volume, |value: f32| {
            Message::from(PreviewMessage::MediaVolumeChanged(value))
        })
        .step(AUDIO_VOLUME_SLIDER_STEP)
        .width(Length::Fixed(VOLUME_SLIDER_WIDTH)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}
