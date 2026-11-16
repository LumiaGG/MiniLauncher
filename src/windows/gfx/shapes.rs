use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;

pub fn colorref_to_d2d(c: u32, alpha: f32) -> D2D1_COLOR_F {
    let r = (c & 0xFF) as f32 / 255.0;
    let g = ((c >> 8) & 0xFF) as f32 / 255.0;
    let b = ((c >> 16) & 0xFF) as f32 / 255.0;
    // Brush and Clear colors are straight RGBA; Direct2D premultiplies them.
    D2D1_COLOR_F { r, g, b, a: alpha }
}

pub struct ClipGuard<'a>(&'a ID2D1RenderTarget);

impl<'a> ClipGuard<'a> {
    pub unsafe fn new(target: &'a ID2D1RenderTarget, rect: &D2D_RECT_F) -> Self {
        target.PushAxisAlignedClip(rect, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        Self(target)
    }
}

impl Drop for ClipGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            self.0.PopAxisAlignedClip();
        }
    }
}

pub unsafe fn fill_round_rect(
    target: &ID2D1RenderTarget,
    rect: &D2D_RECT_F,
    radius: f32,
    color: D2D1_COLOR_F,
) -> windows::core::Result<()> {
    let brush = crate::windows::gfx::GfxDevice::solid_brush(target, color)?;
    let rounded = D2D1_ROUNDED_RECT {
        rect: *rect,
        radiusX: radius,
        radiusY: radius,
    };
    target.FillRoundedRectangle(&rounded, &brush);
    Ok(())
}

pub unsafe fn stroke_round_rect(
    target: &ID2D1RenderTarget,
    rect: &D2D_RECT_F,
    radius: f32,
    color: D2D1_COLOR_F,
    stroke_width: f32,
) -> windows::core::Result<()> {
    let brush = crate::windows::gfx::GfxDevice::solid_brush(target, color)?;
    let rounded = D2D1_ROUNDED_RECT {
        rect: *rect,
        radiusX: radius,
        radiusY: radius,
    };
    target.DrawRoundedRectangle(&rounded, &brush, stroke_width, None);
    Ok(())
}

pub unsafe fn fill_rect(
    target: &ID2D1RenderTarget,
    rect: &D2D_RECT_F,
    color: D2D1_COLOR_F,
) -> windows::core::Result<()> {
    let brush = crate::windows::gfx::GfxDevice::solid_brush(target, color)?;
    target.FillRectangle(rect, &brush);
    Ok(())
}

pub unsafe fn draw_horizontal_line(
    target: &ID2D1RenderTarget,
    left: f32,
    right: f32,
    y: f32,
    height: f32,
    color: D2D1_COLOR_F,
) -> windows::core::Result<()> {
    fill_rect(
        target,
        &D2D_RECT_F {
            left,
            top: y,
            right,
            bottom: y + height,
        },
        color,
    )
}
