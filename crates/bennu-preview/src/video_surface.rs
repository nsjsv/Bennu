//! 视频画面部件。显卡渲染（wgpu）时把 ffmpeg 解出的 YUV 原样传上显卡，颜色换算和缩放在
//! 着色器里做：CPU 不逐像素转 RGB，纹理按尺寸复用，不再每帧新建一张图片。软件渲染
//! （wgpu 不可用退到 tiny-skia）时自定义图元画不出来，改由 CPU 用同一套公式转成图片再画。

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use iced::advanced::image::{self as advanced_image, Renderer as _};
use iced::advanced::widget::Tree;
use iced::advanced::{layout, mouse, renderer, Layout, Widget};
use iced::widget::image;
use iced::widget::shader::{self, Viewport};
use iced::{wgpu, Element, Length, Rectangle, Size, Theme};
use iced_renderer::fallback::Renderer as FallbackRenderer;
use iced_renderer::wgpu::primitive::Renderer as _;

use crate::video_preview::VideoColor;

mod color;

use color::ColorParams;
pub(crate) use color::SampleDepth;

/// 缩小显示时每轴最多取几点：右侧面板放 1080p 也够用（约 5 倍缩小）。
const MAX_TAPS_PER_AXIS: f32 = 4.0;
/// 多少次渲染没用到的纹理就放掉（换内容、关窗口之后）。
const IDLE_TRIMS_BEFORE_RELEASE: u32 = 120;

static NEXT_PICTURE_ID: AtomicU64 = AtomicU64::new(1);

/// 一帧画面：ffmpeg nv12 / p010le 的原样字节，亮度层在前、色度交错层在后。宽高是偶数。
pub struct VideoPicture {
    id: u64,
    width: u32,
    height: u32,
    depth: SampleDepth,
    color: VideoColor,
    bytes: Vec<u8>,
    /// 软件渲染时才用的 RGBA 图，第一次画时转。
    software: OnceLock<image::Handle>,
}

impl VideoPicture {
    pub(crate) fn new(
        width: u32,
        height: u32,
        depth: SampleDepth,
        color: VideoColor,
        bytes: Vec<u8>,
    ) -> Self {
        debug_assert_eq!(bytes.len(), picture_bytes(width, height, depth));
        Self {
            id: NEXT_PICTURE_ID.fetch_add(1, Ordering::Relaxed),
            width,
            height,
            depth,
            color,
            bytes,
            software: OnceLock::new(),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    #[cfg(test)]
    pub(crate) fn same_pixels(&self, other: &VideoPicture) -> bool {
        self.bytes == other.bytes
    }

    fn luma_bytes(&self) -> &[u8] {
        &self.bytes[..self.width as usize * self.height as usize * self.depth.bytes()]
    }

    fn chroma_bytes(&self) -> &[u8] {
        &self.bytes[self.width as usize * self.height as usize * self.depth.bytes()..]
    }

    fn software_image(&self) -> &image::Handle {
        self.software
            .get_or_init(|| image::Handle::from_rgba(self.width, self.height, software_rgba(self)))
    }
}

impl fmt::Debug for VideoPicture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VideoPicture")
            .field("id", &self.id)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("depth", &self.depth)
            .finish()
    }
}

/// 一帧 4:2:0 画面的字节数。
pub(crate) fn picture_bytes(width: u32, height: u32, depth: SampleDepth) -> usize {
    width as usize * height as usize * 3 / 2 * depth.bytes()
}

/// 画面按比例放进可用区域、居中；四周交给外层的黑底。
pub(crate) fn video_surface<'a, Message: 'a>(picture: Arc<VideoPicture>) -> Element<'a, Message> {
    Element::new(VideoSurface { picture })
}

struct VideoSurface {
    picture: Arc<VideoPicture>,
}

impl<Message> Widget<Message, Theme, iced::Renderer> for VideoSurface {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let picture = &self.picture;
        let scale =
            (bounds.width / picture.width as f32).min(bounds.height / picture.height as f32);
        if !(scale.is_finite() && scale > 0.0) {
            return;
        }
        let size = Size::new(picture.width as f32 * scale, picture.height as f32 * scale);
        let fitted = Rectangle::new(
            bounds.center() - iced::Vector::new(size.width / 2.0, size.height / 2.0),
            size,
        );
        match renderer {
            FallbackRenderer::Primary(gpu) => gpu.draw_primitive(
                fitted,
                VideoPrimitive {
                    picture: Arc::clone(picture),
                },
            ),
            FallbackRenderer::Secondary(software) => software.draw_image(
                advanced_image::Image::new(picture.software_image().clone())
                    .filter_method(advanced_image::FilterMethod::Linear),
                fitted,
                bounds,
            ),
        }
    }
}

/// 软件渲染兜底：逐像素按 `ColorParams::display_rgb` 转；色度一块 2×2 共用（不插值）。
fn software_rgba(picture: &VideoPicture) -> Vec<u8> {
    let params = ColorParams::new(picture.color, picture.depth);
    let width = picture.width as usize;
    let height = picture.height as usize;
    let sample = |bytes: &[u8], index: usize| -> f32 {
        match picture.depth {
            SampleDepth::Eight => f32::from(bytes[index]),
            SampleDepth::Sixteen => {
                f32::from(u16::from_le_bytes([bytes[index * 2], bytes[index * 2 + 1]]))
            }
        }
    };
    let luma = picture.luma_bytes();
    let chroma = picture.chroma_bytes();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        for column in 0..width {
            let chroma_index = (row / 2) * width + (column / 2) * 2;
            let rgb = params.display_rgb(
                sample(luma, row * width + column),
                sample(chroma, chroma_index),
                sample(chroma, chroma_index + 1),
            );
            rgba.extend(rgb.map(|channel| (channel * 255.0).round() as u8));
            rgba.push(u8::MAX);
        }
    }
    rgba
}

#[derive(Debug)]
struct VideoPrimitive {
    picture: Arc<VideoPicture>,
}

impl shader::Primitive for VideoPrimitive {
    type Pipeline = VideoPipeline;

    fn prepare(
        &self,
        pipeline: &mut VideoPipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        pipeline.prepare(device, queue, &self.picture, bounds, viewport);
    }

    fn draw(&self, pipeline: &VideoPipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        pipeline.draw(self.picture.id, render_pass);
        true
    }
}

/// 所有视频画面共用的着色器管线和按尺寸复用的纹理。
struct VideoPipeline {
    render: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    target_is_srgb: bool,
    slots: Vec<TextureSlot>,
}

/// 一组纹理：装着 `picture_id` 那一帧；同尺寸的新帧直接覆盖上传。
struct TextureSlot {
    picture_id: u64,
    width: u32,
    height: u32,
    depth: SampleDepth,
    luma: wgpu::Texture,
    chroma: wgpu::Texture,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    used_since_trim: bool,
    idle_trims: u32,
}

impl shader::Pipeline for VideoPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bennu_preview.video.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("video_surface/video.wgsl").into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bennu_preview.video.layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_BYTES as u64),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bennu_preview.video.pipeline_layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let render = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bennu_preview.video.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        Self {
            render,
            layout,
            target_is_srgb: format.is_srgb(),
            slots: Vec::new(),
        }
    }

    fn trim(&mut self) {
        for slot in &mut self.slots {
            slot.idle_trims = if slot.used_since_trim {
                0
            } else {
                slot.idle_trims + 1
            };
            slot.used_since_trim = false;
        }
        self.slots
            .retain(|slot| slot.idle_trims < IDLE_TRIMS_BEFORE_RELEASE);
    }
}

/// 着色器 `Params` 是 6 个 vec4<f32>。
const UNIFORM_BYTES: usize = 6 * 4 * 4;

impl VideoPipeline {
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        picture: &VideoPicture,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let index = match self
            .slots
            .iter()
            .position(|slot| slot.picture_id == picture.id)
        {
            Some(index) => index,
            None => {
                let index = self
                    .slots
                    .iter()
                    .position(|slot| {
                        !slot.used_since_trim
                            && (slot.width, slot.height, slot.depth)
                                == (picture.width, picture.height, picture.depth)
                    })
                    .unwrap_or_else(|| {
                        self.slots.push(self.new_slot(device, picture));
                        self.slots.len() - 1
                    });
                upload(queue, &self.slots[index], picture);
                self.slots[index].picture_id = picture.id;
                index
            }
        };
        let scale = viewport.scale_factor();
        let uniforms = uniform_bytes(
            picture,
            Size::new(bounds.width * scale, bounds.height * scale),
            self.target_is_srgb,
        );
        let slot = &mut self.slots[index];
        queue.write_buffer(&slot.uniforms, 0, &uniforms);
        slot.used_since_trim = true;
    }

    fn draw(&self, picture_id: u64, render_pass: &mut wgpu::RenderPass<'_>) {
        let Some(slot) = self.slots.iter().find(|slot| slot.picture_id == picture_id) else {
            return;
        };
        render_pass.set_pipeline(&self.render);
        render_pass.set_bind_group(0, &slot.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }

    fn new_slot(&self, device: &wgpu::Device, picture: &VideoPicture) -> TextureSlot {
        let (luma_format, chroma_format) = match picture.depth {
            SampleDepth::Eight => (wgpu::TextureFormat::R8Uint, wgpu::TextureFormat::Rg8Uint),
            SampleDepth::Sixteen => (wgpu::TextureFormat::R16Uint, wgpu::TextureFormat::Rg16Uint),
        };
        let texture = |label, width, height, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let luma = texture(
            "bennu_preview.video.luma",
            picture.width,
            picture.height,
            luma_format,
        );
        let chroma = texture(
            "bennu_preview.video.chroma",
            picture.width / 2,
            picture.height / 2,
            chroma_format,
        );
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bennu_preview.video.uniforms"),
            size: UNIFORM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bennu_preview.video.bind_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &luma.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &chroma.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniforms.as_entire_binding(),
                },
            ],
        });
        TextureSlot {
            picture_id: 0,
            width: picture.width,
            height: picture.height,
            depth: picture.depth,
            luma,
            chroma,
            uniforms,
            bind_group,
            used_since_trim: false,
            idle_trims: 0,
        }
    }
}

fn upload(queue: &wgpu::Queue, slot: &TextureSlot, picture: &VideoPicture) {
    let row_bytes = picture.width * picture.depth.bytes() as u32;
    let write = |texture: &wgpu::Texture, bytes: &[u8], width, height| {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            // 色度层每行是一半宽度的两个分量交错，行字节数和亮度层一样。
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    };
    write(
        &slot.luma,
        picture.luma_bytes(),
        picture.width,
        picture.height,
    );
    write(
        &slot.chroma,
        picture.chroma_bytes(),
        picture.width / 2,
        picture.height / 2,
    );
}

/// 按 `video.wgsl` 的 `Params` 排好的字节。`output` 是画面在屏幕上的物理像素大小。
fn uniform_bytes(picture: &VideoPicture, output: Size, target_is_srgb: bool) -> Vec<u8> {
    let color = ColorParams::new(picture.color, picture.depth);
    let footprint = [
        picture.width as f32 / output.width.max(1.0),
        picture.height as f32 / output.height.max(1.0),
    ];
    let taps = footprint.map(|ratio| ratio.ceil().clamp(1.0, MAX_TAPS_PER_AXIS));
    let mut modes = color.modes;
    modes[3] = if target_is_srgb { 1.0 } else { 0.0 };
    let values: [[f32; 4]; 6] = [
        color.rows[0],
        color.rows[1],
        color.rows[2],
        modes,
        [
            picture.width as f32,
            picture.height as f32,
            taps[0],
            taps[1],
        ],
        [footprint[0], footprint[1], 0.0, 0.0],
    ];
    values
        .iter()
        .flatten()
        .flat_map(|value| value.to_ne_bytes())
        .collect()
}

#[cfg(test)]
#[path = "video_surface/tests.rs"]
mod tests;
