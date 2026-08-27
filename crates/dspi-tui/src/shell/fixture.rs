//! Fixture models for tests, the gallery and `dspi screenshot`.

use dspi_proto::FilterType;
use dspi_proto::dsp;

use super::model::{ChannelItem, Selection, ShellModel, VolumeMode};
use crate::graph::GraphCurve;
use crate::theme::{ChannelRole, Theme};

fn item(
    theme: &Theme,
    name: &str,
    role: ChannelRole,
    index: u8,
    num_outputs: u8,
    level: f32,
) -> ChannelItem {
    ChannelItem {
        name: name.into(),
        descriptor: role.descriptor(num_outputs),
        role,
        color: theme.role_color(role),
        level,
        peak: level * 1.2,
        clipped: false,
        visible: true,
        inactive: false,
        index,
    }
}

fn tuning() -> Vec<dsp::Band> {
    [
        (FilterType::LowShelf, 105.0, 0.707, 8.8),
        (FilterType::Peaking, 64.0, 0.30, -6.2),
        (FilterType::Peaking, 2856.0, 3.58, -8.6),
        (FilterType::Peaking, 1880.0, 1.69, 3.6),
        (FilterType::Peaking, 6749.0, 4.74, 4.0),
    ]
    .iter()
    .map(|(t, f, q, g)| dsp::Band {
        filter_type: *t,
        freq: *f,
        q: *q,
        gain_db: *g,
        bypass: false,
    })
    .collect()
}

fn curve(item: &ChannelItem, bands: &[dsp::Band], gain: f64, selected: bool) -> GraphCurve {
    GraphCurve {
        descriptor: item.descriptor.clone(),
        color: item.color,
        magnitude: dsp::curve(bands, gain),
        phase: Some(dsp::phase_curve(bands)),
        selected,
        visible: item.visible,
    }
}

/// An RP2350 with eight inputs, eight S/PDIF outputs and the sub, with a
/// headphone tuning on the first stereo pair and a crossover on the sub.
pub fn rp2350(theme: &Theme) -> ShellModel {
    let mut m = ShellModel::empty();
    m.platform = "RP2350".into();
    m.firmware = "1.1.6".into();
    m.serial_short = "A1B2C3D4".into();
    m.connected = true;
    m.devices = vec!["DSPi A1B2C3D4".into()];
    m.preset_label = "3: Living Room".into();
    m.preset_dirty = true;
    let names = ["FL", "FR", "FC", "LFE", "BL", "BR", "SL", "SR"];
    let levels = [0.62, 0.48, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    m.inputs = names
        .iter()
        .zip(levels)
        .enumerate()
        .map(|(i, (n, l))| item(theme, n, ChannelRole::Input(i as u8), i as u8, 9, l))
        .collect();
    let out_names = [
        "OUT L", "OUT R", "OUT 3", "OUT 4", "OUT 5", "OUT 6", "OUT 7", "OUT 8",
    ];
    let out_levels = [0.31, 0.31, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    m.outputs = out_names
        .iter()
        .zip(out_levels)
        .enumerate()
        .map(|(o, (n, l))| item(theme, n, ChannelRole::Output(o as u8), 8 + o as u8, 9, l))
        .collect();
    let mut sub = item(theme, "Sub", ChannelRole::Sub, 16, 9, 0.72);
    sub.clipped = true;
    m.outputs.push(sub);
    m.outputs[4].inactive = true;
    m.outputs[5].inactive = true;
    m.linked_pairs = vec![(0, 1)];
    m.selection = Selection::Input(0);
    m.strip[1].state = Some(true);
    m.strip[4].state = Some(true);
    m.source = Some((vec!["USB".into(), "S/PDIF".into(), "I2S".into()], 0));
    m.volume_mode = VolumeMode::User;
    m.volume_db = -12.0;
    m.cpu = (31, 74);
    m.cursor_hz = None;
    m.marker_hz = Some(2856.0);
    m.echo = ":eq in.1 3 freq 2856".into();

    let tuned = tuning();
    let flat: Vec<dsp::Band> = Vec::new();
    let lp = vec![dsp::Band {
        filter_type: FilterType::LowPass,
        freq: 80.0,
        q: 0.707,
        gain_db: 0.0,
        bypass: false,
    }];
    let hp = vec![dsp::Band {
        filter_type: FilterType::HighPass,
        freq: 80.0,
        q: 0.707,
        gain_db: 0.0,
        bypass: false,
    }];
    let mut curves = Vec::new();
    for (i, it) in m.inputs.iter().enumerate() {
        let bands = if i < 2 { &tuned } else { &flat };
        curves.push(curve(it, bands, 0.0, m.selection == Selection::Input(i)));
    }
    for (o, it) in m.outputs.iter().enumerate() {
        let bands = if o < 2 {
            &hp
        } else if o == 8 {
            &lp
        } else {
            &flat
        };
        let gain = if o == 8 { -3.0 } else { 0.0 };
        curves.push(curve(it, bands, gain, false));
    }
    m.curves = curves;
    m
}

/// An RP2040: two inputs, four outputs and the sub.
pub fn rp2040(theme: &Theme) -> ShellModel {
    let mut m = rp2350(theme);
    m.platform = "RP2040".into();
    m.inputs.truncate(2);
    m.outputs = m.outputs.iter().take(4).cloned().collect();
    for (o, it) in m.outputs.iter_mut().enumerate() {
        it.role = ChannelRole::Output(o as u8);
        it.descriptor = it.role.descriptor(5);
        it.color = theme.role_color(it.role);
        it.index = 2 + o as u8;
        it.inactive = false;
    }
    let mut sub = item(theme, "Sub", ChannelRole::Sub, 6, 5, 0.4);
    sub.descriptor = "OUT5".into();
    m.outputs.push(sub);
    m.curves.truncate(2);
    let flat: Vec<dsp::Band> = Vec::new();
    for it in &m.outputs {
        m.curves.push(curve(it, &flat, 0.0, false));
    }
    m
}
