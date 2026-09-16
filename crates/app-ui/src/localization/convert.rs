pub(super) fn translate(text: &str) -> Option<String> {
    let exact = match text {
        "Convert Format" => "转换格式",
        "Convert Format..." => "转换格式...",
        "Images" => "图片",
        "Videos" => "视频",
        "Audio" => "音频",
        "Quality" => "质量",
        "Low" => "低",
        "Medium" => "中",
        "High" => "高",
        "Target size" => "目标体积",
        "Target size per file" => "每个文件的目标体积",
        // 转格式的缩放行用独立 key:裸 "Size" 是文件大小概念(属性面板/
        // 列表列头/分组菜单共用,译"大小"),不能被这里的尺寸语义遮蔽。
        "Resize" => "尺寸",
        "Custom" => "自定义",
        "Keep" => "保持",
        "Mono" => "单声道",
        "Frame rate" => "帧率",
        "Channels" => "声道",
        "Convert" => "转换",
        "Cancel" => "取消",
        "PNG is lossless; quality and target size do not apply." => {
            "PNG 为无损格式,质量与目标体积不适用。"
        }
        "FLAC is lossless; quality and target size do not apply." => {
            "FLAC 为无损格式,质量与目标体积不适用。"
        }
        "This format is lossless; quality and target size do not apply." => {
            "该格式为无损,质量与目标体积不适用。"
        }
        "Install ffmpeg to convert videos." => "安装 ffmpeg 后可转换视频。",
        "Install ffmpeg to convert audio files." => "安装 ffmpeg 后可转换音频。",
        "ffmpeg is missing; WebP and AVIF are unavailable." => "缺少 ffmpeg;WebP 和 AVIF 不可用。",
        "Checking ffmpeg availability..." => "正在检测 ffmpeg...",
        "ffmpeg is missing; video, audio, WebP and AVIF conversion is disabled. Install ffmpeg to enable them." => {
            "缺少 ffmpeg:视频、音频、WebP 和 AVIF 转换已禁用,安装 ffmpeg 后可用。"
        }
        "ffmpeg is missing for the selected target format. Install ffmpeg and retry." => {
            "所选目标格式缺少 ffmpeg 支持,请安装 ffmpeg 后重试。"
        }
        "Enter a target size like 500KB or 2MB." => "请输入目标体积,例如 500KB 或 2MB。",
        "Enter a custom width in pixels." => "请输入自定义宽度(像素)。",
        "Select at least one convertible file." => "请至少选择一个可转换的文件。",
        "Converted files are written next to the originals; existing files are never replaced." => {
            "转换结果写入原文件所在目录;不覆盖已有文件。"
        }
        "Sources: converted files are written next to the originals." => {
            "来源:转换结果写入原文件所在目录。"
        }
        "Some files could not be converted" => "部分文件转换失败",
        _ => return None,
    };
    Some(exact.to_owned())
}
