//! Windows desktop image capture through DXGI Desktop Duplication.
//!
//! The library reads DXGI dirty and moved rectangles, compares the affected
//! 32×32 tiles, and returns rectangle-update bytes in High Color or True Color.
//! It does not create recordings, files, network connections, or a user
//! interface. See the crate README and `docs/` for the wire layout and examples.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod rect;

#[cfg(windows)]
mod diff;
#[cfg(windows)]
mod dxgi;
#[cfg(windows)]
pub mod screen;
#[cfg(windows)]
mod session;

#[cfg(windows)]
pub use dxgi::CaptureColorDepth;
#[cfg(windows)]
pub use dxgi::DxgiCapture;
pub use rect::ScreenRect;
#[cfg(windows)]
pub use screen::{
    build_cmd0_rect_stream, encode_png_frame, CompressedScreenFrame, PayloadCompression,
    ScreenEncoder,
};
#[cfg(windows)]
pub use session::ScreenCaptureSession;
