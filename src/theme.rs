use std::sync::Arc;

use gpui::{FontFeatures, FontWeight, Rgba, Styled, rgb, rgba};

/// 为当前 GPUI 版本补齐常用字重语义，保持 UI 声明式代码易读。
pub(crate) trait FontWeightExt: Styled + Sized {
    fn font_medium(self) -> Self {
        self.font_weight(FontWeight::MEDIUM)
    }

    fn font_semibold(self) -> Self {
        self.font_weight(FontWeight::SEMIBOLD)
    }
}

impl<T: Styled> FontWeightExt for T {}

/// 数字排印：本产品正文以数字为主（延迟、速率、流量、命中数），
/// 比例数字会随数值变化左右跳动，开启 tnum 后各列对齐、刷新不抖。
pub(crate) trait TabularNums: Styled + Sized {
    fn tabnum(self) -> Self {
        self.font_features(FontFeatures(Arc::new(vec![("tnum".to_owned(), 1)])))
    }
}

impl<T: Styled> TabularNums for T {}

/// 界面排版档位。
///
/// 刻意不使用 GPUI 的 Tailwind 字号档位（xs=12 / sm=14 …）：本产品的字号阶梯
/// 是 10.5 / 11.5 / 12.5 / 13.5 这种半像素级微差，档位化之后层次会被压平。
/// 因此统一用 `text_size(px(..))` 直接给值，并在这里集中列出，避免各页各写一套。
pub(crate) mod text_size {
    pub(crate) const HERO_NUM: f32 = 52.0;
    pub(crate) const HERO_TITLE: f32 = 27.0;
    pub(crate) const STAT_NUM: f32 = 25.0;
    pub(crate) const PAGE_TITLE: f32 = 14.0;
    pub(crate) const CARD_TITLE: f32 = 13.5;
    pub(crate) const BODY: f32 = 13.0;
    pub(crate) const ROW: f32 = 12.5;
    pub(crate) const META: f32 = 11.5;
    pub(crate) const LABEL: f32 = 11.0;
    pub(crate) const HINT: f32 = 10.5;
    pub(crate) const TINY: f32 = 10.0;
}

/// 配色令牌。
///
/// 层次靠「底色 → 表面 → 抬升面」的明度差表达，而不是给每个盒子描边：
/// 描边只用于真正需要分隔的地方，否则整页会退化成等宽的细线网格。
///
/// 品牌紫取自 logo。**强调色与品牌色同值**：上一版把 accent 做成另一个蓝，
/// 结果一屏里出现紫、蓝两个互不相干的强调体系；现在紫是唯一的实色强调，
/// 绿/琥珀/红只承担状态语义，不参与装饰。
///
/// 圆角按「容器 > 控件」取档：卡片 rounded_lg，按钮与输入 rounded_md，浮层 rounded_xl。
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    /// 应用底色：整个窗口的最低层。
    pub(crate) background: Rgba,
    /// 表面：卡片、列表行等抬升一层的容器。
    pub(crate) surface: Rgba,
    /// 表面替代色：次级容器、选中行底色。
    pub(crate) surface_alt: Rgba,
    /// 表面高光：栈顶元素（浮层、主面板）用，明度高于 surface。
    pub(crate) surface_high: Rgba,
    /// 分隔线：只用于结构性分隔，不用于每个盒子的轮廓。
    pub(crate) border: Rgba,
    pub(crate) text: Rgba,
    pub(crate) muted: Rgba,
    /// 品牌色：logo 紫罗兰，用于品牌标识与选中态。
    pub(crate) brand: Rgba,
    /// 品牌色的低饱和底：承载品牌色的图标底、标签底。
    pub(crate) brand_soft: Rgba,
    /// 交互色：与品牌色同值，见结构体文档。
    pub(crate) accent: Rgba,
    pub(crate) accent_soft: Rgba,
    pub(crate) success: Rgba,
    pub(crate) success_soft: Rgba,
    /// 危险色：错误提示、额度超限、延迟超时。
    pub(crate) danger: Rgba,
    pub(crate) danger_soft: Rgba,
    /// 侧栏底色：与页面底色刻意不同，让侧栏成为「另一块面板」而不是同色区域。
    pub(crate) sidebar: Rgba,
    /// 嵌入底：表头、轨道、次级按钮——比所在容器更暗/更淡一档。
    pub(crate) inset: Rgba,
    /// 三级文字：说明、单位、时间戳。必须仍满足正文对比度下限。
    pub(crate) faint: Rgba,
    /// 品牌描边：选中态容器的 1px 轮廓。
    pub(crate) brand_line: Rgba,
    /// 警示色：延迟中档、额度将尽。
    pub(crate) amber: Rgba,
    pub(crate) amber_soft: Rgba,
    /// 进度轨道。
    pub(crate) track: Rgba,
    /// 开关关闭态轨道。
    pub(crate) off: Rgba,
    /// 开关滑块。
    pub(crate) knob: Rgba,
    /// 卡片阴影色。
    pub(crate) shadow: Rgba,
    /// 主面板/浮层阴影色，比 shadow 更重。
    pub(crate) shadow_strong: Rgba,
    /// 品牌发光色；浅色主题为全透明（浅底上发光会显脏）。
    pub(crate) glow: Rgba,
}

impl Palette {
    /// 浅色主题「纸白控制台」：白卡浮在浅灰底上，靠阴影而不是描边分层次。
    pub(crate) fn light() -> Self {
        Self {
            background: rgb(0xf2f3f6),
            surface: rgb(0xffffff),
            surface_alt: rgb(0xf4f5f8),
            surface_high: rgb(0xffffff),
            border: rgba(0x10182812),
            text: rgb(0x1a1d26),
            muted: rgb(0x656c7a),
            brand: rgb(0x745bff),
            brand_soft: rgba(0x745bff1a),
            accent: rgb(0x745bff),
            accent_soft: rgba(0x745bff1a),
            success: rgb(0x15803d),
            success_soft: rgb(0xdcfce7),
            danger: rgb(0xb91c1c),
            danger_soft: rgb(0xfee2e2),
            sidebar: rgb(0xffffff),
            inset: rgb(0xf4f5f8),
            faint: rgb(0x8a9099),
            brand_line: rgba(0x745bff47),
            amber: rgb(0xb45309),
            amber_soft: rgb(0xfef3c7),
            track: rgba(0x10182814),
            off: rgb(0xd1d5db),
            knob: rgb(0xffffff),
            shadow: rgba(0x10182814),
            shadow_strong: rgba(0x10182824),
            glow: rgba(0x745bff00),
        }
    }

    /// 深色主题「午夜仪表」：冷调蓝黑，三级明度把卡片抬起来。
    pub(crate) fn dark() -> Self {
        Self {
            background: rgb(0x0a0c14),
            surface: rgb(0x161a28),
            surface_alt: rgb(0x1f2536),
            surface_high: rgb(0x1c2138),
            border: rgba(0xffffff13),
            text: rgb(0xe9edf7),
            muted: rgb(0x8b93a7),
            brand: rgb(0x8b75ff),
            brand_soft: rgba(0x8b75ff29),
            accent: rgb(0x8b75ff),
            accent_soft: rgba(0x8b75ff29),
            success: rgb(0x5ed69a),
            success_soft: rgba(0x5ed69a26),
            danger: rgb(0xe0705f),
            danger_soft: rgba(0xe0705f26),
            sidebar: rgb(0x12151f),
            inset: rgba(0xffffff0d),
            faint: rgb(0x6b7286),
            brand_line: rgba(0x8b75ff57),
            amber: rgb(0xe8b060),
            amber_soft: rgba(0xe8b06026),
            track: rgba(0xffffff14),
            off: rgba(0xffffff26),
            knob: rgb(0xffffff),
            shadow: rgba(0x00000073),
            shadow_strong: rgba(0x0000008c),
            glow: rgba(0x8b75ff59),
        }
    }
}
