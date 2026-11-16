//! Optional demo: scripts/install-text-panel-demo.ps1 builds and packages this example.
use mini_launcher::plugin_rpc;
use serde_json::{json, Value};
use std::{collections::HashMap, io};

const HELP: &str = "在搜索框输入以下命令：\n\ntextdemo single — 单区块\ntextdemo compare — 横向双栏\ntextdemo four — 自动布局四区块\ntextdemo vertical — 纵向四区块\ntextdemo unequal — 长短内容对照\ntextdemo long — 两个可独立滚动的长文区\ntextdemo unicode — 多语言、Emoji 和换行\ntextdemo static — 可选与静态正文混排\ntextdemo blank — 空正文占位\ntextdemo empty — 无区块的空状态\ntextdemo loading — 加载状态，重试后完成\ntextdemo error — 错误状态，重试后恢复\ntextdemo retained — 保留正文并提示错误\ntextdemo append — 重试追加日志\ntextdemo replace — 重试替换正文\ntextdemo actions — 整项复制、菜单与标题拖出\ntextdemo limit — 单块接近 64 KiB\n\n各场景独立展示，避免多个面板挤占可读空间。";
const INTERACTION: &str = "鼠标拖选正文，Ctrl+C 复制选区。\nCtrl+A 全选当前区块。\n搜索框 Tab 进入正文，Shift+Tab 从最后一块进入。\n正文 Tab / Shift+Tab 切换区块，越界返回搜索框。\nEsc / Ctrl+F 返回搜索框；直接输入或粘贴会继续搜索。\n正文 Enter 不执行结果；搜索框 Enter 复制整项。\n\n右键面板总标题查看插件扩展菜单；从总标题拖出整项文本。";
const SCENES: &[&str] = &[
    "help", "single", "compare", "four", "vertical", "unequal", "long", "unicode", "static",
    "blank", "empty", "loading", "error", "retained", "append", "replace", "actions", "limit",
];

#[derive(Default)]
struct Demo {
    // Counters are bounded by the fixed scene names, never by arbitrary user input.
    visits: HashMap<&'static str, usize>,
}

fn block(id: &str, title: &str, text: impl AsRef<str>) -> Value {
    json!({"id":id,"title":title,"text":text.as_ref()})
}

fn lines(count: usize) -> String {
    (1..=count)
        .map(|i| format!("{i:03}  长文滚动示例 · 中文与 English 😀\n这一行用于观察自动换行，以及拖选跨行文本时的复制结果。\n"))
        .collect()
}

fn item(scene: &str, title: &str, panel: Value) -> Value {
    let blocks = panel["blocks"].as_array().cloned().unwrap_or_default();
    // Preserve genuinely empty bodies; never copy the host's placeholder.
    let text = blocks
        .iter()
        .map(|b| b["text"].as_str().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n\n");
    // Avoid repeating a near-limit body four times in one JSON frame. A single
    // block's copy action already copies the entire item.
    let mut actions = Vec::new();
    if blocks.len() != 1 {
        actions.push(json!({
            "title":"复制整项正文", "action":{"type":"copy","text":text}
        }));
    }
    for b in &blocks {
        actions.push(json!({
            "title":format!("复制：{}", b["title"].as_str().unwrap_or("正文")),
            "action":{"type":"copy","text":b["text"]}
        }));
    }
    json!({
        "id":format!("text-demo:{scene}"), "title":title,
        "subtitle":"Tab 进入正文 · Ctrl+C 复制选区 · 在总标题上右键查看扩展动作",
        "action":{"type":"copy","text":text},
        "context_actions":actions, "text_panel":panel
    })
}

impl Demo {
    fn query(&mut self, params: &Value) -> Value {
        if params["max_results"].as_u64() == Some(0) {
            return json!({"items":[]});
        }
        let query = params["query"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let unknown = !query.is_empty() && !SCENES.contains(&query.as_str());
        let scene = if unknown || query.is_empty() {
            "help"
        } else {
            query.as_str()
        };
        let (title, panel) = match scene {
            "single" => (
                "单区块 · 选择与复制",
                json!({"blocks":[
                    block("body", "操作指南", INTERACTION)
                ]}),
            ),
            "compare" => (
                "横向双栏 · 对照阅读",
                json!({"layout":"horizontal","blocks":[
                    block("source", "原文", "The quick brown fox jumps over the lazy dog.\n\nSelect a sentence and copy it.\nEach section has its own selection and scrolling."),
                    block("translation", "译文", "敏捷的棕色狐狸跃过慵懒的狗。\n\n可以选中一句话再复制。\n每个区块分别管理选择和滚动。")
                ]}),
            ),
            "four" | "vertical" => (
                "四区块 · 布局与焦点导航",
                json!({
                    "layout":if scene == "vertical" {"vertical"} else {"auto"},
                    "blocks":[
                        block("first", "① 需求", "展示四个区块。\n从搜索框按 Tab 进入第一块。"),
                        block("second", "② 方案", "auto 在宽窗口最多两列。\nvertical 始终单列。"),
                        block("third", "③ 验证", "用 Tab 依次切换。\n用 Shift+Tab 反向切换。"),
                        block("fourth", "④ 结论", "最后一块再按 Tab 返回搜索框。\n可以单独选择、复制每块正文。")
                    ]
                }),
            ),
            "unequal" => (
                "长短对照 · 自适应宽度",
                json!({"layout":"horizontal","blocks":[
                    block("summary", "摘要", "短摘要。"),
                    block("detail", "详细内容", "这段正文比左侧摘要长得多，用于观察宿主在双栏布局中按内容长度分配宽度。\n\n文字会按区块宽度自动换行。窗口较窄时自动改为单列，不需要插件计算像素尺寸。\n\n布局、文字测量与鼠标命中由宿主统一管理。".repeat(5))
                ]}),
            ),
            "long" => (
                "长文 · 两个独立滚动区",
                json!({"layout":"horizontal","blocks":[
                    block("left", "文档 A · 120 段", lines(120)),
                    block("right", "文档 B · 90 段", lines(90))
                ]}),
            ),
            "unicode" => (
                "Unicode · 多语言与换行",
                json!({"blocks":[
                    block("languages", "多语言", "中文：你好，世界！\nEnglish: Hello, world!\n日本語：こんにちは世界\n한국어: 안녕하세요\nالعربية: مرحبا بالعالم\nעברית: שלום עולם\n\nEmoji: 😀 🚀 🎉 👩‍💻 👨‍👩‍👧‍👦 🇨🇳\n组合字符：e\u{301} / é\n\n尝试跨中英文、Emoji 和组合字符拖选复制。字形显示取决于系统字体。"),
                    block("whitespace", "空白与纯文本", "第一行\n\n第三行（前面有空行）\n    四个空格缩进\n\t制表符缩进\nCRLF 示例：\r\n下一行\n\n# Markdown 标记保持原文\n**粗体标记** 与 <b>HTML 标签</b> 都是纯文本。\nhttps://example.com 作为普通文字展示。")
                ]}),
            ),
            "static" => (
                "混排 · 可选正文与静态说明",
                json!({"layout":"vertical","blocks":[
                    {"id":"notice","title":"静态说明（不可选）","text":"本块 selectable=false，Tab 应跳过这里。仍可通过整项菜单复制其原始正文。","selectable":false},
                    block("editable-looking", "只读正文（可选）", "本块 selectable=true（默认）。\n可以选择、全选、复制，但不能修改正文。\n直接输入文字或粘贴会返回搜索框。"),
                    {"id":"footer","title":"静态补充（不可选）","text":"主题、字体、DPI 和焦点由宿主管理。","selectable":false}
                ]}),
            ),
            "blank" => (
                "空正文 · 占位不参与复制",
                json!({"blocks":[
                    block("blank", "空字符串", ""),
                    block("explanation", "说明", "左侧正文是真正的空字符串。\n宿主可显示“暂无内容”占位；菜单“复制：空字符串”复制的仍是空值。")
                ]}),
            ),
            "empty" => (
                "空状态 · 没有正文区块",
                json!({
                    "state":"empty", "message":"当前没有内容。输入 textdemo blank 可对比空正文区块。"
                }),
            ),
            "loading" | "error" => {
                let key = if scene == "loading" {
                    "loading"
                } else {
                    "error"
                };
                let visit = self.visits.entry(key).or_default();
                *visit += 1;
                if *visit == 1 {
                    (
                        "状态演示 · 重试后恢复",
                        json!({
                            "state":key, "retry":true,
                            "message":if key == "loading" {
                                "模拟加载中。点击重试或按 Ctrl+R 完成本次演示（没有后台轮询）。"
                            } else {
                                "模拟请求失败。点击重试或按 Ctrl+R 恢复，不访问网络。"
                            }
                        }),
                    )
                } else {
                    (
                        "状态演示 · 已恢复",
                        json!({"retry":true,
                            "message":format!("第 {visit} 次查询：已恢复正常。进程重启后重新展示初始状态。"),
                            "blocks":[block("result", "查询结果", "重试执行的是当前查询，没有触发默认复制动作。\n此处是 ready 状态，也可以继续重试。")]
                        }),
                    )
                }
            }
            "retained" => (
                "错误状态 · 保留已有正文",
                json!({
                    "state":"error", "retry":true,
                    "message":"模拟刷新失败；原有内容仍可阅读、选择和复制。重试仍返回此错误场景。",
                    "blocks":[block("cached", "上次成功的内容", "这是保留下来的正文。\n\n可拖选本段，或使用右键菜单复制。\n状态行与正文区域分别处理鼠标操作。")]
                }),
            ),
            "append" | "replace" => {
                let key = if scene == "append" {
                    "append"
                } else {
                    "replace"
                };
                let visit = self.visits.entry(key).or_default();
                *visit = (*visit + 1).min(60);
                let text = if key == "append" {
                    let mut text =
                        String::from("固定前缀：选择这段文字，再按 Ctrl+R 追加内容。\n\n");
                    text.push_str(&lines(40));
                    for n in 1..=*visit {
                        text.push_str(&format!("追加日志 {n:02}：保留原文，只在尾部追加。\n"));
                    }
                    text
                } else if *visit % 2 == 1 {
                    format!(
                        "版本 A / 第 {visit} 次查询\n\n蓝色天空，白色云朵。\n{}",
                        lines(45)
                    )
                } else {
                    format!(
                        "版本 B / 第 {visit} 次查询\n\n夜幕降临，星光闪烁。\n{}",
                        lines(45)
                    )
                };
                (
                    "刷新演示 · 稳定结果与区块 ID",
                    json!({"retry":true,
                        "message":format!("第 {visit} 次查询（最多 60）。点击重试 / Ctrl+R；查询文字不变。"),
                        "blocks":[
                            block("document", "动态正文", text),
                            block("guide", "观察方法", if key == "append" {
                                "先滚动并选择正文中的固定文字，松开鼠标，再按 Ctrl+R。\n追加使用相同的结果 ID 和区块 ID，观察仍有效的选区和滚动位置。\n每次追加一条，最多 60 条；进程重启后清零。"
                            } else {
                                "选择正文开头的版本或句子，再按 Ctrl+R。\nA/B 两份内容交替替换，被替换的选中文字应清除。\n结果 ID 与区块 ID 保持不变；也可观察另一块未变化的选区。"
                            })
                        ]
                    }),
                )
            }
            "actions" => (
                "通用动作 · 复制、菜单与拖出",
                json!({"blocks":[
                    block("first", "第一段", "第一段正文。\n保留原始换行。"),
                    block("second", "第二段", "第二段正文。\n\n搜索框 Enter 复制两段正文，以空行分隔。\n在总标题上右键，可分别复制每一段。\n从总标题拖到支持文本拖放的程序，获得整项正文；拖选正文用于选择文字。")
                ]}),
            ),
            "limit" => {
                // Exactly 65,520 UTF-8 bytes, below the 65,536-byte block/action limit.
                let text = "中A😀\n".repeat(7_280);
                (
                    "容量边界 · 65,520 字节单区块",
                    json!({"blocks":[
                        block("large", "接近 64 KiB 的 Unicode 正文", text)
                    ]}),
                )
            }
            _ => (
                "文本面板 Demo · 场景导航",
                json!({"layout":"horizontal","blocks":[
                block("scenes", "场景命令", HELP),
                block("interaction", "交互说明", INTERACTION)
            ],"message":if unknown {"未知场景，请按左侧列表输入完整命令。"} else {"只读纯文本 · 无网络访问 · 无持久化数据"}}),
            ),
        };
        json!({"items":[item(scene, title, panel)]})
    }

    fn dispatch(&mut self, request: &Value) -> Result<Value, String> {
        match request["method"].as_str().unwrap_or("") {
            "initialize" => {
                self.visits.clear();
                Ok(json!({"protocol_version":2}))
            }
            "query" => Ok(self.query(&request["params"])),
            method => Err(format!("不支持的方法：{method}")),
        }
    }
}

fn main() {
    let mut demo = Demo::default();
    if let Err(error) = plugin_rpc::serve(
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        |request| demo.dispatch(request),
    ) {
        eprintln!("{error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_launcher::result::TextPanel;

    fn query(demo: &mut Demo, scene: &str) -> Value {
        demo.query(&json!({"query":scene,"max_results":100}))
    }

    #[test]
    fn all_scenes_fit_host_panel_action_and_frame_limits() {
        let mut demo = Demo::default();
        for scene in SCENES.iter().copied().chain(["", &"未知场景".repeat(200)]) {
            let result = query(&mut demo, scene);
            let entry = &result["items"][0];
            assert!(entry["id"].as_str().unwrap().len() <= 128);
            let panel: TextPanel = serde_json::from_value(entry["text_panel"].clone()).unwrap();
            panel.validate().unwrap();
            assert!(
                entry["action"]["text"].as_str().unwrap().len() <= 65_536,
                "{scene}"
            );
            for action in entry["context_actions"].as_array().unwrap() {
                assert!(action["action"]["text"].as_str().unwrap().len() <= 65_536);
            }
            let mut output = Vec::new();
            plugin_rpc::write_frame(
                &mut output,
                &json!({"jsonrpc":"2.0","id":2,"result":result}),
            )
            .unwrap_or_else(|error| panic!("{scene}: {error}"));
        }
        let limit = query(&mut demo, "limit");
        assert_eq!(
            limit["items"][0]["text_panel"]["blocks"][0]["text"]
                .as_str()
                .unwrap()
                .len(),
            65_520
        );
        assert_eq!(
            query(&mut demo, "blank")["items"][0]["context_actions"][1]["action"]["text"],
            ""
        );
    }

    #[test]
    fn retries_recover_or_update_content_with_stable_ids() {
        let mut demo = Demo::default();
        for scene in ["loading", "error", "append", "replace"] {
            let first = query(&mut demo, scene);
            let next = query(&mut demo, scene);
            assert_eq!(first["items"][0]["id"], next["items"][0]["id"]);
            let first = &first["items"][0]["text_panel"];
            let next = &next["items"][0]["text_panel"];
            assert_eq!(next["retry"], true);
            if scene == "loading" || scene == "error" {
                assert_eq!(first["state"], scene);
                assert!(next.get("state").is_none()); // Default ready.
                assert!(next["blocks"].as_array().is_some());
            } else {
                assert_eq!(first["blocks"][0]["id"], next["blocks"][0]["id"]);
                let before = first["blocks"][0]["text"].as_str().unwrap();
                let after = next["blocks"][0]["text"].as_str().unwrap();
                assert_ne!(before, after);
                if scene == "append" {
                    assert!(after.starts_with(before));
                }
                assert_eq!(first["blocks"][1], next["blocks"][1]);
            }
        }
        for _ in 0..65 {
            query(&mut demo, "append");
        }
        assert_eq!(demo.visits["append"], 60);
        assert_eq!(
            demo.query(&json!({"query":"append","max_results":0})),
            json!({"items":[]})
        );
        demo.dispatch(&json!({"method":"initialize"})).unwrap();
        assert!(demo.visits.is_empty());
    }

    #[test]
    fn ndjson_server_negotiates_and_ignores_cancel_notifications() {
        let mut demo = Demo::default();
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocol_version\":2}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"cancel\",\"params\":{\"id\":9}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"query\",\"params\":{\"query\":\"compare\"}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"shutdown\"}\n"
        );
        let mut output = Vec::new();
        plugin_rpc::serve(&mut io::Cursor::new(input), &mut output, |request| {
            demo.dispatch(request)
        })
        .unwrap();
        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["result"]["protocol_version"], 2);
        assert_eq!(
            responses[1]["result"]["items"][0]["text_panel"]["layout"],
            "horizontal"
        );
    }
}
