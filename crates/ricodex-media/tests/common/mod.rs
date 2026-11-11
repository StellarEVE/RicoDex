//! Generated fixtures for the media pipeline tests. Everything is
//! built with the system ffmpeg (plus plain file writes for the
//! moving box frames), into a per test temporary directory.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A fresh temporary directory for one test.
pub fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ricodex-media-test-{}-{}-{:?}",
        name,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(args)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg failed for fixture: {args:?}");
}

/// The cut fixture: 4.5 seconds at 24 fps, 320x240, H.264.
/// Frames 0..55 (0 to 2.2917 s): bright yellow with a black box
/// moving 5 px per frame. Frames 55..108: solid dark blue. The hard
/// cut sits at exactly 55/24 = 2.2917 seconds.
pub fn make_cut_fixture(dir: &Path) -> PathBuf {
    let frames_dir = dir.join("box-frames");
    std::fs::create_dir_all(&frames_dir).unwrap();
    for i in 0..55usize {
        write_box_frame(&frames_dir.join(format!("f{i:03}.ppm")), i);
    }
    let pattern = frames_dir.join("f%03d.ppm");
    let out = dir.join("cut.mp4");
    run_ffmpeg(&[
        "-y",
        "-v",
        "error",
        "-framerate",
        "24",
        "-i",
        pattern.to_str().unwrap(),
        "-f",
        "lavfi",
        "-i",
        "color=c=0x102060:s=320x240:r=24:d=2.2",
        "-filter_complex",
        "[0:v][1:v]concat=n=2:v=1:a=0,format=yuv420p[v]",
        "-map",
        "[v]",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "18",
        out.to_str().unwrap(),
    ]);
    out
}

/// One PPM frame of the moving box segment: yellow (255, 210, 0)
/// with a 70x70 black box whose left edge sits at (frame * 5) mod
/// 220, vertically centred on rows 80..150.
fn write_box_frame(path: &Path, frame: usize) {
    let (w, h) = (320usize, 240usize);
    let box_x = (frame * 5) % 220;
    let mut pixels = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let offset = (y * w + x) * 3;
            if (box_x..box_x + 70).contains(&x) && (80..150).contains(&y) {
                pixels[offset] = 0;
                pixels[offset + 1] = 0;
                pixels[offset + 2] = 0;
            } else {
                pixels[offset] = 255;
                pixels[offset + 1] = 210;
                pixels[offset + 2] = 0;
            }
        }
    }
    let mut data = format!("P6\n{w} {h}\n255\n").into_bytes();
    data.extend_from_slice(&pixels);
    std::fs::write(path, data).unwrap();
}

/// The black fixture: 4.5 seconds at 24 fps, 320x240, H.264. White
/// for 1.5 s, black for 1.5 s, white for 1.5 s.
pub fn make_black_fixture(dir: &Path) -> PathBuf {
    let out = dir.join("black.mp4");
    run_ffmpeg(&[
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=white:s=320x240:r=24:d=1.5",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=320x240:r=24:d=1.5",
        "-f",
        "lavfi",
        "-i",
        "color=c=white:s=320x240:r=24:d=1.5",
        "-filter_complex",
        "[0:v][1:v][2:v]concat=n=3:v=1:a=0,format=yuv420p[v]",
        "-map",
        "[v]",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "18",
        out.to_str().unwrap(),
    ]);
    out
}
