use std::num::NonZero;

use super::*;
use crate::audio_preview::test_fixtures::sine_fixture;

const STEREO_48K: PcmFormat = PcmFormat {
    channels: NonZero::new(2).unwrap(),
    sample_rate: NonZero::new(48_000).unwrap(),
};

fn stream_from_chunks(start: Duration, chunks: Vec<Vec<f32>>) -> PcmStream {
    let (sender, receiver) = sync_channel(chunks.len().max(1));
    for chunk in chunks {
        sender.send(chunk).expect("queue chunk");
    }
    PcmStream {
        start,
        first_chunk: None,
        chunks: receiver,
    }
}

fn source_with(exchange: &Arc<PlaybackExchange>) -> FfmpegPcmSource {
    FfmpegPcmSource::new(Arc::clone(exchange), STEREO_48K)
}

#[test]
fn position_counts_consumed_frames_from_the_stream_start() {
    let exchange = Arc::new(PlaybackExchange::new(STEREO_48K.sample_rate));
    // 两块各 24 000 帧（半秒）。
    let half_second = vec![0.5_f32; 48_000];
    exchange.replace_stream(stream_from_chunks(
        Duration::from_secs(10),
        vec![half_second.clone(), half_second],
    ));
    let mut source = source_with(&exchange);

    // 交换生效前按目标位置算。
    assert_eq!(exchange.position(), Duration::from_secs(10));
    for _ in 0..48_000 * 2 {
        assert_eq!(source.next(), Some(0.5));
    }
    // 第二块取空后再取一次才把它记进进度。
    let _ = source.next();
    assert_eq!(exchange.position(), Duration::from_secs(11));
    assert!(exchange.has_ended());
}

#[test]
fn underrun_outputs_whole_silent_frames_without_advancing_position() {
    let exchange = Arc::new(PlaybackExchange::new(STEREO_48K.sample_rate));
    let (_sender, receiver) = sync_channel::<Vec<f32>>(1);
    exchange.replace_stream(PcmStream {
        start: Duration::ZERO,
        first_chunk: None,
        chunks: receiver,
    });
    let mut source = source_with(&exchange);

    for _ in 0..8 {
        assert_eq!(source.next(), Some(0.0));
    }
    assert_eq!(exchange.position(), Duration::ZERO);
    // 发送端还在：是欠载，不是播完。
    assert!(!exchange.has_ended());
}

#[test]
fn replacing_the_stream_switches_on_a_frame_boundary_and_clears_the_end() {
    let exchange = Arc::new(PlaybackExchange::new(STEREO_48K.sample_rate));
    exchange.replace_stream(stream_from_chunks(Duration::ZERO, vec![vec![1.0, -1.0]]));
    let mut source = source_with(&exchange);
    assert_eq!(source.next(), Some(1.0));

    // 帧中间来了新流：先把这帧的右声道输出完再换。
    exchange.replace_stream(stream_from_chunks(
        Duration::from_secs(30),
        vec![vec![0.25, 0.75]],
    ));
    assert_eq!(source.next(), Some(-1.0));
    assert_eq!(source.next(), Some(0.25));
    assert_eq!(source.next(), Some(0.75));
    assert_eq!(exchange.position(), Duration::from_secs(30));
}

#[test]
fn failure_message_keeps_the_last_ffmpeg_line() {
    assert_eq!(
        ffmpeg_failure_message(
            "[in#0] something\nStream map '0:a:0' matches no streams.\n",
            "exit status: 1"
        ),
        "Stream map '0:a:0' matches no streams."
    );
    assert_eq!(
        ffmpeg_failure_message("", "exit status: 1"),
        "ffmpeg exited with exit status: 1"
    );
}

fn drain_frames(mut stream: PcmStream) -> usize {
    let mut samples = stream.first_chunk.take().map_or(0, |chunk| chunk.len());
    while let Ok(chunk) = stream.chunks.recv() {
        samples += chunk.len();
    }
    samples / STEREO_48K.channel_count()
}

#[test]
fn ffmpeg_stream_decodes_any_format_and_starts_at_the_seek_position() {
    let directory = tempfile::tempdir().expect("temp dir");
    let Some(opus) = sine_fixture(directory.path(), "tone.opus", 2) else {
        return;
    };
    let failure = Arc::new(StreamFailure::default());

    let mut whole = spawn_pcm_stream(&opus, Duration::ZERO, STEREO_48K, Arc::clone(&failure))
        .expect("spawn ffmpeg");
    whole.wait_for_audio(&failure).expect("first chunk");
    let frames = drain_frames(whole);
    assert!((95_000..=97_000).contains(&frames), "{frames}");

    let mut tail = spawn_pcm_stream(
        &opus,
        Duration::from_secs(1),
        STEREO_48K,
        Arc::clone(&failure),
    )
    .expect("spawn ffmpeg");
    tail.wait_for_audio(&failure).expect("first chunk");
    let frames = drain_frames(tail);
    assert!((47_000..=49_000).contains(&frames), "{frames}");
    assert!(failure.take().is_none());
}

#[test]
fn undecodable_file_fails_before_playback_starts() {
    let directory = tempfile::tempdir().expect("temp dir");
    if sine_fixture(directory.path(), "probe.wav", 1).is_none() {
        return;
    }
    let broken = directory.path().join("broken.opus");
    std::fs::write(&broken, b"not audio at all").expect("write fixture");
    let failure = Arc::new(StreamFailure::default());

    let mut stream = spawn_pcm_stream(&broken, Duration::ZERO, STEREO_48K, Arc::clone(&failure))
        .expect("spawn ffmpeg");
    let error = stream.wait_for_audio(&failure).expect_err("cannot decode");
    assert!(!error.is_empty());
}
