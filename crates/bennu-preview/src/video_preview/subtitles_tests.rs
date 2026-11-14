use std::time::Duration;

use super::*;
use crate::video_preview::frames::{read_picture, stream_command};
use crate::video_preview::test_fixtures::video_fixture;
use crate::video_preview::{
    FramePlan, VideoColor, VideoPrimaries, VideoStreamFacts, VideoTransfer, YuvMatrix,
};

fn embedded(ordinal: usize, language: Option<&str>, is_default: bool) -> SubtitleChoice {
    SubtitleChoice::Embedded(SubtitleTrack {
        ordinal,
        codec: "subrip".to_owned(),
        is_image: false,
        language: language.map(str::to_owned),
        title: None,
        is_default,
    })
}

fn external(name: &str, language: Option<&str>) -> SubtitleChoice {
    SubtitleChoice::External(ExternalSubtitle {
        path: PathBuf::from(name),
        language: language.map(str::to_owned),
    })
}

#[test]
fn finds_same_named_subtitles_beside_the_video() {
    let directory = tempfile::tempdir().expect("temp dir");
    for name in [
        "movie.mkv",
        "movie.srt",
        "movie.zh.srt",
        "movie.chs.forced.ASS",
        "movie.txt",
        "other.srt",
        "movies.srt",
    ] {
        std::fs::write(directory.path().join(name), "").expect("write");
    }
    let found = external_subtitles_blocking(&directory.path().join("movie.mkv"));
    let summary = found
        .iter()
        .map(|subtitle| {
            (
                subtitle
                    .path
                    .file_name()
                    .expect("name")
                    .to_string_lossy()
                    .into_owned(),
                subtitle.language.clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        summary,
        [
            ("movie.srt".to_owned(), None),
            ("movie.chs.forced.ASS".to_owned(), Some("chs".to_owned())),
            ("movie.zh.srt".to_owned(), Some("zh".to_owned())),
        ]
    );
}

#[test]
fn default_prefers_external_then_flagged_then_interface_language_then_first() {
    use UiLanguage::{Chinese, English};
    // 外挂优先，有和界面语言一致的选它。
    let choices = [
        embedded(0, Some("eng"), true),
        external("movie.en.srt", Some("en")),
        external("movie.zh.srt", Some("zh")),
    ];
    assert_eq!(default_subtitle(&choices, Chinese), Some(2));
    assert_eq!(default_subtitle(&choices, English), Some(1));
    // 没有外挂：标了默认的。
    let choices = [
        embedded(0, Some("eng"), false),
        embedded(1, Some("jpn"), true),
    ];
    assert_eq!(default_subtitle(&choices, Chinese), Some(1));
    // 没标默认：界面语言。
    let choices = [
        embedded(0, Some("eng"), false),
        embedded(1, Some("chi"), false),
    ];
    assert_eq!(default_subtitle(&choices, Chinese), Some(1));
    // 都对不上：第一条；没有字幕：不显示。
    assert_eq!(default_subtitle(&choices[..1], Chinese), Some(0));
    assert_eq!(default_subtitle(&[], Chinese), None);
}

#[test]
fn subtitle_paths_are_escaped_for_both_filter_levels() {
    assert_eq!(
        text_subtitle_filter(Path::new("/tmp/a'b:c,d[e].ass")),
        r"subtitles=filename=/tmp/a\\\'b\\:c\,d\[e\].ass"
    );
}

#[test]
fn external_subtitles_are_copied_as_utf8_with_their_extension() {
    let directory = tempfile::tempdir().expect("temp dir");
    let source = directory.path().join("movie.srt");
    let text = "1\n00:00:01,000 --> 00:00:02,000\n这是一句用简体中文写的字幕，用来检查编码识别。\n";
    std::fs::write(&source, encoding_rs::GBK.encode(text).0).expect("write gbk");

    let SubtitleBurn::Text(copy) = copy_external_as_utf8(&source).expect("copy") else {
        panic!("expected a text subtitle");
    };
    assert_eq!(copy.extension().and_then(|ext| ext.to_str()), Some("srt"));
    assert_eq!(std::fs::read_to_string(&copy).expect("utf-8"), text);
}

#[tokio::test]
async fn embedded_text_subtitles_are_extracted_once_and_burned_in() {
    let directory = tempfile::tempdir().expect("temp dir");
    let srt = directory.path().join("lines.srt");
    std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:02,000\nHello\n").expect("write srt");
    let srt_argument = srt.to_string_lossy().into_owned();
    let Some(video) = video_fixture(
        directory.path(),
        "movie.mkv",
        "320x240",
        1,
        &["-i", &srt_argument, "-c:s", "srt"],
    ) else {
        return;
    };

    let burn = prepare_subtitle(video.clone(), embedded(0, None, true))
        .await
        .expect("extract");
    let SubtitleBurn::Text(file) = &burn else {
        panic!("expected a text subtitle");
    };
    let extracted = std::fs::read_to_string(file).expect("read extracted");
    assert!(extracted.contains("Hello"), "{extracted}");

    let facts = VideoStreamFacts {
        ordinal: 0,
        codec: "mpeg4".to_owned(),
        width: 320,
        height: 240,
        sample_aspect: (1, 1),
        frame_rate: Some((30, 1)),
        rotation: 0,
        pixel_format: None,
        color: VideoColor {
            matrix: YuvMatrix::Bt601,
            transfer: VideoTransfer::Sdr,
            primaries: VideoPrimaries::Bt709,
        },
        interlaced: false,
        bit_rate: None,
    };
    let plan = FramePlan::new(&facts, iced::Size::new(1920, 1080));
    let mut child = stream_command(&video, &plan, Duration::ZERO, None, Some(&burn))
        .spawn()
        .expect("spawn ffmpeg");
    let mut stdout = child.stdout.take().expect("stdout");
    assert!(read_picture(&mut stdout, &plan)
        .await
        .expect("read")
        .is_some());
}

#[test]
fn image_subtitles_need_no_preparation() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let choice = SubtitleChoice::Embedded(SubtitleTrack {
        ordinal: 2,
        codec: "hdmv_pgs_subtitle".to_owned(),
        is_image: true,
        language: None,
        title: None,
        is_default: false,
    });
    let burn = runtime
        .block_on(prepare_subtitle(PathBuf::from("/nonexistent.mkv"), choice))
        .expect("image subtitle");
    assert!(matches!(burn, SubtitleBurn::Image { ordinal: 2 }));
}
