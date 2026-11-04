//! High-level DXGI session returning a keyframe followed by changed-tile updates.

use crate::{
    diff::{tiles_for_rectangles, TILE_EDGE},
    dxgi::DxgiCapture,
    CaptureColorDepth, CompressedScreenFrame, ScreenEncoder, ScreenRect,
};

/// High-level DXGI capture session with a changed-tile comparison baseline.
pub struct ScreenCaptureSession {
    capture: DxgiCapture,
    encoder: ScreenEncoder,
    pending_tiles: Vec<ScreenRect>,
}

impl ScreenCaptureSession {
    /// Starts capture and returns the session with its initial full-screen keyframe.
    pub fn start(
        color_depth: CaptureColorDepth,
        timestamp_ms: u64,
    ) -> Result<(Self, CompressedScreenFrame), String> {
        let mut capture = DxgiCapture::new(color_depth)?;
        capture.capture_initial()?;
        let (width, height) = capture.dimensions();
        let (encoder, keyframe) = ScreenEncoder::from_pixels(
            width,
            height,
            color_depth,
            capture.color_masks(),
            capture.pixels().to_vec(),
            timestamp_ms,
        )?;
        Ok((
            Self {
                capture,
                encoder,
                pending_tiles: Vec::new(),
            },
            keyframe,
        ))
    }

    /// Captures and immediately encodes any changed tiles since the previous frame.
    /// Captures and encodes any changed tiles since the previous call.
    pub fn capture_next(
        &mut self,
        timestamp_ms: u64,
    ) -> Result<Option<CompressedScreenFrame>, String> {
        self.poll_changes()?;
        self.flush_pending(timestamp_ms)
    }

    /// Polls DXGI for new dirty/move tiles and accumulates them into `pending_tiles`,
    /// returning the number of newly affected 32×32 tiles in this step.
    /// Captures DXGI metadata and accumulates newly affected tiles.
    pub fn poll_changes(&mut self) -> Result<u64, String> {
        let Some(new_tiles) = self.capture.capture_next()? else {
            return Ok(0);
        };
        let step_count = new_tiles.len() as u64;
        if step_count > 0 {
            let (width, height) = self.dimensions();
            self.pending_tiles.extend(new_tiles);
            self.pending_tiles = tiles_for_rectangles(width, height, &self.pending_tiles)?;
        }
        Ok(step_count)
    }

    /// Returns whether there are accumulated tile changes waiting to be encoded.
    /// Returns whether accumulated tiles are waiting to be encoded.
    pub fn has_pending_changes(&self) -> bool {
        !self.pending_tiles.is_empty()
    }

    /// Encodes all accumulated `pending_tiles` against the last emitted LREC frame.
    /// Encodes all accumulated tile changes into one update frame.
    pub fn flush_pending(
        &mut self,
        timestamp_ms: u64,
    ) -> Result<Option<CompressedScreenFrame>, String> {
        if self.pending_tiles.is_empty() {
            return Ok(None);
        }
        let tiles = std::mem::take(&mut self.pending_tiles);
        self.encoder
            .encode_tiles(timestamp_ms, self.capture.pixels(), &tiles)
    }

    /// Emits a complete screen keyframe and resets the delta comparison base.
    /// Emits a full-screen keyframe and resets the comparison baseline.
    pub fn force_keyframe(&mut self, timestamp_ms: u64) -> Result<CompressedScreenFrame, String> {
        let _ = self.capture.capture_next()?;
        self.pending_tiles.clear();
        let (width, height) = self.dimensions();
        let (encoder, frame) = ScreenEncoder::from_pixels(
            width,
            height,
            self.color_depth(),
            self.color_masks(),
            self.capture.pixels().to_vec(),
            timestamp_ms,
        )?;
        self.encoder = encoder;
        Ok(frame)
    }

    /// Returns the number of 32×32 tiles in the captured output.
    pub fn total_tiles(&self) -> u64 {
        let (width, height) = self.dimensions();
        u64::from(width.div_ceil(TILE_EDGE)).saturating_mul(u64::from(height.div_ceil(TILE_EDGE)))
    }

    /// Returns the captured output dimensions in pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        self.capture.dimensions()
    }

    /// Returns the current packed pixel format.
    pub fn color_depth(&self) -> CaptureColorDepth {
        self.encoder.color_depth()
    }

    /// Returns the channel masks associated with the pixel format.
    pub fn color_masks(&self) -> [u32; 3] {
        self.encoder.color_masks()
    }

    /// Returns the latest tightly packed image buffer.
    pub fn pixels(&self) -> &[u8] {
        self.capture.pixels()
    }

    /// Returns the whole output as a rectangle beginning at `(0, 0)`.
    pub fn screen_bounds(&self) -> ScreenRect {
        let (width, height) = self.dimensions();
        ScreenRect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }
}
