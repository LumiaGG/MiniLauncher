use super::*;
use mini_launcher::history::Edit;

impl Settings {
    pub(super) fn reload_app_rules(&mut self) {
        match crate::applications::Rules::load(&self.draft.path) {
            Ok(mut rules) => {
                rules.names = std::mem::take(&mut self.app_rules.names);
                self.app_rules = rules;
                self.app_rules_error = None;
            }
            Err(error) => self.app_rules_error = Some(error),
        }
    }

    pub(super) fn build_app_names(&mut self, cx: &mut Context<Self>) {
        if let Ok(settings) = self.draft.value.clone().try_into::<config::Settings>() {
            let cache_key = (
                self.draft.value.get("search_paths").cloned().unwrap_or(Value::Array(vec![])),
                self.draft.value.get("aliases").cloned().unwrap_or(Value::Table(Default::default())),
            );
            if self.app_names_key.as_ref() == Some(&cache_key) { return; }
            self.app_names_key = Some(cache_key);
            self.app_names_generation += 1;
            let generation = self.app_names_generation;
            self.app_names_loading = true;
            let task = cx.background_executor().spawn(async move {
                mini_launcher::search::build_index(&settings)
            });
            self.app_names_task = Some(cx.spawn(async move |this, cx| {
                let index = task.await;
                let _ = this.update(cx, |this, cx| {
                    if this.app_names_generation == generation {
                        this.app_rules.set_names(index);
                        this.app_names_loading = false;
                        cx.notify();
                    }
                });
            }));
        }
    }

    pub(super) fn application_sections(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut sections = vec![div().text_sm().text_color(cx.theme().muted_foreground)
            .child("在搜索结果右键菜单中置顶或移除应用。置顶仅对对应搜索词生效；移除只隐藏搜索结果，不卸载程序。修改后自动保存并生效。")
            .into_any_element()];
        sections.push(div().flex().flex_col().gap_2()
            .child("筛选已置顶和已移除应用")
            .child(Input::new(&self.list_filter).disabled(self.busy))
            .when(self.app_names_loading, |el| el.child(div().text_sm()
                .text_color(cx.theme().muted_foreground).child("正在后台补全应用名称，列表仍可操作…")))
            .child(Button::new("refresh-app-names").label("刷新应用名称").small()
                .disabled(self.busy || self.app_names_loading)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.app_names_key = None;
                    this.build_app_names(cx);
                })))
            .into_any_element());
        if let Some(error) = &self.app_rules_error {
            sections.push(
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone())
                    .into_any_element(),
            );
            return sections;
        }
        for removed in [false, true] {
            let rows = self.app_rules.rows(removed);
            let total = rows.len();
            let filter = self.list_filter.read(cx).value();
            let rows: Vec<_> = rows.into_iter().filter(|row| model::matches_filter(
                &filter, &[&row.title, &row.key, row.query.as_deref().unwrap_or("")]
            )).collect();
            let heading = if removed { "已移除" } else { "已置顶" };
            let mut section = div()
                .flex()
                .flex_col()
                .gap_2()
                .child(format!("{heading}（{} / {total}）", rows.len()));
            if rows.is_empty() {
                section = section.child(self.empty_note(
                    if !filter.trim().is_empty() { "没有匹配的应用，请调整筛选条件。" }
                    else if removed {
                        "没有已移除的应用。"
                    } else {
                        "没有已置顶的应用。"
                    },
                    cx,
                ));
            }
            for (index, row) in rows.into_iter().enumerate() {
                let edit = match &row.query {
                    Some(query) => Edit::Unpin {
                        key: row.key.clone(),
                        query: query.clone(),
                    },
                    None => Edit::Restore {
                        key: row.key.clone(),
                    },
                };
                let mut title = div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().min_w_0().truncate().child(row.title));
                if let Some(query) = row.query {
                    title = title.child(Tag::secondary().small().child(format!("搜索词：{query}")));
                }
                let details = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(title)
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(row.key),
                    );
                section = section.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_md()
                        .child(details)
                        .child(
                            Button::new((if removed { "restore-app" } else { "unpin-app" }, index))
                                .label(if removed { "恢复" } else { "取消置顶" })
                                .small()
                                .flex_shrink_0()
                                .disabled(self.busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.busy {
                                        this.app_rules.edit(edit.clone());
                                        this.error = None;
                                        this.queue_save(cx);
                                    }
                                })),
                        ),
                );
            }
            sections.push(section.into_any_element());
        }
        sections
    }
}
