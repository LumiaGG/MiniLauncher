use crate::plugin_catalog::Plugin;

// Explicit keywords remain exclusive; wildcard plugins receive the whole input.
pub(super) fn route_plugins<'a>(plugins: &[Plugin], text: &'a str) -> (Vec<Plugin>, &'a str, bool) {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        let mut direct: Vec<_> = plugins
            .iter()
            .filter(|p| p.manifest.direct_display)
            .cloned()
            .collect();
        sort_by_priority(&mut direct);
        let exclusive = !direct.is_empty();
        return (direct, "", exclusive);
    }
    if trimmed.starts_with('>') || trimmed.trim_end() == "设置" {
        return (Vec::new(), trimmed, false);
    }
    let split = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let keyword = trimmed[..split].to_ascii_lowercase();
    if let Some(plugin) = plugins
        .iter()
        .find(|plugin| keyword != "*" && plugin.manifest.keywords.contains(&keyword))
    {
        return (vec![plugin.clone()], trimmed[split..].trim_start(), true);
    }
    let mut plugins: Vec<_> = plugins
        .iter()
        .filter(|plugin| {
            plugin
                .manifest
                .keywords
                .iter()
                .any(|keyword| keyword == "*")
                && plugin.manifest.global_trigger.matches(text)
        })
        .cloned()
        .collect();
    sort_by_priority(&mut plugins);
    (plugins, text, false)
}

fn sort_by_priority(plugins: &mut [Plugin]) {
    // Stable sorting preserves discovery order for equal priorities.
    plugins.sort_by(|a, b| b.manifest.priority.cmp(&a.manifest.priority));
}
