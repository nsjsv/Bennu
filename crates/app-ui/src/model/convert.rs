//! 转换对话框的状态与规则：每类媒体一块（来源、选项、「更多」是否展开），
//! 由它产出交给引擎的请求。纯数据，不碰异步和界面。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use file_core::{
    AudioChannelSpec, AudioTargetFormat, ConversionEncoderAvailability, ConversionQualityPreset,
    ConversionRequest, ConversionTarget, ImageTargetFormat, MediaEdits, OutputLocation,
    PhotoMetadata, QualitySpec, ResizeSpec, VideoTargetFormat,
};

mod media_options;
pub(crate) use media_options::{
    AudioConvertOptions, AudioEdit, ImageConvertOptions, ImageEdit, MediaOptions,
};
mod video_options;
pub(crate) use video_options::{VideoClip, VideoConvertOptions, VideoEdit, VideoTarget};

pub(crate) const IMAGE_TARGET_FORMATS: [ImageTargetFormat; 9] = [
    ImageTargetFormat::Jpeg,
    ImageTargetFormat::Png,
    ImageTargetFormat::Webp,
    ImageTargetFormat::Avif,
    ImageTargetFormat::Jxl,
    ImageTargetFormat::Gif,
    ImageTargetFormat::Tiff,
    ImageTargetFormat::Bmp,
    ImageTargetFormat::Ico,
];
pub(crate) const VIDEO_TARGET_FORMATS: [VideoTargetFormat; 9] = [
    VideoTargetFormat::Mp4H264,
    VideoTargetFormat::Mp4H265,
    VideoTargetFormat::MkvH265,
    VideoTargetFormat::MkvAv1,
    VideoTargetFormat::WebmVp9,
    VideoTargetFormat::WebmAv1,
    VideoTargetFormat::MovH264,
    VideoTargetFormat::AviMpeg4,
    VideoTargetFormat::Gif,
];
pub(crate) const AUDIO_TARGET_FORMATS: [AudioTargetFormat; 6] = [
    AudioTargetFormat::Mp3,
    AudioTargetFormat::M4a,
    AudioTargetFormat::Opus,
    AudioTargetFormat::Ogg,
    AudioTargetFormat::Flac,
    AudioTargetFormat::Wav,
];
pub(crate) const QUALITY_PRESETS: [ConversionQualityPreset; 3] = [
    ConversionQualityPreset::Low,
    ConversionQualityPreset::Medium,
    ConversionQualityPreset::High,
];
pub(crate) const RESIZE_PERCENT_CHOICES: [u8; 3] = [100, 75, 50];
pub(crate) const FPS_CHOICES: [u32; 3] = [60, 30, 24];
pub(crate) const SIZE_UNITS: [SizeUnit; 3] = [SizeUnit::KiB, SizeUnit::MiB, SizeUnit::GiB];
pub(crate) const CONVERT_SCENES: [ConvertScene; 3] = [
    ConvertScene::MostCompatible,
    ConvertScene::ForWeb,
    ConvertScene::Smallest,
];

const DEFAULT_LONG_EDGE_TEXT: &str = "1920";
/// 「分享网页」把长边压到这么多：网页和聊天软件常见的显示上限。
const WEB_LONG_EDGE: u32 = 1920;
/// 小标题里最多列几种源格式，再多就是噪音。
const SOURCE_FORMAT_LIMIT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConvertibleMediaKind {
    Image,
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConvertSource {
    pub(crate) path: PathBuf,
    pub(crate) kind: ConvertibleMediaKind,
    pub(crate) bytes: u64,
}

/// 质量一行里的按钮：三个档位，或按每个文件的目标大小。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QualityChoice {
    Preset(ConversionQualityPreset),
    TargetSize,
}

/// 目标格式决定质量一行能怎么选。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QualityControl {
    /// 无损格式：没有质量可调，不显示这一行。
    Lossless,
    /// GIF 调色板编码：只有档位，没有码率可按大小反推。
    PresetOnly,
    PresetOrSize,
}

/// 目标大小的单位，跟在数字输入框后面点选，用户不用自己打单位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SizeUnit {
    KiB,
    MiB,
    GiB,
}

impl SizeUnit {
    fn bytes(self) -> f64 {
        match self {
            Self::KiB => 1024.0,
            Self::MiB => 1024.0 * 1024.0,
            Self::GiB => 1024.0 * 1024.0 * 1024.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QualitySelection {
    /// 选「按大小」时也留着：切到只认档位的 GIF 直接用它。
    pub(crate) preset: ConversionQualityPreset,
    pub(crate) by_target_size: bool,
    /// 只有数字（可带小数），单位在 `target_size_unit`。
    pub(crate) target_size_text: String,
    pub(crate) target_size_unit: SizeUnit,
}

impl Default for QualitySelection {
    fn default() -> Self {
        Self {
            preset: ConversionQualityPreset::Medium,
            by_target_size: false,
            // 默认空着：大小因文件而异，预填一个数只会被当成建议值。
            target_size_text: String::new(),
            target_size_unit: SizeUnit::MiB,
        }
    }
}

impl QualitySelection {
    pub(crate) fn choice(&self) -> QualityChoice {
        if self.by_target_size {
            QualityChoice::TargetSize
        } else {
            QualityChoice::Preset(self.preset)
        }
    }

    fn select(&mut self, choice: QualityChoice) {
        match choice {
            QualityChoice::Preset(preset) => {
                self.preset = preset;
                self.by_target_size = false;
            }
            QualityChoice::TargetSize => self.by_target_size = true,
        }
    }

    fn spec(&self, control: QualityControl) -> Result<QualitySpec, ConvertInputError> {
        match control {
            QualityControl::Lossless => Ok(QualitySpec::Lossless),
            QualityControl::PresetOrSize if self.by_target_size => {
                target_size_bytes(&self.target_size_text, self.target_size_unit)
                    .map(QualitySpec::TargetBytes)
            }
            QualityControl::PresetOnly | QualityControl::PresetOrSize => {
                Ok(QualitySpec::Preset(self.preset))
            }
        }
    }
}

fn image_quality_control(format: ImageTargetFormat) -> QualityControl {
    match format {
        ImageTargetFormat::Png
        | ImageTargetFormat::Tiff
        | ImageTargetFormat::Bmp
        | ImageTargetFormat::Ico => QualityControl::Lossless,
        ImageTargetFormat::Gif => QualityControl::PresetOnly,
        ImageTargetFormat::Jpeg
        | ImageTargetFormat::Webp
        | ImageTargetFormat::Avif
        | ImageTargetFormat::Jxl => QualityControl::PresetOrSize,
    }
}

fn video_quality_control(format: VideoTargetFormat) -> QualityControl {
    match format {
        VideoTargetFormat::Gif => QualityControl::PresetOnly,
        _ => QualityControl::PresetOrSize,
    }
}

fn audio_quality_control(format: AudioTargetFormat) -> QualityControl {
    match format {
        AudioTargetFormat::Flac | AudioTargetFormat::Wav => QualityControl::Lossless,
        _ => QualityControl::PresetOrSize,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResizeChoice {
    /// 100 = 原尺寸。
    Percent(u8),
    LongEdge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResizeSelection {
    pub(crate) choice: ResizeChoice,
    pub(crate) long_edge_text: String,
}

impl Default for ResizeSelection {
    fn default() -> Self {
        Self {
            choice: ResizeChoice::Percent(100),
            long_edge_text: DEFAULT_LONG_EDGE_TEXT.to_owned(),
        }
    }
}

impl ResizeSelection {
    fn spec(&self) -> Result<ResizeSpec, ConvertInputError> {
        match self.choice {
            ResizeChoice::Percent(percent) if percent >= 100 => Ok(ResizeSpec::Keep),
            ResizeChoice::Percent(percent) => Ok(ResizeSpec::Percent(percent)),
            ResizeChoice::LongEdge => self
                .long_edge_text
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|long_edge| *long_edge > 0)
                .map(ResizeSpec::LongEdge)
                .ok_or(ConvertInputError::LongEdge),
        }
    }

    fn set_spec(&mut self, spec: ResizeSpec) {
        match spec {
            ResizeSpec::Keep => self.choice = ResizeChoice::Percent(100),
            ResizeSpec::Percent(percent) => self.choice = ResizeChoice::Percent(percent),
            ResizeSpec::LongEdge(long_edge) => {
                self.choice = ResizeChoice::LongEdge;
                self.long_edge_text = long_edge.to_string();
            }
        }
    }
}

/// 常用场景：一键填好各类的格式、质量、缩放。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConvertScene {
    MostCompatible,
    ForWeb,
    Smallest,
}

/// 输入写错或还不能提交的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConvertInputError {
    /// 编码器还没探测完，或目标格式的编码器缺失；顶部的 ffmpeg 提示负责说明。
    MissingEncoder,
    TargetSize,
    LongEdge,
    /// 截取时间写错，或结束不晚于开始。
    Trim,
    OutputDirectory,
}

/// 一类媒体交给引擎的设置；对这一类的每个源文件都一样。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConversionSettings {
    target: ConversionTarget,
    quality: QualitySpec,
    resize: ResizeSpec,
    fps_override: Option<u32>,
    audio_channels: AudioChannelSpec,
    edits: MediaEdits,
    /// 只对图片有意义；视频、音频填 Keep，引擎不看。
    photo_metadata: PhotoMetadata,
}

impl ConversionSettings {
    /// 对话框里这个目标的出厂设置，不看用户上次记住的选项：动作里的转换每次结果都一样。
    pub(crate) fn factory_default(
        target: ConversionTarget,
        encoders: &ConversionEncoderAvailability,
    ) -> Result<Self, ConvertInputError> {
        match target {
            ConversionTarget::Image(target_format) => ImageConvertOptions {
                target_format,
                ..ImageConvertOptions::default()
            }
            .settings(encoders),
            ConversionTarget::Video(format) => VideoConvertOptions {
                target: VideoTarget::Video(format),
                ..VideoConvertOptions::default()
            }
            .settings(encoders),
            // 从视频里取声音和转音频是同一组设置（缩放、帧率、旋转都用不上）。
            ConversionTarget::Audio(target_format) => AudioConvertOptions {
                target_format,
                ..AudioConvertOptions::default()
            }
            .settings(encoders),
        }
    }

    pub(crate) fn request(&self, source: &Path, output: &OutputLocation) -> ConversionRequest {
        ConversionRequest {
            source: source.to_path_buf(),
            target: self.target,
            quality: self.quality,
            resize: self.resize,
            fps_override: self.fps_override,
            audio_channels: self.audio_channels,
            edits: self.edits,
            photo_metadata: self.photo_metadata,
            output: output.clone(),
        }
    }
}

/// 校验通过后的整次转换：每类的源文件配上它的设置。只借用路径，视图每帧算也不复制。
struct ConversionPlan<'a> {
    output: OutputLocation,
    groups: Vec<(&'a [PathBuf], ConversionSettings)>,
}

/// 每类媒体上次提交时的选项。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ConvertDefaults {
    pub(crate) image: ImageConvertOptions,
    pub(crate) video: VideoConvertOptions,
    pub(crate) audio: AudioConvertOptions,
}

/// 一类媒体的源文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceGroup {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) total_bytes: u64,
    /// 源格式（大写扩展名），文件多的在前。
    formats: Vec<String>,
}

impl SourceGroup {
    fn new(sources: Vec<(PathBuf, u64)>) -> Self {
        let mut format_counts: HashMap<String, usize> = HashMap::new();
        for (path, _) in &sources {
            if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
                *format_counts
                    .entry(extension.to_ascii_uppercase())
                    .or_default() += 1;
            }
        }
        let mut formats: Vec<_> = format_counts.into_iter().collect();
        formats.sort_by(|(left, left_count), (right, right_count)| {
            right_count.cmp(left_count).then_with(|| left.cmp(right))
        });
        Self {
            total_bytes: sources.iter().map(|(_, bytes)| bytes).sum(),
            paths: sources.into_iter().map(|(path, _)| path).collect(),
            formats: formats.into_iter().map(|(format, _)| format).collect(),
        }
    }

    /// 最多列 3 种，再多的用“…”表示。
    pub(crate) fn format_summary(&self, separator: &str) -> String {
        let mut summary = self
            .formats
            .iter()
            .take(SOURCE_FORMAT_LIMIT)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(separator);
        if self.formats.len() > SOURCE_FORMAT_LIMIT {
            summary.push('…');
        }
        summary
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MediaSection<O> {
    pub(crate) sources: SourceGroup,
    pub(crate) options: O,
    pub(crate) more_expanded: bool,
}

/// 一块卡片上的改动；质量和「更多」三类都有，其余按类分开。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SectionEdit<E> {
    MoreToggled,
    Quality(QualityChoice),
    TargetSize(String),
    TargetSizeUnit(SizeUnit),
    Options(E),
}

impl<O: MediaOptions> MediaSection<O> {
    fn new(sources: Vec<(PathBuf, u64)>, options: &O) -> Option<Self> {
        (!sources.is_empty()).then(|| Self {
            sources: SourceGroup::new(sources),
            options: options.clone(),
            more_expanded: false,
        })
    }

    fn apply(&mut self, edit: SectionEdit<O::Edit>) {
        match edit {
            SectionEdit::MoreToggled => self.more_expanded = !self.more_expanded,
            SectionEdit::Quality(choice) => self.options.quality_mut().select(choice),
            SectionEdit::TargetSize(text) => self.options.quality_mut().target_size_text = text,
            SectionEdit::TargetSizeUnit(unit) => self.options.quality_mut().target_size_unit = unit,
            SectionEdit::Options(edit) => self.options.apply(edit),
        }
    }

    fn apply_scene(&mut self, scene: ConvertScene, encoders: &ConversionEncoderAvailability) {
        if let Some(options) = self.options.with_scene(scene, encoders) {
            self.options = options;
        }
    }

    fn matches_scene(&self, scene: ConvertScene, encoders: &ConversionEncoderAvailability) -> bool {
        self.options.with_scene(scene, encoders).as_ref() == Some(&self.options)
    }

    fn plan(
        &self,
        encoders: &ConversionEncoderAvailability,
    ) -> Result<(&[PathBuf], ConversionSettings), ConvertInputError> {
        Ok((&self.sources.paths, self.options.settings(encoders)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EncoderProbe {
    Pending,
    Probed(ConversionEncoderAvailability),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConvertEdit {
    Scene(ConvertScene),
    OutputDirectory(String),
    Image(SectionEdit<ImageEdit>),
    Video(SectionEdit<VideoEdit>),
    Audio(SectionEdit<AudioEdit>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConvertState {
    pub(crate) image: Option<MediaSection<ImageConvertOptions>>,
    pub(crate) video: Option<MediaSection<VideoConvertOptions>>,
    pub(crate) audio: Option<MediaSection<AudioConvertOptions>>,
    /// 空 = 放在原文件旁边。
    pub(crate) output_directory_text: String,
    pub(crate) encoders: EncoderProbe,
}

impl ConvertState {
    /// 一个能转的文件都没有时为 None。
    pub(crate) fn new(sources: Vec<ConvertSource>, defaults: &ConvertDefaults) -> Option<Self> {
        let (mut images, mut videos, mut audios) = (Vec::new(), Vec::new(), Vec::new());
        for source in sources {
            let group = match source.kind {
                ConvertibleMediaKind::Image => &mut images,
                ConvertibleMediaKind::Video => &mut videos,
                ConvertibleMediaKind::Audio => &mut audios,
            };
            group.push((source.path, source.bytes));
        }
        let state = Self {
            image: MediaSection::new(images, &defaults.image),
            video: MediaSection::new(videos, &defaults.video),
            audio: MediaSection::new(audios, &defaults.audio),
            output_directory_text: String::new(),
            encoders: EncoderProbe::Pending,
        };
        (state.image.is_some() || state.video.is_some() || state.audio.is_some()).then_some(state)
    }

    pub(crate) fn accept_encoder_probe(&mut self, encoders: ConversionEncoderAvailability) {
        if let Some(section) = &mut self.image {
            section.options.fall_back_to_available(&encoders);
        }
        if let Some(section) = &mut self.video {
            section.options.fall_back_to_available(&encoders);
        }
        if let Some(section) = &mut self.audio {
            section.options.fall_back_to_available(&encoders);
        }
        self.encoders = EncoderProbe::Probed(encoders);
    }

    /// 还没探测完时为 None。
    pub(crate) fn probed_encoders(&self) -> Option<&ConversionEncoderAvailability> {
        match &self.encoders {
            EncoderProbe::Pending => None,
            EncoderProbe::Probed(encoders) => Some(encoders),
        }
    }

    pub(crate) fn source_count(&self) -> usize {
        self.image
            .as_ref()
            .map_or(0, |section| section.sources.paths.len())
            + self
                .video
                .as_ref()
                .map_or(0, |section| section.sources.paths.len())
            + self
                .audio
                .as_ref()
                .map_or(0, |section| section.sources.paths.len())
    }

    pub(crate) fn apply(&mut self, edit: ConvertEdit) {
        match edit {
            ConvertEdit::Scene(scene) => self.apply_scene(scene),
            ConvertEdit::OutputDirectory(text) => self.output_directory_text = text,
            ConvertEdit::Image(edit) => {
                if let Some(section) = &mut self.image {
                    section.apply(edit);
                }
            }
            ConvertEdit::Video(edit) => {
                if let Some(section) = &mut self.video {
                    section.apply(edit);
                }
            }
            ConvertEdit::Audio(edit) => {
                if let Some(section) = &mut self.audio {
                    section.apply(edit);
                }
            }
        }
    }

    /// 场景按钮在探测完之前不可点：候选回退要看编码器。
    fn apply_scene(&mut self, scene: ConvertScene) {
        let EncoderProbe::Probed(encoders) = &self.encoders else {
            return;
        };
        if let Some(section) = &mut self.image {
            section.apply_scene(scene, encoders);
        }
        if let Some(section) = &mut self.video {
            section.apply_scene(scene, encoders);
        }
        if let Some(section) = &mut self.audio {
            section.apply_scene(scene, encoders);
        }
    }

    /// 当前设置正好等于某个场景时，那个按钮亮着。
    pub(crate) fn active_scene(&self) -> Option<ConvertScene> {
        let encoders = self.probed_encoders()?;
        CONVERT_SCENES.into_iter().find(|scene| {
            section_matches(&self.image, *scene, encoders)
                && section_matches(&self.video, *scene, encoders)
                && section_matches(&self.audio, *scene, encoders)
        })
    }

    pub(crate) fn output_location(&self) -> Result<OutputLocation, ConvertInputError> {
        let text = self.output_directory_text.trim();
        if text.is_empty() {
            return Ok(OutputLocation::BesideSource);
        }
        let directory = Path::new(text);
        if !directory.is_absolute() {
            return Err(ConvertInputError::OutputDirectory);
        }
        Ok(OutputLocation::Directory(directory.to_path_buf()))
    }

    fn plan(&self) -> Result<ConversionPlan<'_>, ConvertInputError> {
        let encoders = self
            .probed_encoders()
            .ok_or(ConvertInputError::MissingEncoder)?;
        let output = self.output_location()?;
        let mut groups = Vec::with_capacity(3);
        if let Some(section) = &self.image {
            groups.push(section.plan(encoders)?);
        }
        if let Some(section) = &self.video {
            groups.push(section.plan(encoders)?);
        }
        if let Some(section) = &self.audio {
            groups.push(section.plan(encoders)?);
        }
        Ok(ConversionPlan { output, groups })
    }

    /// 还不能提交的原因；能提交时为 None。
    pub(crate) fn input_error(&self) -> Option<ConvertInputError> {
        self.plan().err()
    }

    pub(crate) fn requests(&self) -> Result<Vec<ConversionRequest>, ConvertInputError> {
        let plan = self.plan()?;
        let mut requests = Vec::with_capacity(self.source_count());
        for (sources, settings) in plan.groups {
            requests.extend(
                sources
                    .iter()
                    .map(|source| settings.request(source, &plan.output)),
            );
        }
        Ok(requests)
    }

    /// 这次出现的类换成这次的选项，其余沿用上次的。
    pub(crate) fn remembered(&self, previous: &ConvertDefaults) -> ConvertDefaults {
        ConvertDefaults {
            image: remembered_options(&self.image, &previous.image),
            video: remembered_options(&self.video, &previous.video),
            audio: remembered_options(&self.audio, &previous.audio),
        }
    }
}

fn section_matches<O: MediaOptions>(
    section: &Option<MediaSection<O>>,
    scene: ConvertScene,
    encoders: &ConversionEncoderAvailability,
) -> bool {
    section
        .as_ref()
        .is_none_or(|section| section.matches_scene(scene, encoders))
}

fn remembered_options<O: MediaOptions>(section: &Option<MediaSection<O>>, previous: &O) -> O {
    section
        .as_ref()
        .map_or_else(|| previous.clone(), |section| section.options.remembered())
}

fn first_available<F: Copy>(current: F, formats: &[F], available: impl Fn(F) -> bool) -> F {
    if available(current) {
        return current;
    }
    formats
        .iter()
        .copied()
        .find(|format| available(*format))
        .unwrap_or(current)
}

/// 数字乘单位；0 和不到 1 字节都没有意义，一并拒绝。
fn target_size_bytes(text: &str, unit: SizeUnit) -> Result<u64, ConvertInputError> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite() && *number > 0.0)
        .map(|number| (number * unit.bytes()).round() as u64)
        .filter(|bytes| *bytes > 0)
        .ok_or(ConvertInputError::TargetSize)
}

#[cfg(test)]
mod tests;
