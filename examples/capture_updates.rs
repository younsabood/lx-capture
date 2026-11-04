#[cfg(windows)]
fn main() -> Result<(), String> {
    use std::{
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    use lx_capture::{CaptureColorDepth, ScreenCaptureSession};

    let start_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis() as u64;
    let (mut capture, keyframe) =
        ScreenCaptureSession::start(CaptureColorDepth::HighColor, start_ms)?;
    let (width, height) = capture.dimensions();
    println!(
        "Captured {width}x{height}; initial payload {} bytes",
        keyframe.payload.len()
    );
    print_frame(&keyframe);

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        thread::sleep(Duration::from_millis(200));
        let timestamp_ms = start_ms + start.elapsed().as_millis() as u64;
        if let Some(frame) = capture.capture_next(timestamp_ms)? {
            print_frame(&frame);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn print_frame(frame: &lx_capture::CompressedScreenFrame) {
    println!(
        "t={}ms keyframe={} rectangles={} tiles={} compression={:?} payload={} bytes",
        frame.timestamp_ms,
        frame.keyframe,
        frame.rects.len(),
        frame.dirty_blocks,
        frame.compression,
        frame.payload.len(),
    );
}

#[cfg(not(windows))]
fn main() {
    eprintln!("LX Capture desktop capture requires Windows and DXGI.");
}
