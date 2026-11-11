//! Platform tests against a generated H.264 fixture: the capability
//! report reflects the linked FFmpeg build, and trial decode reports
//! real outcomes, software succeeding on a normal file.

use std::path::PathBuf;
use std::process::Command;

use ricodex_platform::{capability_report, probe, trial_decode};

fn fixture_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ricodex-platform-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A two second H.264 MP4 at 320x240, generated with the system
/// ffmpeg.
fn make_fixture(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("fixture.mp4");
    if path.is_file() {
        return path;
    }
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=320x240:r=24:d=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            path.to_str().unwrap(),
        ])
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg fixture generation failed");
    path
}

#[test]
fn capability_report_reflects_the_linked_build() {
    let report = capability_report();
    assert!(!report.os.is_empty());
    assert!(!report.cpu.is_empty());
    // This machine's FFmpeg 8.1.2 build reports hardware acceleration
    // methods (vdpau, cuda, vaapi, qsv, drm, opencl when checked with
    // ffmpeg -hwaccels), so the report must list some.
    assert!(
        !report.hwaccels.is_empty(),
        "expected the linked build to report hardware methods"
    );
    for method in &report.hwaccels {
        assert!(!method.name.is_empty());
    }
    // Software H.264 decode is the baseline fallback and exists.
    assert!(report.software_fallback);
}

#[test]
fn probe_reads_container_metadata() {
    let dir = fixture_dir();
    let path = make_fixture(&dir);
    let info = probe(&path).unwrap();
    assert_eq!(info.width, 320);
    assert_eq!(info.height, 240);
    assert_eq!(info.codec_name, "h264");
    let seconds = info.duration.to_seconds();
    assert!(
        (1.8..2.2).contains(&seconds),
        "duration {seconds} not near 2 seconds"
    );
    assert!((info.frame_rate - 24.0).abs() < 0.5);
}

#[test]
fn trial_decode_software_succeeds_on_h264() {
    let dir = fixture_dir();
    let path = make_fixture(&dir);
    let report = trial_decode(&path, None);
    assert!(report.success, "software trial failed: {:?}", report.failure);
    assert!(report.frames_decoded >= 1);
    assert_eq!(report.method, None);
    assert_eq!(report.failure, None);
}

#[test]
fn trial_decode_unknown_method_reports_failure_reason() {
    let dir = fixture_dir();
    let path = make_fixture(&dir);
    let report = trial_decode(&path, Some("no-such-method"));
    assert!(!report.success);
    assert_eq!(report.method.as_deref(), Some("no-such-method"));
    let failure = report.failure.expect("a failure reason is reported");
    assert!(failure.contains("no-such-method"), "reason: {failure}");
}

#[test]
fn trial_decode_hardware_method_reports_an_honest_outcome() {
    // vaapi is a method this FFmpeg build lists, but this machine has
    // no GPU device behind it: the trial must either genuinely
    // succeed or fail with a reason. Both are honest outcomes; a
    // panic or an empty report is not.
    let dir = fixture_dir();
    let path = make_fixture(&dir);
    let report = trial_decode(&path, Some("vaapi"));
    assert_eq!(report.method.as_deref(), Some("vaapi"));
    if report.success {
        assert!(report.frames_decoded >= 1);
    } else {
        assert!(
            report.failure.as_deref().map(|f| !f.is_empty()) == Some(true),
            "failed trial must carry a reason"
        );
    }
}
