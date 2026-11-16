use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=MINILAUNCHER_PAYLOAD_ZIP");
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    // 1. 检查或生成 payload.zip
    let payload_path = output.join("payload.zip");
    if let Ok(custom_zip) = env::var("MINILAUNCHER_PAYLOAD_ZIP") {
        let custom_path = PathBuf::from(custom_zip);
        if custom_path.exists() {
            fs::copy(&custom_path, &payload_path).unwrap();
        } else {
            create_dummy_zip(&payload_path);
        }
    } else if !payload_path.exists() {
        create_dummy_zip(&payload_path);
    }

    // 2. 生成图标与 Manifest (启用 ComCtl32 v6 现代视觉样式与 PerMonitorV2 DPI)
    let icon = output.join("setup.ico");
    fs::write(&icon, make_icon()).unwrap();

    let manifest = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity version="1.0.0.0" processorArchitecture="*" name="MiniLauncher.Installer" type="win32"/>
  <description>MiniLauncher Setup</description>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>"#;
    let manifest_file = output.join("setup.manifest");
    fs::write(&manifest_file, manifest).unwrap();

    let resource = output.join("setup.rc");
    fs::write(
        &resource,
        format!(
            "1 ICON \"{}\"\n1 24 \"{}\"\n",
            icon.display().to_string().replace('\\', "/"),
            manifest_file.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();

    embed_resource::compile(resource, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}

fn create_dummy_zip(path: &PathBuf) {
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("README.txt", options).unwrap();
    use std::io::Write;
    zip.write_all(b"MiniLauncher Payload Placeholder").unwrap();
    zip.finish().unwrap();
}

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
