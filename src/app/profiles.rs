//! 配置页：内置默认配置行、订阅列表与添加/更新/删除/激活操作。

use gpui::{AnyElement, Context, SharedString, Styled, div, px, relative};
use rust_i18n::t;

use super::*;
use crate::assets::{
    ICON_CIRCLE_CHECK, ICON_FILE_CODE, ICON_FOLDER, ICON_REFRESH_CW, ICON_SEND, ICON_SHIELD_CHECK,
    ICON_WINDOW_CLOSE,
};
use crate::config::{ProfileMeta, SubscriptionUsage};
use crate::theme::{FontWeightExt, Palette, text_size};
use crate::ui::kit;

/// GPUI 内部拖动载荷与预览；以稳定 ID 定位，不携带订阅链接。
#[derive(Clone)]
struct ProfileDrag {
    /// 用户配置的持久 ID；内置默认配置不参与拖动。
    id: String,
    /// 预览中显示的配置名称。
    name: SharedString,
    /// 当前主题配色。
    palette: Palette,
}

impl Render for ProfileDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .max_w(px(320.0))
            .rounded_md()
            .bg(self.palette.surface)
            .border_1()
            .border_color(self.palette.accent)
            .shadow_md()
            .text_sm()
            .text_color(self.palette.text)
            .child(div().truncate().child(self.name.clone()))
    }
}

pub(super) fn render_profiles(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let busy = app.profile_actions_locked();
    div()
        .p_6()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            // 工具行：概要在左，视图切换与主操作在右。
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_size(px(text_size::META))
                        .text_color(palette.faint)
                        .child(tr("profiles.detail")),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("profiles-view-toggle")
                        .h(px(30.0))
                        .px(px(15.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .bg(palette.surface)
                        .border_1()
                        .border_color(palette.border)
                        .text_size(px(text_size::ROW))
                        .text_color(palette.muted)
                        .hover(|style| style.text_color(palette.text))
                        .child(tr(if app.profiles_view.is_grid() {
                            "profiles.view_list"
                        } else {
                            "profiles.view_grid"
                        }))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_profiles_view(cx))),
                )
                .child(
                    div()
                        .id("profiles-update-all")
                        .h(px(30.0))
                        .px(px(15.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .gap_1()
                        .map(|button| {
                            if busy {
                                button.opacity(0.5)
                            } else {
                                button.cursor_pointer()
                            }
                        })
                        .bg(palette.surface)
                        .border_1()
                        .border_color(palette.border)
                        .text_size(px(text_size::ROW))
                        .text_color(palette.muted)
                        .hover(|style| style.text_color(palette.text))
                        .child(icon(ICON_REFRESH_CW, palette.muted, 13.0))
                        .child(tr("profiles.update_all"))
                        .when(!busy, |button| {
                            button.on_click(
                                cx.listener(|this, _, _, cx| this.update_all_profiles(cx)),
                            )
                        }),
                )
                .child(
                    div()
                        .id("profiles-add")
                        .h(px(30.0))
                        .px(px(15.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .gap_1()
                        .map(|button| {
                            if busy {
                                button.opacity(0.5)
                            } else {
                                button.cursor_pointer()
                            }
                        })
                        .bg(palette.accent)
                        .text_size(px(text_size::ROW))
                        .font_medium()
                        .text_color(palette.knob)
                        .child(tr("profiles.add"))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_profile_form(cx))),
                ),
        )
        .when(app.profile_form_open, |page| {
            page.child(render_profile_form(app, palette, cx))
        })
        .when_some(app.profile_error.as_ref(), |page, error| {
            page.child(
                div()
                    .p_3()
                    .rounded_md()
                    .bg(palette.danger_soft)
                    .text_size(px(text_size::META))
                    .text_color(palette.danger)
                    .child(error.clone()),
            )
        })
        .when_some(app.profile_busy.as_ref(), |page, busy| {
            page.child(
                div()
                    .p_3()
                    .rounded_md()
                    .bg(palette.inset)
                    .text_size(px(text_size::META))
                    .text_color(palette.accent)
                    .child(busy.clone()),
            )
        })
        .child({
            let mut rows: Vec<AnyElement> = Vec::new();
            if app.profiles_view.is_grid() {
                // 网格视图只渲染卡片；编辑器统一由页面级弹窗承担。
                rows.push(profile_grid(app, palette, cx));
            } else {
                // 内置默认配置常驻首行：无激活订阅时即为选中态。
                rows.push(default_profile_row(app, palette, cx));
                for (index, meta) in app.profiles.iter().enumerate() {
                    rows.push(profile_row(app, index, meta, palette, cx));
                }
            }
            div().flex().flex_col().gap_3().children(rows)
        })
        .when(app.profiles.is_empty(), |page| {
            page.child(
                kit::card(palette)
                    .p_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(text_size::BODY))
                            .text_color(palette.text)
                            .child(tr("profiles.empty_title")),
                    )
                    .child(kit::hint(tr("profiles.empty_detail"), palette)),
            )
        })
        .into_any_element()
}

/// 添加配置的内联表单：名称可选，可下载 URL 订阅或选择本地 YAML。
fn render_profile_form(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let busy = app.profile_actions_locked();
    div()
        .mt_4()
        .p_4()
        .rounded_md()
        .flex()
        .flex_col()
        .gap_3()
        .bg(palette.surface)
        .border_1()
        .border_color(palette.accent)
        .child(
            div()
                .text_sm()
                .font_medium()
                .text_color(palette.text)
                .child(tr("profiles.form_title")),
        )
        .child(
            div()
                .p_2()
                .rounded_sm()
                .bg(palette.surface_alt)
                .border_1()
                .border_color(palette.border)
                .text_sm()
                .text_color(palette.text)
                .line_height(px(20.))
                .child(app.profile_form_name.clone()),
        )
        .child(
            div()
                .p_2()
                .rounded_sm()
                .bg(palette.surface_alt)
                .border_1()
                .border_color(palette.border)
                .text_sm()
                .text_color(palette.text)
                .line_height(px(20.))
                .child(app.profile_form_url.clone()),
        )
        // 可选的自动更新间隔：留空或 0 表示关闭；非法值在提交时反馈。
        .child(
            div()
                .p_2()
                .rounded_sm()
                .bg(palette.surface_alt)
                .border_1()
                .border_color(palette.border)
                .text_sm()
                .text_color(palette.text)
                .line_height(px(20.))
                .child(app.profile_form_interval.clone()),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .id("profile-form-submit")
                        .flex_1()
                        .h(px(32.0))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .map(|button| {
                            if busy {
                                button.opacity(0.5)
                            } else {
                                button.cursor_pointer()
                            }
                        })
                        .bg(palette.accent)
                        .text_xs()
                        .font_medium()
                        .text_color(palette.surface)
                        .gap_1()
                        .child(icon(ICON_SEND, palette.surface, 14.0))
                        .child(tr("profiles.form_submit"))
                        .on_click(cx.listener(|this, _, _, cx| this.add_subscription(cx))),
                )
                .child(
                    div()
                        .id("profile-form-import-local")
                        .flex_1()
                        .h(px(32.0))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .map(|button| {
                            if busy {
                                button.opacity(0.5)
                            } else {
                                button.cursor_pointer()
                            }
                        })
                        .bg(palette.accent_soft)
                        .text_xs()
                        .font_medium()
                        .text_color(palette.accent)
                        .child(icon(ICON_FOLDER, palette.accent, 14.0))
                        .child(tr("profiles.import_local"))
                        .on_click(cx.listener(|this, _, _, cx| this.import_local_profile(cx))),
                )
                .child(
                    div()
                        .id("profile-form-cancel")
                        .flex_1()
                        .h(px(32.0))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .map(|button| {
                            if busy {
                                button.opacity(0.5)
                            } else {
                                button.cursor_pointer()
                            }
                        })
                        .bg(palette.surface_alt)
                        .text_xs()
                        .text_color(palette.muted)
                        .child(tr("profiles.form_cancel"))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_profile_form(cx))),
                ),
        )
        .into_any_element()
}

/// 格式化更新时间为 UTC 日期；时间戳为 0 时显示“未更新”。
pub(super) fn format_profile_time(timestamp: u64) -> SharedString {
    if timestamp == 0 {
        return tr("profiles.never_updated");
    }
    // 无chrono 依赖，用天数推算 UTC 日期，展示足够用的粗粒度时间。
    let days = timestamp / 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    SharedString::from(format!("{year:04}-{month:02}-{day:02}"))
}

/// 从 UNIX 天数转换为民用日期（Howard Hinnant 算法）。
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 字节数的可读表示；用量文案用，保留一位小数。
fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.1}G", value / GB)
    } else if value >= MB {
        format!("{:.0}M", value / MB)
    } else {
        format!("{:.0}K", value / 1024.0)
    }
}

/// 用量摘要文案：`7.4G / 60.0G (12%) · 剩 8 天`。
///
/// 机场不返回用量头时返回 None，由调用方显示「用量未知」——不把未知当 0。
pub(super) fn usage_summary(usage: &SubscriptionUsage) -> Option<String> {
    let now = crate::profile::now_secs();
    let used = format_bytes(usage.used());
    let mut parts = Vec::new();
    match usage.ratio() {
        Some(ratio) => parts.push(format!(
            "{} / {} ({:.0}%)",
            used,
            format_bytes(usage.total),
            ratio * 100.0
        )),
        // 不限量机场：只报已用流量。
        None => parts.push(format!("已用 {used}")),
    }
    match usage.days_left(now) {
        Some(days) if days <= 0 => parts.push(tr("profiles.usage_expired").to_string()),
        Some(days) => {
            parts.push(t!("profiles.usage_days_left", days = days.to_string()).into_owned())
        }
        None => {}
    }
    Some(parts.join(" · "))
}

/// 进度条：轨道铺满卡片宽度，填充按比例缩放；不限量时只给文字说明。
///
/// 卡片宽度是自适应的，因此这里不能用固定像素：轨道用 w_full，填充用
/// relative(ratio)，两者都跟着卡片走。
fn usage_bar(usage: Option<&SubscriptionUsage>, palette: Palette) -> AnyElement {
    let Some(usage) = usage else {
        return div()
            .text_xs()
            .text_color(palette.muted)
            .child(tr("profiles.usage_unknown"))
            .into_any_element();
    };
    let mut column = div().flex().flex_col().gap_1();
    if let Some(ratio) = usage.ratio() {
        column = column.child(
            div()
                .h(px(6.0))
                .w_full()
                .rounded_full()
                .bg(palette.track)
                .child(
                    div()
                        .h(px(6.0))
                        // 留 2% 最小可见宽度，避免极小用量看不出进度。
                        .w(relative(ratio.max(0.02)))
                        .rounded_full()
                        // 接近用满时转红，提示该换套餐或节点了；
                        // 其余时候用成功绿——用量条是状态，不是品牌装饰。
                        .bg(if ratio >= 0.9 {
                            palette.danger
                        } else {
                            palette.success
                        }),
                ),
        );
    }
    if let Some(summary) = usage_summary(usage) {
        column = column.child(
            div()
                .text_xs()
                .text_color(palette.muted)
                .child(SharedString::from(summary)),
        );
    }
    if usage.expire_at > 0 {
        column = column.child(div().text_xs().text_color(palette.muted).child(t!(
            "profiles.usage_expires",
            date = format_profile_time(usage.expire_at).to_string()
        )));
    }
    column.into_any_element()
}

/// 卡片网格：每张卡显示名称、来源、用量进度条与到期时间。
fn profile_grid(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    let busy = app.profile_actions_locked();
    let mut cards: Vec<AnyElement> = Vec::new();
    cards.push(default_profile_card(app, palette, busy, cx));
    for (index, meta) in app.profiles.iter().enumerate() {
        cards.push(profile_card(app, index, meta, palette, busy, cx));
    }
    div()
        .flex()
        .flex_wrap()
        // 行内均分由卡片的 flex_1 完成；这里只负责换行与行间距。
        .gap_3()
        .children(cards)
        .into_any_element()
}

/// 卡片头部：图标 + 名称 + 选中状态。
fn card_head(name: SharedString, active: bool, palette: Palette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(icon(
            if active {
                ICON_CIRCLE_CHECK
            } else {
                ICON_FILE_CODE
            },
            if active { palette.brand } else { palette.muted },
            16.0,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(text_size::ROW))
                .font_medium()
                .text_color(palette.text)
                .truncate()
                .child(name),
        )
        .child(if active {
            // 「当前使用」用药丸承载：状态是名词，不是一句提示语。
            div()
                .px_2()
                .py(px(2.0))
                .rounded_full()
                .bg(palette.brand_soft)
                .text_size(px(text_size::TINY))
                .text_color(palette.brand)
                .child(tr("profiles.active"))
                .into_any_element()
        } else {
            div()
                .text_size(px(text_size::TINY))
                .text_color(palette.faint)
                .child(tr("profiles.activate"))
                .into_any_element()
        })
        .into_any_element()
}

/// 内置默认配置卡片。
fn default_profile_card(
    app: &PureClash,
    palette: Palette,
    busy: bool,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let active = app.active_profile.is_none();
    kit::card(palette)
        .id("profile-card-builtin")
        // 自适应宽度：一行放得下几张由可用宽度决定，卡片再均分该行空间，
        // 避免固定宽度导致右侧留出大片空白。
        .flex_1()
        .min_w(px(250.0))
        .p_3()
        .flex()
        .flex_col()
        .gap_1()
        .map(|card| {
            if busy {
                card.opacity(0.7)
            } else {
                card.cursor_pointer()
            }
        })
        .when(active, |card| card.border_color(palette.brand_line))
        .child(card_head(tr("profiles.builtin_name"), active, palette))
        .child(
            div()
                .text_xs()
                .text_color(palette.muted)
                .child(tr("profiles.builtin_detail")),
        )
        .on_click(cx.listener(|this, _, _, cx| this.activate_default_profile(cx)))
        .into_any_element()
}

/// 单个订阅卡片。
fn profile_card(
    app: &PureClash,
    index: usize,
    meta: &ProfileMeta,
    palette: Palette,
    busy: bool,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let active = app.active_profile.as_deref() == Some(meta.id.as_str());
    let source = if meta.url.is_some() {
        tr("profiles.source_subscription")
    } else {
        tr("profiles.source_local")
    };
    let updated = format_profile_time(meta.updated_at);
    let menu_id = meta.id.clone();
    kit::card(palette)
        .id(SharedString::from(format!("profile-card-{}", meta.id)))
        // 与内置卡片一致：自适应宽度并均分行内空间。
        .flex_1()
        .min_w(px(250.0))
        .p_3()
        .flex()
        .flex_col()
        .gap_1()
        .map(|card| {
            if busy {
                card.opacity(0.7)
            } else {
                card.cursor_pointer()
            }
        })
        .when(active, |card| {
            // 当前使用的卡：品牌细环 + 顶部品牌短条，双重但不吵。
            card.border_color(palette.brand_line).relative().child(
                div()
                    .absolute()
                    .top(px(0.))
                    .left(px(14.))
                    .right(px(14.))
                    .h(px(3.))
                    .rounded_b_full()
                    .bg(palette.brand),
            )
        })
        .child(card_head(meta.name.clone().into(), active, palette))
        .child(div().text_xs().text_color(palette.muted).child(t!(
            "profiles.updated",
            source = source.to_string(),
            updated = updated.to_string()
        )))
        .child(usage_bar(meta.usage.as_ref(), palette))
        .on_click(cx.listener(move |this, _, _, cx| this.activate_profile_clicked(index, cx)))
        .on_aux_click(cx.listener(move |this, _, _, cx| {
            this.open_profile_menu(menu_id.clone(), cx)
        }))
        .into_any_element()
}

fn default_profile_row(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let active = app.active_profile.is_none();
    let busy = app.profile_actions_locked();
    div()
        .id("profile-builtin")
        .min_h(px(76.0))
        .p_4()
        .rounded_md()
        .flex()
        .items_center()
        .gap_3()
        .map(|row| {
            if busy {
                row.opacity(0.7)
            } else {
                row.cursor_pointer()
            }
        })
        .bg(palette.surface)
        .border_1()
        .border_color(if active {
            palette.accent
        } else {
            palette.border
        })
        .child(
            div()
                .size_9()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .bg(if active {
                    palette.accent_soft
                } else {
                    palette.surface_alt
                })
                .child(icon(
                    if active {
                        ICON_CIRCLE_CHECK
                    } else {
                        ICON_SHIELD_CHECK
                    },
                    if active {
                        palette.accent
                    } else {
                        palette.muted
                    },
                    17.0,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .text_color(palette.text)
                        .child(tr("profiles.builtin_name")),
                )
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(palette.muted)
                        .child(tr("profiles.builtin_detail")),
                ),
        )
        .child(
            div().flex().items_center().gap_2().child(
                div()
                    .text_xs()
                    .text_color(if active {
                        palette.success
                    } else {
                        palette.muted
                    })
                    .child(tr(if active {
                        "profiles.active"
                    } else {
                        "profiles.activate"
                    })),
            ),
        )
        .on_click(cx.listener(|this, _, _, cx| this.activate_default_profile(cx)))
        .into_any_element()
}

fn profile_row(
    app: &PureClash,
    index: usize,
    meta: &ProfileMeta,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let active = app.active_profile.as_deref() == Some(meta.id.as_str());
    // 行内编辑或后台自动更新进行中时锁定全部行的操作。
    let busy = app.profile_actions_locked();
    let target_id = meta.id.clone();
    let source = if meta.url.is_some() {
        tr("profiles.source_subscription")
    } else {
        tr("profiles.source_local")
    };
    // 副标题在“来源 · 更新时间”后追加自动更新状态。
    let mut detail = t!(
        "profiles.updated",
        source = source.to_string(),
        updated = format_profile_time(meta.updated_at).to_string()
    )
    .into_owned();
    if meta.update_interval_minutes > 0 {
        detail.push_str(" · ");
        detail.push_str(
            &t!(
                "profiles.auto_interval",
                interval = meta.update_interval_minutes.to_string()
            )
            .into_owned(),
        );
    }
    if meta.last_auto_attempt_at > meta.updated_at {
        detail.push_str(" · ");
        detail.push_str(&tr("profiles.auto_failed"));
    }
    // 用量摘要跟在时间信息之后：订阅页最常看的数字放在一起。
    // 机场不提供用量时不加占位，避免列表行过长。
    if let Some(usage) = meta.usage.as_ref()
        && let Some(summary) = usage_summary(usage)
    {
        detail.push_str(" · ");
        detail.push_str(&summary);
    }
    div()
        .id(SharedString::from(format!("profile-{}", meta.id)))
        .min_h(px(76.0))
        .p_4()
        .rounded_md()
        .flex()
        .items_center()
        .gap_3()
        .map(|row| {
            if busy {
                row.opacity(0.7)
            } else {
                row.cursor_pointer()
            }
        })
        .bg(palette.surface)
        .border_1()
        .border_color(if active {
            palette.accent
        } else {
            palette.border
        })
        .when(!busy, |row| {
            // 高亮目标行；释放后按当前稳定 ID 排序，不会因旧下标移动错误的配置。
            row.drag_over::<ProfileDrag>(move |style, _, _, _| {
                style.bg(palette.accent_soft).border_color(palette.accent)
            })
            .on_drop(cx.listener(move |this, dragged: &ProfileDrag, _, cx| {
                cx.stop_propagation();
                this.reorder_profiles(&dragged.id, &target_id, cx);
            }))
        })
        .child(
            div()
                .id(SharedString::from(format!("profile-drag-{}", meta.id)))
                .w(px(18.0))
                .h_9()
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_1()
                // 六点手柄直接绘制，避免引入只用于拖动的图标依赖。
                .children((0..3).map(|_| {
                    div().flex().gap_1().children(
                        (0..2).map(|_| div().size(px(3.0)).rounded_full().bg(palette.muted)),
                    )
                }))
                .on_click(|_, _, cx| cx.stop_propagation())
                .when(!busy, |handle| {
                    handle.cursor_grab().on_drag(
                        ProfileDrag {
                            id: meta.id.clone(),
                            name: meta.name.clone().into(),
                            palette,
                        },
                        |dragged, _, _, cx| cx.new(|_| dragged.clone()),
                    )
                }),
        )
        .child(
            div()
                .size_9()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .bg(if active {
                    palette.accent_soft
                } else {
                    palette.surface_alt
                })
                .child(icon(
                    if active {
                        ICON_CIRCLE_CHECK
                    } else {
                        ICON_FILE_CODE
                    },
                    if active {
                        palette.accent
                    } else {
                        palette.muted
                    },
                    17.0,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .text_color(palette.text)
                        .child(meta.name.clone()),
                )
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(palette.muted)
                        .child(detail),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .children(if active {
                    vec![
                        div()
                            .text_xs()
                            .text_color(palette.success)
                            .child(tr("profiles.active"))
                            .into_any_element(),
                    ]
                } else {
                    vec![
                        div()
                            .text_xs()
                            .text_color(palette.muted)
                            .child(tr("profiles.activate"))
                            .into_any_element(),
                    ]
                })
                // 链接与更新间隔共用编辑入口；行内按钮不冒泡触发激活。
                .children(meta.url.as_ref().map(|_| {
                    div()
                        .id(SharedString::from(format!("profile-edit-{index}")))
                        .px_2()
                        .h(px(26.0))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .map(|button| {
                            if busy || app.profile_form_open {
                                button.opacity(0.5)
                            } else {
                                button.cursor_pointer()
                            }
                        })
                        .bg(palette.surface_alt)
                        .text_xs()
                        .text_color(palette.muted)
                        .child(tr("profiles.edit"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.edit_profile(index, cx)
                        }))
                        .into_any_element()
                }))
                .children(meta.url.as_ref().map(|_| {
                    div()
                        .id(SharedString::from(format!("profile-update-{index}")))
                        .px_2()
                        .h(px(26.0))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .map(|button| {
                            if busy {
                                button.opacity(0.5)
                            } else {
                                button.cursor_pointer()
                            }
                        })
                        .bg(palette.surface_alt)
                        .text_xs()
                        .text_color(palette.muted)
                        .child(tr("profiles.update"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.update_profile(index, cx);
                        }))
                        .into_any_element()
                }))
                .children(Some({
                    div()
                        .id(SharedString::from(format!("profile-delete-{index}")))
                        .px_2()
                        .h(px(26.0))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .map(|button| {
                            if busy {
                                button.opacity(0.5)
                            } else {
                                button.cursor_pointer()
                            }
                        })
                        .bg(palette.surface_alt)
                        .text_xs()
                        .text_color(palette.danger)
                        .child(tr("profiles.delete"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.delete_profile(index, cx);
                        }))
                        .into_any_element()
                })),
        )
        .on_click(cx.listener(move |this, _, _, cx| this.activate_profile_clicked(index, cx)))
        .into_any_element()
}

/// 订阅行内编辑器：链接修改后立即更新，仅改间隔时不触发下载。
fn profile_editor(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    // 弹窗自带背景与圆角，内层只负责排版，不再叠一层卡片。
    let editing_name = app
        .editing_profile_index
        .and_then(|index| app.profiles.get(index))
        .map(|meta| SharedString::from(meta.name.clone()))
        .unwrap_or_else(|| tr("profiles.menu_edit"));
    div()
        .id("profile-editor")
        .p(px(22.0))
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pb_2()
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_semibold()
                        .text_color(palette.text)
                        .truncate()
                        .child(editing_name),
                )
                .child(
                    div()
                        .id("profile-editor-close")
                        .flex_none()
                        .cursor_pointer()
                        .hover(|style| style.opacity(0.7))
                        .child(icon(ICON_WINDOW_CLOSE, palette.muted, 14.0))
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_profile_edit(cx))),
                ),
        )
        .child(
            div()
                .text_sm()
                .font_medium()
                .text_color(palette.text)
                .child(tr("profiles.url_label")),
        )
        .child(
            div()
                .p_2()
                .rounded_sm()
                .bg(palette.surface_alt)
                .border_1()
                .border_color(palette.border)
                // 订阅链接往往远超输入框宽度，必须裁剪，否则会撑破弹窗。
                .overflow_hidden()
                .text_sm()
                .text_color(palette.text)
                .line_height(px(20.))
                .child(app.profile_edit_url.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(palette.muted)
                .child(tr("profiles.url_edit_hint")),
        )
        .child(
            div()
                .text_sm()
                .font_medium()
                .text_color(palette.text)
                .child(tr("profiles.interval_label")),
        )
        .child(
            div()
                .text_xs()
                .text_color(palette.muted)
                .child(tr("profiles.interval_hint")),
        )
        .child(
            div()
                .p_2()
                .rounded_sm()
                .bg(palette.surface_alt)
                .border_1()
                .border_color(palette.border)
                .text_sm()
                .text_color(palette.text)
                .line_height(px(20.))
                .child(app.profile_form_interval.clone()),
        )
        .child(
            div()
                .text_sm()
                .font_medium()
                .text_color(palette.text)
                .child(tr("profiles.ua_label")),
        )
        .child(
            div()
                .text_xs()
                .text_color(palette.muted)
                .child(tr("profiles.ua_hint")),
        )
        .child(
            div()
                .p_2()
                .rounded_sm()
                .bg(palette.surface_alt)
                .border_1()
                .border_color(palette.border)
                .overflow_hidden()
                .text_sm()
                .text_color(palette.text)
                .line_height(px(20.))
                .child(app.profile_edit_user_agent.clone()),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .id("profile-interval-save")
                        .flex_1()
                        .h(px(32.0))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .bg(palette.accent)
                        .text_xs()
                        .font_medium()
                        .text_color(palette.surface)
                        .child(tr("profiles.interval_save"))
                        .on_click(cx.listener(|this, _, _, cx| this.save_profile_edits(cx))),
                )
                .child(
                    div()
                        .id("profile-interval-cancel")
                        .flex_1()
                        .h(px(32.0))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .bg(palette.surface_alt)
                        .text_xs()
                        .text_color(palette.muted)
                        .child(tr("profiles.interval_cancel"))
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_profile_edit(cx))),
                ),
        )
        .into_any_element()
}

/// 订阅卡片右键菜单：编辑 / 立即更新 / 设为当前 / 删除。
///
/// 卡片视图把整卡点击留给「激活」，其余操作集中到右键菜单，
/// 避免在卡片里塞一排图标按钮。
pub(super) fn render_profile_menu(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let Some(id) = app.profile_menu_for.clone() else {
        return div().into_any_element();
    };
    let Some(index) = app.config.profiles.iter().position(|meta| meta.id == id) else {
        return div().into_any_element();
    };
    let meta = &app.config.profiles[index];
    let name = SharedString::from(meta.name.clone());
    let is_active = app.active_profile.as_deref() == Some(meta.id.as_str());
    let can_update = meta.url.is_some();

    let mut rows: Vec<AnyElement> = Vec::new();
    rows.push(menu_item(
        "profile-menu-edit",
        tr("profiles.menu_edit"),
        ICON_FILE_CODE,
        palette,
        cx.listener(move |this, _, _, cx| {
            this.close_profile_menu(cx);
            this.edit_profile(index, cx);
        }),
    ));
    if can_update {
        rows.push(menu_item(
            "profile-menu-update",
            tr("profiles.menu_update"),
            ICON_REFRESH_CW,
            palette,
            cx.listener(move |this, _, _, cx| {
                this.close_profile_menu(cx);
                this.update_profile(index, cx);
            }),
        ));
    }
    if !is_active {
        rows.push(menu_item(
            "profile-menu-activate",
            tr("profiles.menu_activate"),
            ICON_CIRCLE_CHECK,
            palette,
            cx.listener(move |this, _, _, cx| {
                this.close_profile_menu(cx);
                this.activate_profile_clicked(index, cx);
            }),
        ));
    }
    rows.push(menu_item(
        "profile-menu-delete",
        tr("profiles.menu_delete"),
        ICON_WINDOW_CLOSE,
        palette,
        cx.listener(move |this, _, _, cx| {
            this.close_profile_menu(cx);
            this.delete_profile(index, cx);
        }),
    ));

    div()
        .id("profile-menu-scrim")
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x00000099))
        // 点遮罩关闭；面板自身吞掉点击，避免误关。
        .on_click(cx.listener(|this, _, _, cx| this.close_profile_menu(cx)))
        .child(
            div()
                .id("profile-menu-panel")
                .w(px(280.0))
                .flex()
                .flex_col()
                .rounded_lg()
                .bg(palette.surface_high)
                .border_1()
                .border_color(palette.border)
                .shadow_lg()
                // 必须显式阻止冒泡：空 handler 不拦截事件，点击输入框会冒泡到遮罩。
                .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(palette.border)
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .text_color(palette.text)
                                .truncate()
                                .child(name),
                        ),
                )
                .children(rows),
        )
        .into_any_element()
}

fn menu_item(
    id: &'static str,
    label: SharedString,
    icon_path: &'static str,
    palette: Palette,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .h(px(38.0))
        .px_4()
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .hover(|style| style.bg(palette.inset))
        .child(icon(icon_path, palette.muted, 14.0))
        .child(div().text_sm().text_color(palette.text).child(label))
        .on_click(handler)
        .into_any_element()
}

/// 订阅编辑弹窗：卡片与列表共用同一个浮层编辑器。
///
/// 原先编辑器内联在列表行下方，卡片视图没有落点；改成页面级浮层后
/// 两种视图行为一致，也不会把列表撑长。
pub(super) fn render_profile_editor_overlay(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    div()
        .id("profile-editor-scrim")
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x00000099))
        .on_click(cx.listener(|this, _, _, cx| this.cancel_profile_edit(cx)))
        .child(
            div()
                .id("profile-editor-panel")
                .w(px(480.0))
                .flex()
                .flex_col()
                .rounded_lg()
                .bg(palette.surface_high)
                .border_1()
                .border_color(palette.border)
                .shadow_lg()
                // 必须显式阻止冒泡：空 handler 不拦截事件，点击输入框会冒泡到遮罩。
                .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                .child(profile_editor(app, palette, cx)),
        )
        .into_any_element()
}
