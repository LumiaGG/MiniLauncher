use super::*;

const REPOSITORY: &str = "https://github.com/LumiaGG/MiniLauncher";

impl Settings {
    fn open_about_file(&mut self, name: &str, cx: &mut Context<Self>) {
        let installed = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|directory| directory.join(name)));
        let path = installed.filter(|path| path.is_file()).or_else(|| {
            if cfg!(debug_assertions) {
                let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join(name);
                path.is_file().then_some(path)
            } else {
                None
            }
        });
        let result = path
            .ok_or_else(|| format!("缺少 {name}，请使用完整发行包。"))
            .and_then(|path| {
                std::process::Command::new(if name.ends_with(".html") {
                    "explorer.exe"
                } else {
                    "notepad.exe"
                })
                .arg(path)
                .spawn()
                .map(|_| ())
                .map_err(|error| format!("无法打开 {name}：{error}"))
            });
        if let Err(error) = result {
            self.error = Some(error);
            cx.notify();
        }
    }

    pub(super) fn render_about(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let card = |title: &'static str, body: String| {
            div()
                .p_4()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_lg().child(title))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(body),
                )
                .into_any_element()
        };
        vec![
            card(
                "MiniLauncher",
                format!(
                    "版本 {} · Windows x64\n原生轻量桌面启动器\n{}",
                    env!("CARGO_PKG_VERSION"),
                    REPOSITORY,
                ),
            ),
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .child(Button::new("about-source").label("项目主页").on_click(|_, _, cx| {
                    cx.open_url(REPOSITORY);
                }))
                .child(Button::new("about-issues").label("反馈问题").on_click(|_, _, cx| {
                    cx.open_url("https://github.com/LumiaGG/MiniLauncher/issues");
                }))
                .into_any_element(),
            card("项目许可", "GPL-3.0-or-later\nCopyright © 2026 LumiaGG and contributors\n本软件不提供担保；第三方组件遵循各自许可证。".into()),
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .child(Button::new("about-license").label("许可证正文").on_click(
                    cx.listener(|this, _, _, cx| this.open_about_file("LICENSE", cx)),
                ))
                .child(Button::new("about-third-party").label("第三方许可").on_click(
                    cx.listener(|this, _, _, cx| this.open_about_file("THIRD_PARTY_NOTICES.html", cx)),
                ))
                .child(Button::new("about-corresponding-source").label("源码说明").on_click(
                    cx.listener(|this, _, _, cx| this.open_about_file("SOURCE.md", cx)),
                ))
                .into_any_element(),
            card("第三方组件", include_str!("../../THIRD_PARTY_NOTICES.txt").into()),
        ]
    }
}
