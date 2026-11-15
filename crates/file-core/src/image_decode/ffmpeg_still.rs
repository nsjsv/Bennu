//! 内置解码器不认的图片（HEIC、JPEG XL、PSD、TGA、EXR……）交给 ffmpeg：先用 ffprobe 看清
//! 要解哪一路（HEIC 的大图是拼起来的，要按拼图组解；同一文件里还有缩略图），再让 ffmpeg
//! 按要的大小直接出 RGBA。ffmpeg 会按文件记的方向自己转正。

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};

use super::{DecodeBound, ImageDecodeError};

/// ffprobe 的输出只取几十行，超过这么多说明不是图片，不再往下读。
const PROBE_OUTPUT_LIMIT: u64 = 1024 * 1024;
const ERROR_DETAIL_LIMIT: usize = 400;

/// ffprobe 看到的要解的那一路：滤镜输入写法和转正后的尺寸。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StillPlan {
    input: String,
    pub(super) width: u32,
    pub(super) height: u32,
}

pub(super) fn probe(path: &Path) -> Result<StillPlan, ImageDecodeError> {
    let mut child = spawn(
        Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_streams",
                "-show_stream_groups",
                "-of",
                "flat",
            ])
            .arg(path),
    )?;
    let mut output = String::new();
    child
        .stdout
        .take()
        .expect("stdout is piped")
        .take(PROBE_OUTPUT_LIMIT)
        .read_to_string(&mut output)?;
    let status = child.wait()?;
    if !status.success() {
        return Err(ImageDecodeError::Ffmpeg(format!(
            "ffprobe exited with {status}"
        )));
    }
    plan_from_probe(&output)
        .ok_or_else(|| ImageDecodeError::Ffmpeg("no picture in this file".to_owned()))
}

/// 拼图组（HEIC 大图）优先，标了默认的优先；没有拼图就取标了默认的、否则最大的那张。
fn plan_from_probe(output: &str) -> Option<StillPlan> {
    let fields: HashMap<&str, &str> = output
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key, value.trim_matches('"')))
        .collect();
    let number = |key: String| {
        fields
            .get(key.as_str())
            .and_then(|value| value.parse::<i64>().ok())
    };

    let mut best_group: Option<(bool, StillPlan)> = None;
    for index in 0.. {
        let prefix = format!("stream_groups.stream_group.{index}");
        let Some(kind) = fields.get(format!("{prefix}.type").as_str()) else {
            break;
        };
        if *kind != "Tile Grid" {
            continue;
        }
        let component = format!("{prefix}.components.component.0");
        let (Some(width), Some(height)) = (
            number(format!("{component}.width")),
            number(format!("{component}.height")),
        ) else {
            continue;
        };
        let is_default = number(format!("{prefix}.disposition.default")) == Some(1);
        let plan = StillPlan {
            input: format!("0:g:{index}"),
            width: u32::try_from(width).ok()?,
            height: u32::try_from(height).ok()?,
        };
        if best_group
            .as_ref()
            .is_none_or(|(best_default, _)| is_default && !best_default)
        {
            best_group = Some((is_default, plan));
        }
    }
    if let Some((_, plan)) = best_group {
        return Some(plan);
    }

    let mut best_stream: Option<(bool, u64, StillPlan)> = None;
    for index in 0.. {
        let prefix = format!("streams.stream.{index}");
        let Some(kind) = fields.get(format!("{prefix}.codec_type").as_str()) else {
            break;
        };
        if *kind != "video" {
            continue;
        }
        let (Some(width), Some(height)) = (
            number(format!("{prefix}.width")),
            number(format!("{prefix}.height")),
        ) else {
            continue;
        };
        let (width, height) = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
        // 一路的附加数据只有寥寥几项，旋转记在其中某项里。
        let quarter_turn = (0..8)
            .filter_map(|side| number(format!("{prefix}.side_data_list.side_data.{side}.rotation")))
            .any(|rotation| rotation.rem_euclid(180) == 90);
        let (width, height) = if quarter_turn {
            (height, width)
        } else {
            (width, height)
        };
        let is_default = number(format!("{prefix}.disposition.default")) == Some(1);
        let area = u64::from(width) * u64::from(height);
        let better = best_stream
            .as_ref()
            .is_none_or(|(best_default, best_area, _)| {
                (is_default, area) > (*best_default, *best_area)
            });
        if better {
            best_stream = Some((
                is_default,
                area,
                StillPlan {
                    input: format!("0:{index}"),
                    width,
                    height,
                },
            ));
        }
    }
    best_stream.map(|(_, _, plan)| plan)
}

/// 按计划解一张，最大不超过 `bound`（只缩不放）。
pub(super) fn decode(
    path: &Path,
    plan: &StillPlan,
    bound: DecodeBound,
) -> Result<image::RgbaImage, ImageDecodeError> {
    let (width, height) = bound.fit(plan.width, plan.height);
    // 和内置解码器同一个内存上限：全尺寸解一张巨幅 EXR 不该把内存吃光。
    let max_bytes = image::Limits::default().max_alloc.unwrap_or(u64::MAX);
    if u64::from(width) * u64::from(height) * 4 > max_bytes {
        return Err(ImageDecodeError::Image(image::ImageError::Limits(
            image::error::LimitError::from_kind(image::error::LimitErrorKind::InsufficientMemory),
        )));
    }
    let filter = if (width, height) == (plan.width, plan.height) {
        format!("[{}]format=rgba", plan.input)
    } else {
        format!(
            "[{}]scale={width}:{height}:flags=area,format=rgba",
            plan.input
        )
    };
    let output = spawn(
        Command::new("ffmpeg")
            .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-filter_complex", &filter])
            .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "pam", "-"])
            .stderr(Stdio::piped()),
    )?
    .wait_with_output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail: String = detail.trim().chars().take(ERROR_DETAIL_LIMIT).collect();
        return Err(ImageDecodeError::Ffmpeg(if detail.is_empty() {
            format!("ffmpeg exited with {}", output.status)
        } else {
            detail
        }));
    }
    rgba_from_pam(output.stdout)
}

fn spawn(command: &mut Command) -> Result<std::process::Child, ImageDecodeError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    match command.spawn() {
        Ok(child) => Ok(child),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(ImageDecodeError::FfmpegMissing)
        }
        Err(error) => Err(error.into()),
    }
}

/// PAM 头是几行 `键 值`，以 `ENDHDR` 结束；后面紧跟 RGBA 像素。
fn rgba_from_pam(mut bytes: Vec<u8>) -> Result<image::RgbaImage, ImageDecodeError> {
    let malformed = || ImageDecodeError::Ffmpeg("ffmpeg wrote an unreadable picture".to_owned());
    let marker = b"ENDHDR\n";
    let header_end = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .ok_or_else(malformed)?
        + marker.len();
    let header = std::str::from_utf8(&bytes[..header_end]).map_err(|_| malformed())?;
    let field = |name: &str| {
        header
            .lines()
            .find_map(|line| line.strip_prefix(name)?.trim().parse::<u32>().ok())
    };
    let (Some(width), Some(height), Some(4), Some(255)) = (
        field("WIDTH"),
        field("HEIGHT"),
        field("DEPTH"),
        field("MAXVAL"),
    ) else {
        return Err(malformed());
    };
    bytes.drain(..header_end);
    image::RgbaImage::from_raw(width, height, bytes).ok_or_else(malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_group_wins_over_its_tiles() {
        let output = "\
stream_groups.stream_group.0.index=0
stream_groups.stream_group.0.type=\"Tile Grid\"
stream_groups.stream_group.0.components.component.0.width=2560
stream_groups.stream_group.0.components.component.0.height=1440
stream_groups.stream_group.0.disposition.default=1
streams.stream.0.codec_type=\"video\"
streams.stream.0.width=1280
streams.stream.0.height=720
";
        assert_eq!(
            plan_from_probe(output),
            Some(StillPlan {
                input: "0:g:0".to_owned(),
                width: 2560,
                height: 1440
            })
        );
    }

    #[test]
    fn default_stream_wins_and_quarter_turns_swap_the_size() {
        let output = "\
streams.stream.0.codec_type=\"video\"
streams.stream.0.width=320
streams.stream.0.height=212
streams.stream.0.disposition.default=0
streams.stream.1.codec_type=\"video\"
streams.stream.1.width=1280
streams.stream.1.height=854
streams.stream.1.disposition.default=1
streams.stream.1.side_data_list.side_data.0.side_data_type=\"Display Matrix\"
streams.stream.1.side_data_list.side_data.0.rotation=-90
";
        assert_eq!(
            plan_from_probe(output),
            Some(StillPlan {
                input: "0:1".to_owned(),
                width: 854,
                height: 1280
            })
        );
        assert_eq!(
            plan_from_probe("streams.stream.0.codec_type=\"audio\"\n"),
            None
        );
    }

    #[test]
    fn reads_pam_pixels() {
        let mut bytes =
            b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();
        bytes.extend([1, 2, 3, 4, 5, 6, 7, 8]);
        let image = rgba_from_pam(bytes).expect("valid PAM");
        assert_eq!(image.dimensions(), (2, 1));
        assert_eq!(image.get_pixel(1, 0).0, [5, 6, 7, 8]);
        assert!(rgba_from_pam(b"P7\nWIDTH 2\nENDHDR\n".to_vec()).is_err());
    }
}
