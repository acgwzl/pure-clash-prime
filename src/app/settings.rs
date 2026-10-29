//! 设置页：内核与系统代理、界面语言、主题与运行目录信息。

use gpui::{Animation, AnimationExt, AnyElement, Context, SharedString, Styled, div, px};

use super::overview::integration_error_banner;
use super::*;
use crate::assets::ICON_REFRESH_CW;
use crate::config::Language;
use crate::theme::{FontWeightExt, Palette, TabularNums, text_size};
use crate::ui::kit;

pub(super) fn render_settings(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    div()
        .p_6()
        .flex()
        .flex_col()
        .gap_5()
        .when_some(app.integration_error.as_ref(), |page, error| {
            page.child(integration_error_banner(error, palette))
        })
        .child(
            // 分组标题在卡片上方，卡顶 = 标题下 8px：标题不再被卡顶线横穿。
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(kit::section_head(
                    tr("settings.kernel_proxy"),
                    tr("settings.integration_detail"),
                    palette,
                ))
                .child(
                    kit::card(palette)
                        .child(setting_row(
                            "setting-autostart",
                            tr("settings.autostart"),
                            if app.autostart_available {
                                tr("settings.autostart_detail")
                            } else {
                                tr("settings.autostart_unavailable")
                            },
                            app.autostart_enabled,
                            app.autostart_available,
                            palette,
                            cx.listener(|this, _, _, cx| this.toggle_autostart(cx)),
                        ))
                        .child(setting_row(
                            "setting-system-proxy",
                            tr("settings.system_proxy"),
                            tr("settings.system_proxy_detail"),
                            app.system_proxy,
                            true,
                            palette,
                            cx.listener(|this, _, _, cx| this.toggle_system_proxy(cx)),
                        ))
                        .child(setting_row(
                            "setting-tun",
                            tr("settings.tun"),
                            if app.tun_switch_state() == SwitchState::Starting {
                                tr("settings.tun_starting_detail")
                            } else {
                                tr("settings.tun_detail")
                            },
                            app.tun_switch_state(),
                            app.core_operable(),
                            palette,
                            cx.listener(|this, _, _, cx| this.toggle_tun(cx)),
                        ))
                        .child(setting_row(
                            "setting-find-process",
                            tr("settings.find_process"),
                            tr("settings.find_process_detail"),
                            app.config.find_process_always,
                            app.baseline.is_some(),
                            palette,
                            cx.listener(|this, _, _, cx| this.toggle_find_process(cx)),
                        ))
                        .child(setting_row(
                            "setting-convert-subscription",
                            tr("settings.convert_subscription"),
                            tr("settings.convert_subscription_detail"),
                            app.config.convert_base64_subscription,
                            true,
                            palette,
                            cx.listener(|this, _, _, cx| this.toggle_convert_subscription(cx)),
                        ))
                        .child(setting_row(
                            "setting-dark-theme",
                            tr("settings.dark_theme"),
                            tr("settings.dark_theme_detail"),
                            app.config.theme.is_dark(),
                            true,
                            palette,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_theme(cx);
                            }),
                        ))
                        .child(language_setting_row(app.config.language, palette, cx)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(kit::section_head(
                    tr("settings.geodata"),
                    tr("settings.geodata_detail"),
                    palette,
                ))
                .child(kit::card(palette).child(geodata_update_row(app, palette, cx))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(kit::section_head(
                    tr("settings.runtime_dir"),
                    tr("settings.runtime_dir_detail"),
                    palette,
                ))
                .child(
                    kit::card(palette)
                        .child(path_row(
                            tr("settings.main_config"),
                            app.paths.config_display(),
                            palette,
                            true,
                        ))
                        .child(path_row(
                            tr("settings.data_dir"),
                            app.paths.data_display(),
                            palette,
                            true,
                        ))
                        .child(path_row(
                            tr("settings.mihomo_config"),
                            app.paths.runtime_mihomo_config_display(),
                            palette,
                            true,
                        ))
                        .child(path_row(
                            tr("settings.mihomo_data"),
                            app.paths.mihomo_data_display(),
                            palette,
                            true,
                        ))
                        .child(path_row(
                            tr("settings.controller"),
                            SharedString::from(
                                app.baseline
                                    .as_ref()
                                    .map(|baseline| baseline.controller_addr.clone())
                                    .unwrap_or_else(|| "—".to_owned()),
                            ),
                            palette,
                            false,
                        )),
                ),
        )
        .into_any_element()
}

/// Geo 数据是随包资源的用户态副本；更新按钮只替换数据目录，不修改安装目录。
fn geodata_update_row(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let revision = short_revision(&app.geodata_info.revision);
    let updated = super::profiles::format_profile_time(app.geodata_info.updated_at);
    div()
        .min_h(px(56.0))
        .px(px(20.0))
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .text_color(palette.text)
                        .child(tr("settings.geodata_files")),
                )
                .child(
                    div().mt_1().text_xs().text_color(palette.muted).child(
                        t!(
                            "settings.geodata_version",
                            revision = revision,
                            updated = updated.to_string()
                        )
                        .into_owned(),
                    ),
                )
                .children(app.geodata_status.as_ref().map(|status| {
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(palette.muted)
                        .child(status.clone())
                })),
        )
        .child(
            div()
                .id("update-geodata")
                .h(px(30.0))
                .px_3()
                .rounded_sm()
                .flex()
                .items_center()
                .gap_2()
                .map(|button| {
                    if app.geodata_updating {
                        button.opacity(0.6)
                    } else {
                        button.cursor_pointer()
                    }
                })
                .bg(palette.accent)
                .text_xs()
                .font_medium()
                .text_color(palette.knob)
                .child(icon(ICON_REFRESH_CW, palette.knob, 13.0))
                .child(if app.geodata_updating {
                    tr("settings.geodata_updating")
                } else {
                    tr("settings.geodata_update")
                })
                .on_click(cx.listener(|this, _, _, cx| this.update_geodata(cx))),
        )
        .into_any_element()
}

fn setting_row(
    id: &'static str,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    state: impl Into<SwitchState>,
    available: bool,
    palette: Palette,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let title = title.into();
    let detail = detail.into();
    let state = state.into();
    let enabled = state == SwitchState::On;
    let pending = state == SwitchState::Starting;
    // 启动中保持文案可读，用居中的呼吸滑块表达过渡；不注册点击事件。
    let thumb = div().size_4().rounded_full().bg(palette.knob);
    let thumb = if pending {
        thumb
            .with_animation(
                "tun-starting-thumb",
                Animation::new(std::time::Duration::from_millis(1200))
                    .repeat()
                    .with_max_fps(20.0),
                |thumb, progress| {
                    thumb.opacity(0.55 + 0.45 * (progress * std::f32::consts::TAU).cos().abs())
                },
            )
            .into_any_element()
    } else {
        thumb.into_any_element()
    };
    div()
        // 这些行是同一张卡片的直接子元素且容器无间距，底边线是行与行之间
        // 唯一的分隔，必须保留；只把行高从 62 收紧到 54。
        .min_h(px(62.0))
        .px(px(20.0))
        .flex()
        .items_center()
        .gap_3()
        .border_b_1()
        .border_color(palette.border)
        .opacity(if available || pending { 1.0 } else { 0.5 })
        .child(
            div()
                .flex_1()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_medium()
                        .text_color(palette.text)
                        .child(title),
                )
                .child(
                    div()
                        .mt_1()
                        .text_size(px(text_size::HINT))
                        .text_color(palette.muted)
                        .child(detail),
                ),
        )
        .when(pending, |row| {
            row.child(
                div()
                    .text_xs()
                    .text_color(palette.accent)
                    .child(state.label()),
            )
        })
        .child(
            div()
                .id(id)
                .w(px(38.0))
                .h(px(22.0))
                .p(px(3.0))
                .rounded_full()
                .flex()
                .items_center()
                .bg(if pending || enabled {
                    palette.accent
                } else {
                    palette.off
                })
                .when(pending, |toggle| toggle.justify_center())
                .when(enabled, |toggle| toggle.justify_end())
                .when(!pending && !enabled, |toggle| toggle.justify_start())
                .child(thumb)
                .when(available && !pending, |toggle| {
                    toggle.cursor_pointer().on_click(handler)
                }),
        )
        .into_any_element()
}

fn language_setting_row(
    language: Language,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    div()
        // 卡内最后一行：分隔线交给卡片底缘，不再画一条悬空线。
        // 行高与左右内边距必须和 setting_row 一致，否则这一行会明显贴边。
        .min_h(px(62.0))
        .px(px(20.0))
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_1()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_medium()
                        .text_color(palette.text)
                        .child(tr("settings.language")),
                )
                .child(
                    div()
                        .mt_1()
                        .text_size(px(text_size::HINT))
                        .text_color(palette.muted)
                        .child(tr("settings.language_detail")),
                ),
        )
        .child(
            div()
                .flex()
                .p_1()
                .rounded_full()
                .bg(palette.inset)
                .child(language_option(
                    Language::Chinese,
                    tr("settings.language_chinese"),
                    language,
                    palette,
                    cx,
                ))
                .child(language_option(
                    Language::English,
                    tr("settings.language_english"),
                    language,
                    palette,
                    cx,
                )),
        )
        .into_any_element()
}

fn language_option(
    language: Language,
    label: SharedString,
    selected: Language,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let active = language == selected;
    div()
        .id(match language {
            Language::Chinese => "language-zh-cn",
            Language::English => "language-en-us",
        })
        .h(px(28.0))
        .px(px(14.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_size(px(text_size::META))
        .font_medium()
        .bg(if active {
            palette.brand_soft
        } else {
            palette.inset
        })
        .text_color(if active { palette.brand } else { palette.muted })
        .child(label)
        .on_click(cx.listener(move |this, _, _, cx| this.set_language(language, cx)))
        .into_any_element()
}

fn path_row(
    label: impl Into<SharedString>,
    path: impl Into<SharedString>,
    palette: Palette,
    divider: bool,
) -> AnyElement {
    let label = label.into();
    let path = path.into();
    let row = div()
        .min_h(px(46.0))
        // 卡片本身没有内边距，行必须自带，否则文字贴着卡片边框。
        .px(px(20.0))
        .flex()
        .items_center()
        .gap(px(16.0))
        .child(
            // 标签固定不压缩，路径用剩余空间并省略，避免长路径把卡片撑破。
            div()
                .flex_none()
                .text_size(px(text_size::ROW))
                .text_color(palette.text)
                .child(label),
        )
        .child(
            div().flex_1().min_w_0().flex().justify_end().child(
                div()
                    .min_w_0()
                    .font_family("Bahnschrift")
                    .text_size(px(text_size::META))
                    .tabnum()
                    .text_color(palette.muted)
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(path),
            ),
        );
    // 卡内最后一行不画线：底缘圆角旁边悬空一条线是评审点过名的瑕疵。
    if divider {
        row.border_b_1()
            .border_color(palette.border)
            .into_any_element()
    } else {
        row.into_any_element()
    }
}
