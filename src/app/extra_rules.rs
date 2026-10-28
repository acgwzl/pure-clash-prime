//! 附加规则页：用户自建规则，始终排在内核规则之前，且与激活的配置无关。
//!
//! 分成两张卡片：上面是编辑与操作，下面是规则列表。选择列表放在编辑器下方的
//! 独立定高滚动区里，而不是浮层叠加——GPUI 当前没有弹层原语，浮层会遮挡
//! 相邻内容并与页面的纵向滚动互相干扰。

use super::overview::integration_error_banner;
use super::rules::state_block;
use super::rules_ops::{RULE_TYPES, draft_extra_rule, extra_rule_targets};
use super::*;
use crate::assets::{ICON_CHEVRON_DOWN, ICON_CIRCLE_CHECK, ICON_PLUS, ICON_WINDOW_CLOSE};
use crate::config::ExtraRule;
use crate::theme::Palette;
use gpui::rgba;

/// 弹窗一次渲染的选项上限。弹窗内是全局唯一的活动滚动区，正常列表会一次
/// 全部铺开；该上限只是极端长的代理组列表的兜底，超过时用「显示更多」展开。
pub(super) const OPTION_PAGE_SIZE: usize = 200;

pub(super) fn render_extra_rules(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    div()
        .p_6()
        .child(editor_card(app, palette, cx))
        .child(list_card(app, palette, cx))
        .into_any_element()
}

/// 第一张卡片：说明、添加入口与编辑器。
fn editor_card(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    let total = app.config.extra_rules.len();
    let enabled = app.config.extra_rules.iter().filter(|r| r.enabled).count();
    div()
        .p_4()
        .mb_4()
        .rounded_md()
        .bg(palette.surface)
        .border_1()
        .border_color(palette.border)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section_heading(
                    tr("extra.title"),
                    tr("extra.detail"),
                    ICON_PLUS,
                    palette,
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .when(total > 0, |row| {
                            row.child(
                                div().text_xs().text_color(palette.muted).child(
                                    SharedString::from(
                                        t!(
                                            "extra.count",
                                            total = total.to_string(),
                                            enabled = enabled.to_string()
                                        )
                                        .into_owned(),
                                    ),
                                ),
                            )
                        })
                        .child(plain_button(
                            "extra-rule-open",
                            tr("extra.add"),
                            palette,
                            cx.listener(|this, _, _, cx| this.open_extra_rule_editor(cx)),
                        )),
                ),
        )
        .when_some(app.extra_rule_error.as_ref(), |card, error| {
            card.child(div().mt_3().child(integration_error_banner(error, palette)))
        })
        // 失败优先：有错误时不再展示上一次的成功提示，避免两条横幅同时出现。
        .when_some(
            app.extra_rule_notice
                .as_ref()
                .filter(|_| app.extra_rule_error.is_none()),
            |card, notice| card.child(div().mt_3().child(notice_banner(notice, palette, cx))),
        )
        .when(app.extra_rule_editor_open, |card| {
            card.child(editor(app, palette, cx))
        })
        .into_any_element()
}

/// 附加规则变更后的结果回显。
///
/// 热重载成功此前完全静默：既不写日志也不更新界面，用户无法判断规则是否
/// 已经生效。同时已建立的连接会继续走旧路由，直到它们自行重连，所以这里
/// 一并给出断开动作，让新规则立即对后续流量生效。
fn notice_banner(notice: &str, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .p_3()
        .rounded_sm()
        .bg(palette.surface_alt)
        .border_1()
        .border_color(palette.border)
        .text_xs()
        .text_color(palette.accent)
        .child(SharedString::from(notice.to_owned()))
        .child(plain_button(
            "extra-rule-disconnect",
            tr("extra.disconnect_existing"),
            palette,
            cx.listener(|this, _, _, cx| this.close_all_connections(cx)),
        ))
        .into_any_element()
}

/// 编辑器：输入框独占一行，下面是类型、目标、操作与实时识别预览。
///
/// 识别结果在渲染时现算：父组件读输入框内容即可，不需要额外的预览状态，
/// 输入变化由 new() 里的 observer 触发重绘。
fn editor(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    let raw = app.extra_rule_payload.read(cx).content().trim().to_owned();
    let preview = if raw.is_empty() {
        None
    } else {
        draft_extra_rule(app, &raw).map(|rule| {
            if rule.payload.is_empty() {
                format!("{} → {}", rule.rule_type, rule.target)
            } else {
                format!("{} · {} → {}", rule.rule_type, rule.payload, rule.target)
            }
        })
    };

    div()
        .mt_4()
        .p_4()
        .rounded_sm()
        .bg(palette.surface_alt)
        .border_1()
        .border_color(palette.border)
        // 第一行：输入框独占整行，长 URL 不再被截断。
        .child(
            div()
                .child(
                    div()
                        .text_xs()
                        .text_color(palette.muted)
                        .child(tr("extra.content")),
                )
                .child(
                    div()
                        .mt_1()
                        .p_2()
                        .rounded_sm()
                        .bg(palette.surface)
                        .border_1()
                        .border_color(palette.border)
                        .text_sm()
                        .text_color(palette.text)
                        .line_height(px(20.))
                        .child(app.extra_rule_payload.clone()),
                ),
        )
        // 第二行：识别结果实时回显，让人确认系统到底切成了什么。
        .when(raw.is_empty(), |panel| {
            panel.child(
                div()
                    .mt_2()
                    .text_xs()
                    .text_color(palette.muted)
                    .child(tr("extra.preview_hint")),
            )
        })
        .when(!raw.is_empty(), |panel| {
            panel.child(match preview {
                Some(text) => div()
                    .mt_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(palette.accent)
                    .child(icon(ICON_CIRCLE_CHECK, palette.accent, 13.0))
                    .child(SharedString::from(
                        t!("extra.preview", rule = text).into_owned(),
                    )),
                None => div()
                    .mt_2()
                    .text_xs()
                    .text_color(palette.muted)
                    .child(tr("extra.unrecognized")),
            })
        })
        // 第三行：类型、目标与操作。
        .child(
            div()
                .mt_3()
                .flex()
                .items_end()
                .gap_3()
                .child(picker_field(
                    "extra-rule-type",
                    tr("extra.type"),
                    app.extra_rule_type.clone(),
                    palette,
                    cx.listener(|this, _, _, cx| this.toggle_extra_rule_type_picker(cx)),
                ))
                .child(picker_field(
                    "extra-rule-target",
                    tr("extra.target"),
                    app.extra_rule_target.clone(),
                    palette,
                    cx.listener(|this, _, _, cx| this.toggle_extra_rule_target_picker(cx)),
                ))
                .child(div().flex_1())
                .child(plain_button(
                    "extra-rule-confirm",
                    tr("extra.confirm"),
                    palette,
                    cx.listener(|this, _, _, cx| this.submit_extra_rule(cx)),
                ))
                .child(plain_button(
                    "extra-rule-cancel",
                    tr("extra.cancel"),
                    palette,
                    cx.listener(|this, _, _, cx| this.close_extra_rule_editor(cx)),
                )),
        )
        .into_any_element()
}

/// 选择弹窗：遮罩 + 居中面板，由 mod.rs 的 Render 挂在页面容器之外。
///
/// 列表在这里可以安全地自己滚动：它不在页面的滚动容器里，滚轮只有这一层，
/// 不会出现「必须先滚到页面底部才能滚到列表」的嵌套争抢。
pub(super) fn render_picker_overlay(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let picking_type = app.extra_rule_type_picker;
    let options: Vec<String> = if picking_type {
        RULE_TYPES.iter().map(|name| (*name).to_owned()).collect()
    } else {
        extra_rule_targets(app)
    };
    let selected = if picking_type {
        app.extra_rule_type.clone()
    } else {
        app.extra_rule_target.clone()
    };
    let total = options.len();
    let shown = app.extra_rule_options_shown.min(total);

    let mut rows: Vec<AnyElement> = options
        .into_iter()
        .take(shown)
        .map(|option| {
            let value = option.clone();
            let picked = option == selected;
            div()
                .id(SharedString::from(format!("extra-pick-{option}")))
                .h(px(36.0))
                .px_4()
                .flex()
                .items_center()
                .justify_between()
                .cursor_pointer()
                .border_b_1()
                .border_color(palette.border)
                .text_sm()
                .text_color(if picked { palette.accent } else { palette.text })
                .when(picked, |row| row.bg(palette.accent_soft))
                .child(SharedString::from(option))
                .when(picked, |row| {
                    row.child(icon(ICON_CIRCLE_CHECK, palette.accent, 14.0))
                })
                .on_click(
                    cx.listener(move |this, _, _, cx| on_pick_option(this, value.clone(), cx)),
                )
                .into_any_element()
        })
        .collect();
    let remaining = total - shown;
    if remaining > 0 {
        rows.push(
            div()
                .id("extra-pick-more")
                .h(px(36.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_xs()
                .text_color(palette.accent)
                .child(SharedString::from(
                    t!("rules.more", count = remaining.to_string()).into_owned(),
                ))
                .on_click(cx.listener(|this, _, _, cx| this.show_more_extra_rule_options(cx)))
                .into_any_element(),
        );
    }

    div()
        .id("extra-picker-scrim")
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x00000099))
        // 点遮罩空白处关闭；面板自身吞掉点击，避免误关。
        .on_click(cx.listener(|this, _, _, cx| this.close_extra_rule_picker(cx)))
        .child(
            div()
                .id("extra-picker-panel")
                .w(px(520.0))
                .max_h(px(460.0))
                .flex()
                .flex_col()
                .rounded_lg()
                // 浮层在明度上就要在栈顶：仅靠描边加阴影，在深色底上
                // 会和底下的卡片糊成同一层。
                .bg(palette.surface_high)
                .border_1()
                .border_color(palette.border)
                .shadow_lg()
                .on_click(|_, _, _| {})
                .child(
                    div()
                        .p_4()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_between()
                        .border_b_1()
                        .border_color(palette.border)
                        .child(
                            div()
                                .text_sm()
                                .text_color(palette.text)
                                .child(if picking_type {
                                    tr("extra.type")
                                } else {
                                    tr("extra.target")
                                }),
                        )
                        .child(plain_button(
                            "extra-pick-cancel",
                            tr("extra.cancel"),
                            palette,
                            cx.listener(|this, _, _, cx| this.close_extra_rule_picker(cx)),
                        )),
                )
                .child(
                    // 弹窗内是全局唯一的活动滚动区，选项多时在这里滚动。
                    div()
                        .id("extra-picker-list")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(rows),
                ),
        )
        .into_any_element()
}

/// 选择字段：显示当前值，点击进入整页选择。
fn picker_field(
    id: &'static str,
    label: SharedString,
    value: String,
    palette: Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .w(px(190.0))
        .flex_none()
        .child(div().text_xs().text_color(palette.muted).child(label))
        .child(
            div()
                .id(id)
                .mt_1()
                .h(px(36.0))
                .px_3()
                .rounded_sm()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .cursor_pointer()
                .bg(palette.surface)
                .border_1()
                .border_color(palette.border)
                .text_sm()
                .text_color(palette.text)
                .child(div().min_w_0().text_ellipsis().whitespace_nowrap().child(
                    SharedString::from(if value.is_empty() {
                        "—".to_owned()
                    } else {
                        value
                    }),
                ))
                .child(icon(ICON_CHEVRON_DOWN, palette.muted, 13.0))
                .on_click(on_click),
        )
        .into_any_element()
}

/// 选择项的点击回调：两个选择页共用同一套派发逻辑。
fn on_pick_option(app: &mut PureClash, value: String, cx: &mut Context<PureClash>) {
    if app.extra_rule_type_picker {
        app.select_extra_rule_type(value, cx);
    } else {
        app.select_extra_rule_target(value, cx);
    }
}

/// 第二张卡片：规则列表；列宽与表头保持一致，避免长内容挤动列。
fn list_card(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    let rules = &app.config.extra_rules;
    div()
        .p_4()
        .rounded_md()
        .bg(palette.surface)
        .border_1()
        .border_color(palette.border)
        .child(section_heading(
            tr("extra.list_title"),
            tr("extra.list_detail"),
            ICON_WINDOW_CLOSE,
            palette,
        ))
        .when(rules.is_empty(), |card| {
            card.child(state_block(
                tr("extra.empty"),
                tr("extra.empty_hint"),
                palette,
            ))
        })
        .when(!rules.is_empty(), |card| {
            card.child(table_header(palette))
                .children(rules.iter().map(|rule| table_row(rule, palette, cx)))
        })
        .into_any_element()
}

fn table_header(palette: Palette) -> AnyElement {
    div()
        .mt_4()
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .h(px(34.0))
        .bg(palette.surface_alt)
        .text_xs()
        .text_color(palette.muted)
        .child(div().w(px(48.0)).child(tr("extra.col_state")))
        .child(div().w(px(190.0)).child(tr("extra.col_type")))
        .child(div().flex_1().child(tr("extra.col_payload")))
        .child(div().w(px(190.0)).child(tr("extra.col_target")))
        .child(div().w(px(44.0)))
        .into_any_element()
}

fn table_row(rule: &ExtraRule, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    let enabled = rule.enabled;
    let toggle_id = rule.id.clone();
    let remove_id = rule.id.clone();
    div()
        .min_h(px(44.0))
        .px_3()
        .flex()
        .items_center()
        .gap_3()
        .border_b_1()
        .border_color(palette.border)
        .text_xs()
        .child(
            // 启停开关：停用的规则仍留在 app.json，只是不进入内核配置。
            div().w(px(48.0)).child(
                div()
                    .id(SharedString::from(format!("extra-rule-toggle-{}", rule.id)))
                    .h(px(22.0))
                    .px_2()
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(if enabled {
                        palette.success_soft
                    } else {
                        palette.surface_alt
                    })
                    .border_1()
                    .border_color(if enabled {
                        palette.success
                    } else {
                        palette.border
                    })
                    .text_color(if enabled {
                        palette.success
                    } else {
                        palette.muted
                    })
                    .child(if enabled {
                        tr("status.on")
                    } else {
                        tr("status.off")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_extra_rule(toggle_id.clone(), cx);
                    })),
            ),
        )
        .child(
            div()
                .w(px(190.0))
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(if enabled { palette.text } else { palette.muted })
                .child(SharedString::from(rule.rule_type.clone())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(if enabled { palette.text } else { palette.muted })
                .child(SharedString::from(if rule.payload.is_empty() {
                    "—".to_owned()
                } else {
                    rule.payload.clone()
                })),
        )
        .child(
            div()
                .w(px(190.0))
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(if enabled {
                    palette.accent
                } else {
                    palette.muted
                })
                .child(SharedString::from(rule.target.clone())),
        )
        .child(
            div()
                .id(SharedString::from(format!("extra-rule-remove-{}", rule.id)))
                .w(px(44.0))
                .flex()
                .justify_center()
                .cursor_pointer()
                .child(icon(ICON_WINDOW_CLOSE, palette.muted, 13.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.remove_extra_rule(remove_id.clone(), cx);
                })),
        )
        .into_any_element()
}

/// 卡片内通用小按钮。
fn plain_button(
    id: &'static str,
    label: SharedString,
    palette: Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .h(px(32.0))
        .flex_none()
        .px_3()
        .rounded_sm()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(palette.surface_alt)
        .border_1()
        .border_color(palette.border)
        .text_xs()
        .text_color(palette.muted)
        .child(label)
        .on_click(on_click)
        .into_any_element()
}
