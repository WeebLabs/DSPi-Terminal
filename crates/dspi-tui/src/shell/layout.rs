//! Where everything goes, by terminal size. The density table is
//! `docs/plan/DESIGN.md` section 8.

use ratatui::layout::Rect;

use super::model::GraphHeight;

/// The hard minimum. Below this the shell draws a message and nothing else.
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    /// 80x24: the minimum.
    Compact,
    /// 120x40: the reference layout.
    Normal,
    /// 160x50 and up.
    Roomy,
    /// 200x60 and up.
    Wide,
}

impl Density {
    pub fn of(width: u16, height: u16) -> Self {
        if width >= 200 && height >= 60 {
            Self::Wide
        } else if width >= 160 && height >= 50 {
            Self::Roomy
        } else if width >= 120 && height >= 40 {
            Self::Normal
        } else {
            Self::Compact
        }
    }

    pub fn sidebar_width(self) -> u16 {
        match self {
            Self::Compact => 22,
            Self::Normal => 24,
            Self::Roomy => 26,
            Self::Wide => 28,
        }
    }

    /// Rows for the graph at the medium height (the plot plus its label
    /// row; the legend row is separate).
    pub fn graph_rows(self, h: GraphHeight) -> u16 {
        let medium = match self {
            Self::Compact => 7,
            Self::Normal => 12,
            Self::Roomy => 16,
            Self::Wide => 20,
        };
        match h {
            GraphHeight::Hidden => 0,
            GraphHeight::Small => (medium * 2 / 3).max(5),
            GraphHeight::Medium => medium,
            GraphHeight::Large => medium * 3 / 2,
        }
    }

    /// Whether parameter captions are shown.
    pub fn captions(self) -> bool {
        self != Self::Compact
    }
}

/// The shell's regions for one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Regions {
    pub density: Density,
    pub title: Rect,
    /// The whole sidebar box, border included.
    pub sidebar: Rect,
    pub sidebar_list: Rect,
    pub sidebar_footer: Rect,
    /// The whole detail pane box, border included.
    pub pane: Rect,
    /// Inside the pane: graph plot area (may be zero-height) and the detail
    /// region beneath it.
    pub graph: Rect,
    pub detail: Rect,
    pub echo: Rect,
    pub keys: Rect,
}

/// The footer block needs seven rows: divider, strip, preset, source, volume
/// label, slider, cpu.
pub const FOOTER_ROWS: u16 = 7;

pub fn compute(area: Rect, graph_height: GraphHeight) -> Option<Regions> {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return None;
    }
    let density = Density::of(area.width, area.height);
    let title = Rect::new(area.x, area.y, area.width, 1);
    let echo = Rect::new(area.x, area.y + area.height - 2, area.width, 1);
    let keys = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 3);

    let sw = density.sidebar_width();
    let sidebar = Rect::new(body.x, body.y, sw, body.height);
    let pane = Rect::new(body.x + sw, body.y, body.width - sw, body.height);

    let inner_side = inset(sidebar);
    let footer_h = FOOTER_ROWS.min(inner_side.height);
    let sidebar_list = Rect::new(
        inner_side.x,
        inner_side.y,
        inner_side.width,
        inner_side.height - footer_h,
    );
    let sidebar_footer = Rect::new(
        inner_side.x,
        inner_side.y + inner_side.height - footer_h,
        inner_side.width,
        footer_h,
    );

    let inner_pane = inset(pane);
    let mut g = density.graph_rows(graph_height);
    // The detail always keeps at least six rows.
    if g + 6 > inner_pane.height {
        g = inner_pane.height.saturating_sub(6);
    }
    let graph = Rect::new(inner_pane.x, inner_pane.y, inner_pane.width, g);
    let detail = Rect::new(
        inner_pane.x,
        inner_pane.y + g,
        inner_pane.width,
        inner_pane.height.saturating_sub(g),
    );

    Some(Regions {
        density,
        title,
        sidebar,
        sidebar_list,
        sidebar_footer,
        pane,
        graph,
        detail,
        echo,
        keys,
    })
}

/// Inside a one-cell border.
pub fn inset(r: Rect) -> Rect {
    Rect::new(
        r.x + 1,
        r.y + 1,
        r.width.saturating_sub(2),
        r.height.saturating_sub(2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_layout_has_a_twelve_row_graph() {
        let r = compute(Rect::new(0, 0, 120, 40), GraphHeight::Medium).unwrap();
        assert_eq!(r.density, Density::Normal);
        assert_eq!(r.sidebar.width, 24);
        assert_eq!(r.graph.height, 12);
        assert_eq!(r.detail.y, r.graph.y + 12);
        assert_eq!(r.echo.y, 38);
        assert_eq!(r.keys.y, 39);
        assert_eq!(r.sidebar_footer.height, FOOTER_ROWS);
        assert_eq!(r.sidebar_list.height + r.sidebar_footer.height, 35);
    }

    #[test]
    fn the_minimum_fits_and_below_it_does_not() {
        let r = compute(Rect::new(0, 0, 80, 24), GraphHeight::Medium).unwrap();
        assert_eq!(r.density, Density::Compact);
        assert_eq!(r.graph.height, 7);
        assert!(r.detail.height >= 6, "{:?}", r.detail);
        assert!(compute(Rect::new(0, 0, 79, 24), GraphHeight::Medium).is_none());
        assert!(compute(Rect::new(0, 0, 80, 23), GraphHeight::Medium).is_none());
        let hidden = compute(Rect::new(0, 0, 80, 24), GraphHeight::Hidden).unwrap();
        assert_eq!(hidden.graph.height, 0);
        assert!(hidden.detail.height > r.detail.height);
    }

    #[test]
    fn wide_terminals_get_a_bigger_sidebar_and_graph() {
        let r = compute(Rect::new(0, 0, 200, 60), GraphHeight::Medium).unwrap();
        assert_eq!(r.density, Density::Wide);
        assert_eq!(r.sidebar.width, 28);
        assert_eq!(r.graph.height, 20);
        let r = compute(Rect::new(0, 0, 200, 60), GraphHeight::Large).unwrap();
        assert_eq!(r.graph.height, 30);
    }

    #[test]
    fn a_large_graph_still_leaves_the_detail_six_rows() {
        let r = compute(Rect::new(0, 0, 80, 24), GraphHeight::Large).unwrap();
        assert!(r.detail.height >= 6);
        assert_eq!(r.graph.height + r.detail.height, 19, "the pane's inside");
        assert_eq!(GraphHeight::Large.next(), GraphHeight::Hidden);
    }
}
