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

/// The fixture's firmware: beta 4, the one release that speaks wire V32.
pub const FIRMWARE: &str = "1.1.6 beta 4";

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
    m.firmware = FIRMWARE.into();
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

/// Point the model's graph at `selection`, computing the curves from a
/// device state the way the runner does: the channel's PEQ and crossover
/// bands with its trim folded in, and for the linked FL/FR pair the partner
/// underneath in grey. The gallery uses it so `--screen output` graphs the
/// output it shows.
pub fn select(m: &mut ShellModel, state: &DeviceState, theme: &Theme, selection: Selection) {
    m.selection = selection;
    let ni = state.caps.num_inputs as usize;
    let no = state.caps.num_outputs as usize;
    let channel = match selection {
        Selection::Overview => None,
        Selection::Input(i) => Some(i),
        Selection::Output(o) => Some(ni + o),
    };
    let bands_of = |ch: usize| -> (Vec<dsp::Band>, f64) {
        let band = |p: &dspi_proto::value::EqParamPacket| dsp::Band {
            filter_type: p.filter_type,
            freq: p.freq,
            q: p.q,
            gain_db: p.gain_db,
            bypass: p.bypass,
        };
        let mut bands: Vec<dsp::Band> = state.bands(ch as u8).iter().map(band).collect();
        if ch >= ni {
            bands.extend(state.xover_bands(ch as u8).iter().map(band));
            (bands, state.output(ch - ni).gain_db as f64)
        } else {
            (bands, 0.0)
        }
    };
    let mut curves = Vec::new();
    if let Some(ch) = channel {
        let partner = m
            .linked_pairs
            .iter()
            .find_map(|(a, b)| (ch == *a).then_some(*b).or((ch == *b).then_some(*a)));
        if let Some(p) = partner {
            let (bands, gain) = bands_of(p);
            let role = ChannelRole::of(p as u8, ni as u8, no as u8);
            curves.push(GraphCurve {
                descriptor: role.descriptor(no as u8),
                color: theme.dim,
                magnitude: dsp::curve(&bands, gain),
                phase: None,
                selected: false,
            });
        }
        let (bands, gain) = bands_of(ch);
        let role = ChannelRole::of(ch as u8, ni as u8, no as u8);
        curves.push(GraphCurve {
            descriptor: role.descriptor(no as u8),
            color: theme.role_color(role),
            magnitude: dsp::curve(&bands, gain),
            phase: Some(dsp::phase_curve(&bands)),
            selected: true,
        });
    }
    m.curves = curves;
    m.graph_channel = channel.map(|ch| state.channel_name(ch));
}

/// The device gone, projected as the runner projects it: no channel rows, no
/// curves, and the overview selected. The gallery's `--screen nodevice`.
pub fn disconnect(m: &mut ShellModel, state: &mut DeviceState) {
    state.connected = false;
    m.connected = false;
    m.inputs.clear();
    m.outputs.clear();
    m.curves.clear();
    m.graph_channel = None;
    m.selection = Selection::Overview;
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

/// The worst case for the overview grid: all seventeen channels tuned, no
/// two alike and none flat, so nothing collapses. Each input gets a peaking
/// band at its own frequency; each output its own crossover corner, with
/// OUT 5 back on.
pub fn full_packet() -> Vec<u8> {
    let mut b = busy_packet();
    let eq = section("eq");
    for ch in 0..8 {
        // Band 10 (index 9) is unused by every busy tuning.
        let off = eq + (ch * 12 + 9) * 16;
        b[off] = FilterType::Peaking.to_raw();
        b[off + 4..off + 8].copy_from_slice(&(300.0f32 + 400.0 * ch as f32).to_le_bytes());
        b[off + 8..off + 12].copy_from_slice(&1.5f32.to_le_bytes());
        b[off + 12..off + 16].copy_from_slice(&(2.0f32 + 0.5 * ch as f32).to_le_bytes());
    }
    let xo = section("crossovers");
    for o in 0..9 {
        let ch = 8 + o;
        let off = xo + ch * 4 * 16;
        // LR4 high passes at rising corners; the sub keeps its low pass.
        if o < 8 {
            b[off] = 35;
            b[off + 4..off + 8].copy_from_slice(&(60.0f32 + 20.0 * o as f32).to_le_bytes());
            b[off + 8..off + 12].copy_from_slice(&0.707f32.to_le_bytes());
        }
    }
    let outs = section("outputs");
    b[outs + 4 * 12] = 1;
    b
}

/// [`full_packet`] as a device state.
pub fn full_state() -> DeviceState {
    DeviceState::new(
        caps(),
        BulkPacket::decode(full_packet()).expect("fixture packet"),
    )
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
        // The firmware the Terminal speaks to, spelt as the probe spells it.
        firmware: FIRMWARE.into(),
        firmware_version: dspi_proto::packets::FirmwareVersion::new(1, 1, 6, 4),
        build_info: None,
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

/// Give `state` a Tube Modeller: the probe's `tube_preamp` feature, and the
/// firmware's power-on values (tube.h:61-73) in the tube section at
/// `WireTubeParams` (bulk_params.h:403-427), with the master switch as asked.
/// The plain fixture leaves the section zeroed, which no device reports.
pub fn tube(state: &mut DeviceState, enabled: bool) {
    use dspi_proto::generated::{ranges as r, tube as t};
    state.caps.features.push(dspi_session::probe::Feature {
        name: "tube_preamp".into(),
        present: true,
        evidence: "fixture".into(),
    });
    let o = section("tube");
    // enabled, tube_type, rectifier, xfmr_enabled (TUBE_DEFAULT_XFMR_ENABLED).
    let head = [
        u8::from(enabled),
        t::TUBE_DEFAULT_TUBE_TYPE as u8,
        t::TUBE_DEFAULT_RECTIFIER as u8,
        1,
    ];
    state.bulk.patch(o, &head);
    state
        .bulk
        .patch(o + 4, &t::TUBE_DEFAULT_OUTPUT_MASK.to_le_bytes());
    let floats = [
        r::TUBE_DEFAULT_DRIVE,
        r::TUBE_DEFAULT_BIAS,
        r::TUBE_DEFAULT_ASYM,
        r::TUBE_DEFAULT_HARDNESS,
        r::TUBE_DEFAULT_SAG,
        r::TUBE_DEFAULT_XFMR_DAMPING,
        r::TUBE_DEFAULT_XFMR_RES,
        r::TUBE_DEFAULT_MIX,
        r::TUBE_DEFAULT_TRIM,
    ];
    for (i, v) in floats.iter().enumerate() {
        state.bulk.patch(o + 8 + 4 * i, &v.to_le_bytes());
    }
}

/// Give the shared analyser an RP2350's caps and a picture of the first
/// output, bins and all, so the Spectrum Analyser panel has something to draw
/// without a device. `channels` picks what is chosen at the output tap.
pub fn spectrum(shared: &crate::screens::Shared, channels: &[u8]) {
    crate::screens::spectrum::demo::load(shared, dspi_session::rta::TAP_OUTPUT, channels);
}
