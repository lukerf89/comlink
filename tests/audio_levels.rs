//! Regression coverage for LF-38 part (b): captured-audio level measurement is
//! the ground truth behind the near-silent capture warning. These exercise the
//! committed fixtures end-to-end through the public `comlink::audio` API so the
//! silence classification cannot silently regress.

use std::path::{Path, PathBuf};

use comlink::audio;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

#[test]
fn silence_fixture_measures_as_near_silent() {
    let samples =
        audio::read_wav_level_samples(&fixture("silence.wav")).expect("silence.wav is 16-bit PCM");
    let level = audio::session_audio_level([samples]).expect("measurable samples");
    assert!(
        level.is_near_silent(),
        "silence.wav mean {} dBFS should classify as near-silent",
        level.mean_dbfs
    );
}

#[test]
fn speech_fixture_is_not_near_silent() {
    // Guards against false positives: the real-speech fixture (also used by the
    // phase-7 meeting E2E as every chunk) must stay above the silence floor.
    let samples =
        audio::read_wav_level_samples(&fixture("short.wav")).expect("short.wav is 16-bit PCM");
    let level = audio::session_audio_level([samples]).expect("measurable samples");
    assert!(
        !level.is_near_silent(),
        "short.wav mean {} dBFS should be above the near-silent threshold",
        level.mean_dbfs
    );
}
