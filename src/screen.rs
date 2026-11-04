//! High Color / True Color screen updates built from DXGI dirty/move tiles.
//!
//! DXGI supplies dirty and moved rectangles; this module packs only affected
//! 32×32 tiles and compresses their rectangle payloads.

use std::io::Write;

use crate::{
    diff::{merge_adjacent_rects, tiles_for_rectangles, TILE_EDGE},
    dxgi::CaptureColorDepth,
    rect::{validate_rect, ScreenRect},
};

const COMPRESSION_THRESHOLD_BYTES: usize = 64;
const REFERENCE_ZLIB_LEVEL: u32 = 9;
const REFERENCE_SNAPSHOT_ZLIB_LEVEL: u32 = 9;

/// Encoding applied to the rectangle update stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadCompression {
    Raw,
    Zlib,
}

/// Ready bytes for one full-screen keyframe or one changed-screen update.
///
/// `payload` contains command-0 rectangles in the original little-endian layout.
/// Each pixel is encoded in the selected High Color or True Color layout.
#[derive(Clone, Debug)]
pub struct CompressedScreenFrame {
    pub timestamp_ms: u64,
    pub payload: Vec<u8>,
    pub compression: PayloadCompression,
    pub keyframe: bool,
    pub rects: Vec<ScreenRect>,
    pub dirty_blocks: u64,
    pub color_depth: CaptureColorDepth,
}

/// Screen encoder for 16-bit High Color and 24-bit True Color modes.
pub struct ScreenEncoder {
    width: u32,
    height: u32,
    color_depth: CaptureColorDepth,
    color_masks: [u32; 3],
    previous: Vec<u8>,
}

impl ScreenEncoder {
    /// Creates a full-screen keyframe from tightly packed 16-bit or 24-bit pixels.
    pub fn from_pixels(
        width: u32,
        height: u32,
        color_depth: CaptureColorDepth,
        color_masks: [u32; 3],
        pixels: Vec<u8>,
        timestamp_ms: u64,
    ) -> Result<(Self, CompressedScreenFrame), String> {
        if width == 0 || height == 0 {
            return Err("screen dimensions must be nonzero".to_owned());
        }
        let pixel_bytes = color_depth.bytes_per_pixel();
        let expected_len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|value| value.checked_mul(pixel_bytes))
            .ok_or_else(|| "screen dimensions overflow".to_owned())?;
        if pixels.len() != expected_len {
            return Err("pixel buffer size does not match screen dimensions".to_owned());
        }
        if color_depth == CaptureColorDepth::HighColor && !valid_masks(color_masks) {
            return Err("High Color requires three non-overlapping RGB masks".to_owned());
        }
        if color_depth == CaptureColorDepth::TrueColor && color_masks != [0; 3] {
            return Err("True Color BI_RGB does not use channel masks".to_owned());
        }

        let encoder = Self {
            width,
            height,
            color_depth,
            color_masks,
            previous: pixels.clone(),
        };
        let frame = encoder.prepare_rects(
            timestamp_ms,
            &pixels,
            vec![ScreenRect {
                x: 0,
                y: 0,
                width,
                height,
            }],
            dirty_block_count(width, height),
            true,
        )?;
        Ok((encoder, frame))
    }

    /// Encodes only metadata-selected tiles whose pixels actually changed.
    ///
    /// Adjacent 32×32 tiles are merged before trimming outer bounds so text and
    /// PDF pages coalesce into contiguous regions rather than hundreds of
    /// fragmented sub-rectangles.
    pub fn encode_tiles(
        &mut self,
        timestamp_ms: u64,
        pixels: &[u8],
        tiles: &[ScreenRect],
    ) -> Result<Option<CompressedScreenFrame>, String> {
        if pixels.len() != self.previous.len() {
            return Err("frame size mismatch".to_owned());
        }
        let candidate_tiles = tiles_for_rectangles(self.width, self.height, tiles)?;
        let pixel_bytes = self.color_depth.bytes_per_pixel();
        let mut active_tiles = Vec::new();
        for tile in candidate_tiles {
            if tile_has_changes(&self.previous, pixels, self.width, pixel_bytes, tile) {
                active_tiles.push(tile);
            }
        }
        if active_tiles.is_empty() {
            return Ok(None);
        }
        let changed_tile_count = active_tiles.len() as u64;

        // دمج البلاطات المتجاورة بأبعادها القياسية 32×32 أولاً لمنع تفتت الصفحة
        let merged_tiles = merge_adjacent_rects(&active_tiles);
        let mut trimmed_rects = Vec::with_capacity(merged_tiles.len());
        for rect in merged_tiles {
            if let Some(bounds) =
                changed_bounds(&self.previous, pixels, self.width, pixel_bytes, rect)
            {
                trimmed_rects.push(bounds);
            }
        }
        if trimmed_rects.is_empty() {
            return Ok(None);
        }
        let final_rects = merge_adjacent_rects(&trimmed_rects);

        let frame = self.prepare_rects(
            timestamp_ms,
            pixels,
            final_rects.clone(),
            changed_tile_count,
            false,
        )?;
        for rect in final_rects {
            copy_rect(pixels, &mut self.previous, self.width, pixel_bytes, rect);
        }
        Ok(Some(frame))
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn color_depth(&self) -> CaptureColorDepth {
        self.color_depth
    }

    pub fn color_masks(&self) -> [u32; 3] {
        self.color_masks
    }

    fn prepare_rects(
        &self,
        timestamp_ms: u64,
        pixels: &[u8],
        rects: Vec<ScreenRect>,
        dirty_blocks: u64,
        keyframe: bool,
    ) -> Result<CompressedScreenFrame, String> {
        let raw = build_cmd0_rect_stream(
            self.width,
            self.height,
            self.color_depth.bytes_per_pixel(),
            pixels,
            &rects,
        )?;
        let raw_size = raw.len();
        let (payload, compression) = maybe_deflate(raw, raw_size, keyframe)?;
        Ok(CompressedScreenFrame {
            timestamp_ms,
            payload,
            compression,
            keyframe,
            rects,
            dirty_blocks,
            color_depth: self.color_depth,
        })
    }
}

/// Raw stream: command-0 + LE rectangle header + tightly packed pixel rows.
pub fn build_cmd0_rect_stream(
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
    pixels: &[u8],
    rects: &[ScreenRect],
) -> Result<Vec<u8>, String> {
    if !(2..=3).contains(&bytes_per_pixel) {
        return Err("color capture requires 2-byte or 3-byte pixels".to_owned());
    }
    let row_stride = (width as usize)
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| "screen dimensions overflow".to_owned())?;
    let expected_len = row_stride
        .checked_mul(height as usize)
        .ok_or_else(|| "screen dimensions overflow".to_owned())?;
    if width == 0 || height == 0 || pixels.len() != expected_len {
        return Err("pixel buffer dimensions mismatch".to_owned());
    }

    let mut raw = Vec::with_capacity(rects.len().saturating_mul(21));
    for rect in rects {
        validate_rect(*rect, width, height)?;
        let row_bytes = (rect.width as usize)
            .checked_mul(bytes_per_pixel)
            .ok_or_else(|| "rect size overflow".to_owned())?;
        let data_len = row_bytes
            .checked_mul(rect.height as usize)
            .ok_or_else(|| "rect size overflow".to_owned())?;
        let data_len_u32 = u32::try_from(data_len).map_err(|_| "rect too large".to_owned())?;
        raw.push(0);
        raw.extend_from_slice(&rect.x.to_le_bytes());
        raw.extend_from_slice(&rect.y.to_le_bytes());
        raw.extend_from_slice(&rect.width.to_le_bytes());
        raw.extend_from_slice(&rect.height.to_le_bytes());
        raw.extend_from_slice(&data_len_u32.to_le_bytes());
        for row in 0..rect.height as usize {
            let y = rect.y as usize + row;
            let start = y * row_stride + rect.x as usize * bytes_per_pixel;
            raw.extend_from_slice(&pixels[start..start + row_bytes]);
        }
    }
    Ok(raw)
}

/// Converts a captured 16-bit or 24-bit frame to a compressed RGB8 PNG.
pub fn encode_png_frame(
    width: u32,
    height: u32,
    color_depth: CaptureColorDepth,
    pixels: &[u8],
) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("PNG dimensions must be nonzero".to_owned());
    }
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| "PNG dimensions overflow".to_owned())?;
    let source_stride = (width as usize)
        .checked_mul(color_depth.bytes_per_pixel())
        .ok_or_else(|| "PNG row size overflow".to_owned())?;
    let expected_len = pixel_count
        .checked_mul(color_depth.bytes_per_pixel())
        .ok_or_else(|| "PNG image size overflow".to_owned())?;
    if pixels.len() != expected_len {
        return Err("PNG pixel buffer does not match dimensions".to_owned());
    }
    let scanline_len = (width as usize)
        .checked_mul(3)
        .and_then(|row| row.checked_add(1))
        .and_then(|row| row.checked_mul(height as usize))
        .ok_or_else(|| "PNG scanline size overflow".to_owned())?;
    let mut scanlines = Vec::with_capacity(scanline_len);
    for row in 0..height as usize {
        scanlines.push(0); // PNG filter: None
        let source = &pixels[row * source_stride..(row + 1) * source_stride];
        match color_depth {
            CaptureColorDepth::TrueColor => {
                for pixel in source.chunks_exact(3) {
                    scanlines.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
                }
            }
            CaptureColorDepth::HighColor => {
                for pixel in source.chunks_exact(2) {
                    let value = u16::from_le_bytes([pixel[0], pixel[1]]);
                    let red = ((value >> 10) & 0x1f) as u8;
                    let green = ((value >> 5) & 0x1f) as u8;
                    let blue = (value & 0x1f) as u8;
                    scanlines.extend_from_slice(&[
                        (red << 3) | (red >> 2),
                        (green << 3) | (green >> 2),
                        (blue << 3) | (blue >> 2),
                    ]);
                }
            }
        }
    }

    let mut deflater = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    deflater
        .write_all(&scanlines)
        .map_err(|error| format!("PNG zlib encode: {error}"))?;
    let image_data = deflater
        .finish()
        .map_err(|error| format!("PNG zlib finish: {error}"))?;

    let mut png = Vec::with_capacity(image_data.len().saturating_add(57));
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]); // RGB8, no interlace
    append_png_chunk(&mut png, b"IHDR", &header)?;
    append_png_chunk(&mut png, b"IDAT", &image_data)?;
    append_png_chunk(&mut png, b"IEND", &[])?;
    Ok(png)
}

fn append_png_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<(), String> {
    let length = u32::try_from(data.len()).map_err(|_| "PNG chunk exceeds 4GB".to_owned())?;
    png.extend_from_slice(&length.to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    png.extend_from_slice(&crc.finalize().to_be_bytes());
    Ok(())
}

fn valid_masks(masks: [u32; 3]) -> bool {
    masks.iter().all(|mask| *mask != 0)
        && masks[0] & masks[1] == 0
        && masks[0] & masks[2] == 0
        && masks[1] & masks[2] == 0
}

fn tile_has_changes(
    previous: &[u8],
    current: &[u8],
    screen_width: u32,
    bytes_per_pixel: usize,
    rect: ScreenRect,
) -> bool {
    let stride = screen_width as usize * bytes_per_pixel;
    let row_bytes = rect.width as usize * bytes_per_pixel;
    for row in 0..rect.height as usize {
        let start = (rect.y as usize + row) * stride + rect.x as usize * bytes_per_pixel;
        if previous[start..start + row_bytes] != current[start..start + row_bytes] {
            return true;
        }
    }
    false
}

fn changed_bounds(
    previous: &[u8],
    current: &[u8],
    screen_width: u32,
    bytes_per_pixel: usize,
    rect: ScreenRect,
) -> Option<ScreenRect> {
    let stride = screen_width as usize * bytes_per_pixel;
    let row_bytes = rect.width as usize * bytes_per_pixel;
    let (mut min_x, mut max_x) = (usize::MAX, 0_usize);
    let (mut min_y, mut max_y) = (usize::MAX, 0_usize);
    for row in 0..rect.height as usize {
        let start = (rect.y as usize + row) * stride + rect.x as usize * bytes_per_pixel;
        let before = &previous[start..start + row_bytes];
        let after = &current[start..start + row_bytes];
        if before == after {
            continue;
        }
        let mut first = None;
        let mut last = 0_usize;
        for column in 0..rect.width as usize {
            let offset = column * bytes_per_pixel;
            if before[offset..offset + bytes_per_pixel] != after[offset..offset + bytes_per_pixel] {
                first.get_or_insert(column);
                last = column;
            }
        }
        if let Some(first) = first {
            min_x = min_x.min(first);
            max_x = max_x.max(last);
            min_y = min_y.min(row);
            max_y = max_y.max(row);
        }
    }
    if min_y == usize::MAX {
        None
    } else {
        Some(ScreenRect {
            x: rect.x + min_x as u32,
            y: rect.y + min_y as u32,
            width: (max_x - min_x + 1) as u32,
            height: (max_y - min_y + 1) as u32,
        })
    }
}

fn copy_rect(
    source: &[u8],
    destination: &mut [u8],
    screen_width: u32,
    bytes_per_pixel: usize,
    rect: ScreenRect,
) {
    let row_bytes = rect.width as usize * bytes_per_pixel;
    for row in 0..rect.height as usize {
        let start =
            ((rect.y as usize + row) * screen_width as usize + rect.x as usize) * bytes_per_pixel;
        destination[start..start + row_bytes].copy_from_slice(&source[start..start + row_bytes]);
    }
}

fn maybe_deflate(
    raw: Vec<u8>,
    raw_size: usize,
    keyframe: bool,
) -> Result<(Vec<u8>, PayloadCompression), String> {
    let compressed = if keyframe || raw_size >= COMPRESSION_THRESHOLD_BYTES {
        let mut encoder = flate2::write::ZlibEncoder::new(
            Vec::new(),
            flate2::Compression::new(if keyframe {
                REFERENCE_SNAPSHOT_ZLIB_LEVEL
            } else {
                REFERENCE_ZLIB_LEVEL
            }),
        );
        encoder
            .write_all(&raw)
            .map_err(|e| format!("zlib encode: {e}"))?;
        let mut bytes = encoder.finish().map_err(|e| format!("zlib finish: {e}"))?;
        if bytes.len() >= 2 {
            bytes[..2].copy_from_slice(&[0x78, 0x9c]);
        }
        Some(bytes)
    } else {
        None
    };
    match compressed {
        Some(c) if c.len() < raw_size => Ok((c, PayloadCompression::Zlib)),
        _ => Ok((raw, PayloadCompression::Raw)),
    }
}

fn dirty_block_count(width: u32, height: u32) -> u64 {
    (width.div_ceil(TILE_EDGE) as u64).saturating_mul(height.div_ceil(TILE_EDGE) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    const RGB555: [u32; 3] = [0x7c00, 0x03e0, 0x001f];

    #[test]
    fn rectangle_bytes_match_rgb555_storage_layout() {
        let pixels = [1, 2, 3, 4, 5, 6, 7, 8];
        let rect = ScreenRect {
            x: 1,
            y: 0,
            width: 1,
            height: 2,
        };
        let actual = build_cmd0_rect_stream(2, 2, 2, &pixels, &[rect]).unwrap();
        let expected = [
            0, // command 0
            1, 0, 0, 0, // x
            0, 0, 0, 0, // y
            1, 0, 0, 0, // width
            2, 0, 0, 0, // height
            4, 0, 0, 0, // pixel byte count
            3, 4, 7, 8, // packed 16-bit pixels from the selected rectangle
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn zlib_payload_inflates_to_the_same_rectangle_stream() {
        let width = 64;
        let height = 64;
        let pixels = vec![0x44; (width * height * 3) as usize];
        let (_encoder, frame) = ScreenEncoder::from_pixels(
            width,
            height,
            CaptureColorDepth::TrueColor,
            [0; 3],
            pixels.clone(),
            0,
        )
        .unwrap();
        assert_eq!(frame.compression, PayloadCompression::Zlib);
        let mut decoded = Vec::new();
        flate2::read::ZlibDecoder::new(frame.payload.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        let expected = build_cmd0_rect_stream(
            width,
            height,
            3,
            &pixels,
            &[ScreenRect {
                x: 0,
                y: 0,
                width,
                height,
            }],
        )
        .unwrap();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn png_encoder_writes_valid_rgb_chunks_and_converts_bgr24() {
        let png = encode_png_frame(1, 1, CaptureColorDepth::TrueColor, &[3, 2, 1]).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let mut offset = 8;
        let mut decoded = Vec::new();
        while offset + 12 <= png.len() {
            let length = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
            let kind = &png[offset + 4..offset + 8];
            let data_start = offset + 8;
            let data_end = data_start + length;
            let crc_end = data_end + 4;
            let mut crc = crc32fast::Hasher::new();
            crc.update(kind);
            crc.update(&png[data_start..data_end]);
            assert_eq!(
                crc.finalize(),
                u32::from_be_bytes(png[data_end..crc_end].try_into().unwrap())
            );
            if kind == b"IDAT" {
                flate2::read::ZlibDecoder::new(&png[data_start..data_end])
                    .read_to_end(&mut decoded)
                    .unwrap();
            }
            offset = crc_end;
        }
        assert_eq!(decoded, [0, 1, 2, 3]);
    }

    #[test]
    fn png_encoder_expands_rgb555_to_rgb8() {
        let white = 0x7fff_u16.to_le_bytes();
        let png = encode_png_frame(1, 1, CaptureColorDepth::HighColor, &white).unwrap();
        let mut decoded = Vec::new();
        let mut offset = 8;
        while offset + 12 <= png.len() {
            let length = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
            let kind = &png[offset + 4..offset + 8];
            let data_start = offset + 8;
            let data_end = data_start + length;
            if kind == b"IDAT" {
                flate2::read::ZlibDecoder::new(&png[data_start..data_end])
                    .read_to_end(&mut decoded)
                    .unwrap();
                break;
            }
            offset = data_end + 4;
        }
        assert_eq!(decoded, [0, 255, 255, 255]);
    }

    #[test]
    fn delta_advances_reference_and_skips_identical_frames() {
        let width = 64;
        let height = 64;
        let previous = vec![0; (width * height * 2) as usize];
        let (mut encoder, _) = ScreenEncoder::from_pixels(
            width,
            height,
            CaptureColorDepth::HighColor,
            RGB555,
            previous.clone(),
            0,
        )
        .unwrap();
        let mut current = previous;
        current[0] = 1;
        let all_tiles = [ScreenRect {
            x: 0,
            y: 0,
            width,
            height,
        }];
        let update = encoder
            .encode_tiles(100, &current, &all_tiles)
            .unwrap()
            .unwrap();
        assert_eq!(
            update.rects,
            [ScreenRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }]
        );
        assert!(encoder
            .encode_tiles(200, &current, &all_tiles)
            .unwrap()
            .is_none());
    }
}
