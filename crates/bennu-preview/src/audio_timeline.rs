//! 可点击跳转的进度图：波形或声谱图。按下、拖动发 `AudioPreviewSeekRequested`，松开发
//! `AudioPreviewSeekCommitted`，和滑杆是同一套消息，跳转的节流（松手才起新流）不用另写。

use std::sync::Arc;

use iced::advanced::image::{self as advanced_image, Renderer as _};
use iced::advanced::widget::{self, tree};
use iced::advanced::{layout, mouse, renderer, Clipboard, Layout, Renderer as _, Shell, Widget};
use iced::widget::image;
use iced::{Border, Color, Element, Event, Length, Point, Rectangle, Size};

use bennu_theme::ui_colors;

use crate::audio_preview::AudioWaveform;
use crate::preview_message::PreviewMessage;

const BAR_WIDTH: f32 = 2.0;
const BAR_PITCH: f32 = 3.0;
const MIN_BAR_HEIGHT: f32 = 2.0;
const PLAYHEAD_WIDTH: f32 = 2.0;
/// 声谱图未播部分盖一层多深的黑。
const UNPLAYED_SPECTROGRAM_DIM: f32 = 0.45;

pub(crate) enum TimelinePicture {
    Waveform(Arc<AudioWaveform>),
    Spectrogram(image::Handle),
}

pub(crate) fn audio_timeline<'a, Message>(
    picture: TimelinePicture,
    position_seconds: f32,
    duration_seconds: f32,
    height: f32,
) -> Element<'a, Message>
where
    Message: 'a + Clone + From<PreviewMessage>,
{
    Element::new(AudioTimeline {
        picture,
        played: if duration_seconds > 0.0 {
            (position_seconds / duration_seconds).clamp(0.0, 1.0)
        } else {
            0.0
        },
        duration_seconds,
        height,
    })
}

struct AudioTimeline {
    picture: TimelinePicture,
    /// 已播比例 0.0–1.0。
    played: f32,
    duration_seconds: f32,
    height: f32,
}

#[derive(Default)]
struct TimelineState {
    dragging: bool,
}

impl AudioTimeline {
    fn seconds_at(&self, bounds: Rectangle, point: Point) -> f32 {
        let fraction = ((point.x - bounds.x) / bounds.width.max(1.0)).clamp(0.0, 1.0);
        fraction * self.duration_seconds
    }
}

/// 每根竖条代表的那一段里最响的峰值；竖条比峰值多时（短文件）就近取。
pub(crate) fn bar_levels(peaks: &[f32], bars: usize) -> Vec<f32> {
    if peaks.is_empty() || bars == 0 {
        return vec![0.0; bars];
    }
    (0..bars)
        .map(|bar| {
            let start = bar * peaks.len() / bars;
            let end = ((bar + 1) * peaks.len() / bars)
                .max(start + 1)
                .min(peaks.len());
            peaks[start..end].iter().copied().fold(0.0, f32::max)
        })
        .collect()
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for AudioTimeline
where
    Message: Clone + From<PreviewMessage>,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<TimelineState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(TimelineState::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height))
    }

    fn layout(
        &mut self,
        _tree: &mut widget::Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, Length::Fixed(self.height), Size::ZERO))
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<TimelineState>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(point) = cursor.position_over(bounds) {
                    state.dragging = true;
                    shell.publish(Message::from(PreviewMessage::AudioPreviewSeekRequested(
                        self.seconds_at(bounds, point),
                    )));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.dragging => {
                shell.publish(Message::from(PreviewMessage::AudioPreviewSeekRequested(
                    self.seconds_at(bounds, *position),
                )));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.dragging => {
                state.dragging = false;
                shell.publish(Message::from(PreviewMessage::AudioPreviewSeekCommitted));
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let dragging = tree.state.downcast_ref::<TimelineState>().dragging;
        if dragging || cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let colors = ui_colors(theme);
        let played_x = bounds.x + bounds.width * self.played;
        match &self.picture {
            TimelinePicture::Waveform(waveform) => {
                let bars = (bounds.width / BAR_PITCH).floor().max(0.0) as usize;
                for (index, level) in bar_levels(&waveform.peaks, bars).into_iter().enumerate() {
                    let x = bounds.x + index as f32 * BAR_PITCH;
                    let height = (level * bounds.height).max(MIN_BAR_HEIGHT);
                    let color = if x < played_x {
                        colors.primary
                    } else {
                        colors.outline_variant
                    };
                    fill(
                        renderer,
                        Rectangle {
                            x,
                            y: bounds.center_y() - height / 2.0,
                            width: BAR_WIDTH,
                            height,
                        },
                        color,
                    );
                }
            }
            TimelinePicture::Spectrogram(handle) => {
                renderer.draw_image(
                    advanced_image::Image::new(handle.clone())
                        .filter_method(advanced_image::FilterMethod::Linear),
                    bounds,
                    bounds,
                );
                // 渲染器在同一图层里先画所有矩形、后画图片，同层画的矩形会被图片盖住；
                // 进度要另起一层才在图片上面。
                renderer.with_layer(bounds, |renderer| {
                    // 还没播的部分压暗，和波形的已播/未播两种颜色一样一眼看出播到哪。
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: Rectangle {
                                x: played_x,
                                width: bounds.x + bounds.width - played_x,
                                ..bounds
                            },
                            ..renderer::Quad::default()
                        },
                        Color::from_rgba(0.0, 0.0, 0.0, UNPLAYED_SPECTROGRAM_DIM),
                    );
                    fill(
                        renderer,
                        Rectangle {
                            x: played_x - PLAYHEAD_WIDTH / 2.0,
                            y: bounds.y,
                            width: PLAYHEAD_WIDTH,
                            height: bounds.height,
                        },
                        colors.primary,
                    );
                });
            }
        }
    }
}

fn fill(renderer: &mut iced::Renderer, bounds: Rectangle, color: Color) {
    renderer.fill_quad(
        renderer::Quad {
            bounds,
            border: Border::default().rounded(BAR_WIDTH / 2.0),
            ..renderer::Quad::default()
        },
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_take_the_loudest_peak_of_their_slice() {
        let peaks = [0.1, 0.9, 0.2, 0.3, 0.4, 0.5];
        assert_eq!(bar_levels(&peaks, 3), vec![0.9, 0.3, 0.5]);
        // 竖条比峰值多：每根取最近的一个，不越界。
        assert_eq!(bar_levels(&[0.5, 1.0], 4), vec![0.5, 0.5, 1.0, 1.0]);
        assert_eq!(bar_levels(&[], 2), vec![0.0, 0.0]);
    }
}
