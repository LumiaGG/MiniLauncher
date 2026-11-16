use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    // 生成专属设置齿轮 Fluent 图标
    let icon = output.join("config.ico");
    fs::write(&icon, make_config_icon()).unwrap();

    let resource = output.join("config.rc");
    fs::write(
        &resource,
        format!(
            "1 ICON \"{}\"\n",
            icon.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();

    // gpui 框架本身已包含 Windows 运行时清单 (manifest)，这里仅嵌入应用程序图标
    let _ = embed_resource::compile(resource, embed_resource::NONE);
}

// 纯代码生成多分辨率 Windows 11 Fluent 风格设置齿轮图标
fn make_config_icon() -> Vec<u8> {
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
                // 4x4 超采样抗锯齿
                for sy in 0..4 {
                    for sx in 0..4 {
                        let u = (x as f64 + (sx as f64 + 0.5) / 4.0) * 32.0 / size as f64;
                        let v = (y as f64 + (sy as f64 + 0.5) / 4.0) * 32.0 / size as f64;
                        let dx = (u - 16.0).abs() - 8.0;
                        let dy = (v - 16.0).abs() - 8.0;
                        // 背景圆角方块
                        if dx.max(0.0).hypot(dy.max(0.0)) > 6.0 {
                            continue;
                        }

                        // 计算齿轮距离与角度
                        let cx = u - 16.0;
                        let cy = v - 16.0;
                        let dist = cx.hypot(cy);
                        let angle = cy.atan2(cx); // -PI ~ PI

                        // 6 齿齿轮轮廓调制
                        let teeth = 6.0;
                        let tooth_wave = (angle * teeth).cos();
                        let r_outer = 9.2 + tooth_wave * 1.5;
                        let r_hole = 3.6;

                        let is_gear = dist <= r_outer && dist >= r_hole;

                        if is_gear {
                            // 齿轮主体：明亮半透明微光白
                            red += 255.0;
                            green += 255.0;
                            blue += 255.0;
                        } else {
                            // 背景底色：Fluent 雅致 Slate 蓝紫色渐变
                            red += 0.0 + (v * 1.5);
                            green += 120.0 + (v * 2.0);
                            blue += 215.0 + (v * 0.8);
                        }
                        alpha += 1.0;
                    }
                }
                if alpha == 0.0 {
                    image.extend([0, 0, 0, 0]);
                } else {
                    image.extend([
                        (blue / alpha).clamp(0.0, 255.0) as u8,
                        (green / alpha).clamp(0.0, 255.0) as u8,
                        (red / alpha).clamp(0.0, 255.0) as u8,
                        (alpha / 16.0 * 255.0).clamp(0.0, 255.0) as u8,
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
