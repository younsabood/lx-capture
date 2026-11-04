# Quick start

LX Capture uses the Windows DXGI Desktop Duplication API. Start from a Windows MSVC Rust project and add:

```toml
[dependencies]
lx-capture = "0.1"
```

## Capture a keyframe and changed updates

```rust,no_run
use std::{thread, time::{Duration, Instant}};
use lx_capture::{CaptureColorDepth, ScreenCaptureSession};

fn main() -> Result<(), String> {
    let (mut screen, first) =
        ScreenCaptureSession::start(CaptureColorDepth::HighColor, 0)?;
    let (width, height) = screen.dimensions();
    println!("screen: {width}×{height}");
    consume(first.payload);

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        thread::sleep(Duration::from_millis(200));
        let timestamp_ms = start.elapsed().as_millis() as u64;
        if let Some(frame) = screen.capture_next(timestamp_ms)? {
            println!("keyframe={}, rectangles={}, compressed={:?}, bytes={}",
                frame.keyframe, frame.rects.len(), frame.compression,
                frame.payload.len());
            consume(frame.payload);
        }
    }
    Ok(())
}

fn consume(_bytes: Vec<u8>) {
    // Pass bytes to your own transport, image encoder, or recording container.
}
```

Run the example included with this crate:

```powershell
cargo run --locked --example capture_updates
```

## Select color depth

- `CaptureColorDepth::HighColor` returns packed RGB555 pixels (2 bytes per pixel).
- `CaptureColorDepth::TrueColor` returns BGR24 pixels (3 bytes per pixel).

The keyframe and each delta identify their selected depth in `CompressedScreenFrame::color_depth`. Read [API and byte format](api.md) before handing these bytes to a decoder.

## Capture lifecycle

`ScreenCaptureSession::start` creates DXGI duplication and returns the initial full-screen keyframe. Call `capture_next` on the cadence your application needs. It returns `None` when there is no changed screen content, and a rectangle update when visible pixels differ. Call `force_keyframe` when a downstream consumer needs a new full-screen baseline.

The library selects one desktop output and currently rejects rotated outputs. See [Architecture](architecture.md) for the capture and metadata flow.
