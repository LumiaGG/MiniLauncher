use std::fs;
use std::io::{self, Cursor, Read, Seek};
use std::path::Path;

/// 编译期嵌入的 payload.zip 压缩包
static EMBEDDED_PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));

/// 解压载荷到指定目录
/// progress_callback: (当前索引, 总文件数, 当前文件名) -> ()
pub fn extract_payload<F>(
    target_dir: &Path,
    custom_zip: Option<&Path>,
    portable_mode: bool,
    progress_callback: F,
) -> Result<(), String>
where
    F: FnMut(usize, usize, &str),
{
    fs::create_dir_all(target_dir)
        .map_err(|e| format!("无法创建目标安装目录 {}: {e}", target_dir.display()))?;

    if let Some(zip_path) = custom_zip {
        let f = fs::File::open(zip_path)
            .map_err(|e| format!("打开指定载荷压缩包失败 {}: {e}", zip_path.display()))?;
        let archive = zip::ZipArchive::new(f).map_err(|e| format!("解析 Zip 失败: {e}"))?;
        extract_archive(archive, target_dir, portable_mode, progress_callback)
    } else {
        let cursor = Cursor::new(EMBEDDED_PAYLOAD);
        let archive =
            zip::ZipArchive::new(cursor).map_err(|e| format!("解析内置 Zip 失败: {e}"))?;
        extract_archive(archive, target_dir, portable_mode, progress_callback)
    }
}

fn extract_archive<R, F>(
    mut archive: zip::ZipArchive<R>,
    target_dir: &Path,
    portable_mode: bool,
    mut progress_callback: F,
) -> Result<(), String>
where
    R: Read + Seek,
    F: FnMut(usize, usize, &str),
{
    let total = archive.len();
    for i in 0..total {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("读取压缩包文件 #{i} 失败: {e}"))?;

        let raw_name = file.name().replace('\\', "/");
        // 安全检查：防止 Zip Slip 路径穿越
        if raw_name.contains("..") {
            continue;
        }

        let out_path = target_dir.join(raw_name.trim_start_matches('/'));

        progress_callback(i + 1, total, file.name());

        if file.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|e| format!("创建目录失败 {}: {e}", out_path.display()))?;
        } else {
            if let Some(parent) = out_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent).map_err(|e| format!("创建父目录失败: {e}"))?;
                }
            }

            // 写入文件，若遇到目标文件被锁则最多重试 3 次
            let mut retries = 0;
            loop {
                match fs::File::create(&out_path) {
                    Ok(mut outfile) => {
                        io::copy(&mut file, &mut outfile)
                            .map_err(|e| format!("解压文件失败 {}: {e}", out_path.display()))?;
                        break;
                    }
                    Err(e) => {
                        retries += 1;
                        if retries > 3 {
                            return Err(format!("写入文件失败 {}: {e}", out_path.display()));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                }
            }
        }
    }

    // 处理便携模式
    if portable_mode {
        let portable_marker = target_dir.join("portable.toml");
        if !portable_marker.exists() {
            let _ = fs::write(&portable_marker, "enabled = true\n");
        }
        let data_dir = target_dir.join("data");
        let _ = fs::create_dir_all(&data_dir);
    }

    // 将安装器自身拷贝为 uninstall.exe
    if let Ok(current_exe) = std::env::current_exe() {
        let uninst_path = target_dir.join("uninstall.exe");
        let _ = fs::copy(&current_exe, &uninst_path);
    }

    Ok(())
}
