//! 窗口框架渲染：标题栏、窗口控制按钮，
//! 以及 Linux 客户端装饰（圆角、阴影、拖拽缩放热区）。

use gpui::{AnyElement, App, ClickEvent, Context, SharedString, Styled, Window, div, px};

#[cfg(target_os = "windows")]
use gpui::WindowControlArea;
#[cfg(target_os = "linux")]
use gpui::{
    Bounds, BoxShadow, CursorStyle, Decorations, Div, HitboxBehavior, MouseButton, Pixels, Point,
    ResizeEdge, Size, Tiling, canvas, point, size, transparent_black,
};

use super::*;
use crate::assets::{ICON_APP, ICON_MOON, ICON_SUN};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::assets::{ICON_WINDOW_CLOSE, ICON_WINDOW_MAXIMIZE, ICON_WINDOW_MINIMIZE};
use crate::theme::{FontWeightExt, Palette, text_size};

#[cfg(target_os = "linux")]
pub(super) fn linux_client_side_decorations(
    content: Div,
    palette: Palette,
    window: &mut Window,
) -> AnyElement {
    use crate::platform::CLIENT_SIDE_DECORATION_SIZE;

    const BORDER_SIZE: Pixels = px(1.0);
    let decorations = window.window_decorations();
    let tiling = match decorations {
        Decorations::Server => Tiling::default(),
        Decorations::Client { tiling } => tiling,
    };
    let is_client = matches!(decorations, Decorations::Client { .. });

    window.set_client_inset(if is_client {
        CLIENT_SIDE_DECORATION_SIZE
    } else {
        px(0.0)
    });

    let content = if is_client {
        content.rounded_linux_window(tiling).overflow_hidden()
    } else {
        content
    };

    div()
        .id("window-backdrop")
        .size_full()
        .bg(transparent_black())
        .when(is_client, |backdrop| {
            backdrop
                .rounded_linux_window(tiling)
                .when(!tiling.top, |backdrop| {
                    backdrop.pt(CLIENT_SIDE_DECORATION_SIZE)
                })
                .when(!tiling.bottom, |backdrop| {
                    backdrop.pb(CLIENT_SIDE_DECORATION_SIZE)
                })
                .when(!tiling.left, |backdrop| {
                    backdrop.pl(CLIENT_SIDE_DECORATION_SIZE)
                })
                .when(!tiling.right, |backdrop| {
                    backdrop.pr(CLIENT_SIDE_DECORATION_SIZE)
                })
                .on_mouse_move(|_, window, _| window.refresh())
                .on_mouse_down(MouseButton::Left, move |event, window, _| {
                    if let Some(edge) = linux_resize_edge(
                        event.position,
                        CLIENT_SIDE_DECORATION_SIZE,
                        window.window_bounds().get_bounds().size,
                        tiling,
                    ) {
                        window.start_window_resize(edge);
                    }
                })
        })
        .child(
            div()
                .size_full()
                .cursor(CursorStyle::Arrow)
                .when(is_client, |frame| {
                    frame
                        .rounded_linux_window(tiling)
                        .border_color(palette.border)
                        .when(!tiling.top, |frame| frame.border_t(BORDER_SIZE))
                        .when(!tiling.bottom, |frame| frame.border_b(BORDER_SIZE))
                        .when(!tiling.left, |frame| frame.border_l(BORDER_SIZE))
                        .when(!tiling.right, |frame| frame.border_r(BORDER_SIZE))
                        .when(!tiling.is_tiled(), |frame| {
                            frame.shadow(vec![BoxShadow {
                                color: gpui::black().opacity(0.4),
                                offset: point(px(0.0), px(0.0)),
                                blur_radius: CLIENT_SIDE_DECORATION_SIZE / 2.0,
                                spread_radius: px(0.0),
                                inset: false,
                            }])
                        })
                })
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .child(content),
        )
        .when(is_client, |backdrop| {
            backdrop.child(
                canvas(
                    |_bounds, window, _| {
                        window.insert_hitbox(
                            Bounds::new(
                                point(px(0.0), px(0.0)),
                                window.window_bounds().get_bounds().size,
                            ),
                            HitboxBehavior::Normal,
                        )
                    },
                    move |_bounds, hitbox, window, _| {
                        let Some(edge) = linux_resize_edge(
                            window.mouse_position(),
                            CLIENT_SIDE_DECORATION_SIZE,
                            window.window_bounds().get_bounds().size,
                            tiling,
                        ) else {
                            return;
                        };
                        window.set_cursor_style(linux_resize_cursor(edge), &hitbox);
                    },
                )
                .size_full()
                .absolute(),
            )
        })
        .into_any_element()
}

#[cfg(target_os = "linux")]
trait LinuxClientDecorationsExt: Styled + Sized {
    fn rounded_linux_window(mut self, tiling: Tiling) -> Self {
        use crate::platform::CLIENT_SIDE_DECORATION_SIZE;

        if !tiling.top && !tiling.left {
            self = self.rounded_tl(CLIENT_SIDE_DECORATION_SIZE);
        }
        if !tiling.top && !tiling.right {
            self = self.rounded_tr(CLIENT_SIDE_DECORATION_SIZE);
        }
        if !tiling.bottom && !tiling.left {
            self = self.rounded_bl(CLIENT_SIDE_DECORATION_SIZE);
        }
        if !tiling.bottom && !tiling.right {
            self = self.rounded_br(CLIENT_SIDE_DECORATION_SIZE);
        }
        self
    }
}

#[cfg(target_os = "linux")]
impl<T: Styled> LinuxClientDecorationsExt for T {}

#[cfg(target_os = "linux")]
fn linux_resize_edge(
    position: Point<Pixels>,
    shadow_size: Pixels,
    window_size: Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let inner_bounds = Bounds::new(Point::default(), window_size).inset(shadow_size * 1.5);
    if inner_bounds.contains(&position) {
        return None;
    }

    let corner_size = size(shadow_size * 1.5, shadow_size * 1.5);
    let top_left = Bounds::new(Point::default(), corner_size);
    let top_right = Bounds::new(
        Point::new(window_size.width - corner_size.width, px(0.0)),
        corner_size,
    );
    let bottom_left = Bounds::new(
        Point::new(px(0.0), window_size.height - corner_size.height),
        corner_size,
    );
    let bottom_right = Bounds::new(
        Point::new(
            window_size.width - corner_size.width,
            window_size.height - corner_size.height,
        ),
        corner_size,
    );

    if !tiling.top && !tiling.left && top_left.contains(&position) {
        Some(ResizeEdge::TopLeft)
    } else if !tiling.top && !tiling.right && top_right.contains(&position) {
        Some(ResizeEdge::TopRight)
    } else if !tiling.bottom && !tiling.left && bottom_left.contains(&position) {
        Some(ResizeEdge::BottomLeft)
    } else if !tiling.bottom && !tiling.right && bottom_right.contains(&position) {
        Some(ResizeEdge::BottomRight)
    } else if !tiling.top && position.y < shadow_size {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && position.y > window_size.height - shadow_size {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && position.x < shadow_size {
        Some(ResizeEdge::Left)
    } else if !tiling.right && position.x > window_size.width - shadow_size {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_resize_cursor(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

// Windows 的关闭按钮走原生 WindowControlArea，参数中的 window 仅 Linux 客户端装饰使用。
#[cfg_attr(target_os = "windows", allow(unused_variables))]
pub(super) fn render_titlebar(
    app: &PureClash,
    palette: Palette,
    window: &mut Window,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let titlebar_content = div()
        .h_full()
        .flex_1()
        .flex()
        .items_center()
        .gap_3()
        .px_4()
        .child(
            // 应用图标包含背景色，使用 img 保留 SVG 原色；svg 元素只适合单色遮罩图标。
            gpui::img(ICON_APP).size(px(24.0)).flex_none(),
        )
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(palette.text)
                .child("Pure Clash"),
        )
        .child(div().h_4().border_l_1().border_color(palette.border))
        .child(
            // 当前页名用正文级字重：顶栏是「我在哪」，弱化它等于让人迷路。
            div()
                .text_size(px(text_size::PAGE_TITLE))
                .font_semibold()
                .text_color(palette.text)
                .child(app.page.label()),
        );

    // 只有 Windows 使用无边框窗口，因此需要把标题栏内容标记为可拖拽区域。
    #[cfg(target_os = "windows")]
    let titlebar_content = titlebar_content.window_control_area(WindowControlArea::Drag);

    // Linux 使用客户端装饰，按住应用顶栏时把移动操作交给 Wayland 合成器或 X11 窗口管理器。
    #[cfg(target_os = "linux")]
    let titlebar_content = titlebar_content
        .on_mouse_down(MouseButton::Left, |_event, window, _cx| {
            window.start_window_move()
        });

    let titlebar = div()
        .h(px(44.0))
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        // 顶栏与侧栏同色：两者合成一条连续的 chrome 带，内容区因此整体「下沉」。
        .bg(palette.sidebar)
        .border_b_1()
        .border_color(palette.border)
        .child(titlebar_content)
        // 全局状态放全局位置：代理 / TUN 快捷开关与内核启停常驻顶栏，
        // 任何页面都能一眼看到、一点切换；页内因此不再需要重复的页头。
        .child(status_chip(
            "titlebar-system-proxy",
            tr("status.system_proxy"),
            app.system_proxy,
            palette,
            cx.listener(|this, _, _, cx| this.toggle_system_proxy(cx)),
        ))
        .child(status_chip(
            "titlebar-tun",
            tr("status.tun"),
            app.tun_switch_state(),
            palette,
            cx.listener(|this, _, _, cx| this.toggle_tun(cx)),
        ))
        .child(core_toggle_button(app, palette, cx))
        .child(language_button(app.config.language, palette, cx))
        .child(theme_button(app.config.theme.is_dark(), palette, cx));

    // 当前 GPUI 的 overflow 裁剪不包含圆角，实际着色的标题栏也必须应用上圆角，
    // 否则它会越过外框的圆角，在透明阴影区留下实心直角像素。
    #[cfg(target_os = "linux")]
    let titlebar = match window.window_decorations() {
        Decorations::Client { tiling } => titlebar
            .when(!tiling.top && !tiling.left, |titlebar| {
                titlebar.rounded_tl(crate::platform::CLIENT_SIDE_DECORATION_SIZE)
            })
            .when(!tiling.top && !tiling.right, |titlebar| {
                titlebar.rounded_tr(crate::platform::CLIENT_SIDE_DECORATION_SIZE)
            }),
        Decorations::Server => titlebar,
    };

    // Windows 无边框窗口使用原生命中测试，保留贴靠布局和系统按钮语义。
    #[cfg(target_os = "windows")]
    let titlebar = titlebar
        .child(window_button(
            "window-minimize",
            ICON_WINDOW_MINIMIZE,
            WindowControlArea::Min,
            palette,
        ))
        .child(window_button(
            "window-maximize",
            ICON_WINDOW_MAXIMIZE,
            WindowControlArea::Max,
            palette,
        ))
        .child(window_button(
            "window-close",
            ICON_WINDOW_CLOSE,
            WindowControlArea::Close,
            palette,
        ));

    // Linux 客户端装饰模式下不绘制窗口控件，由应用补齐标准操作。
    #[cfg(target_os = "linux")]
    let titlebar = if let Decorations::Client { tiling } = window.window_decorations() {
        titlebar
            .child(linux_window_button(
                "linux-window-minimize",
                ICON_WINDOW_MINIMIZE,
                false,
                false,
                palette,
                |_, window, _| window.minimize_window(),
            ))
            .child(linux_window_button(
                "linux-window-maximize",
                ICON_WINDOW_MAXIMIZE,
                false,
                false,
                palette,
                |_, window, _| window.zoom_window(),
            ))
            .child(linux_window_button(
                "linux-window-close",
                ICON_WINDOW_CLOSE,
                true,
                !tiling.top && !tiling.right,
                palette,
                cx.listener(|_, _, window, _| {
                    // AppShell 持有长期业务状态；这里只销毁窗口及其平台渲染资源。
                    window.remove_window();
                }),
            ))
    } else {
        titlebar
    };

    titlebar.into_any_element()
}

/// 顶栏状态芯片：点 = 状态色，文案 = 「名称 状态」。
/// 待启动态用交互色描边并禁点，避免连点造成状态机抖动。
#[allow(clippy::type_complexity)]
fn status_chip(
    id: &'static str,
    label: SharedString,
    state: impl Into<SwitchState>,
    palette: Palette,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let state = state.into();
    let enabled = state == SwitchState::On;
    let pending = state == SwitchState::Starting;
    div()
        .id(id)
        .h(px(26.0))
        .px_2()
        .mr_1()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(6.0))
        .when(!pending, |chip| {
            chip.cursor_pointer()
                .hover(|style| style.bg(palette.surface_alt))
                .on_click(on_toggle)
        })
        .border_1()
        .border_color(if pending {
            palette.accent
        } else if enabled {
            palette.success
        } else {
            palette.border
        })
        .text_size(px(text_size::META))
        .text_color(if pending {
            palette.accent
        } else if enabled {
            palette.success
        } else {
            palette.muted
        })
        .child(div().size_2().rounded_full().bg(if pending {
            palette.accent
        } else if enabled {
            palette.success
        } else {
            palette.border
        }))
        .child(format!("{label} {}", state.label()))
        .into_any_element()
}

/// 顶栏内核启停钮：运行中 = 嵌入底「停止」，停止 = 品牌实色「启动」。
fn core_toggle_button(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    div()
        .id("toggle-core-titlebar")
        .h(px(26.0))
        .px_3()
        .mr_2()
        .rounded_md()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
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
            tr("app.stop")
        } else {
            tr("app.start")
        })
        .on_click(cx.listener(|this, _, _, cx| this.toggle_core(cx)))
        .into_any_element()
}

fn language_button(
    language: Language,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let target_label = match language {
        Language::Chinese => tr("language.short_en"),
        Language::English => tr("language.short_zh"),
    };

    div()
        .id("titlebar-language-toggle")
        .w(px(36.0))
        .h_full()
        .ml_2()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_xs()
        .font_medium()
        .text_color(palette.muted)
        .hover(|style| style.bg(palette.surface_alt).text_color(palette.text))
        .child(target_label)
        .on_click(cx.listener(|this, _, _, cx| this.toggle_language(cx)))
        .into_any_element()
}

fn theme_button(dark_mode: bool, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    let icon_path = if dark_mode { ICON_SUN } else { ICON_MOON };

    div()
        .id("titlebar-theme-toggle")
        .w(px(36.0))
        .h_full()
        .mr_1()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(palette.surface_alt))
        .child(icon(icon_path, palette.muted, 15.0))
        .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)))
        .into_any_element()
}

#[cfg(target_os = "windows")]
fn window_button(
    id: &'static str,
    icon_path: &'static str,
    area: WindowControlArea,
    palette: Palette,
) -> AnyElement {
    let is_close = area == WindowControlArea::Close;
    div()
        .id(id)
        .w(px(44.0))
        .h_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .window_control_area(area)
        .when(is_close, |button| {
            button.hover(|style| style.bg(rgb(0xc42b1c)))
        })
        .when(!is_close, |button| {
            button.hover(|style| style.bg(palette.surface_alt))
        })
        .child(icon(
            icon_path,
            if is_close {
                rgb(0xc95146)
            } else {
                palette.muted
            },
            13.0,
        ))
        .into_any_element()
}

#[cfg(target_os = "linux")]
fn linux_window_button(
    id: &'static str,
    icon_path: &'static str,
    is_close: bool,
    round_top_right: bool,
    palette: Palette,
    action: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .w(px(44.0))
        .h_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(round_top_right, |button| {
            button.rounded_tr(crate::platform::CLIENT_SIDE_DECORATION_SIZE)
        })
        .when(is_close, |button| {
            button.hover(|style| style.bg(rgb(0xc42b1c)))
        })
        .when(!is_close, |button| {
            button.hover(|style| style.bg(palette.surface_alt))
        })
        .child(icon(
            icon_path,
            if is_close {
                rgb(0xc95146)
            } else {
                palette.muted
            },
            13.0,
        ))
        .on_click(action)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_window_resize_edges_cover_corners_and_sides() {
        let window_size = size(px(1100.0), px(720.0));
        let shadow = px(10.0);
        let tiling = Tiling::default();

        assert_eq!(
            linux_resize_edge(point(px(2.0), px(2.0)), shadow, window_size, tiling),
            Some(ResizeEdge::TopLeft)
        );
        assert_eq!(
            linux_resize_edge(point(px(1098.0), px(2.0)), shadow, window_size, tiling),
            Some(ResizeEdge::TopRight)
        );
        assert_eq!(
            linux_resize_edge(point(px(2.0), px(718.0)), shadow, window_size, tiling),
            Some(ResizeEdge::BottomLeft)
        );
        assert_eq!(
            linux_resize_edge(point(px(1098.0), px(718.0)), shadow, window_size, tiling),
            Some(ResizeEdge::BottomRight)
        );
        assert_eq!(
            linux_resize_edge(point(px(550.0), px(2.0)), shadow, window_size, tiling),
            Some(ResizeEdge::Top)
        );
        assert_eq!(
            linux_resize_edge(point(px(550.0), px(718.0)), shadow, window_size, tiling),
            Some(ResizeEdge::Bottom)
        );
        assert_eq!(
            linux_resize_edge(point(px(2.0), px(360.0)), shadow, window_size, tiling),
            Some(ResizeEdge::Left)
        );
        assert_eq!(
            linux_resize_edge(point(px(1098.0), px(360.0)), shadow, window_size, tiling),
            Some(ResizeEdge::Right)
        );
        assert_eq!(
            linux_resize_edge(point(px(550.0), px(360.0)), shadow, window_size, tiling),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_window_resize_edges_respect_tiling() {
        let window_size = size(px(1100.0), px(720.0));
        let shadow = px(10.0);
        let tiling = Tiling {
            top: true,
            left: true,
            right: false,
            bottom: false,
        };

        assert_eq!(
            linux_resize_edge(point(px(2.0), px(2.0)), shadow, window_size, tiling),
            None
        );
        assert_eq!(
            linux_resize_edge(point(px(2.0), px(360.0)), shadow, window_size, tiling),
            None
        );
        assert_eq!(
            linux_resize_edge(point(px(1098.0), px(360.0)), shadow, window_size, tiling),
            Some(ResizeEdge::Right)
        );
        assert_eq!(
            linux_resize_edge(point(px(550.0), px(718.0)), shadow, window_size, tiling),
            Some(ResizeEdge::Bottom)
        );
    }
}
