//! 侧边栏：品牌区、页面导航与底部内核状态卡片。
//!
//! 层次规则（与浏览器原型 chrome.js 一致）：
//! 侧栏是「另一块面板」——底色用 sidebar 令牌而不是 surface，
//! 选中态 = 品牌软底 + 品牌细环 + 左侧发光条三件套，缺一层选中感就塌。

#[cfg(target_os = "linux")]
use gpui::Decorations;
use gpui::{AnyElement, Context, Styled, Window, div, px};

use super::*;
use crate::assets::{ICON_APP, ICON_INFO};
use crate::theme::{FontWeightExt, Palette, text_size};
use crate::ui::kit;

pub(super) fn render_sidebar(
    app: &PureClash,
    palette: Palette,
    window: &mut Window,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let sidebar = div()
        .w(px(220.0))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .bg(palette.sidebar)
        .border_r_1()
        .border_color(palette.border)
        .child(
            // 品牌区：图形承载品牌色，字标靠字重区分层级。
            // 品牌方块带一圈柔光——logo 是页面里唯一的「灯」，让它亮着。
            div()
                .px_4()
                .pt_5()
                .pb_4()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .size_7()
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(palette.brand_soft)
                        .shadow(kit::glow(palette, 14.0))
                        .child(icon(ICON_APP, palette.brand, 16.0)),
                )
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .text_size(px(15.0))
                        .text_color(palette.text)
                        .child(div().font_medium().child("Pure"))
                        .child(
                            div()
                                .font_semibold()
                                .text_color(palette.brand)
                                .child("Clash"),
                        ),
                ),
        )
        .child(
            div().px_2().flex().flex_col().gap_1().children(
                Page::all()
                    .into_iter()
                    .map(|page| nav_item(page, app.page, palette, cx)),
            ),
        )
        .child(div().flex_1())
        .child(sidebar_runtime(app, palette, cx))
        .child(
            div()
                .px_3()
                .pb_3()
                .pt_2()
                .border_t_1()
                .border_color(palette.border)
                .child(
                    div()
                        .id("sidebar-about")
                        .h(px(34.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .text_size(px(text_size::META))
                        .text_color(palette.muted)
                        .hover(|style| style.bg(palette.surface_alt).text_color(palette.text))
                        .child(icon(ICON_INFO, palette.muted, 14.0))
                        .child(tr("page.about"))
                        .on_click(cx.listener(|this, _, _, cx| this.select_page(Page::About, cx))),
                ),
        );

    #[cfg(target_os = "linux")]
    let sidebar = match window.window_decorations() {
        Decorations::Client { tiling } if !tiling.bottom && !tiling.left => {
            sidebar.rounded_bl(crate::platform::CLIENT_SIDE_DECORATION_SIZE)
        }
        _ => sidebar,
    };

    #[cfg(not(target_os = "linux"))]
    let _ = window;

    sidebar.into_any_element()
}

fn nav_item(
    page: Page,
    selected: Page,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let active = page == selected;
    let item = div()
        .id(page.label())
        .h(px(36.0))
        .px_3()
        .rounded_md()
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .text_size(px(13.0))
        .font_medium()
        .text_color(if active { palette.brand } else { palette.muted })
        .hover(|style| style.bg(palette.surface_alt).text_color(palette.text))
        .child(icon(
            page.icon(),
            if active { palette.brand } else { palette.muted },
            16.0,
        ))
        .child(page.label())
        .on_click(cx.listener(move |this, _, _, cx| this.select_page(page, cx)));

    // 选中态三件套：软底、品牌细环、左侧发光条。
    // 指示条用绝对定位贴左缘，不占布局宽度，图标列因此始终对齐。
    if active {
        item.bg(palette.brand_soft)
            .border_1()
            .border_color(palette.brand_line)
            .relative()
            .child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(7.))
                    .bottom(px(7.))
                    .w(px(3.))
                    .rounded_full()
                    .bg(palette.brand)
                    .shadow(kit::glow(palette, 10.0)),
            )
            .into_any_element()
    } else {
        item.into_any_element()
    }
}

fn sidebar_runtime(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    // 内核状态卡：走 kit::card，与页面卡片同一套「明度 + 边 + 阴影」语言。
    kit::card(palette)
        .mx_3()
        .mb_2()
        .p_3()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    // 状态点的颜色即状态本身：绿 = 运行，交互色 = 启动中，灰 = 停止。
                    kit::status_dot(
                        palette,
                        match app.core_state {
                            CoreState::Running => palette.success,
                            CoreState::Starting => palette.accent,
                            CoreState::Stopped => palette.muted,
                        },
                    ),
                )
                .child(
                    div()
                        .text_size(px(text_size::META))
                        .font_medium()
                        .text_color(palette.text)
                        .child(tr("app.core")),
                )
                .child(
                    div()
                        .ml_auto()
                        .text_size(px(text_size::META))
                        .text_color(match app.core_state {
                            CoreState::Running => palette.success,
                            CoreState::Starting => palette.accent,
                            CoreState::Stopped => palette.muted,
                        })
                        .child(match app.core_state {
                            CoreState::Running => tr("app.ready"),
                            CoreState::Starting => tr("app.core_starting"),
                            CoreState::Stopped => tr("app.stopped"),
                        }),
                ),
        )
        .child(
            // 内核子信息：版本说真话，内置/缺失由真实字段驱动——
            // 语言文件里写死的「内置内核」掩盖过文件缺失的可能。
            div()
                .mt_2()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(text_size::TINY))
                        .text_color(palette.muted)
                        .child(SharedString::from(format!(
                            "v{}",
                            app.config.mihomo_version
                        ))),
                )
                .child(
                    div()
                        .text_size(px(text_size::TINY))
                        .text_color(if app.kernel_available {
                            palette.muted
                        } else {
                            palette.danger
                        })
                        .child(tr(if app.kernel_available {
                            "app.kernel_bundled"
                        } else {
                            "app.kernel_missing"
                        })),
                ),
        )
        .child(
            div()
                .id("toggle-core-sidebar")
                .mt_3()
                .h(px(30.0))
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .map(|button| {
                    if app.core_operable() {
                        button.cursor_pointer()
                    } else {
                        button.opacity(0.6)
                    }
                })
                // 运行中 = 停止（嵌入底），停止 = 启动（品牌实色，页面唯一的主按钮之一）。
                .bg(if app.mihomo_running() {
                    palette.inset
                } else {
                    palette.accent
                })
                .text_size(px(text_size::META))
                .font_medium()
                .text_color(if app.mihomo_running() {
                    palette.text
                } else {
                    palette.knob
                })
                .child(if app.mihomo_running() {
                    tr("app.stop_core")
                } else {
                    tr("app.start_core")
                })
                .on_click(cx.listener(|this, _, _, cx| this.toggle_core(cx))),
        )
        .when_some(app.mihomo_error.as_ref(), |card, error| {
            card.child(
                div()
                    .mt_2()
                    .text_size(px(text_size::META))
                    .text_color(palette.danger)
                    .child(error.clone()),
            )
        })
        .into_any_element()
}
