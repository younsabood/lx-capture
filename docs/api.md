# API and byte format

The Rust crate is imported as `lx_capture`. DXGI capture types are only exported on Windows.

## Main API

| Type or function | Role |
| --- | --- |
| `CaptureColorDepth` | Selects 16-bit `HighColor` or 24-bit `TrueColor` |
| `ScreenCaptureSession::start` | Starts DXGI capture and returns `(session, initial_keyframe)` |
| `ScreenCaptureSession::capture_next` | Returns `None` or the next changed-screen update |
| `ScreenCaptureSession::poll_changes` / `flush_pending` | Accumulates changed tiles and emits them when requested |
| `ScreenCaptureSession::force_keyframe` | Captures and returns a new full-screen keyframe |
| `CompressedScreenFrame` | Carries the timestamp, payload, compression, rectangle list, and color depth |
| `build_cmd0_rect_stream` | Builds the raw command-0 stream for caller-owned pixel buffers |
| `encode_png_frame` | Converts a packed capture buffer into RGB8 PNG bytes |
| `ScreenRect` | Describes a rectangle in output pixel coordinates |

## Pixel layouts

| Mode | Storage per pixel | Bytes | Details |
| --- | --- | ---: | --- |
| High Color | RGB555 in little-endian `u16` | 2 | Red bits 10–14, green bits 5–9, blue bits 0–4; masks `0x7C00`, `0x03E0`, `0x001F` |
| True Color | B, G, R bytes | 3 | Packed BGR24 without row padding |

In High Color capture, the DXGI path normalizes near-white and near-black pixels and clears the low bit of each five-bit channel for most other colors. This is part of the current capture output and improves repeated pixel patterns.

## Rectangle stream

The uncompressed payload is a concatenation of command-0 rectangles. Each rectangle starts with a 21-byte header:

| Offset | Size | Value |
| ---: | ---: | --- |
| 0 | 1 byte | Command, currently `0` |
| 1 | 4 bytes | `x`, unsigned little-endian |
| 5 | 4 bytes | `y`, unsigned little-endian |
| 9 | 4 bytes | `width`, unsigned little-endian |
| 13 | 4 bytes | `height`, unsigned little-endian |
| 17 | 4 bytes | Pixel-data length, unsigned little-endian |
| 21 | variable | Tightly packed rows in the selected pixel layout |

`CompressedScreenFrame::payload` is either this raw stream or a zlib-wrapped stream containing it. Check `compression` before decoding. Compression is used only when the zlib result is smaller than the raw stream. The LREC container header is not part of this library payload.

## Frame semantics

- The initial frame is a keyframe containing the whole output.
- Delta frames contain only changed rectangles; they are not independent images.
- `rects` gives the coordinates represented by the payload.
- `dirty_blocks` counts changed 32×32 tiles selected for the update.
- `timestamp_ms` is supplied by the caller and is copied to the result.
- `None` means there is no changed pixel update to emit.

Treat all rectangle coordinates and lengths as untrusted when parsing bytes received from elsewhere. Validate dimensions, lengths, and arithmetic before allocating or indexing buffers.
