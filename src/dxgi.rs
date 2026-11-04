//! Minimal DXGI Desktop Duplication capture with dirty/move metadata and partial readback.

use std::mem::size_of;
use std::{thread, time::Duration};

use windows::{
    core::Interface,
    Win32::{
        Foundation::RECT,
        Graphics::{
            Direct3D::{
                D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
                D3D_FEATURE_LEVEL_11_0,
            },
            Direct3D11::{
                D3D11CreateDevice, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX,
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
                D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
            },
            Dxgi::{
                Common::{
                    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY, DXGI_SAMPLE_DESC,
                },
                CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1,
                IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_WAIT_TIMEOUT,
                DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_MOVE_RECT,
            },
        },
        UI::WindowsAndMessaging::{GetCursorInfo, CURSORINFO, CURSOR_SHOWING},
    },
};

use crate::{
    diff::{tiles_for_rectangles, TILE_EDGE},
    rect::{validate_rect, ScreenRect},
};

// Matches the 16-bit RGB555 layout consumed by the existing LREC player.
const RGB555_MASKS: [u32; 3] = [0x7c00, 0x03e0, 0x001f];

/// Fallback for drivers that do not report a cursor shape before the first
/// cursor update.
const CURSOR_FALLBACK_SIZE: u32 = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Pixel packing used for captured frames.
pub enum CaptureColorDepth {
    /// RGB555 packed into a little-endian 16-bit pixel.
    HighColor,
    /// Packed BGR24, with three bytes per pixel.
    TrueColor,
}

impl CaptureColorDepth {
    /// Returns the packed output depth in bits per pixel.
    pub const fn bits_per_pixel(self) -> u16 {
        match self {
            Self::HighColor => 16,
            Self::TrueColor => 24,
        }
    }

    /// Returns the packed pixel size in bytes.
    pub const fn bytes_per_pixel(self) -> usize {
        (self.bits_per_pixel() / 8) as usize
    }

    /// Returns channel masks for High Color, or zero masks for True Color.
    pub const fn color_masks(self) -> [u32; 3] {
        match self {
            Self::HighColor => RGB555_MASKS,
            Self::TrueColor => [0; 3],
        }
    }
}

/// Low-level owner of a DXGI duplicated output and its current packed pixels.
pub struct DxgiCapture {
    duplication: IDXGIOutputDuplication,
    context: ID3D11DeviceContext,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    color_depth: CaptureColorDepth,
    pixels: Vec<u8>,
    cursor_width: u32,
    cursor_height: u32,
}

impl DxgiCapture {
    /// Creates a duplication session for the selected desktop output.
    pub fn new(color_depth: CaptureColorDepth) -> Result<Self, String> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
            .map_err(|error| format!("تعذر إنشاء DXGI factory: {error}"))?;
        let (adapter1, output) = primary_output(&factory)?;
        let adapter: IDXGIAdapter = adapter1
            .cast()
            .map_err(|error| format!("تعذر فتح DXGI adapter: {error}"))?;

        let mut device = None;
        let mut context = None;
        let feature_levels = [
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|error| format!("تعذر إنشاء D3D11 device: {error}"))?;
        let device = device.ok_or_else(|| "D3D11 device غير متاح".to_owned())?;
        let context = context.ok_or_else(|| "D3D11 context غير متاح".to_owned())?;
        let duplication = unsafe { output.DuplicateOutput(&device) }
            .map_err(|error| format!("تعذر بدء DXGI Desktop Duplication: {error}"))?;
        let description = unsafe { duplication.GetDesc() };
        if description.Rotation != DXGI_MODE_ROTATION_IDENTITY {
            return Err("التقاط الشاشة المدورة غير مدعوم حاليًا".to_owned());
        }
        let width = description.ModeDesc.Width;
        let height = description.ModeDesc.Height;
        if width == 0 || height == 0 {
            return Err("DXGI أعاد أبعاد شاشة غير صالحة".to_owned());
        }
        let pixel_bytes = color_depth.bytes_per_pixel();
        let pixel_count = (width as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| "حجم صورة الشاشة تجاوز الحدود".to_owned())?;
        let buffer_len = pixel_count
            .checked_mul(pixel_bytes)
            .ok_or_else(|| "حجم صورة الشاشة تجاوز الحدود".to_owned())?;
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        unsafe { device.CreateTexture2D(&staging_desc, None, Some(&mut staging)) }
            .map_err(|error| format!("تعذر إنشاء مخزن DXGI المؤقت: {error}"))?;
        let staging = staging.ok_or_else(|| "DXGI لم ينشئ مخزن القراءة".to_owned())?;

        Ok(Self {
            duplication,
            context,
            staging,
            width,
            height,
            color_depth,
            pixels: vec![0; buffer_len],
            cursor_width: CURSOR_FALLBACK_SIZE,
            cursor_height: CURSOR_FALLBACK_SIZE,
        })
    }

    /// Returns the captured output dimensions in pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Returns the selected packed pixel format.
    pub fn color_depth(&self) -> CaptureColorDepth {
        self.color_depth
    }

    /// Returns the channel masks associated with the selected color format.
    pub fn color_masks(&self) -> [u32; 3] {
        self.color_depth.color_masks()
    }

    /// Returns the latest tightly packed CPU-side image buffer.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Captures a complete initial frame to seed the CPU-side image.
    /// Acquires and reads a complete initial frame.
    pub fn capture_initial(&mut self) -> Result<(), String> {
        thread::sleep(Duration::from_millis(50));
        let (resource, lease, info) = self
            .acquire_frame(5_000)
            .map_err(|error| format!("تعذر التقاط إطار DXGI الأول: {error}"))?;
        self.update_cursor_shape(info.PointerShapeBufferSize)?;
        let texture = resource
            .cast::<ID3D11Texture2D>()
            .map_err(|error| format!("تعذر فتح texture الشاشة: {error}"))?;
        let tiles = all_tiles(self.width, self.height);
        self.read_tiles(&texture, &tiles)?;
        drop(lease);

        if self.pixels.iter().all(|pixel| *pixel == 0) {
            for _ in 0..4 {
                let (resource, lease, info) = match self.acquire_frame(250) {
                    Ok(frame) => frame,
                    Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => break,
                    Err(error) => {
                        return Err(format!("تعذر تهيئة أول صورة من DXGI: {error}"));
                    }
                };
                self.update_cursor_shape(info.PointerShapeBufferSize)?;
                let texture = resource
                    .cast::<ID3D11Texture2D>()
                    .map_err(|error| format!("تعذر فتح texture الشاشة: {error}"))?;
                self.read_tiles(&texture, &tiles)?;
                drop(lease);
                if self.pixels.iter().any(|pixel| *pixel != 0) {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Applies move rectangles in memory, reads only dirty tiles from the GPU,
    /// and returns the tile rectangles that need an output update.
    pub fn capture_next(&mut self) -> Result<Option<Vec<ScreenRect>>, String> {
        let acquired = match self.acquire_frame(0) {
            Ok(frame) => frame,
            Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            Err(error) => return Err(format!("تعذر التقاط DXGI frame: {error}")),
        };
        let (resource, _lease, frame_info) = acquired;
        let texture = resource
            .cast::<ID3D11Texture2D>()
            .map_err(|error| format!("تعذر فتح texture الشاشة: {error}"))?;
        self.update_cursor_shape(frame_info.PointerShapeBufferSize)?;
        let metadata_size = frame_info.TotalMetadataBufferSize;
        if metadata_size == 0 {
            return Ok(None);
        }
        let moves = self.get_move_rectangles(metadata_size)?;
        let dirty = self
            .get_dirty_rectangles(metadata_size)?
            .into_iter()
            .map(rect_from_win32)
            .collect::<Result<Vec<_>, _>>()?;

        let cursor_area = if frame_info.LastMouseUpdateTime != 0
            && !frame_info.PointerPosition.Visible.as_bool()
            && cursor_may_be_showing()
        {
            cursor_mask_rect(
                frame_info.PointerPosition.Position.x,
                frame_info.PointerPosition.Position.y,
                self.cursor_width,
                self.cursor_height,
                self.width,
                self.height,
            )
        } else {
            None
        };

        let mut move_destinations = Vec::with_capacity(moves.len());
        let mut move_copies = Vec::with_capacity(moves.len());
        for movement in &moves {
            let source_width = movement
                .DestinationRect
                .right
                .checked_sub(movement.DestinationRect.left)
                .ok_or_else(|| "عرض move rect غير صالح".to_owned())?;
            let source_height = movement
                .DestinationRect
                .bottom
                .checked_sub(movement.DestinationRect.top)
                .ok_or_else(|| "ارتفاع move rect غير صالح".to_owned())?;
            let source = ScreenRect {
                x: u32::try_from(movement.SourcePoint.x)
                    .map_err(|_| "مصدر move rect خارج حدود الشاشة".to_owned())?,
                y: u32::try_from(movement.SourcePoint.y)
                    .map_err(|_| "مصدر move rect خارج حدود الشاشة".to_owned())?,
                width: u32::try_from(source_width)
                    .map_err(|_| "عرض move rect غير صالح".to_owned())?,
                height: u32::try_from(source_height)
                    .map_err(|_| "ارتفاع move rect غير صالح".to_owned())?,
            };
            let destination = rect_from_win32(movement.DestinationRect)?;
            validate_rect(source, self.width, self.height)?;
            validate_rect(destination, self.width, self.height)?;
            let bytes = copy_rect_from_buffer(
                &self.pixels,
                self.width,
                self.color_depth.bytes_per_pixel(),
                source,
            );
            move_destinations.push(destination);
            move_copies.push((destination, bytes));
        }
        for (destination, bytes) in move_copies {
            write_rect_to_buffer(
                &mut self.pixels,
                self.width,
                self.color_depth.bytes_per_pixel(),
                destination,
                &bytes,
            );
        }

        let saved_cursor_area = cursor_area.map(|rect| {
            copy_rect_from_buffer(
                &self.pixels,
                self.width,
                self.color_depth.bytes_per_pixel(),
                rect,
            )
        });
        let dirty_tiles = tiles_for_rectangles(self.width, self.height, &dirty)?;
        self.read_tiles(&texture, &dirty_tiles)?;
        if let (Some(rect), Some(bytes)) = (cursor_area, saved_cursor_area) {
            write_rect_to_buffer(
                &mut self.pixels,
                self.width,
                self.color_depth.bytes_per_pixel(),
                rect,
                &bytes,
            );
        }
        let mut changed_regions = move_destinations;
        changed_regions.extend(dirty);
        let changed_tiles = tiles_for_rectangles(self.width, self.height, &changed_regions)?;
        Ok(Some(changed_tiles))
    }

    fn acquire_frame(
        &self,
        timeout_ms: u32,
    ) -> Result<(IDXGIResource, FrameLease, DXGI_OUTDUPL_FRAME_INFO), windows::core::Error> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        unsafe {
            self.duplication
                .AcquireNextFrame(timeout_ms, &mut info, &mut resource)?;
        }
        let lease = FrameLease {
            duplication: self.duplication.clone(),
        };
        let resource = resource.ok_or_else(|| {
            windows::core::Error::new(
                windows::core::HRESULT(0x8000_4003_u32 as i32),
                "DXGI returned no frame resource",
            )
        })?;
        Ok((resource, lease, info))
    }

    fn get_move_rectangles(&self, buffer_size: u32) -> Result<Vec<DXGI_OUTDUPL_MOVE_RECT>, String> {
        let item_size = size_of::<DXGI_OUTDUPL_MOVE_RECT>();
        let count = (buffer_size as usize).div_ceil(item_size).max(1);
        let mut rectangles = vec![DXGI_OUTDUPL_MOVE_RECT::default(); count];
        let capacity = u32::try_from(rectangles.len() * item_size)
            .map_err(|_| "بيانات move rect أكبر من الحد المسموح".to_owned())?;
        let mut required = 0;
        unsafe {
            self.duplication
                .GetFrameMoveRects(capacity, rectangles.as_mut_ptr(), &mut required)
        }
        .map_err(|error| format!("تعذر قراءة move rects: {error}"))?;
        if required as usize % item_size != 0 {
            return Err("حجم بيانات move rect غير صالح".to_owned());
        }
        rectangles.truncate(required as usize / item_size);
        Ok(rectangles)
    }

    fn get_dirty_rectangles(&self, buffer_size: u32) -> Result<Vec<RECT>, String> {
        let item_size = size_of::<RECT>();
        let count = (buffer_size as usize).div_ceil(item_size).max(1);
        let mut rectangles = vec![RECT::default(); count];
        let capacity = u32::try_from(rectangles.len() * item_size)
            .map_err(|_| "بيانات dirty rect أكبر من الحد المسموح".to_owned())?;
        let mut required = 0;
        unsafe {
            self.duplication
                .GetFrameDirtyRects(capacity, rectangles.as_mut_ptr(), &mut required)
        }
        .map_err(|error| format!("تعذر قراءة dirty rects: {error}"))?;
        if required as usize % item_size != 0 {
            return Err("حجم بيانات dirty rect غير صالح".to_owned());
        }
        rectangles.truncate(required as usize / item_size);
        Ok(rectangles)
    }

    fn update_cursor_shape(&mut self, shape_buffer_size: u32) -> Result<(), String> {
        if shape_buffer_size == 0 {
            return Ok(());
        }
        let mut shape_bytes = vec![0_u8; shape_buffer_size as usize];
        let mut bytes_required = 0;
        let mut shape = windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        unsafe {
            self.duplication.GetFramePointerShape(
                shape_buffer_size,
                shape_bytes.as_mut_ptr().cast(),
                &mut bytes_required,
                &mut shape,
            )
        }
        .map_err(|error| format!("تعذر قراءة شكل مؤشر DXGI: {error}"))?;
        if bytes_required > shape_buffer_size {
            return Err("حجم شكل مؤشر DXGI تجاوز المخزن المخصص".to_owned());
        }
        let height = if shape.Type
            == windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32
        {
            shape.Height / 2
        } else {
            shape.Height
        };
        if shape.Width > 0 && height > 0 {
            self.cursor_width = shape.Width;
            self.cursor_height = height;
        }
        Ok(())
    }

    fn read_tiles(&mut self, source: &ID3D11Texture2D, tiles: &[ScreenRect]) -> Result<(), String> {
        if tiles.is_empty() {
            return Ok(());
        }
        for tile in tiles {
            validate_rect(*tile, self.width, self.height)?;
            let source_box = D3D11_BOX {
                left: tile.x,
                top: tile.y,
                front: 0,
                right: tile.x + tile.width,
                bottom: tile.y + tile.height,
                back: 1,
            };
            unsafe {
                self.context.CopySubresourceRegion(
                    &self.staging,
                    0,
                    tile.x,
                    tile.y,
                    0,
                    source,
                    0,
                    Some(&source_box),
                );
            }
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|error| format!("تعذر قراءة بلاطات DXGI: {error}"))?;
        let row_pitch = mapped.RowPitch as usize;
        let min_row_pitch = match (self.width as usize).checked_mul(4) {
            Some(value) => value,
            None => {
                unsafe { self.context.Unmap(&self.staging, 0) };
                return Err("حجم صف DXGI تجاوز الحدود".to_owned());
            }
        };
        if mapped.pData.is_null() || row_pitch < min_row_pitch {
            unsafe { self.context.Unmap(&self.staging, 0) };
            return Err("DXGI أعاد مخزن بكسلات غير صالح".to_owned());
        }
        let source_len = match row_pitch.checked_mul(self.height as usize) {
            Some(value) => value,
            None => {
                unsafe { self.context.Unmap(&self.staging, 0) };
                return Err("حجم DXGI mapped surface تجاوز الحدود".to_owned());
            }
        };
        if (mapped.DepthPitch as usize) < source_len {
            unsafe { self.context.Unmap(&self.staging, 0) };
            return Err("DXGI أعاد عمق مخزن أصغر من سطح الصورة".to_owned());
        }
        let bgra = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), source_len) };
        for tile in tiles {
            for row in 0..tile.height as usize {
                let source_start = (tile.y as usize + row) * row_pitch + tile.x as usize * 4;
                let destination_start = ((tile.y as usize + row) * self.width as usize
                    + tile.x as usize)
                    * self.color_depth.bytes_per_pixel();
                for column in 0..tile.width as usize {
                    let source_pixel = source_start + column * 4;
                    let destination =
                        destination_start + column * self.color_depth.bytes_per_pixel();
                    let blue = bgra[source_pixel];
                    let green = bgra[source_pixel + 1];
                    let red = bgra[source_pixel + 2];
                    match self.color_depth {
                        CaptureColorDepth::TrueColor => {
                            self.pixels[destination..destination + 3]
                                .copy_from_slice(&[blue, green, red]);
                        }
                        CaptureColorDepth::HighColor => {
                            // توحيد الخلفية البيضاء وتخفيف ضجيج ClearType لرفع نسبة ضغط Zlib
                            let (r5, g5, b5) = if red > 246 && green > 246 && blue > 246 {
                                (31_u16, 31_u16, 31_u16)
                            } else if red < 8 && green < 8 && blue < 8 {
                                (0_u16, 0_u16, 0_u16)
                            } else {
                                (
                                    u16::from((red >> 3) & 0x1e),
                                    u16::from((green >> 3) & 0x1e),
                                    u16::from((blue >> 3) & 0x1e),
                                )
                            };
                            let value = (r5 << 10) | (g5 << 5) | b5;
                            self.pixels[destination..destination + 2]
                                .copy_from_slice(&value.to_le_bytes());
                        }
                    }
                }
            }
        }
        unsafe { self.context.Unmap(&self.staging, 0) };
        Ok(())
    }
}

struct FrameLease {
    duplication: IDXGIOutputDuplication,
}

impl Drop for FrameLease {
    fn drop(&mut self) {
        let _ = unsafe { self.duplication.ReleaseFrame() };
    }
}

fn primary_output(factory: &IDXGIFactory1) -> Result<(IDXGIAdapter1, IDXGIOutput1), String> {
    let mut fallback = None;
    for adapter_index in 0..32 {
        let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(_) => break,
        };
        for output_index in 0..32 {
            let output = match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(output) => output,
                Err(_) => break,
            };
            let description = unsafe { output.GetDesc() }
                .map_err(|error| format!("تعذر قراءة DXGI output: {error}"))?;
            if !description.AttachedToDesktop.as_bool() {
                continue;
            }
            let output1 = output
                .cast::<IDXGIOutput1>()
                .map_err(|error| format!("تعذر فتح IDXGIOutput1: {error}"))?;
            if description.DesktopCoordinates.left == 0 && description.DesktopCoordinates.top == 0 {
                return Ok((adapter, output1));
            }
            if fallback.is_none() {
                fallback = Some((adapter.clone(), output1));
            }
        }
    }
    fallback.ok_or_else(|| "لم يتم العثور على شاشة DXGI متصلة".to_owned())
}

fn all_tiles(width: u32, height: u32) -> Vec<ScreenRect> {
    let columns = width.div_ceil(TILE_EDGE);
    let rows = height.div_ceil(TILE_EDGE);
    let mut tiles = Vec::with_capacity((columns * rows) as usize);
    for tile_y in 0..rows {
        for tile_x in 0..columns {
            let x = tile_x * TILE_EDGE;
            let y = tile_y * TILE_EDGE;
            tiles.push(ScreenRect {
                x,
                y,
                width: TILE_EDGE.min(width - x),
                height: TILE_EDGE.min(height - y),
            });
        }
    }
    tiles
}

fn cursor_mask_rect(
    cursor_x: i32,
    cursor_y: i32,
    cursor_width: u32,
    cursor_height: u32,
    screen_width: u32,
    screen_height: u32,
) -> Option<ScreenRect> {
    let left = i64::from(cursor_x).clamp(0, i64::from(screen_width));
    let top = i64::from(cursor_y).clamp(0, i64::from(screen_height));
    let right = i64::from(cursor_x)
        .saturating_add(i64::from(cursor_width))
        .clamp(0, i64::from(screen_width));
    let bottom = i64::from(cursor_y)
        .saturating_add(i64::from(cursor_height))
        .clamp(0, i64::from(screen_height));
    if right <= left || bottom <= top {
        return None;
    }
    Some(ScreenRect {
        x: left as u32,
        y: top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

fn cursor_may_be_showing() -> bool {
    let mut info = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetCursorInfo(&mut info) }
        .map(|()| (info.flags.0 & CURSOR_SHOWING.0) != 0)
        .unwrap_or(true)
}

fn rect_from_win32(rect: RECT) -> Result<ScreenRect, String> {
    let x = u32::try_from(rect.left).map_err(|_| "move rect خارج حدود الشاشة".to_owned())?;
    let y = u32::try_from(rect.top).map_err(|_| "move rect خارج حدود الشاشة".to_owned())?;
    let right = u32::try_from(rect.right).map_err(|_| "move rect غير صالح".to_owned())?;
    let bottom = u32::try_from(rect.bottom).map_err(|_| "move rect غير صالح".to_owned())?;
    if right <= x || bottom <= y {
        return Err("DXGI أعاد مستطيلًا فارغًا أو معكوسًا".to_owned());
    }
    Ok(ScreenRect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

fn copy_rect_from_buffer(
    pixels: &[u8],
    screen_width: u32,
    bytes_per_pixel: usize,
    rect: ScreenRect,
) -> Vec<u8> {
    let row_bytes = rect.width as usize * bytes_per_pixel;
    let mut copy = Vec::with_capacity(row_bytes * rect.height as usize);
    for row in 0..rect.height as usize {
        let start =
            ((rect.y as usize + row) * screen_width as usize + rect.x as usize) * bytes_per_pixel;
        copy.extend_from_slice(&pixels[start..start + row_bytes]);
    }
    copy
}

fn write_rect_to_buffer(
    pixels: &mut [u8],
    screen_width: u32,
    bytes_per_pixel: usize,
    rect: ScreenRect,
    source: &[u8],
) {
    let row_bytes = rect.width as usize * bytes_per_pixel;
    for row in 0..rect.height as usize {
        let source_start = row * row_bytes;
        let destination_start =
            ((rect.y as usize + row) * screen_width as usize + rect.x as usize) * bytes_per_pixel;
        pixels[destination_start..destination_start + row_bytes]
            .copy_from_slice(&source[source_start..source_start + row_bytes]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_mask_is_clipped_to_screen_edges() {
        assert_eq!(
            cursor_mask_rect(100, 100, 32, 48, 500, 400),
            Some(ScreenRect {
                x: 100,
                y: 100,
                width: 32,
                height: 48,
            })
        );
        assert_eq!(
            cursor_mask_rect(-20, -10, 48, 48, 500, 400),
            Some(ScreenRect {
                x: 0,
                y: 0,
                width: 28,
                height: 38,
            })
        );
        assert_eq!(cursor_mask_rect(600, 100, 48, 48, 500, 400), None);
    }
}
