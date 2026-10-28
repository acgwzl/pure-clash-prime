//! 共享 UI 组件库：六页共用的容器、徽章、开关、文本档位。
//!
//! 与浏览器原型（D:\pure-clash-tools\web\pages.css）一一对应：
//! 这里改动一处 = 六页同步生效。页面代码只允许通过本库取容器与文字档位，
//! 禁止各自手写 `.bg(p.surface)` + 圆角 + 阴影的三件套，否则跨页必然漂移。

use gpui::{BoxShadow, Div, ParentElement, SharedString, Styled, div, px, relative};

use crate::theme::{FontWeightExt, Palette, text_size};

// ---------------------------------------------------------------- 容器

/// 卡片：页面里抬升一层的容器。
/// 深浅两主题都靠「明度差 + 1px 边 + 阴影」三层表达深度；
/// 单独改掉任何一层，另一主题就会失去立体感。
pub(crate) fn card(p: Palette) -> Div {
    div()
        .bg(p.surface)
        .border_1()
        .border_color(p.border)
        .rounded_xl()
        .shadow(card_shadow(p))
}

/// 主面板：比普通卡片再抬一档，概览英雄区专用。
pub(crate) fn hero_card(p: Palette) -> Div {
    div()
        .bg(p.surface_high)
        .border_1()
        .border_color(p.border)
        .rounded_2xl()
        .shadow(hero_shadow(p))
}

/// 嵌入底：表头、节点网格的衬底、次级按钮——比所在容器更淡一档。
pub(crate) fn inset_panel(p: Palette) -> Div {
    div().bg(p.inset).rounded_xl()
}

/// 卡片阴影：向下 10px、28px 模糊，颜色来自主题（浅色主题用淡墨、深色用纯黑）。
pub(crate) fn card_shadow(p: Palette) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(10.), p.shadow.into()).blur_radius(px(28.))]
}

/// 主面板阴影：更重的一档。
pub(crate) fn hero_shadow(p: Palette) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(18.), p.shadow_strong.into()).blur_radius(px(44.))]
}

/// 品牌发光：状态点、选中导航的柔光。
/// 浅色主题的 glow 是全透明，发光自然关闭——不发光是设计，不是遗漏。
pub(crate) fn glow(p: Palette, radius: f32) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(0.), p.glow.into()).blur_radius(px(radius))]
}

// ---------------------------------------------------------------- 文本档位

/// 页面级标签：「当前出站」「实时速率」这类小节标题。
pub(crate) fn label(text: impl Into<SharedString>, p: Palette) -> Div {
    div()
        .text_size(px(text_size::LABEL))
        .text_color(p.muted)
        .child(text.into())
}

/// 三级说明：单位、时间戳、行号。颜色必须仍满足对比度下限，
/// 评审已确认 faint 在两套主题里都 ≥4.5:1，改色前先复测。
pub(crate) fn hint(text: impl Into<SharedString>, p: Palette) -> Div {
    div()
        .text_size(px(text_size::HINT))
        .text_color(p.faint)
        .child(text.into())
}

/// 卡片标题。
pub(crate) fn title(text: impl Into<SharedString>, p: Palette) -> Div {
    div()
        .text_size(px(text_size::CARD_TITLE))
        .font_semibold()
        .text_color(p.text)
        .child(text.into())
}

/// 分组标题：标题在卡片上方，卡顶 = 标题下 8px——
/// 这是评审定下的规则，设置页三处分组都走这里，别在页面里重画。
pub(crate) fn section_head(
    title_text: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    p: Palette,
) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(text_size::CARD_TITLE))
                .font_semibold()
                .text_color(p.text)
                .child(title_text.into()),
        )
        .child(
            div()
                .text_size(px(text_size::HINT))
                .text_color(p.faint)
                .child(subtitle.into()),
        )
}

// ---------------------------------------------------------------- 控件

/// 状态点：常驻「运行中」的绿点，带品牌级柔光。
pub(crate) fn status_dot(p: Palette, color: gpui::Rgba) -> Div {
    div()
        .w(px(8.))
        .h(px(8.))
        .rounded_full()
        .bg(color)
        .shadow(glow(p, 10.))
}

/// 进度条：轨道恒为 inset 一档，填充按语义取色。
/// ratio 下限 2%：空条和满条看起来不能一样。
pub(crate) fn progress(p: Palette, ratio: f32, fill: gpui::Rgba) -> Div {
    div().w_full().h(px(6.)).rounded_full().bg(p.track).child(
        div()
            .h_full()
            .rounded_full()
            .bg(fill)
            .w(relative(ratio.max(0.02))),
    )
}
