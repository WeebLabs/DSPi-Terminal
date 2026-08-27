//! What the shell draws from: a view model the application fills in from the
//! device state. Keeping it separate lets the shell be rendered and tested
//! with fixtures, and lets the screens stay ignorant of USB.

use ratatui::style::Color;

use crate::graph::{GraphCurve, GraphSettings};
use crate::theme::ChannelRole;

/// One row in the sidebar.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelItem {
    pub name: String,
    /// `IN1`, `OUT3`.
    pub descriptor: String,
    pub role: ChannelRole,
    pub color: Color,
    /// Linear 0..1 as the device reports it.
    pub level: f32,
    pub peak: f32,
    pub clipped: bool,
    /// Whether its curve is shown on the graph.
    pub visible: bool,
    /// Muted or disabled: drawn dim.
    pub inactive: bool,
    /// The unified channel index, for the screens.
    pub index: u8,
}

/// The sidebar selection, which drives the detail region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Overview,
    /// Index into `ShellModel::inputs`.
    Input(usize),
    /// Index into `ShellModel::outputs`.
    Output(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeMode {
    User,
    Master,
}

/// One cell of the quick strip under the channel list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripItem {
    pub key: char,
    pub label: &'static str,
    /// `Some(on)` for the toggleable features; `None` for plain openers.
    pub state: Option<bool>,
}

/// The graph's share of the detail pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphHeight {
    Small,
    #[default]
    Medium,
    Large,
    Hidden,
}

impl GraphHeight {
    pub fn next(self) -> Self {
        match self {
            Self::Small => Self::Medium,
            Self::Medium => Self::Large,
            Self::Large => Self::Hidden,
            Self::Hidden => Self::Small,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShellModel {
    pub platform: String,
    pub firmware: String,
    pub serial_short: String,
    pub connected: bool,
    /// Every attached device, by display name; more than one enables the
    /// picker.
    pub devices: Vec<String>,
    /// `3: Living Room`, or `Empty`.
    pub preset_label: String,
    pub preset_dirty: bool,
    pub inputs: Vec<ChannelItem>,
    pub outputs: Vec<ChannelItem>,
    pub selection: Selection,
    /// Two inputs that are linked highlight together.
    pub linked_pairs: Vec<(usize, usize)>,
    pub strip: Vec<StripItem>,
    pub source: Option<(Vec<String>, usize)>,
    pub volume_mode: VolumeMode,
    pub volume_db: f64,
    pub cpu: (u8, u8),
    pub graph: GraphSettings,
    pub graph_height: GraphHeight,
    pub curves: Vec<GraphCurve>,
    pub cursor_hz: Option<f64>,
    pub marker_hz: Option<f64>,
    /// The echo line: the last command, dimmed.
    pub echo: String,
    /// A status message that overrides the echo line for a few seconds.
    pub status: Option<String>,
}

impl ShellModel {
    /// The Console's quick strip, in its order.
    pub fn default_strip() -> Vec<StripItem> {
        vec![
            StripItem {
                key: 'M',
                label: "Matrix",
                state: None,
            },
            StripItem {
                key: 'X',
                label: "Xfeed",
                state: Some(false),
            },
            StripItem {
                key: 'L',
                label: "Loud",
                state: Some(false),
            },
            StripItem {
                key: 'V',
                label: "Lev",
                state: Some(false),
            },
            StripItem {
                key: 'P',
                label: "Bass",
                state: Some(false),
            },
            StripItem {
                key: 'T',
                label: "Stats",
                state: None,
            },
            StripItem {
                key: ',',
                label: "Settings",
                state: None,
            },
            StripItem {
                key: 'b',
                label: "Bypass",
                state: Some(false),
            },
        ]
    }

    pub fn empty() -> Self {
        Self {
            platform: String::new(),
            firmware: String::new(),
            serial_short: String::new(),
            connected: false,
            devices: Vec::new(),
            preset_label: "Empty".into(),
            preset_dirty: false,
            inputs: Vec::new(),
            outputs: Vec::new(),
            selection: Selection::Overview,
            linked_pairs: Vec::new(),
            strip: Self::default_strip(),
            source: None,
            volume_mode: VolumeMode::User,
            volume_db: 0.0,
            cpu: (0, 0),
            graph: GraphSettings::default(),
            graph_height: GraphHeight::Medium,
            curves: Vec::new(),
            cursor_hz: None,
            marker_hz: None,
            echo: String::new(),
            status: None,
        }
    }

    /// The sidebar rows in order: inputs then outputs.
    pub fn channel_count(&self) -> usize {
        self.inputs.len() + self.outputs.len()
    }

    /// The selection a sidebar row index corresponds to.
    pub fn selection_of_row(&self, row: usize) -> Selection {
        if row < self.inputs.len() {
            Selection::Input(row)
        } else {
            Selection::Output(row - self.inputs.len())
        }
    }

    pub fn row_of_selection(&self) -> Option<usize> {
        match self.selection {
            Selection::Overview => None,
            Selection::Input(i) => Some(i),
            Selection::Output(o) => Some(self.inputs.len() + o),
        }
    }

    /// The channel item for the selection, if any.
    pub fn selected_item(&self) -> Option<&ChannelItem> {
        match self.selection {
            Selection::Overview => None,
            Selection::Input(i) => self.inputs.get(i),
            Selection::Output(o) => self.outputs.get(o),
        }
    }
}
