#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Integer pixel coordinates for a rectangle on the captured output.
pub struct ScreenRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Validates that `rect` fits within an image of `width` × `height` pixels.
pub fn validate_rect(rect: ScreenRect, width: u32, height: u32) -> Result<(), String> {
    let right = rect
        .x
        .checked_add(rect.width)
        .ok_or_else(|| "مستطيل خارج حدود العرض".to_owned())?;
    let bottom = rect
        .y
        .checked_add(rect.height)
        .ok_or_else(|| "مستطيل خارج حدود الارتفاع".to_owned())?;
    if right > width || bottom > height {
        return Err("مستطيل خارج حدود الشاشة".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rect_accepts_in_bounds() {
        let r = ScreenRect {
            x: 0,
            y: 0,
            width: 32,
            height: 32,
        };
        assert!(validate_rect(r, 640, 480).is_ok());
    }

    #[test]
    fn validate_rect_rejects_overflow() {
        let r = ScreenRect {
            x: 600,
            y: 0,
            width: 100,
            height: 10,
        };
        assert!(validate_rect(r, 640, 480).is_err());
    }
}
