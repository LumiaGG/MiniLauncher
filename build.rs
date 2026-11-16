use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=resources/app.manifest");
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let icon = output.join("launcher.ico");
    fs::write(&icon, make_icon()).unwrap();
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("resources/app.manifest");
    let resource = output.join("launcher.rc");
    fs::write(
        &resource,
        format!(
            "1 ICON \"{}\"\n1 24 \"{}\"\n",
            icon.display().to_string().replace('\\', "/"),
            manifest.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    embed_resource::compile(resource, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}

// A small code-native Fluent tile icon; no bitmap or graphics dependency at runtime.
fn make_icon() -> Vec<u8> {
    let sizes = [16u32, 32, 48, 64];
    let mut images = Vec::new();
    for size in sizes {
        let mask_stride = size.div_ceil(32) * 4;
        let image_bytes = size * size * 4;
        let mut image = Vec::new();
        for value in [40u32, size, size * 2] {
            image.extend(value.to_le_bytes());
        }
        image.extend(1u16.to_le_bytes());
        image.extend(32u16.to_le_bytes());
        for value in [0, image_bytes, 0, 0, 0, 0] {
            image.extend(value.to_le_bytes());
        }
        for y in (0..size).rev() {
            for x in 0..size {
                let mut blue = 0.0;
                let mut green = 0.0;
                let mut red = 0.0;
                let mut alpha = 0.0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let u = (x as f64 + (sx as f64 + 0.5) / 4.0) * 32.0 / size as f64;
                        let v = (y as f64 + (sy as f64 + 0.5) / 4.0) * 32.0 / size as f64;
                        let dx = (u - 16.0).abs() - 8.0;
                        let dy = (v - 16.0).abs() - 8.0;
                        if dx.max(0.0).hypot(dy.max(0.0)) > 6.0 {
                            continue;
                        }
                        let tile_x = (8.0..14.5).contains(&u) || (17.5..24.0).contains(&u);
                        let tile_y = (8.0..14.5).contains(&v) || (17.5..24.0).contains(&v);
                        let tile = tile_x && tile_y;
                        red += if tile { 255.0 } else { 0.0 };
                        green += if tile { 255.0 } else { 118.0 + v };
                        blue += if tile { 255.0 } else { 220.0 + v / 2.0 };
                        alpha += 1.0;
                    }
                }
                if alpha == 0.0 {
                    image.extend([0, 0, 0, 0]);
                } else {
                    image.extend([
                        (blue / alpha) as u8,
                        (green / alpha) as u8,
                        (red / alpha) as u8,
                        (alpha / 16.0 * 255.0) as u8,
                    ]);
                }
            }
        }
        image.resize(image.len() + (mask_stride * size) as usize, 0);
        images.push(image);
    }
    let mut icon = vec![0, 0, 1, 0, 4, 0];
    let mut offset = 6 + 16 * sizes.len();
    for (size, image) in sizes.into_iter().zip(&images) {
        icon.extend([size as u8, size as u8, 0, 0]);
        icon.extend(1u16.to_le_bytes());
        icon.extend(32u16.to_le_bytes());
        icon.extend((image.len() as u32).to_le_bytes());
        icon.extend((offset as u32).to_le_bytes());
        offset += image.len();
    }
    for image in images {
        icon.extend(image);
    }
    icon
}
