//! The parameter registry: one declarative table describing every parameter the
//! firmware exposes.
//!
//! Everything user-facing is generated from this table rather than written twice:
//! the command grammar, completion, the palette, widget selection, validation,
//! help text, the export schema, and the control-surface binding picker.
//!
//! Adding a firmware parameter is adding a row. The coverage test in
//! `tests/coverage.rs` fails the build if an opcode is neither registered here
//! nor listed in [`EXCLUDED`] with a written reason, which is what makes
//! "controls every setting" a checkable claim rather than an aspiration.

use crate::Dir;
use crate::generated::opcodes as op;
use crate::value::{Repr, Unit, Value, ValueError};

/// Which panel owns a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Main,
    Input,
    Filters,
    Matrix,
    Dynamics,
    Spatial,
    Surfaces,
    Presets,
    System,
    Diagnostics,
}

/// Progressive disclosure. Levels only hide controls; they never change behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Simple,
    Advanced,
    Expert,
}

/// What a caller must supply to address one instance of a parameter.
///
/// These index spaces are **not interchangeable**; conversion between them is
/// `ChannelMap`'s job alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    None,
    /// `0 .. num_channels`, the unified space: inputs then outputs.
    Channel,
    /// `0 .. num_inputs`.
    Input,
    /// `0 .. num_outputs`.
    Output,
    /// A channel plus a filter band.
    ChannelBand,
    /// A matrix crosspoint: input plus output.
    Crosspoint,
    /// A physical S/PDIF or I2S slot.
    Slot,
    /// A physical output pin index.
    PinOutput,
    /// Preset slot 0-9.
    PresetSlot,
    /// Control-surface binding slot.
    CsSlot,
    /// Learned IR command sub-slot.
    CsIrSlot,
    /// Control-surface target group, `CS_MAX_GROUPS` 8.
    CsGroup,
    /// Control-surface macro, `CS_MAX_MACROS` 8.
    CsMacro,
    /// A macro plus one of its steps, `CS_MAX_MACRO_STEPS` 8.
    CsMacroStep,
    /// Display page slot, `CS_MAX_DISPLAY_PAGES` 16.
    CsDisplayPage,
    /// Legacy 3-channel gain/mute space, superseded by the matrix mixer.
    LegacyChannel,
    /// One of the upmixer's parameter ids.
    UpmixParam,
    /// S/PDIF input index, `0..=3` (config.h:448-453).
    SpdifInput,
    /// One of the *optional* S/PDIF inputs, `1..=3`: input 0 is always on and
    /// `REQ_SET_SPDIF_INPUT_ENABLE` rejects it (config.h:454).
    SpdifExtraInput,
    /// I2S RX stereo pair.
    I2sPair,
}

impl Target {
    /// How many index components a caller must supply.
    pub fn arity(self) -> usize {
        match self {
            Target::None => 0,
            Target::ChannelBand | Target::Crosspoint | Target::CsMacroStep => 2,
            _ => 1,
        }
    }
}

/// How the target and value pack into `wValue`.
///
/// `wIndex` never appears here: it is always the vendor interface number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WValue {
    Zero,
    /// `wValue` = the single target index.
    Target,
    /// `(channel << 8) | (band << 3) | param`. The band field is **5 bits**, not
    /// 4, so crossover bands 20-23 are addressable.
    EqScalar(u8),
    /// `(channel << 8) | band`.
    ChannelBand,
    /// `(input << 8) | output`.
    Crosspoint,
    /// `(value << 8) | slot`, used by the output-type switch.
    ValueSlot,
    /// `(gpio << 8) | index`, used by the output pin move.
    ValueIndex,
    /// The value itself rides in `wValue`; there is no data stage. This is how
    /// the write-as-read opcodes carry their parameters.
    ValueOnly,
    /// `(step << 8) | macro`, used by the macro-step write. Indices are given
    /// macro first, so the packing is deliberately reversed here.
    MacroStep,
    /// A fixed constant, e.g. the caps header selector.
    Fixed(u16),
}

/// What could go wrong if this is written carelessly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hazard {
    None,
    /// Interrupts audio briefly.
    Audible,
    /// Applied by the main loop later; the return status means "accepted", not
    /// "applied", so a readback is mandatory.
    Deferred,
    /// Writes flash. Costs a blackout of up to ~45 ms per sector.
    Flash,
    /// Reconfigures physical I/O and runs a muted pipeline reset.
    Reconfig,
    /// No undo. Gate behind explicit confirmation.
    Irreversible,
}

/// Whether a value survives a power cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persistence {
    /// RAM only. Lost on reboot unless a preset is saved afterwards. This is the
    /// default and the most commonly misunderstood part of the protocol.
    LiveOnly,
    /// The write itself reaches flash.
    WritesFlash,
    /// Stored device-global; does not travel with presets.
    DeviceGlobal,
    /// Runtime observation; not settable.
    ReadOnly,
}

/// A gate that must pass before this parameter is offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requires {
    Always,
    /// Only on the larger part.
    Rp2350,
    /// Answered a capability probe under this name.
    Feature(&'static str),
}

/// What sort of value a parameter holds, which also picks its widget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Bool,
    /// An action with no value.
    Trigger,
    Float {
        unit: Unit,
        min: f32,
        max: f32,
    },
    Int {
        unit: Unit,
        min: i64,
        max: i64,
    },
    Choice(&'static [(u8, &'static str)]),
    /// A bit mask over channels or outputs.
    Mask,
    Text {
        max: usize,
    },
    /// A structured packet handled by a dedicated codec.
    Packet,
    /// A status read with no settable value.
    Status,
}

impl Kind {
    /// The wire representation of this kind's data stage.
    pub fn repr(self) -> Repr {
        match self {
            Kind::Bool => Repr::Bool8,
            Kind::Trigger => Repr::None,
            Kind::Float { .. } => Repr::F32,
            Kind::Int { unit: Unit::Hz, .. } => Repr::U32Le,
            Kind::Int { .. } => Repr::U8,
            Kind::Choice(_) => Repr::U8,
            Kind::Mask => Repr::U16Le,
            Kind::Text { max } => Repr::Text(max),
            Kind::Packet | Kind::Status => Repr::Raw,
        }
    }

    pub fn unit(self) -> Unit {
        match self {
            Kind::Float { unit, .. } | Kind::Int { unit, .. } => unit,
            _ => Unit::None,
        }
    }

    /// Reject or clamp a value against this kind's declared range.
    ///
    /// Range checking happens here, before the wire, so a bad value produces an
    /// explanation naming the limits rather than a bare stall from the device.
    pub fn validate(self, v: &Value) -> Result<Value, ValueError> {
        match self {
            Kind::Float { unit, min, max } => {
                let f = v.as_f32().ok_or(ValueError::WrongType {
                    expected: "a number",
                    got: v.type_name(),
                })?;
                if f < min || f > max {
                    return Err(ValueError::OutOfRange {
                        value: f as f64,
                        min: min as f64,
                        max: max as f64,
                        unit,
                    });
                }
                Ok(Value::Float(f))
            }
            Kind::Int { unit, min, max } => {
                let i = v.as_f32().ok_or(ValueError::WrongType {
                    expected: "a whole number",
                    got: v.type_name(),
                })? as i64;
                if i < min || i > max {
                    return Err(ValueError::OutOfRange {
                        value: i as f64,
                        min: min as f64,
                        max: max as f64,
                        unit,
                    });
                }
                Ok(Value::Int(i))
            }
            Kind::Choice(variants) => {
                let n = v.as_u8().ok_or(ValueError::WrongType {
                    expected: "a choice",
                    got: v.type_name(),
                })?;
                if variants.iter().any(|(raw, _)| *raw == n) {
                    Ok(Value::Choice(n))
                } else {
                    Err(ValueError::NotAVariant(n.to_string()))
                }
            }
            Kind::Bool => Ok(Value::Bool(v.as_bool().ok_or(ValueError::WrongType {
                expected: "a yes/no value",
                got: v.type_name(),
            })?)),
            _ => Ok(v.clone()),
        }
    }
}

/// One parameter.
#[derive(Debug, Clone, Copy)]
pub struct ParamDesc {
    /// The address used by the command grammar and the palette.
    pub path: &'static str,
    pub label: &'static str,
    /// One plain-language line, for the context bar.
    pub plain: &'static str,
    pub group: Group,
    pub level: Level,
    pub kind: Kind,
    pub target: Target,
    pub set: Option<u8>,
    pub get: Option<u8>,
    /// Direction of the **set** operation.
    pub dir: Dir,
    pub wvalue: WValue,
    pub hazard: Hazard,
    pub persists: Persistence,
    pub requires: Requires,
}

impl ParamDesc {
    pub fn is_writable(&self) -> bool {
        self.set.is_some()
    }

    pub fn is_readable(&self) -> bool {
        self.get.is_some()
    }

    /// Whether a write must be confirmed by reading the value back.
    ///
    /// Deferred writes return "accepted" before validation runs, and a bad DAC
    /// mute config is dropped silently, so for these the return status is not
    /// evidence the value took effect.
    pub fn needs_readback(&self) -> bool {
        self.get.is_some()
            && matches!(
                self.hazard,
                Hazard::Deferred | Hazard::Flash | Hazard::Reconfig
            )
    }
}

/// Shorthand for building rows without repeating the common defaults.
///
/// One argument per column is the point: a row reads as a table entry, and a
/// missing field is a compile error rather than a silent default.
#[allow(clippy::too_many_arguments)]
const fn p(
    path: &'static str,
    label: &'static str,
    plain: &'static str,
    group: Group,
    level: Level,
    kind: Kind,
    target: Target,
    set: Option<u8>,
    get: Option<u8>,
    dir: Dir,
    wvalue: WValue,
    hazard: Hazard,
    persists: Persistence,
    requires: Requires,
) -> ParamDesc {
    ParamDesc {
        path,
        label,
        plain,
        group,
        level,
        kind,
        target,
        set,
        get,
        dir,
        wvalue,
        hazard,
        persists,
        requires,
    }
}

const SPEED: &[(u8, &str)] = &[(0, "slow"), (1, "medium"), (2, "fast")];
const CF_PRESET: &[(u8, &str)] = &[(0, "default"), (1, "chumoy"), (2, "meier"), (3, "custom")];
const SOURCE: &[(u8, &str)] = &[(0, "usb"), (1, "spdif"), (2, "i2s"), (3, "adat")];
const OUT_TYPE: &[(u8, &str)] = &[(0, "spdif"), (1, "i2s")];
const CLOCK_MODE: &[(u8, &str)] = &[(0, "master"), (1, "slave")];
const MCK_MULT: &[(u8, &str)] = &[(0, "128x"), (1, "256x")];
const PIN_MODE: &[(u8, &str)] = &[(0, "unified"), (1, "split")];
const PERSIST_MODE: &[(u8, &str)] = &[(0, "independent"), (1, "with-preset")];
/// PEQ filter types. Crossover types (32-63) are chosen as a family plus an
/// order in the crossover view, never as a raw number, so they are not listed
/// here.
const FILTER_TYPE: &[(u8, &str)] = &[
    (0, "flat"),
    (1, "peak"),
    (2, "lowshelf"),
    (3, "highshelf"),
    (4, "lowpass"),
    (5, "highpass"),
    (6, "notch"),
    (7, "allpass"),
    (8, "allpass1"),
    (9, "lowshelf1"),
    (10, "highshelf1"),
    (11, "linkwitz"),
    // config.h:924-925, new in v1.1.6. The codes match the Console's file codes
    // LP1 / HP1 so a filter file round-trips between the two apps.
    (12, "lowpass1"),
    (13, "highpass1"),
];
/// Upmixer centre mode. Wire V27 widened this to three: `UPMIX_CENTER_OFF = 2`
/// leaves L/R bit-exact and produces surrounds only (bulk_params.h:34).
const CENTER_MODE: &[(u8, &str)] = &[(0, "passive"), (1, "logic"), (2, "off")];
const SURROUND_MODE: &[(u8, &str)] = &[(0, "off"), (1, "passive"), (2, "logic")];

use Dir::{In as DIn, Out as DOut, WriteAsRead as DWar};
use Group::*;
use Hazard as Hz;
use Kind::*;
use Level::*;
use Persistence as Ps;
use Requires as Rq;
use Target as Tg;
use WValue as Wv;

/// The table. Ordered by group, then by how a user would meet each parameter.
pub static REGISTRY: &[ParamDesc] = &[
    // ---------------------------------------------------------------- volume
    p(
        "vol.user",
        "Volume",
        "The listening volume, shared with the OS volume slider",
        Main,
        Simple,
        Float {
            unit: Unit::Db,
            min: -60.0,
            max: 0.0,
        },
        Tg::None,
        Some(op::REQ_SET_USER_VOLUME),
        Some(op::REQ_GET_USER_VOLUME),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "vol.mute",
        "Mute",
        "Silence the output, independently of the OS mute",
        Main,
        Simple,
        Bool,
        Tg::None,
        Some(op::REQ_SET_USER_MUTE),
        Some(op::REQ_GET_USER_MUTE),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "vol.master",
        "Master volume",
        "A ceiling on output level; -128 means muted",
        Main,
        Advanced,
        Float {
            unit: Unit::Db,
            min: -128.0,
            max: 0.0,
        },
        Tg::None,
        Some(op::REQ_SET_MASTER_VOLUME),
        Some(op::REQ_GET_MASTER_VOLUME),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "vol.master.mode",
        "Master volume storage",
        "Whether master volume travels with presets",
        Presets,
        Expert,
        Choice(PERSIST_MODE),
        Tg::None,
        Some(op::REQ_SET_MASTER_VOLUME_MODE),
        Some(op::REQ_GET_MASTER_VOLUME_MODE),
        DOut,
        Wv::Zero,
        Hz::Flash,
        Ps::DeviceGlobal,
        Rq::Always,
    ),
    p(
        "vol.master.save",
        "Save master volume",
        "Store the current master volume device-wide",
        Presets,
        Expert,
        Trigger,
        Tg::None,
        Some(op::REQ_SAVE_MASTER_VOLUME),
        Some(op::REQ_GET_SAVED_MASTER_VOLUME),
        DWar,
        Wv::Zero,
        Hz::Flash,
        Ps::DeviceGlobal,
        Rq::Always,
    ),
    p(
        "pre",
        "Input trim",
        "Level trim applied to one input before processing",
        Filters,
        Advanced,
        Float {
            unit: Unit::Db,
            min: -60.0,
            max: 20.0,
        },
        Tg::Input,
        Some(op::REQ_SET_PREAMP_CH),
        Some(op::REQ_GET_PREAMP_CH),
        DOut,
        Wv::Target,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "pre.all",
        "Input trim, all",
        "Legacy trim applied to every input at once",
        Filters,
        Expert,
        Float {
            unit: Unit::Db,
            min: -60.0,
            max: 20.0,
        },
        Tg::None,
        Some(op::REQ_SET_PREAMP),
        Some(op::REQ_GET_PREAMP),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "bypass",
        "EQ bypass",
        "Pass audio through untouched by the equaliser",
        Filters,
        Simple,
        Bool,
        Tg::None,
        Some(op::REQ_SET_BYPASS),
        Some(op::REQ_GET_BYPASS),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    // ------------------------------------------------------------------- eq
    // The whole band is written as one 16-byte packet, so the individual scalar
    // rows below exist for reading and for addressing single fields from the
    // command line. The write path assembles the packet.
    p(
        "eq.type",
        "Filter type",
        "The shape of this filter band",
        Filters,
        Advanced,
        Choice(FILTER_TYPE),
        Tg::ChannelBand,
        Some(op::REQ_SET_EQ_PARAM),
        Some(op::REQ_GET_EQ_PARAM),
        DOut,
        Wv::EqScalar(0),
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "eq.freq",
        "Frequency",
        "Centre or corner frequency of this band",
        Filters,
        Advanced,
        Float {
            unit: Unit::Hz,
            min: 10.0,
            max: 24000.0,
        },
        Tg::ChannelBand,
        Some(op::REQ_SET_EQ_PARAM),
        Some(op::REQ_GET_EQ_PARAM),
        DOut,
        Wv::EqScalar(1),
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "eq.q",
        "Q",
        "How narrow the band is; higher is narrower",
        Filters,
        Advanced,
        Float {
            unit: Unit::Q,
            min: 0.1,
            max: 20.0,
        },
        Tg::ChannelBand,
        Some(op::REQ_SET_EQ_PARAM),
        Some(op::REQ_GET_EQ_PARAM),
        DOut,
        Wv::EqScalar(2),
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "eq.gain",
        "Gain",
        "Boost or cut applied by this band",
        Filters,
        Advanced,
        Float {
            unit: Unit::Db,
            min: -24.0,
            max: 24.0,
        },
        Tg::ChannelBand,
        Some(op::REQ_SET_EQ_PARAM),
        Some(op::REQ_GET_EQ_PARAM),
        DOut,
        Wv::EqScalar(3),
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "eq.bypass",
        "Band bypass",
        "Switch this one band out of circuit",
        Filters,
        Advanced,
        Bool,
        Tg::ChannelBand,
        Some(op::REQ_SET_BAND_BYPASS),
        Some(op::REQ_GET_BAND_BYPASS),
        DOut,
        Wv::ChannelBand,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "ch.delay",
        "Delay",
        "Time alignment for this channel",
        Filters,
        Advanced,
        Float {
            unit: Unit::Ms,
            min: 0.0,
            max: 42.0,
        },
        Tg::Channel,
        Some(op::REQ_SET_DELAY),
        Some(op::REQ_GET_DELAY),
        DOut,
        Wv::Target,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "ch.name",
        "Channel name",
        "What this channel is called throughout the app",
        Filters,
        Advanced,
        Text { max: 32 },
        Tg::Channel,
        Some(op::REQ_SET_CHANNEL_NAME),
        Some(op::REQ_GET_CHANNEL_NAME),
        DOut,
        Wv::Target,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    // ---------------------------------------------------------------- matrix
    p(
        "mix",
        "Crosspoint",
        "Route one input to one output, with level and polarity",
        Matrix,
        Advanced,
        Packet,
        Tg::Crosspoint,
        Some(op::REQ_SET_MATRIX_ROUTE),
        Some(op::REQ_GET_MATRIX_ROUTE),
        DOut,
        Wv::Crosspoint,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "out.enable",
        "Output enabled",
        "Whether this output carries audio",
        Matrix,
        Advanced,
        Bool,
        Tg::Output,
        Some(op::REQ_SET_OUTPUT_ENABLE),
        Some(op::REQ_GET_OUTPUT_ENABLE),
        DOut,
        Wv::Target,
        Hz::Audible,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "out.gain",
        "Output gain",
        "Level trim for this output",
        Matrix,
        Advanced,
        Float {
            unit: Unit::Db,
            min: -60.0,
            max: 20.0,
        },
        Tg::Output,
        Some(op::REQ_SET_OUTPUT_GAIN),
        Some(op::REQ_GET_OUTPUT_GAIN),
        DOut,
        Wv::Target,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "out.mute",
        "Output mute",
        "Silence this output alone",
        Matrix,
        Advanced,
        Bool,
        Tg::Output,
        Some(op::REQ_SET_OUTPUT_MUTE),
        Some(op::REQ_GET_OUTPUT_MUTE),
        DOut,
        Wv::Target,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "out.delay",
        "Output delay",
        "Time alignment for this output",
        Matrix,
        Advanced,
        Float {
            unit: Unit::Ms,
            min: 0.0,
            max: 42.0,
        },
        Tg::Output,
        Some(op::REQ_SET_OUTPUT_DELAY),
        Some(op::REQ_GET_OUTPUT_DELAY),
        DOut,
        Wv::Target,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "out.pin",
        "Output GPIO",
        "Which pin this output drives",
        System,
        Expert,
        Int {
            unit: Unit::Gpio,
            min: 0,
            max: 29,
        },
        Tg::PinOutput,
        Some(op::REQ_SET_OUTPUT_PIN),
        Some(op::REQ_GET_OUTPUT_PIN),
        DWar,
        Wv::ValueIndex,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "out.type",
        "Output bus",
        "Whether this slot speaks S/PDIF or I2S",
        System,
        Expert,
        Choice(OUT_TYPE),
        Tg::Slot,
        Some(op::REQ_SET_OUTPUT_TYPE),
        Some(op::REQ_GET_OUTPUT_TYPE),
        DWar,
        Wv::ValueSlot,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Always,
    ),
    // legacy 3-channel controls, superseded by the matrix mixer
    p(
        "legacy.gain",
        "Channel gain (legacy)",
        "Superseded by the matrix mixer",
        Matrix,
        Expert,
        Float {
            unit: Unit::Db,
            min: -60.0,
            max: 20.0,
        },
        Tg::LegacyChannel,
        Some(op::REQ_SET_CHANNEL_GAIN),
        Some(op::REQ_GET_CHANNEL_GAIN),
        DOut,
        Wv::Target,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "legacy.mute",
        "Channel mute (legacy)",
        "Superseded by the matrix mixer",
        Matrix,
        Expert,
        Bool,
        Tg::LegacyChannel,
        Some(op::REQ_SET_CHANNEL_MUTE),
        Some(op::REQ_GET_CHANNEL_MUTE),
        DOut,
        Wv::Target,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    // -------------------------------------------------------------- dynamics
    p(
        "loud.on",
        "Loudness",
        "Restore perceived bass and treble at low volume",
        Dynamics,
        Simple,
        Bool,
        Tg::None,
        Some(op::REQ_SET_LOUDNESS),
        Some(op::REQ_GET_LOUDNESS),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "loud.ref",
        "Reference level",
        "The SPL the compensation is calibrated to",
        Dynamics,
        Advanced,
        Float {
            unit: Unit::Spl,
            min: 40.0,
            max: 100.0,
        },
        Tg::None,
        Some(op::REQ_SET_LOUDNESS_REF),
        Some(op::REQ_GET_LOUDNESS_REF),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "loud.intensity",
        "Intensity",
        "How strongly the compensation is applied",
        Dynamics,
        Advanced,
        Float {
            unit: Unit::Percent,
            min: 0.0,
            max: 200.0,
        },
        Tg::None,
        Some(op::REQ_SET_LOUDNESS_INTENSITY),
        Some(op::REQ_GET_LOUDNESS_INTENSITY),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "loud.mask",
        "Loudness outputs",
        "Which outputs get loudness compensation",
        Dynamics,
        Expert,
        Mask,
        Tg::None,
        Some(op::REQ_SET_LOUDNESS_MASK),
        Some(op::REQ_GET_LOUDNESS_MASK),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Feature("loudness_output_mask"),
    ),
    p(
        "lev.on",
        "Leveller",
        "Even out loud and quiet passages",
        Dynamics,
        Simple,
        Bool,
        Tg::None,
        Some(op::REQ_SET_LEVELLER_ENABLE),
        Some(op::REQ_GET_LEVELLER_ENABLE),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "lev.amount",
        "Amount",
        "How much levelling is applied",
        Dynamics,
        Advanced,
        Float {
            unit: Unit::Percent,
            min: 0.0,
            max: 100.0,
        },
        Tg::None,
        Some(op::REQ_SET_LEVELLER_AMOUNT),
        Some(op::REQ_GET_LEVELLER_AMOUNT),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "lev.speed",
        "Speed",
        "How quickly the leveller reacts",
        Dynamics,
        Advanced,
        Choice(SPEED),
        Tg::None,
        Some(op::REQ_SET_LEVELLER_SPEED),
        Some(op::REQ_GET_LEVELLER_SPEED),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "lev.maxgain",
        "Maximum boost",
        "The most the leveller will lift quiet content",
        Dynamics,
        Advanced,
        Float {
            unit: Unit::Db,
            min: 0.0,
            max: 35.0,
        },
        Tg::None,
        Some(op::REQ_SET_LEVELLER_MAX_GAIN),
        Some(op::REQ_GET_LEVELLER_MAX_GAIN),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "lev.lookahead",
        "Look ahead",
        "Anticipate peaks, at the cost of 10 ms delay",
        Dynamics,
        Advanced,
        Bool,
        Tg::None,
        Some(op::REQ_SET_LEVELLER_LOOKAHEAD),
        Some(op::REQ_GET_LEVELLER_LOOKAHEAD),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "lev.gate",
        "Silence gate",
        "Below this level the leveller stops working",
        Dynamics,
        Advanced,
        Float {
            unit: Unit::Db,
            min: -96.0,
            max: 0.0,
        },
        Tg::None,
        Some(op::REQ_SET_LEVELLER_GATE),
        Some(op::REQ_GET_LEVELLER_GATE),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "lev.masks",
        "Leveller channels",
        "Which channels drive and receive levelling",
        Dynamics,
        Expert,
        Mask,
        Tg::None,
        Some(op::REQ_SET_LEVELLER_MASKS),
        Some(op::REQ_GET_LEVELLER_MASKS),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Feature("leveller_masks"),
    ),
    // --------------------------------------------------------------- spatial
    p(
        "cf.on",
        "Crossfeed",
        "Soften hard stereo separation on headphones",
        Spatial,
        Simple,
        Bool,
        Tg::None,
        Some(op::REQ_SET_CROSSFEED),
        Some(op::REQ_GET_CROSSFEED),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cf.preset",
        "Voicing",
        "A named crossfeed character",
        Spatial,
        Advanced,
        Choice(CF_PRESET),
        Tg::None,
        Some(op::REQ_SET_CROSSFEED_PRESET),
        Some(op::REQ_GET_CROSSFEED_PRESET),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cf.freq",
        "Crossover frequency",
        "Below this, the channels blend",
        Spatial,
        Advanced,
        Float {
            unit: Unit::Hz,
            min: 500.0,
            max: 2000.0,
        },
        Tg::None,
        Some(op::REQ_SET_CROSSFEED_FREQ),
        Some(op::REQ_GET_CROSSFEED_FREQ),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cf.feed",
        "Feed level",
        "How much of each channel crosses over",
        Spatial,
        Advanced,
        Float {
            unit: Unit::Db,
            min: 0.0,
            max: 15.0,
        },
        Tg::None,
        Some(op::REQ_SET_CROSSFEED_FEED),
        Some(op::REQ_GET_CROSSFEED_FEED),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cf.itd",
        "Time delay",
        "Model the delay sound takes to reach the far ear",
        Spatial,
        Advanced,
        Bool,
        Tg::None,
        Some(op::REQ_SET_CROSSFEED_ITD),
        Some(op::REQ_GET_CROSSFEED_ITD),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cf.outputs",
        "Crossfeed outputs",
        "Which output pairs get crossfeed",
        Spatial,
        Expert,
        Mask,
        Tg::None,
        Some(op::REQ_SET_CROSSFEED_OUTPUTS),
        Some(op::REQ_GET_CROSSFEED_OUTPUTS),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Feature("crossfeed_output_mask"),
    ),
    p(
        "bass.on",
        "Psychoacoustic bass",
        "Imply deep bass a small speaker cannot reproduce",
        Spatial,
        Simple,
        Bool,
        Tg::None,
        Some(op::REQ_SET_PSYBASS),
        Some(op::REQ_GET_PSYBASS),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Feature("psychoacoustic_bass"),
    ),
    p(
        "bass.cutoff",
        "Speaker limit",
        "The lowest note your speaker can actually produce",
        Spatial,
        Advanced,
        Float {
            unit: Unit::Hz,
            min: 30.0,
            max: 300.0,
        },
        Tg::None,
        Some(op::REQ_SET_PSYBASS_CUTOFF),
        Some(op::REQ_GET_PSYBASS_CUTOFF),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Feature("psychoacoustic_bass"),
    ),
    p(
        "bass.harmonics",
        "Harmonics",
        "Level of the generated harmonics",
        Spatial,
        Advanced,
        Float {
            unit: Unit::Db,
            min: -24.0,
            max: 12.0,
        },
        Tg::None,
        Some(op::REQ_SET_PSYBASS_HARMONICS),
        Some(op::REQ_GET_PSYBASS_HARMONICS),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Feature("psychoacoustic_bass"),
    ),
    p(
        "bass.drive",
        "Drive",
        "How hard the harmonic generator is pushed",
        Spatial,
        Advanced,
        Float {
            unit: Unit::Db,
            min: 0.0,
            max: 18.0,
        },
        Tg::None,
        Some(op::REQ_SET_PSYBASS_DRIVE),
        Some(op::REQ_GET_PSYBASS_DRIVE),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Feature("psychoacoustic_bass"),
    ),
    p(
        "bass.character",
        "Character",
        "Warm at 0, aggressive at 100",
        Spatial,
        Advanced,
        Float {
            unit: Unit::Percent,
            min: 0.0,
            max: 100.0,
        },
        Tg::None,
        Some(op::REQ_SET_PSYBASS_CHARACTER),
        Some(op::REQ_GET_PSYBASS_CHARACTER),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Feature("psychoacoustic_bass"),
    ),
    p(
        "bass.original",
        "Original bass",
        "How much untouched low end stays in",
        Spatial,
        Advanced,
        Float {
            unit: Unit::Db,
            min: -60.0,
            max: 0.0,
        },
        Tg::None,
        Some(op::REQ_SET_PSYBASS_ORIGINAL),
        Some(op::REQ_GET_PSYBASS_ORIGINAL),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Feature("psychoacoustic_bass"),
    ),
    p(
        "bass.mask",
        "Bass outputs",
        "Which outputs get psychoacoustic bass",
        Spatial,
        Expert,
        Mask,
        Tg::None,
        Some(op::REQ_SET_PSYBASS_MASK),
        Some(op::REQ_GET_PSYBASS_MASK),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Feature("psychoacoustic_bass"),
    ),
    p(
        "up.on",
        "Upmixer",
        "Spread a stereo recording across more speakers",
        Spatial,
        Simple,
        Bool,
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(0),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.center_mode",
        "Centre mode",
        "How the centre channel is derived",
        Spatial,
        Advanced,
        Choice(CENTER_MODE),
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(1),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.surround_mode",
        "Surround mode",
        "How the surround channels are derived",
        Spatial,
        Advanced,
        Choice(SURROUND_MODE),
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(2),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.strength",
        "Strength",
        "How much the upmixer steers the sound",
        Spatial,
        Simple,
        Float {
            unit: Unit::Percent,
            min: 0.0,
            max: 100.0,
        },
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(3),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.width",
        "Centre width",
        "How wide the centre image is",
        Spatial,
        Advanced,
        Float {
            unit: Unit::Percent,
            min: 0.0,
            max: 100.0,
        },
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(4),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.threshold",
        "Correlation threshold",
        "How alike two channels must be to count as centre",
        Spatial,
        Expert,
        Float {
            unit: Unit::Percent,
            min: 0.0,
            max: 100.0,
        },
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(5),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.attack",
        "Attack",
        "How quickly steering responds",
        Spatial,
        Expert,
        Float {
            unit: Unit::Ms,
            min: 0.0,
            max: 1000.0,
        },
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(6),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.release",
        "Release",
        "How quickly steering relaxes",
        Spatial,
        Expert,
        Float {
            unit: Unit::Ms,
            min: 0.0,
            max: 5000.0,
        },
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(7),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.det_hpf",
        "Detector high pass",
        "Ignore bass when deciding where sound belongs",
        Spatial,
        Expert,
        Float {
            unit: Unit::Hz,
            min: 20.0,
            max: 2000.0,
        },
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(8),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.sur_delay",
        "Surround delay",
        "Delay applied to the surround channels",
        Spatial,
        Advanced,
        Float {
            unit: Unit::Ms,
            min: 0.0,
            max: 42.0,
        },
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(9),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.sur_hpf",
        "Surround high pass",
        "Roll off bass from the surround channels",
        Spatial,
        Expert,
        Float {
            unit: Unit::Hz,
            min: 20.0,
            max: 2000.0,
        },
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(10),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.sur_lpf",
        "Surround low pass",
        "Roll off treble from the surround channels",
        Spatial,
        Expert,
        Float {
            unit: Unit::Hz,
            min: 1000.0,
            max: 20000.0,
        },
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(11),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.decorr",
        "Decorrelation",
        "Spread the surrounds so they sound less alike",
        Spatial,
        Expert,
        Float {
            unit: Unit::Percent,
            min: 0.0,
            max: 100.0,
        },
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(12),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.presence",
        "Centre presence",
        "Lift or soften the centre channel",
        Spatial,
        Advanced,
        Float {
            unit: Unit::Db,
            min: -12.0,
            max: 12.0,
        },
        Tg::None,
        Some(op::REQ_UPMIX_SET_PARAM),
        Some(op::REQ_UPMIX_GET_PARAM),
        DOut,
        Wv::Fixed(13),
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.config",
        "Upmixer config",
        "Every upmixer setting at once",
        Spatial,
        Expert,
        Packet,
        Tg::None,
        Some(op::REQ_UPMIX_SET_CONFIG),
        Some(op::REQ_UPMIX_GET_CONFIG),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Rp2350,
    ),
    p(
        "up.status",
        "Upmixer status",
        "What the upmixer is currently doing",
        Spatial,
        Advanced,
        Status,
        Tg::None,
        None,
        Some(op::REQ_UPMIX_GET_STATUS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Rp2350,
    ),
    // ----------------------------------------------------------------- input
    p(
        "in.source",
        "Input",
        "Where the device takes audio from",
        Input,
        Simple,
        Choice(SOURCE),
        Tg::None,
        Some(op::REQ_SET_INPUT_SOURCE),
        Some(op::REQ_GET_INPUT_SOURCE),
        DOut,
        Wv::Zero,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "in.rate",
        "Sample rate",
        "The rate the device runs at when it is the clock master",
        Input,
        Advanced,
        Int {
            unit: Unit::Hz,
            min: 44100,
            max: 96000,
        },
        Tg::None,
        Some(op::REQ_SET_INPUT_RATE),
        Some(op::REQ_GET_INPUT_RATE),
        DOut,
        Wv::Zero,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "in.spdif.pin",
        "S/PDIF input GPIO",
        "Which pin one optical or coaxial receiver uses",
        Input,
        Expert,
        Int {
            unit: Unit::Gpio,
            min: 0,
            max: 29,
        },
        Tg::SpdifInput,
        Some(op::REQ_SET_SPDIF_RX_PIN),
        Some(op::REQ_GET_SPDIF_RX_PIN),
        DWar,
        Wv::ValueIndex,
        Hz::Audible,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "in.spdif.enable",
        "Extra S/PDIF inputs",
        "Turn on the optional second, third and fourth receivers",
        Input,
        Expert,
        Bool,
        // Index 1..3 only: input 0 is always enabled (config.h:454).
        Tg::SpdifExtraInput,
        Some(op::REQ_SET_SPDIF_INPUT_ENABLE),
        // The readback is the whole 6-byte config, {count, enable_mask,
        // gpio[0..3]}, whose bit 0 is input 1 and is always set. A scalar read
        // of one byte therefore sees `count`, not this input's state; Phase 2B
        // gives it a codec that picks the right bit.
        Some(op::REQ_GET_SPDIF_INPUT_CONFIG),
        DWar,
        Wv::ValueIndex,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Feature("spdif_multi_input"),
    ),
    p(
        "in.spdif.status",
        "S/PDIF lock",
        "Whether the receiver has locked, and to what",
        Input,
        Advanced,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_SPDIF_RX_STATUS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    p(
        "in.spdif.channel",
        "S/PDIF channel status",
        "The IEC 60958 metadata the source is sending",
        Input,
        Expert,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_SPDIF_RX_CH_STATUS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    p(
        "in.i2s.pin",
        "I2S input GPIO",
        "Data pin for one I2S input pair",
        Input,
        Expert,
        Int {
            unit: Unit::Gpio,
            min: 0,
            max: 29,
        },
        Tg::I2sPair,
        Some(op::REQ_SET_I2S_RX_PIN),
        Some(op::REQ_GET_I2S_RX_PIN),
        DWar,
        Wv::ValueIndex,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "in.i2s.channels",
        "I2S input channels",
        "How many channels arrive over I2S",
        Input,
        Expert,
        Int {
            unit: Unit::None,
            min: 2,
            max: 8,
        },
        Tg::None,
        Some(op::REQ_SET_I2S_INPUT_CHANNELS),
        Some(op::REQ_GET_I2S_INPUT_CHANNELS),
        DWar,
        Wv::ValueOnly,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Feature("i2s_input_channels"),
    ),
    p(
        "in.i2s.clock",
        "I2S clock",
        "Whether the device generates or follows the clock",
        Input,
        Expert,
        Choice(CLOCK_MODE),
        Tg::None,
        Some(op::REQ_SET_I2S_CLOCK_MODE),
        Some(op::REQ_GET_I2S_CLOCK_MODE),
        DOut,
        Wv::Zero,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Feature("i2s_slave_clock"),
    ),
    p(
        "in.i2s.slave.status",
        "I2S slave status",
        "Whether an external clock is present and locked",
        Input,
        Expert,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_I2S_SLAVE_STATUS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Feature("i2s_slave_clock"),
    ),
    p(
        "in.adat.enable",
        "ADAT input",
        "Take audio from the ADAT lightpipe input",
        Input,
        Expert,
        Bool,
        Tg::None,
        Some(op::REQ_SET_ADAT_INPUT_ENABLE),
        Some(op::REQ_GET_ADAT_INPUT_ENABLE),
        DOut,
        Wv::Zero,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Feature("adat_input"),
    ),
    p(
        "in.adat.pin",
        "ADAT input GPIO",
        "Which pin the ADAT receiver uses",
        Input,
        Expert,
        Int {
            unit: Unit::Gpio,
            min: 0,
            max: 29,
        },
        Tg::None,
        Some(op::REQ_SET_ADAT_INPUT_PIN),
        Some(op::REQ_GET_ADAT_INPUT_PIN),
        DWar,
        Wv::ValueOnly,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Feature("adat_input"),
    ),
    p(
        "in.adat.clock",
        "ADAT clock",
        "Whether ADAT provides the clock",
        Input,
        Expert,
        Choice(CLOCK_MODE),
        Tg::None,
        Some(op::REQ_SET_ADAT_INPUT_CLOCK_MODE),
        Some(op::REQ_GET_ADAT_INPUT_CLOCK_MODE),
        DOut,
        Wv::Zero,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Feature("adat_input"),
    ),
    p(
        "in.adat.status",
        "ADAT input status",
        "Whether the ADAT input has locked",
        Input,
        Expert,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_ADAT_INPUT_STATUS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Feature("adat_input"),
    ),
    p(
        "in.lg",
        "LG Sound Sync",
        "Follow the volume of a connected LG television",
        Input,
        Advanced,
        Bool,
        Tg::None,
        Some(op::REQ_SET_LG_SOUND_SYNC_ENABLE),
        Some(op::REQ_GET_LG_SOUND_SYNC_ENABLE),
        DOut,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Feature("lg_sound_sync"),
    ),
    p(
        "in.lg.status",
        "LG Sound Sync status",
        "Whether an LG source is detected, and its volume",
        Input,
        Advanced,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_LG_SOUND_SYNC_STATUS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Feature("lg_sound_sync"),
    ),
    // ---------------------------------------------------------------- system
    p(
        "i2s.bck",
        "I2S bit clock GPIO",
        "Bit clock pin; the word clock sits on the next pin up",
        System,
        Expert,
        Int {
            unit: Unit::Gpio,
            min: 0,
            max: 29,
        },
        Tg::None,
        Some(op::REQ_SET_I2S_BCK_PIN),
        Some(op::REQ_GET_I2S_BCK_PIN),
        DWar,
        Wv::ValueOnly,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "i2s.clockpins",
        "I2S clock pins",
        "Whether input and output share clock pins",
        System,
        Expert,
        Choice(PIN_MODE),
        Tg::None,
        Some(op::REQ_SET_I2S_CLOCK_PIN_MODE),
        Some(op::REQ_GET_I2S_CLOCK_PIN_MODE),
        DWar,
        Wv::ValueOnly,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "i2s.mck",
        "Master clock",
        "Emit a master clock for an external DAC",
        System,
        Expert,
        Bool,
        Tg::None,
        Some(op::REQ_SET_MCK_ENABLE),
        Some(op::REQ_GET_MCK_ENABLE),
        DWar,
        Wv::ValueOnly,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "i2s.mck.pin",
        "Master clock GPIO",
        "Must be a clock-capable pin",
        System,
        Expert,
        Int {
            unit: Unit::Gpio,
            min: 0,
            max: 29,
        },
        Tg::None,
        Some(op::REQ_SET_MCK_PIN),
        Some(op::REQ_GET_MCK_PIN),
        DWar,
        Wv::ValueOnly,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "i2s.mck.mult",
        "Master clock rate",
        "Multiple of the sample rate to emit",
        System,
        Expert,
        Choice(MCK_MULT),
        Tg::None,
        Some(op::REQ_SET_MCK_MULTIPLIER),
        Some(op::REQ_GET_MCK_MULTIPLIER),
        DWar,
        Wv::ValueOnly,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "adat.enable",
        "ADAT output",
        "Stream eight channels out over ADAT lightpipe",
        System,
        Expert,
        Bool,
        Tg::None,
        Some(op::REQ_SET_ADAT_ENABLE),
        Some(op::REQ_GET_ADAT_ENABLE),
        DWar,
        Wv::ValueOnly,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Feature("adat_output"),
    ),
    p(
        "adat.pin",
        "ADAT output GPIO",
        "Which pin carries the ADAT output",
        System,
        Expert,
        Int {
            unit: Unit::Gpio,
            min: 0,
            max: 29,
        },
        Tg::None,
        Some(op::REQ_SET_ADAT_PIN),
        Some(op::REQ_GET_ADAT_PIN),
        DWar,
        Wv::ValueOnly,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Feature("adat_output"),
    ),
    p(
        "adat.status",
        "ADAT output status",
        "Whether ADAT output is streaming",
        System,
        Expert,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_ADAT_STATUS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Feature("adat_output"),
    ),
    p(
        "dev.dacmute",
        "DAC hardware mute",
        "Drive an external DAC's mute pin",
        System,
        Expert,
        Packet,
        Tg::None,
        Some(op::REQ_SET_DAC_HW_MUTE_CONFIG),
        Some(op::REQ_GET_DAC_HW_MUTE_CONFIG),
        DOut,
        Wv::Zero,
        Hz::Flash,
        Ps::WritesFlash,
        Rq::Feature("dac_hardware_mute"),
    ),
    p(
        "dev.dacmute.test",
        "Test DAC mute",
        "Pulse the mute pin for a second so you can hear it",
        System,
        Expert,
        Trigger,
        Tg::None,
        Some(op::REQ_TEST_DAC_HW_MUTE),
        None,
        DWar,
        Wv::Zero,
        Hz::Audible,
        Ps::LiveOnly,
        Rq::Feature("dac_hardware_mute"),
    ),
    p(
        "dev.uart",
        "UART control",
        "Drive the device from an external serial controller",
        System,
        Expert,
        Packet,
        Tg::None,
        Some(op::REQ_SET_UART_CONFIG),
        Some(op::REQ_GET_UART_CONFIG),
        DOut,
        Wv::Zero,
        Hz::Flash,
        Ps::WritesFlash,
        Rq::Feature("uart_control"),
    ),
    p(
        "dev.i2c",
        "I2C control",
        "Drive the device from an external I2C controller",
        System,
        Expert,
        Packet,
        Tg::None,
        Some(op::REQ_SET_I2C_CONFIG),
        Some(op::REQ_GET_I2C_CONFIG),
        DOut,
        Wv::Zero,
        Hz::Flash,
        Ps::WritesFlash,
        Rq::Feature("i2c_control"),
    ),
    p(
        "dev.ctrl.status",
        "Control interface status",
        "Whether the UART and I2C links came up",
        System,
        Expert,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_CTRL_IFACE_STATUS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    p(
        "dev.reset",
        "Factory reset",
        "Reset live settings; stored presets are kept",
        System,
        Expert,
        Trigger,
        Tg::None,
        Some(op::REQ_FACTORY_RESET),
        None,
        DWar,
        Wv::Zero,
        Hz::Irreversible,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "dev.bootloader",
        "Firmware update mode",
        "Reboot into the bootloader; the device disappears",
        System,
        Expert,
        Trigger,
        Tg::None,
        Some(op::REQ_ENTER_BOOTLOADER),
        None,
        DWar,
        Wv::Zero,
        Hz::Irreversible,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "dev.save",
        "Save settings (legacy)",
        "Legacy whole-state save",
        System,
        Expert,
        Trigger,
        Tg::None,
        Some(op::REQ_SAVE_PARAMS),
        None,
        DWar,
        Wv::Zero,
        Hz::Flash,
        Ps::WritesFlash,
        Rq::Always,
    ),
    p(
        "dev.save.io",
        "Save I/O config",
        "Store the current wiring device-wide",
        System,
        Expert,
        Trigger,
        Tg::None,
        Some(op::REQ_SAVE_OUTPUT_CONFIG),
        None,
        DWar,
        Wv::Zero,
        Hz::Flash,
        Ps::DeviceGlobal,
        Rq::Always,
    ),
    p(
        "dev.serial",
        "Serial number",
        "This device's unique identifier",
        System,
        Advanced,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_SERIAL),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    p(
        "dev.platform",
        "Platform",
        "Which chip and firmware this device runs",
        System,
        Advanced,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_PLATFORM),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    // --------------------------------------------------------------- presets
    p(
        "preset.save",
        "Save preset",
        "Write the current settings into a slot",
        Presets,
        Simple,
        Trigger,
        Tg::PresetSlot,
        Some(op::REQ_PRESET_SAVE),
        None,
        DWar,
        Wv::Target,
        Hz::Flash,
        Ps::WritesFlash,
        Rq::Always,
    ),
    p(
        "preset.load",
        "Load preset",
        "Recall a stored slot",
        Presets,
        Simple,
        Trigger,
        Tg::PresetSlot,
        Some(op::REQ_PRESET_LOAD),
        Some(op::REQ_PRESET_GET_ACTIVE),
        DWar,
        Wv::Target,
        Hz::Audible,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "preset.delete",
        "Delete preset",
        "Erase a stored slot",
        Presets,
        Advanced,
        Trigger,
        Tg::PresetSlot,
        Some(op::REQ_PRESET_DELETE),
        None,
        DWar,
        Wv::Target,
        Hz::Flash,
        Ps::WritesFlash,
        Rq::Always,
    ),
    p(
        "preset.name",
        "Preset name",
        "What this slot is called",
        Presets,
        Simple,
        Text { max: 32 },
        Tg::PresetSlot,
        Some(op::REQ_PRESET_SET_NAME),
        Some(op::REQ_PRESET_GET_NAME),
        DOut,
        Wv::Target,
        Hz::Flash,
        Ps::WritesFlash,
        Rq::Always,
    ),
    p(
        "preset.startup",
        "Startup preset",
        "Which preset loads at power on",
        Presets,
        Advanced,
        Packet,
        Tg::None,
        Some(op::REQ_PRESET_SET_STARTUP),
        Some(op::REQ_PRESET_GET_STARTUP),
        DOut,
        Wv::Zero,
        Hz::Flash,
        Ps::WritesFlash,
        Rq::Always,
    ),
    p(
        "preset.dir",
        "Preset directory",
        // 7 bytes, not the 6 the old spec claimed: {occupied u16 LE, startup_mode,
        // default_slot, last_active, output_config_mode, master_volume_mode}
        // (vendor_commands.c:2391-2411). Nothing decodes it yet; Phase 2B gives
        // it a codec.
        "Which slots are in use, and the storage policy",
        Presets,
        Advanced,
        Status,
        Tg::None,
        None,
        Some(op::REQ_PRESET_GET_DIR),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    p(
        "preset.iomode",
        "I/O storage",
        "Whether wiring travels with presets",
        Presets,
        Expert,
        Choice(PERSIST_MODE),
        Tg::None,
        Some(op::REQ_SET_OUTPUT_CONFIG_MODE),
        Some(op::REQ_GET_OUTPUT_CONFIG_MODE),
        DOut,
        Wv::Zero,
        Hz::Flash,
        Ps::DeviceGlobal,
        Rq::Always,
    ),
    // -------------------------------------------------------------- surfaces
    p(
        "cs.binding",
        "Control binding",
        "One physical control and what it does",
        Surfaces,
        Advanced,
        Packet,
        Tg::CsSlot,
        Some(op::REQ_SET_CS_BINDING),
        Some(op::REQ_GET_CS_BINDING),
        DOut,
        Wv::Target,
        Hz::Deferred,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cs.name",
        "Binding name",
        "A label for this physical control",
        Surfaces,
        Advanced,
        Text { max: 32 },
        Tg::CsSlot,
        Some(op::REQ_SET_CS_NAME),
        Some(op::REQ_GET_CS_NAME),
        DOut,
        Wv::Target,
        Hz::Deferred,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cs.ir",
        "IR command",
        "One learned remote-control button",
        Surfaces,
        Advanced,
        Packet,
        Tg::CsIrSlot,
        Some(op::REQ_SET_CS_IR_CMD),
        Some(op::REQ_GET_CS_IR_CMD),
        DOut,
        Wv::Target,
        Hz::Deferred,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cs.ir.learn",
        "Learn a remote button",
        "Listen for a button press on any remote",
        Surfaces,
        Advanced,
        Trigger,
        Tg::None,
        Some(op::REQ_CS_IR_LEARN),
        None,
        DWar,
        Wv::ValueOnly,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cs.save",
        "Save control surfaces",
        "Commit the previewed bindings to flash",
        Surfaces,
        Advanced,
        Trigger,
        Tg::None,
        Some(op::REQ_CS_SAVE),
        None,
        DWar,
        Wv::Zero,
        Hz::Flash,
        Ps::WritesFlash,
        Rq::Always,
    ),
    p(
        "cs.revert",
        "Discard changes",
        "Restore the bindings stored in flash",
        Surfaces,
        Advanced,
        Trigger,
        Tg::None,
        Some(op::REQ_CS_REVERT),
        None,
        DWar,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cs.caps",
        "Control surface capabilities",
        "What this firmware can bind",
        Surfaces,
        Expert,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_CS_CAPS),
        DIn,
        Wv::Fixed(0xFFFF),
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    p(
        "cs.status",
        "Control surface status",
        "Which bindings are live, and any errors",
        Surfaces,
        Advanced,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_CS_STATUS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    // Groups and macros, caps v9 (config.h:136-146). Availability is reported
    // by the caps header's max_groups / max_macros / max_macro_steps, not by a
    // separate probe, which is why these are `Always`: a firmware without them
    // answers zero and the UI offers nothing.
    p(
        "cs.group",
        "Channel group",
        "A named set of channels a control drives as one",
        Surfaces,
        Advanced,
        Packet,
        Tg::CsGroup,
        Some(op::REQ_SET_CS_GROUP),
        Some(op::REQ_GET_CS_GROUP),
        DOut,
        Wv::Target,
        Hz::Deferred,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cs.macro",
        "Macro",
        "A named sequence of steps one control can fire",
        Surfaces,
        Advanced,
        Packet,
        Tg::CsMacro,
        // The SET takes the 36-byte header alone; the GET answers the whole
        // 132-byte macro, steps included (control_surfaces.h:488-496).
        Some(op::REQ_SET_CS_MACRO),
        Some(op::REQ_GET_CS_MACRO),
        DOut,
        Wv::Target,
        Hz::Deferred,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cs.macro.step",
        "Macro step",
        "One action in a macro, with its delay",
        Surfaces,
        Advanced,
        Packet,
        Tg::CsMacroStep,
        Some(op::REQ_SET_CS_MACRO_STEP),
        // A step has no GET of its own: it reads back inside the whole macro,
        // whose wValue is the macro index alone. Steps are written before the
        // header so a concurrent fire never sees a step_count it cannot reach.
        Some(op::REQ_GET_CS_MACRO),
        DOut,
        Wv::MacroStep,
        Hz::Deferred,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cs.macro.fire",
        "Run a macro",
        "Run one macro now; a running macro is cancelled at its next step",
        Surfaces,
        Advanced,
        Trigger,
        Tg::CsMacro,
        Some(op::REQ_CS_MACRO_FIRE),
        None,
        DWar,
        Wv::Target,
        Hz::Audible,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cs.ext.status",
        "Group and macro status",
        "Which groups and macros are valid, and what is running",
        Surfaces,
        Advanced,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_CS_EXT_STATUS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    // I2C display, caps v10-v13 (config.h:150-157).
    p(
        "cs.display",
        "Display settings",
        "How the attached I2C display behaves",
        Surfaces,
        Advanced,
        Packet,
        Tg::None,
        Some(op::REQ_SET_CS_DISPLAY_CFG),
        // The GET prepends {max_pages, model_count, reserved[2]} to the same
        // 12-byte record the SET takes, so the read is 16 bytes.
        Some(op::REQ_GET_CS_DISPLAY_CFG),
        DOut,
        Wv::Zero,
        Hz::Deferred,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cs.display.page",
        "Display page",
        "What one page of the display shows",
        Surfaces,
        Advanced,
        Packet,
        Tg::CsDisplayPage,
        Some(op::REQ_SET_CS_DISPLAY_PAGE),
        Some(op::REQ_GET_CS_DISPLAY_PAGE),
        DOut,
        Wv::Target,
        Hz::Deferred,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "cs.display.status",
        "Display status",
        "Whether the display is live, and what it is showing",
        Surfaces,
        Advanced,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_CS_DISPLAY_STATUS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    // ---------------------------------------------------------- test signals
    p(
        "sig.config",
        "Test signal",
        "What the generator produces",
        Diagnostics,
        Advanced,
        Packet,
        Tg::None,
        Some(op::REQ_SIGGEN_SET_CONFIG),
        Some(op::REQ_SIGGEN_GET_CONFIG),
        DOut,
        Wv::Zero,
        Hz::Audible,
        Ps::LiveOnly,
        Rq::Feature("test_signals"),
    ),
    p(
        "sig.control",
        "Start or stop",
        "Run or halt the test signal",
        Diagnostics,
        Advanced,
        Choice(&[(0, "stop"), (1, "start"), (2, "stop-now")]),
        Tg::None,
        Some(op::REQ_SIGGEN_CONTROL),
        Some(op::REQ_SIGGEN_GET_STATUS),
        DWar,
        Wv::ValueOnly,
        Hz::Audible,
        Ps::LiveOnly,
        Rq::Feature("test_signals"),
    ),
    p(
        "sig.caps",
        "Signal generator capabilities",
        "Which signals this firmware can produce",
        Diagnostics,
        Expert,
        Status,
        Tg::None,
        None,
        Some(op::REQ_SIGGEN_GET_CAPS),
        DIn,
        Wv::Fixed(0xFFFF),
        Hz::None,
        Ps::ReadOnly,
        Rq::Feature("test_signals"),
    ),
    // ----------------------------------------------------------- diagnostics
    p(
        "meters",
        "Meters",
        "Per-channel levels, clip flags and processor load",
        Main,
        Simple,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_STATUS),
        DIn,
        Wv::Fixed(9),
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    p(
        "meters.clear",
        "Clear clip indicators",
        "Reset the clip latches",
        Main,
        Simple,
        Trigger,
        Tg::None,
        Some(op::REQ_CLEAR_CLIPS),
        None,
        DWar,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "diag.buffers",
        "Buffer statistics",
        "Fill levels and watermarks for every output",
        Diagnostics,
        Expert,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_BUFFER_STATS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    p(
        "diag.buffers.reset",
        "Reset watermarks",
        "Clear the recorded buffer extremes",
        Diagnostics,
        Expert,
        Trigger,
        Tg::None,
        Some(op::REQ_RESET_BUFFER_STATS),
        None,
        DWar,
        Wv::Fixed(1),
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "diag.usb",
        "USB error counters",
        "Reported as zero by the current firmware",
        Diagnostics,
        Expert,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_USB_ERROR_STATS),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    p(
        "diag.usb.reset",
        "Reset USB counters",
        "No effect on current firmware",
        Diagnostics,
        Expert,
        Trigger,
        Tg::None,
        Some(op::REQ_RESET_USB_ERROR_STATS),
        None,
        DWar,
        Wv::Zero,
        Hz::None,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "diag.core1",
        "Second core",
        "What the second processor core is doing",
        Diagnostics,
        Expert,
        Status,
        Tg::None,
        None,
        Some(op::REQ_GET_CORE1_MODE),
        DIn,
        Wv::Zero,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    p(
        "diag.core1.conflict",
        "Core conflict check",
        "Whether enabling an output would clash",
        Diagnostics,
        Expert,
        Status,
        Tg::Output,
        None,
        Some(op::REQ_GET_CORE1_CONFLICT),
        DIn,
        Wv::Target,
        Hz::None,
        Ps::ReadOnly,
        Rq::Always,
    ),
    // ------------------------------------------------------------------ bulk
    p(
        "bulk",
        "All settings",
        "Every parameter in one transfer",
        Diagnostics,
        Expert,
        Packet,
        Tg::None,
        Some(op::REQ_SET_ALL_PARAMS),
        Some(op::REQ_GET_ALL_PARAMS),
        DOut,
        Wv::Zero,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Always,
    ),
    p(
        "bulk.chunk",
        "All settings, chunked",
        "The same, in transfer-sized pieces",
        Diagnostics,
        Expert,
        Packet,
        Tg::None,
        Some(op::REQ_SET_ALL_PARAMS_CHUNK),
        Some(op::REQ_GET_ALL_PARAMS_CHUNK),
        DOut,
        Wv::Target,
        Hz::Reconfig,
        Ps::LiveOnly,
        Rq::Always,
    ),
];

/// Opcodes deliberately not in the registry, each with the reason.
///
/// The coverage test accepts these and nothing else, so dropping an opcode here
/// is a decision that has to be written down rather than an oversight.
/// Currently empty: every one of the firmware's 190 opcodes is reachable from a
/// registry row. Keep it that way. If a future opcode genuinely cannot be
/// driven by a host, add it here with the reason rather than letting the
/// coverage test be weakened.
pub static EXCLUDED: &[(u8, &str)] = &[];

/// Look a parameter up by its command path.
pub fn by_path(path: &str) -> Option<&'static ParamDesc> {
    REGISTRY.iter().find(|d| d.path == path)
}

/// Every opcode the registry references, in either direction.
pub fn referenced_opcodes() -> Vec<u8> {
    let mut v: Vec<u8> = REGISTRY
        .iter()
        .flat_map(|d| [d.set, d.get])
        .flatten()
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for d in REGISTRY {
            assert!(seen.insert(d.path), "duplicate path: {}", d.path);
        }
    }

    #[test]
    fn writable_rows_declare_a_direction_that_makes_sense() {
        for d in REGISTRY {
            if d.set.is_none() {
                assert_eq!(d.dir, Dir::In, "{} has no setter but is not a read", d.path);
            }
        }
    }

    /// Deferred, flash and reconfiguring writes return "accepted" before the
    /// work runs, so a readback is the only proof the value took effect.
    #[test]
    fn risky_writes_are_verifiable() {
        for d in REGISTRY {
            if matches!(d.hazard, Hazard::Deferred | Hazard::Reconfig)
                && d.set.is_some()
                && !matches!(d.kind, Kind::Trigger)
            {
                assert!(
                    d.get.is_some(),
                    "{} is a deferred write with no way to confirm it applied",
                    d.path
                );
            }
        }
    }

    #[test]
    fn ranges_are_the_right_way_round() {
        for d in REGISTRY {
            match d.kind {
                Kind::Float { min, max, .. } => {
                    assert!(min < max, "{} has an inverted range", d.path)
                }
                Kind::Int { min, max, .. } => {
                    assert!(min < max, "{} has an inverted range", d.path)
                }
                _ => {}
            }
        }
    }

    #[test]
    fn validation_rejects_out_of_range_with_the_limits_named() {
        let drive = by_path("bass.drive").unwrap();
        let e = drive.kind.validate(&Value::Float(25.0)).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("18"), "error should name the limit: {msg}");
        assert!(drive.kind.validate(&Value::Float(12.0)).is_ok());
    }

    #[test]
    fn choices_reject_values_outside_the_variant_set() {
        let speed = by_path("lev.speed").unwrap();
        assert!(speed.kind.validate(&Value::Choice(1)).is_ok());
        assert!(speed.kind.validate(&Value::Choice(7)).is_err());
    }

    #[test]
    fn targeted_parameters_declare_their_arity() {
        assert_eq!(by_path("eq.freq").unwrap().target.arity(), 2);
        assert_eq!(by_path("out.gain").unwrap().target.arity(), 1);
        assert_eq!(by_path("vol.user").unwrap().target.arity(), 0);
        assert_eq!(by_path("mix").unwrap().target.arity(), 2);
        // A macro step is addressed by macro and by step number.
        assert_eq!(by_path("cs.macro.step").unwrap().target.arity(), 2);
    }

    /// The twelve opcodes v1.1.6 added, and the direction each is dispatched
    /// on. `REQ_CS_MACRO_FIRE` is the only write-as-read of the group.
    #[test]
    fn the_v1_1_6_control_surface_opcodes_are_all_reachable() {
        let rows: &[(&str, Option<u8>, Option<u8>, Dir)] = &[
            ("cs.group", Some(0x20), Some(0x21), Dir::Out),
            ("cs.macro", Some(0x22), Some(0x23), Dir::Out),
            ("cs.macro.step", Some(0x24), Some(0x23), Dir::Out),
            ("cs.macro.fire", Some(0x25), None, Dir::WriteAsRead),
            ("cs.ext.status", None, Some(0x26), Dir::In),
            ("cs.display", Some(0x27), Some(0x28), Dir::Out),
            ("cs.display.page", Some(0x29), Some(0x2A), Dir::Out),
            ("cs.display.status", None, Some(0x2B), Dir::In),
        ];
        for (path, set, get, dir) in rows {
            let d = by_path(path).unwrap_or_else(|| panic!("{path} is not registered"));
            assert_eq!(d.set, *set, "{path} set opcode");
            assert_eq!(d.get, *get, "{path} get opcode");
            assert_eq!(d.dir, *dir, "{path} direction");
            assert_eq!(d.group, Group::Surfaces, "{path} belongs on the CS screen");
        }
    }

    /// `REQ_SET_CS_MACRO_STEP` packs `(step << 8) | macro`, the reverse of the
    /// usual order, so the indices are given macro first and swapped here.
    #[test]
    fn a_macro_step_packs_the_step_into_the_high_byte() {
        let d = by_path("cs.macro.step").unwrap();
        assert_eq!(d.wvalue, WValue::MacroStep);
    }

    /// The optional S/PDIF inputs are 1..3; input 0 is always on and cannot be
    /// disabled, so it must not share the pin command's index space.
    #[test]
    fn the_two_spdif_index_spaces_are_distinct() {
        assert_eq!(by_path("in.spdif.pin").unwrap().target, Target::SpdifInput);
        assert_eq!(
            by_path("in.spdif.enable").unwrap().target,
            Target::SpdifExtraInput
        );
    }

    /// V27 widened the upmixer centre mode, and v1.1.6 added two PEQ types.
    /// A choice list that has not grown rejects a value the device accepts.
    #[test]
    fn the_choice_lists_match_the_firmwares_ranges() {
        let center = by_path("up.center_mode").unwrap();
        assert!(center.kind.validate(&Value::Choice(2)).is_ok(), "OFF is 2");
        assert!(center.kind.validate(&Value::Choice(3)).is_err());

        let ty = by_path("eq.type").unwrap();
        assert!(ty.kind.validate(&Value::Choice(12)).is_ok(), "LOWPASS1");
        assert!(ty.kind.validate(&Value::Choice(13)).is_ok(), "HIGHPASS1");
        assert!(ty.kind.validate(&Value::Choice(14)).is_err());
    }

    #[test]
    fn readback_is_required_exactly_where_it_matters() {
        assert!(by_path("out.type").unwrap().needs_readback());
        assert!(by_path("dev.dacmute").unwrap().needs_readback());
        // An ordinary live write is confirmed by the notification stream instead.
        assert!(!by_path("vol.user").unwrap().needs_readback());
    }

    #[test]
    fn irreversible_actions_are_expert_only() {
        for d in REGISTRY {
            if d.hazard == Hazard::Irreversible {
                assert_eq!(
                    d.level,
                    Level::Expert,
                    "{} can't be undone and must not be casually reachable",
                    d.path
                );
            }
        }
    }

    #[test]
    fn every_row_has_human_text() {
        for d in REGISTRY {
            assert!(!d.label.is_empty(), "{} has no label", d.path);
            assert!(!d.plain.is_empty(), "{} has no plain description", d.path);
            assert!(
                d.plain.chars().next().unwrap().is_uppercase(),
                "{} description should read as a sentence",
                d.path
            );
        }
    }
}
