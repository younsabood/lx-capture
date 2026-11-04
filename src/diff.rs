//! Maps DXGI dirty and moved rectangles onto a fixed 32×32 tile grid.

use crate::rect::{validate_rect, ScreenRect};

pub const TILE_EDGE: u32 = 32;

pub fn tiles_for_rectangles(
    screen_width: u32,
    screen_height: u32,
    rectangles: &[ScreenRect],
) -> Result<Vec<ScreenRect>, String> {
    if screen_width == 0 || screen_height == 0 {
        return Err("screen dimensions must be nonzero".to_owned());
    }
    let columns = screen_width.div_ceil(TILE_EDGE);
    let rows = screen_height.div_ceil(TILE_EDGE);
    let tile_count = (columns as usize)
        .checked_mul(rows as usize)
        .ok_or_else(|| "tile grid dimensions overflow".to_owned())?;
    let mut dirty = vec![false; tile_count];

    for &rect in rectangles {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        validate_rect(rect, screen_width, screen_height)?;
        let first_x = rect.x / TILE_EDGE;
        let first_y = rect.y / TILE_EDGE;
        let last_x = (rect.x + rect.width - 1) / TILE_EDGE;
        let last_y = (rect.y + rect.height - 1) / TILE_EDGE;
        for tile_y in first_y..=last_y {
            for tile_x in first_x..=last_x {
                dirty[(tile_y * columns + tile_x) as usize] = true;
            }
        }
    }

    let mut tiles = Vec::new();
    for tile_y in 0..rows {
        for tile_x in 0..columns {
            if !dirty[(tile_y * columns + tile_x) as usize] {
                continue;
            }
            let x = tile_x * TILE_EDGE;
            let y = tile_y * TILE_EDGE;
            tiles.push(ScreenRect {
                x,
                y,
                width: TILE_EDGE.min(screen_width - x),
                height: TILE_EDGE.min(screen_height - y),
            });
        }
    }
    Ok(tiles)
}

/// Coalesces edge-adjacent changed rectangles without including unchanged gaps.
pub fn merge_adjacent_rects(rectangles: &[ScreenRect]) -> Vec<ScreenRect> {
    let mut merged = rectangles.to_vec();
    loop {
        let before = merged.len();
        merged.sort_unstable_by_key(|rect| (rect.y, rect.height, rect.x));
        let mut horizontal: Vec<ScreenRect> = Vec::with_capacity(merged.len());
        for rect in merged.drain(..) {
            if let Some(previous) = horizontal.last_mut() {
                if previous.y == rect.y
                    && previous.height == rect.height
                    && previous.x.checked_add(previous.width) == Some(rect.x)
                {
                    previous.width += rect.width;
                    continue;
                }
            }
            horizontal.push(rect);
        }

        horizontal.sort_unstable_by_key(|rect| (rect.x, rect.width, rect.y));
        for rect in horizontal {
            if let Some(previous) = merged.last_mut() {
                if previous.x == rect.x
                    && previous.width == rect.width
                    && previous.y.checked_add(previous.height) == Some(rect.y)
                {
                    previous.height += rect.height;
                    continue;
                }
            }
            merged.push(rect);
        }
        if merged.len() == before {
            break;
        }
    }
    merged.sort_unstable_by_key(|rect| (rect.y, rect.x));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_four_touching_tiles_into_one_rectangle() {
        let tiles = [
            ScreenRect {
                x: 0,
                y: 0,
                width: 32,
                height: 32,
            },
            ScreenRect {
                x: 32,
                y: 0,
                width: 32,
                height: 32,
            },
            ScreenRect {
                x: 0,
                y: 32,
                width: 32,
                height: 32,
            },
            ScreenRect {
                x: 32,
                y: 32,
                width: 32,
                height: 32,
            },
        ];
        assert_eq!(
            merge_adjacent_rects(&tiles),
            [ScreenRect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            }]
        );
    }
}
