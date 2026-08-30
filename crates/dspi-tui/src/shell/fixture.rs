//! Fixture models for tests, the gallery and `dspi screenshot`.

use dspi_proto::dsp;
use dspi_proto::generated::{BULK_SIZE, SECTIONS, wire::WIRE_FORMAT_VERSION};
use dspi_proto::wire::BulkPacket;
use dspi_proto::{FilterType, Platform};
use dspi_session::probe::ChannelInfo;
use dspi_session::{Capabilities, DeviceState};

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
    // FL is selected and linked to FR, so the graph carries FR's curve in
    // grey underneath FL's. The outputs' tunings live in `state()`; the
    // shell model only carries what the graph draws.
    let mut partner = curve(&m.inputs[1], &tuned, 0.0, false);
    partner.color = theme.dim;
    m.curves = vec![partner, curve(&m.inputs[0], &tuned, 0.0, true)];
    m.graph_channel = Some("FL".into());
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
    m
}

fn section(name: &str) -> usize {
    SECTIONS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, o, _)| *o)
        .expect("section")
}

/// A V28 bulk packet for an RP2350 with the same tuning as [`rp2350`]: FL
/// and FR carry the headphone bands, the outputs are enabled, OUT L / OUT R
/// have an 80 Hz high pass, the sub an 80 Hz low pass at -3 dB.
pub fn packet() -> Vec<u8> {
    let mut b = vec![0u8; BULK_SIZE];
    b[0] = WIRE_FORMAT_VERSION as u8;
    b[1] = 1;
    b[2] = 17;
    b[3] = 9;
    b[4] = 8;
    b[5] = 12;
    b[6..8].copy_from_slice(&(BULK_SIZE as u16).to_le_bytes());
    let g = section("global");
    b[g..g + 4].copy_from_slice(&(-5.3f32).to_le_bytes());
    let u = section("user_volume");
    b[u..u + 4].copy_from_slice(&(-12.0f32).to_le_bytes());
    let outs = section("outputs");
    for o in 0..9 {
        b[outs + o * 12] = 1;
    }
    b[outs + 8 * 12 + 4..outs + 8 * 12 + 8].copy_from_slice(&(-3.0f32).to_le_bytes());
    let names = section("channel_names");
    for (i, n) in [
        "FL", "FR", "FC", "LFE", "BL", "BR", "SL", "SR", "OUT L", "OUT R", "OUT 3", "OUT 4",
        "OUT 5", "OUT 6", "OUT 7", "OUT 8", "Sub",
    ]
    .iter()
    .enumerate()
    {
        b[names + i * 32..names + i * 32 + n.len()].copy_from_slice(n.as_bytes());
    }
    let eq = section("eq");
    for ch in 0..2 {
        for (band, t) in tuning().iter().enumerate() {
            let off = eq + (ch * 12 + band) * 16;
            b[off] = t.filter_type.to_raw();
            b[off + 4..off + 8].copy_from_slice(&t.freq.to_le_bytes());
            b[off + 8..off + 12].copy_from_slice(&t.q.to_le_bytes());
            b[off + 12..off + 16].copy_from_slice(&t.gain_db.to_le_bytes());
        }
    }
    let xo = section("crossovers");
    for (ch, ty) in [
        (8usize, FilterType::HighPass),
        (9, FilterType::HighPass),
        (16, FilterType::LowPass),
    ] {
        let off = xo + ch * 4 * 16;
        b[off] = ty.to_raw();
        b[off + 4..off + 8].copy_from_slice(&80.0f32.to_le_bytes());
        b[off + 8..off + 12].copy_from_slice(&0.707f32.to_le_bytes());
    }
    let cross = section("crosspoints");
    for i in 0..8 {
        let off = cross + (i * 9 + i) * 8;
        b[off] = 1;
    }
    b
}

/// The same device with every channel tuned differently, for a review of
/// the overview grid: FL/FR share the headphone tuning, FC, LFE and the
/// BL/BR pair have their own, SL/SR are flat; OUT L/R carry an LR4 high
/// pass and two bands, OUT 3/4 a Butterworth high pass at -2 dB, OUT 5 is
/// off, OUT 6 to 8 are flat, the sub keeps its low pass with 2.5 ms of
/// delay.
pub fn busy_packet() -> Vec<u8> {
    let mut b = packet();
    let eq = section("eq");
    let band = |b: &mut Vec<u8>, off: usize, t: FilterType, f: f32, q: f32, g: f32| {
        b[off] = t.to_raw();
        b[off + 4..off + 8].copy_from_slice(&f.to_le_bytes());
        b[off + 8..off + 12].copy_from_slice(&q.to_le_bytes());
        b[off + 12..off + 16].copy_from_slice(&g.to_le_bytes());
    };
    let peq = |b: &mut Vec<u8>, ch: usize, bands: &[(FilterType, f32, f32, f32)]| {
        for (i, (t, f, q, g)) in bands.iter().enumerate() {
            band(b, eq + (ch * 12 + i) * 16, *t, *f, *q, *g);
        }
    };
    use FilterType::{HighShelf, LowShelf, Notch, Peaking};
    peq(
        &mut b,
        2,
        &[
            (HighShelf, 3200.0, 0.707, -3.0),
            (Peaking, 250.0, 1.2, 2.5),
            (Notch, 60.0, 8.0, 0.0),
        ],
    );
    peq(
        &mut b,
        3,
        &[(LowShelf, 60.0, 0.707, 6.0), (Peaking, 45.0, 2.0, -4.0)],
    );
    for ch in [4, 5] {
        peq(
            &mut b,
            ch,
            &[
                (Peaking, 120.0, 1.0, -2.0),
                (Peaking, 900.0, 2.5, 1.5),
                (Peaking, 4500.0, 3.0, -3.5),
                (HighShelf, 10000.0, 0.707, 2.0),
            ],
        );
    }
    for ch in [8, 9] {
        peq(
            &mut b,
            ch,
            &[(Peaking, 180.0, 1.4, -1.5), (Peaking, 2200.0, 2.0, 1.0)],
        );
    }
    let xo = section("crossovers");
    // LR4 high pass (raw 35) on OUT L/R, BW2 high pass (raw 43) on OUT 3/4,
    // LR4 low pass (raw 34) on the sub.
    for (ch, raw, f) in [
        (8usize, 35u8, 80.0f32),
        (9, 35, 80.0),
        (10, 43, 100.0),
        (11, 43, 100.0),
        (16, 34, 80.0),
    ] {
        let off = xo + ch * 4 * 16;
        b[off] = raw;
        b[off + 4..off + 8].copy_from_slice(&f.to_le_bytes());
        b[off + 8..off + 12].copy_from_slice(&0.707f32.to_le_bytes());
    }
    let outs = section("outputs");
    for o in [2, 3] {
        b[outs + o * 12 + 4..outs + o * 12 + 8].copy_from_slice(&(-2.0f32).to_le_bytes());
    }
    b[outs + 4 * 12] = 0;
    b[outs + 8 * 12 + 8..outs + 8 * 12 + 12].copy_from_slice(&2.5f32.to_le_bytes());
    let pre = section("preamp");
    for i in 0..8 {
        b[pre + i * 4..pre + i * 4 + 4].copy_from_slice(&(-5.3f32).to_le_bytes());
    }
    b
}

/// [`busy_packet`] as a device state.
pub fn busy_state() -> DeviceState {
    DeviceState::new(
        caps(),
        BulkPacket::decode(busy_packet()).expect("fixture packet"),
    )
}

pub fn caps() -> Capabilities {
    Capabilities {
        serial: "E6614C311B8B4E3A".into(),
        platform: Platform::Rp2350,
        firmware: "1.1.6".into(),
        wire_format: WIRE_FORMAT_VERSION as u8,
        num_channels: 17,
        num_inputs: 8,
        num_outputs: 9,
        max_bands: 10,
        band_storage: 12,
        channels: (0..17u8)
            .map(|i| ChannelInfo {
                index: i,
                name: String::new(),
                slug: if i < 8 {
                    format!("in.{}", i + 1)
                } else {
                    format!("out.{}", i - 7)
                },
                is_output: i >= 8,
            })
            .collect(),
        features: Vec::new(),
        cs: None,
        siggen: None,
        active_preset: Some(2),
    }
}

/// A device state built from [`packet`] and [`caps`], for screens' tests.
pub fn state() -> DeviceState {
    DeviceState::new(
        caps(),
        BulkPacket::decode(packet()).expect("fixture packet"),
    )
}
