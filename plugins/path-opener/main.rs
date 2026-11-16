//! A keyword-triggered directory browser using the external plugin protocol.
use mini_launcher::plugin_rpc::MAX_FRAME;
use serde_json::{json, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

fn unquote(mut text: &str) -> &str {
    text = text.trim();
    loop {
        let pair = [('"', '"'), ('\'', '\''), ('“', '”'), ('‘', '’')]
            .into_iter()
            .find(|&(left, right)| {
                text.starts_with(left)
                    && text.ends_with(right)
                    && text.len() >= left.len_utf8() + right.len_utf8()
            });
        let Some((left, right)) = pair else {
            return text;
        };
        text = &text[left.len_utf8()..text.len() - right.len_utf8()];
    }
}

fn decode_uri(text: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut source = text.as_bytes().iter().copied();
    while let Some(byte) = source.next() {
        if byte == b'%' {
            let high = source.next().and_then(|b| (b as char).to_digit(16));
            let low = source.next().and_then(|b| (b as char).to_digit(16));
            let (Some(high), Some(low)) = (high, low) else {
                return Err("file:// 地址包含无效的百分号编码".into());
            };
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).map_err(|_| "file:// 地址不是有效的 UTF-8".into())
}

fn normalize(query: &str, home: &Path) -> Result<PathBuf, String> {
    let mut text = unquote(query).to_owned();
    if text
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
    {
        let uri = text[7..].replace('\\', "/");
        text = if uri.starts_with('/') {
            decode_uri(uri.trim_start_matches('/'))?
        } else if uri.as_bytes().get(1) == Some(&b':') {
            decode_uri(&uri)?
        } else if uri
            .get(..10)
            .is_some_and(|s| s.eq_ignore_ascii_case("localhost/"))
        {
            decode_uri(&uri[10..])?
        } else {
            format!("\\\\{}", decode_uri(&uri)?)
        };
    } else {
        let mut expanded = String::new();
        let mut rest = text.as_str();
        while let Some(start) = rest.find('%') {
            expanded.push_str(&rest[..start]);
            let Some(end) = rest[start + 1..].find('%') else {
                rest = &rest[start..];
                break;
            };
            let name = &rest[start + 1..start + 1 + end];
            expanded.push_str(&std::env::var(name).unwrap_or_else(|_| format!("%{name}%")));
            rest = &rest[start + end + 2..];
        }
        expanded.push_str(rest);
        text = expanded;
    }
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err("请输入有效的文件或文件夹路径".into());
    }
    text = text.replace('/', "\\");
    // Collapse pasted/escaped separators while retaining UNC and device prefixes.
    let unc = text.starts_with("\\\\");
    let mut normalized = String::new();
    for character in text.chars() {
        if character != '\\' || !normalized.ends_with('\\') {
            normalized.push(character);
        }
    }
    if unc {
        normalized.insert(0, '\\');
    }
    if normalized == "~" {
        return Ok(home.to_path_buf());
    }
    let path = if let Some(relative) = normalized.strip_prefix("~\\") {
        home.join(relative)
    } else {
        let path = PathBuf::from(normalized);
        if path.is_absolute() {
            path
        } else {
            home.join(path)
        }
    };
    let path = std::path::absolute(path).map_err(|error| error.to_string())?;
    if path.to_string_lossy().len() > 32768 {
        return Err("路径超出插件协议的长度限制".into());
    }
    Ok(path)
}

fn display_text(text: &str, maximum: usize) -> &str {
    let mut end = text.len().min(maximum);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn item(path: &Path, title: String) -> Value {
    let target = path.to_string_lossy().into_owned();
    json!({
        "title": title,
        "subtitle": display_text(&target, 4096),
        "icon": &target,
        "action": {"type":"open", "target":&target},
        "context_actions": [{"title":"复制路径", "action":{"type":"copy", "text":&target}}]
    })
}

fn query(text: &str, maximum: usize, home: &Path) -> Result<Vec<Value>, String> {
    if text.trim().is_empty() || maximum == 0 {
        return Ok(Vec::new());
    }
    let path = normalize(text, home)?;
    let metadata =
        fs::metadata(&path).map_err(|error| format!("无法访问 {}：{error}", path.display()))?;
    let directory = if metadata.is_dir() {
        path.as_path()
    } else {
        path.parent().ok_or("无法确定文件所在文件夹")?
    };
    let mut first = item(directory, "打开当前文件夹".into());
    let mut entries = Vec::new();
    match fs::read_dir(directory) {
        Ok(reader) => {
            for entry in reader {
                let entry = entry.map_err(|error| format!("读取文件夹失败：{error}"))?;
                let file = entry.file_type().map(|t| !t.is_dir()).unwrap_or(true);
                let name = entry.file_name().to_string_lossy().into_owned();
                let name_lower = name.to_lowercase();
                let path = entry.path();
                entries.push((file, name_lower, name, path));
            }
        }
        Err(error) => {
            // Opening a known directory remains available even when listing is denied.
            first["subtitle"] = json!(display_text(
                &format!("{} · 无法列出内容：{error}", directory.display()),
                4096
            ));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    if !metadata.is_dir() {
        let selected = path.to_string_lossy().to_lowercase();
        entries.retain(|(_, _, _, candidate)| {
            candidate.to_string_lossy().to_lowercase() != selected
        });
        entries.insert(0, (true, String::new(), "打开该文件".into(), path.clone()));
    }
    let mut items = vec![first];
    // Leave room for the RPC envelope; repeated paths in actions count toward the frame.
    let mut bytes = serde_json::to_vec(&items[0]).unwrap().len() + 1024;
    for (_, _, name, path) in entries {
        if items.len() >= maximum.min(100) {
            break;
        }
        let entry = item(&path, name);
        let size = serde_json::to_vec(&entry).unwrap().len() + 1;
        if bytes + size >= MAX_FRAME {
            break;
        }
        // Protocol field limits are byte-based, including Unicode paths.
        if entry["title"].as_str().unwrap().len() > 1024
            || entry["action"]["target"].as_str().unwrap().len() > 32768
        {
            continue;
        }
        bytes += size;
        items.push(entry);
    }
    Ok(items)
}

fn main() {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\"));
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let _ = mini_launcher::plugin_rpc::serve(&mut input, &mut output, |request| {
        match request["method"].as_str().unwrap_or("") {
            "initialize" => Ok(json!({"protocol_version":2})),
            "query" => query(
                request["params"]["query"].as_str().unwrap_or(""),
                request["params"]["max_results"]
                    .as_u64()
                    .unwrap_or(100)
                    .min(100) as usize,
                &home,
            )
            .map(|items| json!({"items":items}))
            .or_else(|error| {
                if request["params"]["is_global"].as_bool() == Some(true) {
                    // Incomplete paths during typing should leave normal search usable.
                    Ok(json!({"items":[]}))
                } else {
                    Err(error)
                }
            }),
            _ => Err("不支持的方法".into()),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_pasted_windows_paths() {
        let home = Path::new(r"C:\Users\Test");
        for input in [
            r#" "D:/Some Folder\中文/file.txt" "#,
            r"D:\\Some Folder\\中文\\file.txt",
            "‘D:/Some Folder/中文/file.txt’",
            "file:///D:/Some%20Folder/%E4%B8%AD%E6%96%87/file.txt",
        ] {
            assert_eq!(
                normalize(input, home).unwrap(),
                Path::new(r"D:\Some Folder\中文\file.txt")
            );
        }
        assert_eq!(normalize(r"~\Desktop", home).unwrap(), home.join("Desktop"));
        assert_eq!(
            normalize(r"%USERPROFILE%", home).unwrap(),
            PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        );
        assert_eq!(
            normalize("file://server/share/a%20b", home).unwrap(),
            Path::new(r"\\server\share\a b")
        );
        assert_eq!(
            normalize("//server//share/a", home).unwrap(),
            Path::new(r"\\server\share\a")
        );
        assert_eq!(
            normalize(r"\\?\D:\中文", home).unwrap(),
            Path::new(r"\\?\D:\中文")
        );
        assert_eq!(normalize("D:/", home).unwrap(), Path::new(r"D:\"));
        assert!(normalize("file:///D:/bad%xx", home).is_err());
        assert!(normalize("D:\0oops", home).is_err());
    }

    #[test]
    fn file_and_directory_queries_keep_directory_first() {
        let root = std::env::temp_dir().join(format!("mini-path-opener-{}", std::process::id()));
        fs::create_dir_all(root.join("z-folder")).unwrap();
        let file = root.join("a 中文.txt");
        fs::write(&file, "test").unwrap();
        let other = root.join("0-other.txt");
        fs::write(&other, "other").unwrap();
        for (input, selected_file) in [
            (root.to_str().unwrap().to_owned(), false),
            (format!("\"{}\"", file.display()), true),
        ] {
            let items = query(&input, 100, &root).unwrap();
            assert_eq!(items.len(), 4);
            assert_eq!(items[0]["action"]["target"], root.to_str().unwrap());
            if selected_file {
                assert_eq!(items[1]["title"], "打开该文件");
                assert_eq!(items[1]["action"]["type"], "open");
                assert_eq!(items[1]["action"]["target"], file.to_str().unwrap());
                assert_eq!(items[2]["title"], "z-folder");
                assert_eq!(items[3]["action"]["target"], other.to_str().unwrap());
            } else {
                assert_eq!(items[1]["title"], "z-folder");
                assert_eq!(items[2]["action"]["target"], other.to_str().unwrap());
                assert_eq!(items[3]["action"]["target"], file.to_str().unwrap());
            }
            assert_eq!(query(&input, 1, &root).unwrap().len(), 1);
            let limited = query(&input, 2, &root).unwrap();
            assert_eq!(limited.len(), 2);
            assert_eq!(limited[1], items[1]);
        }
        let case_variant = root.join("A 中文.TXT");
        let items = query(case_variant.to_str().unwrap(), 100, &root).unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[1]["action"]["target"], case_variant.to_str().unwrap());
        assert!(query(root.join("missing").to_str().unwrap(), 100, &root).is_err());
        assert!(query("", 100, &root).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
