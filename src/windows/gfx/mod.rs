pub mod device;
pub mod image;
pub mod shapes;
pub mod surface;
pub mod text;
pub mod window;

#[cfg(test)]
mod benchmark;

pub use device::GfxDevice;
pub use image::{BitmapImage, ImageRenderer};
pub use shapes::{
    colorref_to_d2d, draw_horizontal_line, fill_rect, fill_round_rect, stroke_round_rect,
};
pub use surface::CompositionSurface;
pub use text::{FontRole, TextEngine};
pub use window::WindowRenderer;
