//! Register both palettes so every themed control and dialog uses the same
//! surfaces, including after switching between system/light/dark appearance.
use gpui_kit::{
    component::{Theme, ThemeConfig},
    App,
};
use std::rc::Rc;

pub(super) fn install(cx: &mut App) {
    let light = palette((*Theme::global(cx).light_theme).clone(), false);
    let dark = palette((*Theme::global(cx).dark_theme).clone(), true);
    Theme::update(cx, |theme| {
        theme.light_theme = Rc::new(light);
        theme.dark_theme = Rc::new(dark);
    });
}

fn palette(mut config: ThemeConfig, dark: bool) -> ThemeConfig {
    let (canvas, surface, sidebar, hover, border, text, muted, primary, selected, selected_text) =
        if dark {
            (
                "#171B22", "#202630", "#13171D", "#2C3542", "#35404F", "#E4E9F0", "#9CA8B8",
            "#3566CB", "#253A5A", "#B7D0FF",
            )
        } else {
            (
                "#F4F4F2", "#FAFBF9", "#EAEEEF", "#E5EBF3", "#D9DFE4", "#24313F", "#64717E",
                "#365FBD", "#DCE6F8", "#2E4F94",
            )
        };
    config.name = if dark {
        "MiniLauncher Charcoal"
    } else {
        "MiniLauncher Pearl"
    }
    .into();
    config.radius = Some(8);
    config.radius_lg = Some(14);
    config.font_family = Some("Microsoft YaHei UI".into());
    let colors = &mut config.colors;
    macro_rules! set {
        ($value:expr; $($field:ident),+ $(,)?) => {
            $(colors.$field = Some($value.into());)+
        };
    }
    set!(canvas; background, list, table, tab_bar, scrollbar);
    set!(surface; group_box, popover, button, tab_active, table_head, table_foot, accordion);
    set!(sidebar; sidebar, title_bar, muted, secondary, tab, tab_bar_segmented);
    set!(hover; accent, button_hover, button_active, secondary_hover, secondary_active, list_hover, table_hover);
    set!(border; border, input, sidebar_border, title_bar_border, table_row_border, switch, scrollbar_thumb);
    set!(text; foreground, group_box_foreground, group_box_title_foreground, popover_foreground,
        button_foreground, secondary_foreground, accent_foreground, sidebar_foreground,
        tab_active_foreground, table_head_foreground, table_foot_foreground);
    set!(muted; muted_foreground, tab_foreground, scrollbar_thumb_hover);
    set!(primary; primary, button_primary, sidebar_primary, ring, list_active_border, table_active_border, progress_bar);
    set!(selected; sidebar_accent, list_active, table_active, list_even, table_even);
    set!(selected_text; sidebar_accent_foreground);
    set!("#F7F9FD"; primary_foreground, button_primary_foreground, sidebar_primary_foreground, switch_thumb);
    if dark {
        set!("#3B6BD1"; primary_hover, button_primary_hover);
        set!("#2B59B8"; primary_active, button_primary_active);
        set!("#8BB4FF"; link, link_hover, link_active, caret);
        set!("#76A5FF55"; selection);
        set!("#0D111980"; overlay);
    } else {
        set!("#2E54AA"; primary_hover, button_primary_hover);
        set!("#254891"; primary_active, button_primary_active);
        set!(primary; link, link_hover, link_active, caret);
        set!("#365FBD33"; selection);
        set!("#24313F40"; overlay);
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_palettes_resolve_layered_surfaces_and_matching_control_tokens() {
        for dark in [false, true] {
            let mut base = ThemeConfig::default();
            base.mode = if dark {
                gpui_kit::component::ThemeMode::Dark
            } else {
                gpui_kit::component::ThemeMode::Light
            };
            let config = Rc::new(palette(base, dark));
            let mut theme = Theme::default();
            theme.apply_config(&config);
            assert_ne!(theme.background, theme.group_box);
            assert_ne!(theme.background, theme.sidebar);
            assert_eq!(theme.primary, theme.button_primary);
            assert_eq!(theme.tokens.group_box.color, theme.group_box);
            assert_eq!(theme.tokens.popover.color, theme.popover);
            assert_eq!(config.radius_lg, Some(14));
        }
    }
}
