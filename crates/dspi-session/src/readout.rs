//! Status and packet rows read whole, for `get`.
//!
//! [`Session::read`] reads a scalar: it asks for the width of the row's wire
//! representation, which for a status or packet row is one byte of raw data.
//! That is right for the confirm-after-write path, which never trusts a
//! packet's first byte, but it made `dspi get limit.meter` print "1 bytes".
//! Here each such row asks for the length its reply actually has, and the
//! reply is decoded with its typed codec into lines for a terminal and a
//! structured value for `--json`.
//!
//! Rows with no entry in [`shape`] are left to [`Session::read`]: the
//! crosspoint, which already reads back as words, the bulk packet, which has
//! its own commands, and the control-surface caps, which are read in pieces.

use dspi_proto::generated::opcodes as op;
use dspi_proto::generated::rta as g;
use dspi_proto::packets::{
    AdatInputStatusPacket, AdatStatus, BufferStatsPacket, BuildInfo, CsAuxStates,
    CsDisplayCfgReply, CsDisplayStatus, CsExtStatusPacket, CsMacro, CsStatusPacket,
    CtrlIfaceStatus, I2sSlaveStatusPacket, LgSoundSyncStatus, LimiterMeter, LimiterStatus,
    PlatformInfo, PresetDirectory, PresetStartup, RtaBandFrame, RtaBinFrame, RtaBinHeader, RtaCaps,
    RtaConfig, RtaStatus, SiggenCapsHeader, SpdifRxStatusPacket, SubharmMeter, SystemStatus,
    UpmixConfigPacket, UpmixStatus, rta_level_dbfs, spec_for_path,
};
use dspi_proto::registry::by_path;
use dspi_transport::{TransportError, with_busy_retry};
use serde_json::{Map, Value as Json, json};

use crate::probe::Capabilities;
use crate::{Session, WriteError};

/// A status or packet row, decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct Readout {
    /// What a terminal shows, one entry per line.
    pub lines: Vec<String>,
    /// The same, structured, for `--json`.
    pub json: Json,
}

impl Readout {
    /// Every line, for a place with room for one, such as the command bar.
    pub fn one_line(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// How long a row's reply is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Always this long; a shorter reply is an error.
    Exact(usize),
    /// Sized by the device, at most this long.
    UpTo(usize),
    /// The analyser's bin frame, read from byte offsets until whole.
    Bins,
}

/// The reply length of every status or packet row `get` reads whole.
fn shape(path: &str, caps: &Capabilities) -> Option<Shape> {
    use Shape::*;
    let outputs = caps.num_outputs as usize;
    Some(match path {
        // NUM_OUTPUT_CHANNELS x uint16 (limiter.h:19, config.h:200): 18 bytes
        // on an RP2350, 10 on an RP2040.
        "limit.meter" | "sub.meter" => UpTo(2 * outputs),
        // engaged, lookahead, block, num_outputs (limiter.h:20).
        "limit.status" => Exact(LimiterStatus::SIZE),
        // A float in dB (config.h:194).
        "sub.headroom" => Exact(4),
        "rta.config" => Exact(RtaConfig::SIZE),
        "rta.caps" => Exact(RtaCaps::SIZE),
        "rta.bands" => Exact(RtaBandFrame::SIZE),
        // One 82-byte frame per selected, live channel (config.h:154); at
        // most 16 channels exist at either tap (rta.h:62).
        "rta.bands.all" => UpTo(RtaBandFrame::SIZE * 16),
        "rta.bins" => Bins,
        "rta.status" => Exact(RtaStatus::SIZE),
        // Every slot's state, then every slot's level (config.h:138-140).
        "cs.aux.all" => Exact(CsAuxStates::SIZE),

        "up.config" => Exact(UpmixConfigPacket::SIZE),
        // config.h:255.
        "up.status" => Exact(UpmixStatus::SIZE),
        "in.spdif.status" => Exact(SpdifRxStatusPacket::SIZE),
        // 24-byte IEC 60958 channel status (config.h:514).
        "in.spdif.channel" => Exact(24),
        // config.h:546.
        "in.i2s.slave.status" => Exact(I2sSlaveStatusPacket::SIZE),
        "in.adat.status" => Exact(AdatInputStatusPacket::SIZE),
        // config.h:562.
        "in.lg.status" => Exact(LgSoundSyncStatus::SIZE),
        // config.h:420.
        "adat.status" => Exact(AdatStatus::SIZE),
        // config.h:575.
        "dev.ctrl.status" => Exact(CtrlIfaceStatus::SIZE),
        "dev.dacmute" | "dev.uart" | "dev.i2c" | "cs.binding" | "cs.ir" | "cs.group"
        | "cs.display.page" | "sig.config" => Exact(spec_len(path)?),
        // 16 characters of the USB serial string (vendor_commands.c:2632-2636).
        "dev.serial" => Exact(16),
        // 4, 6 or 7 bytes by the firmware's age (config.h:325).
        "dev.platform" => UpTo(PlatformInfo::REQUEST_LEN),
        // config.h:326.
        "dev.build" => Exact(BuildInfo::SIZE),
        "preset.startup" => Exact(PresetStartup::SIZE),
        "preset.dir" => Exact(PresetDirectory::SIZE),
        // config.h:164: the whole macro, whichever step was asked for.
        "cs.macro" | "cs.macro.step" => Exact(CsMacro::SIZE),
        // config.h:337, with the tables the caps header sized.
        "cs.status" => {
            let cs = caps.cs.as_ref()?;
            Exact(CsStatusPacket::wire_len(
                cs.max_bindings,
                cs.max_ir_commands,
            ))
        }
        "cs.ext.status" => Exact(CsExtStatusPacket::SIZE),
        // config.h:174.
        "cs.display" => Exact(CsDisplayCfgReply::SIZE),
        "cs.display.status" => Exact(CsDisplayStatus::SIZE),
        // wValue 0xFFFF is the header (config.h:395).
        "sig.caps" => Exact(SiggenCapsHeader::SIZE),
        // config.h:979-985.
        "meters" => Exact(SystemStatus::wire_len(caps.num_channels)),
        "diag.buffers" => Exact(BufferStatsPacket::SIZE),
        // Six u32 counters (vendor_commands.c:3205-3212).
        "diag.usb" => Exact(24),
        // One byte each (vendor_commands.c:2545-2549, 2551-2570).
        "diag.core1" | "diag.core1.conflict" => Exact(1),
        _ => return None,
    })
}

/// The length of a typed packet, from its codec.
fn spec_len(path: &str) -> Option<usize> {
    use dspi_proto::packets::{
        CsBinding, CsDisplayPage, CsGroup, DacHwMuteConfig, I2cCtrlConfig, IrCommand, SiggenConfig,
        UartCtrlConfig,
    };
    Some(match path {
        "dev.dacmute" => DacHwMuteConfig::SIZE,
        "dev.uart" => UartCtrlConfig::SIZE,
        "dev.i2c" => I2cCtrlConfig::SIZE,
        "cs.binding" => CsBinding::SIZE,
        "cs.ir" => IrCommand::SIZE,
        "cs.group" => CsGroup::SIZE,
        "cs.display.page" => CsDisplayPage::SIZE,
        "sig.config" => SiggenConfig::SIZE,
        _ => return None,
    })
}

impl Session {
    /// Read a status or packet row whole and decode it.
    ///
    /// `Ok(None)` means the row is not one of these, and [`Session::read`]
    /// is the way to read it.
    pub fn read_whole(
        &mut self,
        path: &str,
        indices: &[u8],
    ) -> Result<Option<Readout>, WriteError> {
        let d = by_path(path).ok_or_else(|| WriteError::UnknownParam(path.into()))?;
        let Some(shape) = shape(d.path, self.capabilities()) else {
            return Ok(None);
        };
        let get = d
            .get
            .ok_or_else(|| WriteError::ReadOnly { path: path.into() })?;
        self.check_indices(d, indices)?;
        let wvalue = crate::write::read_wvalue_for(d, indices);

        let bytes = match shape {
            Shape::Exact(len) => self.with_transport(|t| {
                with_busy_retry(|| t.control_in(get, wvalue, len as u16), get)
            })?,
            Shape::UpTo(len) => self.with_transport(|t| {
                with_busy_retry(|| t.control_in_upto(get, wvalue, len as u16), get)
            })?,
            Shape::Bins => match self.read_bin_frame()? {
                Some(frame) => frame,
                None => {
                    return Ok(Some(Readout {
                        lines: vec!["no frame yet".into()],
                        json: Json::Null,
                    }));
                }
            },
        };

        // Band levels are in steps above the device's own zero, which only its
        // caps say; the header default stands in when they cannot be read
        // (rta_fft.h:44-45).
        let level_zero = if matches!(d.path, "rta.bands" | "rta.bands.all" | "rta.bins") {
            self.with_transport(|t| t.control_in(op::REQ_RTA_GET_CAPS, 0, RtaCaps::SIZE as u16))
                .ok()
                .and_then(|b| RtaCaps::decode(&b).ok())
                .map_or(g::RTA_LEVEL_ZERO_DBFS as u8, |c| c.level_zero)
        } else {
            g::RTA_LEVEL_ZERO_DBFS as u8
        };

        Ok(Some(describe(
            d.path,
            indices,
            &bytes,
            self.capabilities(),
            level_zero,
        )))
    }

    /// One whole bin frame from 0x0C, or `None` when none is published yet.
    ///
    /// 0x0C answers from a byte offset with no lock (vendor_commands.c:4297-
    /// 4311), so a frame read in pieces can straddle an update; a torn one is
    /// read again from the start (rta.c:128-131, 157-161).
    fn read_bin_frame(&mut self) -> Result<Option<Vec<u8>>, WriteError> {
        // `RTA_ORDER_MAX` points give N / 2 bins (rta_fft.h:40, rta.h:101-110).
        let ceiling = RtaBinHeader::SIZE + (1usize << g::RTA_ORDER_MAX) / 2 + 1;
        for _ in 0..3 {
            let mut frame: Vec<u8> = Vec::new();
            loop {
                let target = RtaBinHeader::decode(&frame)
                    .map(|h| h.frame_len())
                    .unwrap_or(ceiling);
                if frame.len() >= target {
                    break;
                }
                let offset = frame.len();
                let chunk = self.with_transport(|t| {
                    let len = (target - offset)
                        .min(t.max_transfer())
                        .min(u16::MAX as usize);
                    t.control_in_upto(op::REQ_RTA_GET_BINS, offset as u16, len as u16)
                });
                match chunk {
                    // Nothing published yet, or the offset ran past it.
                    Err(TransportError::Stalled { .. }) => return Ok(None),
                    Err(e) => return Err(e.into()),
                    Ok(c) if c.is_empty() => return Ok(None),
                    Ok(c) => frame.extend(c),
                }
                if frame.len() >= RtaBinHeader::SIZE && RtaBinHeader::decode(&frame).is_err() {
                    // A header that does not parse is not going to.
                    return Ok(Some(frame));
                }
            }
            if RtaBinFrame::decode(&frame).is_ok() {
                return Ok(Some(frame));
            }
        }
        Ok(None)
    }
}

/// Decode one row's reply. A reply its codec refuses is shown as bytes.
pub fn describe(
    path: &str,
    indices: &[u8],
    bytes: &[u8],
    caps: &Capabilities,
    level_zero: u8,
) -> Readout {
    decode(path, indices, bytes, caps, level_zero).unwrap_or_else(|| raw(bytes))
}

fn decode(
    path: &str,
    indices: &[u8],
    b: &[u8],
    caps: &Capabilities,
    level_zero: u8,
) -> Option<Readout> {
    Some(match path {
        "limit.meter" => limiter_meter(&LimiterMeter::decode(b).ok()?),
        "limit.status" => limiter_status(&LimiterStatus::decode(b).ok()?),
        "sub.headroom" => {
            let db = f32::from_le_bytes(b.get(..4)?.try_into().ok()?);
            Readout {
                // 0 while the synthesizer is off, or adds nothing.
                lines: vec![if db == 0.0 {
                    "none".into()
                } else {
                    format!("{db:+.1} dB")
                }],
                json: json!({ "headroom_db": db }),
            }
        }
        "sub.meter" => subharm_meter(&SubharmMeter::decode(b).ok()?),
        "rta.config" => rta_config(&RtaConfig::decode(b).ok()?),
        "rta.caps" => rta_caps(&RtaCaps::decode(b).ok()?),
        "rta.status" => rta_status(&RtaStatus::decode(b).ok()?),
        "rta.bands" => rta_bands(&[RtaBandFrame::decode(b).ok()?], level_zero, true),
        "rta.bands.all" => rta_bands(&RtaBandFrame::decode_all(b).ok()?, level_zero, false),
        "rta.bins" => rta_bins(&RtaBinFrame::decode(b).ok()?, level_zero),
        "cs.aux.all" => cs_aux(&CsAuxStates::decode(b).ok()?),

        "dev.dacmute" | "dev.uart" | "dev.i2c" | "cs.binding" | "cs.ir" | "cs.group"
        | "cs.display.page" | "sig.config" => {
            fields(spec_for_path(path).and_then(|s| (s.describe)(b))?)
        }
        "preset.startup" => {
            let p = PresetStartup::decode(b).ok()?;
            let mut f = p.to_fields();
            f.push(("last_active", p.last_active.to_string()));
            fields(f)
        }
        "cs.display" => {
            let r = CsDisplayCfgReply::decode(b).ok()?;
            let mut f = vec![
                ("max_pages", r.max_pages.to_string()),
                ("model_count", r.model_count.to_string()),
            ];
            f.extend(r.cfg.to_fields());
            fields(f)
        }
        "cs.macro.step" => {
            let m = CsMacro::decode(b).ok()?;
            fields(m.step(*indices.get(1)? as usize)?.to_fields())
        }
        "cs.macro" => debug_fields(&CsMacro::decode(b).ok()?),
        "cs.status" => {
            let cs = caps.cs.as_ref()?;
            debug_fields(
                &CsStatusPacket::decode_sized(b, cs.max_bindings, cs.max_ir_commands).ok()?,
            )
        }
        "up.config" => debug_fields(&UpmixConfigPacket::decode(b).ok()?),
        "up.status" => debug_fields(&UpmixStatus::decode(b).ok()?),
        "in.spdif.status" => debug_fields(&SpdifRxStatusPacket::decode(b).ok()?),
        "in.i2s.slave.status" => debug_fields(&I2sSlaveStatusPacket::decode(b).ok()?),
        "in.adat.status" => debug_fields(&AdatInputStatusPacket::decode(b).ok()?),
        "in.lg.status" => debug_fields(&LgSoundSyncStatus::decode(b).ok()?),
        "adat.status" => debug_fields(&AdatStatus::decode(b).ok()?),
        "dev.ctrl.status" => debug_fields(&CtrlIfaceStatus::decode(b).ok()?),
        "preset.dir" => debug_fields(&PresetDirectory::decode(b).ok()?),
        "cs.ext.status" => debug_fields(&CsExtStatusPacket::decode(b).ok()?),
        "cs.display.status" => debug_fields(&CsDisplayStatus::decode(b).ok()?),
        "sig.caps" => debug_fields(&SiggenCapsHeader::decode(b).ok()?),
        "diag.buffers" => debug_fields(&BufferStatsPacket::decode(b).ok()?),
        "dev.build" => {
            let i = BuildInfo::decode(b).ok()?;
            fields(vec![("describe", i.describe), ("date", i.date)])
        }
        "dev.platform" => {
            let p = PlatformInfo::decode(b).ok()?;
            fields(vec![
                ("platform", format!("{:?}", p.platform())),
                ("firmware", p.firmware()),
                ("outputs", p.num_output_channels.to_string()),
            ])
        }
        "dev.serial" => {
            let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
            let s = String::from_utf8_lossy(&b[..end]).to_string();
            Readout {
                lines: vec![s.clone()],
                json: Json::String(s),
            }
        }
        "diag.usb" => {
            const NAMES: [&str; 6] = [
                "total",
                "crc",
                "bitstuff",
                "rx_overflow",
                "rx_timeout",
                "data_seq",
            ];
            let f = NAMES
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    let v = u32::from_le_bytes(b.get(4 * i..4 * i + 4)?.try_into().ok()?);
                    Some((*n, v.to_string()))
                })
                .collect::<Option<Vec<_>>>()?;
            fields(f)
        }
        "diag.core1" => Readout {
            lines: vec![b.first()?.to_string()],
            json: json!(b.first()?),
        },
        "diag.core1.conflict" => {
            let yes = *b.first()? != 0;
            Readout {
                lines: vec![if yes { "conflict" } else { "no conflict" }.into()],
                json: json!({ "conflict": yes }),
            }
        }
        "meters" => meters(
            &SystemStatus::decode_sized(b, caps.num_channels).ok()?,
            caps,
        ),
        _ => return None,
    })
}

// ------------------------------------------------------------------ helpers

/// Bytes a codec refused, shown as they came.
fn raw(b: &[u8]) -> Readout {
    let hex = b
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    Readout {
        lines: vec![format!("{} bytes: {hex}", b.len())],
        json: json!({ "bytes": b }),
    }
}

/// `key  value` lines with the keys aligned, and an object of the same.
fn table(rows: Vec<(String, String, Json)>) -> Readout {
    let width = rows.iter().map(|(k, _, _)| k.len()).max().unwrap_or(0);
    let mut map = Map::new();
    let lines = rows
        .into_iter()
        .map(|(k, text, j)| {
            let line = format!("{k:<width$}  {text}");
            map.insert(k, j);
            line
        })
        .collect();
    Readout {
        lines,
        json: Json::Object(map),
    }
}

/// Named fields as their codec renders them, the same words `set` takes.
fn fields(f: Vec<(&'static str, String)>) -> Readout {
    table(
        f.into_iter()
            .map(|(k, v)| (k.to_string(), v.clone(), scalar(&v)))
            .collect(),
    )
}

/// A number, a yes/no or text, from how a field was written.
fn scalar(v: &str) -> Json {
    let t = v.trim().trim_matches('"');
    if let Ok(i) = t.parse::<i64>() {
        json!(i)
    } else if let Ok(f) = t.parse::<f64>() {
        json!(f)
    } else if let Ok(b) = t.parse::<bool>() {
        json!(b)
    } else {
        json!(t)
    }
}

/// A codec's own field names and values, from its `Debug` form.
///
/// For the older diagnostic rows, whose codecs have no field list of their
/// own: the names are the codec's, which are the firmware's struct members.
fn debug_fields(v: &impl std::fmt::Debug) -> Readout {
    let text = format!("{v:?}");
    let body = text
        .split_once('{')
        .map(|(_, rest)| rest.trim_end().trim_end_matches('}').trim())
        .unwrap_or(&text);
    table(
        split_top(body)
            .into_iter()
            .filter_map(|part| {
                let (k, v) = part.split_once(": ")?;
                Some((k.trim().to_string(), v.trim().to_string(), scalar(v)))
            })
            .collect(),
    )
}

/// Split on the commas that are not inside brackets or quotes.
fn split_top(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut quoted, mut start) = (0i32, false, 0);
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '[' | '{' | '(' if !quoted => depth += 1,
            ']' | '}' | ')' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        parts.push(&s[start..]);
    }
    parts
}

/// A u16 peak on the 0..32767 scale in dBFS; `None` for silence.
fn peak_dbfs(p: u16) -> Option<f32> {
    (p > 0).then(|| 20.0 * (p as f32 / SubharmMeter::FULL_SCALE).log10())
}

// ------------------------------------------------------- limiter and subharm

/// What `limiter_meter_centidb` answers for gain below 1e-6, which is also
/// what an output whose limiter has never run reads: its state is still
/// zeroed (limiter.c:209-219).
const LIMITER_FLOOR: u16 = 12000;

fn limiter_meter(m: &LimiterMeter) -> Readout {
    let rows = m
        .centi_db
        .iter()
        .enumerate()
        .map(|(k, c)| {
            let (text, j) = if *c == LIMITER_FLOOR {
                ("not engaged".to_string(), Json::Null)
            } else {
                let db = *c as f32 / 100.0;
                (format!("{db:.1} dB"), json!(db))
            };
            (format!("out {}", k + 1), text, j)
        })
        .collect::<Vec<_>>();
    let json = json!({ "reduction_db": rows.iter().map(|r| r.2.clone()).collect::<Vec<_>>() });
    Readout {
        json,
        ..table(rows)
    }
}

fn limiter_status(s: &LimiterStatus) -> Readout {
    Readout {
        json: json!({
            "engaged": s.engaged,
            "lookahead_samples": s.lookahead,
            "block_samples": s.block,
            "num_outputs": s.num_outputs,
        }),
        ..table(vec![
            (
                "engaged".into(),
                if s.engaged { "yes" } else { "no" }.into(),
                Json::Null,
            ),
            (
                "lookahead".into(),
                format!("{} samples", s.lookahead),
                Json::Null,
            ),
            ("block".into(), format!("{} samples", s.block), Json::Null),
            ("outputs".into(), s.num_outputs.to_string(), Json::Null),
        ])
    }
}

fn subharm_meter(m: &SubharmMeter) -> Readout {
    let rows = m
        .peaks
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let text = peak_dbfs(*p).map_or("silent".to_string(), |db| format!("{db:.1} dBFS"));
            (format!("out {}", k + 1), text, json!(p))
        })
        .collect::<Vec<_>>();
    Readout {
        json: json!({ "peak": m.peaks, "full_scale": SubharmMeter::FULL_SCALE }),
        ..table(rows)
    }
}

// ------------------------------------------------------------- the analyser

/// `RTA_TAP_*` (rta.h:30-32).
fn tap_name(tap: u8) -> &'static str {
    match tap as u16 {
        g::RTA_TAP_INPUT => "input",
        g::RTA_TAP_OUTPUT => "output",
        _ => "none",
    }
}

/// A channel at a tap, as the Console labels it: "in 1", "out 3".
fn tap_channel(tap: u8, ch: u8) -> String {
    match tap as u16 {
        g::RTA_TAP_INPUT => format!("in {}", ch as u32 + 1),
        _ => format!("out {}", ch as u32 + 1),
    }
}

/// The channels in a mask, as labels.
fn mask_channels(tap: u8, mask: u16) -> String {
    let chans: Vec<String> = (0..16)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| tap_channel(tap, i))
        .collect();
    if chans.is_empty() {
        "none".into()
    } else {
        chans.join(", ")
    }
}

fn rta_config(c: &RtaConfig) -> Readout {
    let manual = c.flags & g::RTA_FLAG_MANUAL as u8 != 0;
    Readout {
        json: json!({
            "tap": tap_name(c.tap),
            "channel_mask": c.channel_mask,
            "fft_order": c.fft_order,
            "avg_ms": c.avg_ms,
            "peak_decay_db_s": c.peak_decay_db_s,
            "manual": manual,
        }),
        ..table(vec![
            ("tap".into(), tap_name(c.tap).into(), Json::Null),
            (
                "channels".into(),
                mask_channels(c.tap, c.channel_mask),
                Json::Null,
            ),
            (
                "fft".into(),
                format!(
                    "order {} ({} points)",
                    c.fft_order,
                    1u32 << c.fft_order.min(31)
                ),
                Json::Null,
            ),
            // 0 turns each off (rta.h:59-70).
            (
                "averaging".into(),
                off_or(c.avg_ms, |v| format!("{v} ms")),
                Json::Null,
            ),
            (
                "peak decay".into(),
                off_or(c.peak_decay_db_s as u16, |v| format!("{v} dB/s")),
                Json::Null,
            ),
            // RTA_FLAG_MANUAL (rta.h:34).
            (
                "run state".into(),
                if manual { "manual" } else { "automatic" }.into(),
                Json::Null,
            ),
        ])
    }
}

fn off_or(v: u16, f: impl Fn(u16) -> String) -> String {
    if v == 0 { "off".into() } else { f(v) }
}

fn rta_caps(c: &RtaCaps) -> Readout {
    Readout {
        json: json!({
            "input_channels": c.input_channels,
            "output_channels": c.output_channels,
            "order_min": c.order_min,
            "order_max": c.order_max,
            "order_default": c.order_default,
            "bass_bands": c.bass_bands,
            "max_bands": c.max_bands,
            "level_zero": c.level_zero,
            "dynamic_range_db": c.dynamic_range_db,
            "idle_timeout_ms": c.idle_timeout_ms,
            "max_bin_frame": c.max_bin_frame,
            "bass_dynamic_range_db": c.bass_dynamic_range_db,
        }),
        ..table(vec![
            (
                "channels".into(),
                format!("{} in, {} out", c.input_channels, c.output_channels),
                Json::Null,
            ),
            (
                "fft order".into(),
                format!(
                    "{} to {}, default {}",
                    c.order_min, c.order_max, c.order_default
                ),
                Json::Null,
            ),
            (
                "bands".into(),
                format!("{} ({} from the bass bank)", c.max_bands, c.bass_bands),
                Json::Null,
            ),
            ("level zero".into(), c.level_zero.to_string(), Json::Null),
            (
                "dynamic range".into(),
                format!(
                    "{} dB, bass {} dB",
                    c.dynamic_range_db, c.bass_dynamic_range_db
                ),
                Json::Null,
            ),
            (
                "idle timeout".into(),
                format!("{} ms", c.idle_timeout_ms),
                Json::Null,
            ),
            (
                "max bin frame".into(),
                format!("{} bytes", c.max_bin_frame),
                Json::Null,
            ),
        ])
    }
}

fn rta_status(s: &RtaStatus) -> Readout {
    // RTA_STATE_* (rta.h:36-38).
    let state = match s.state as u16 {
        g::RTA_STATE_IDLE => "idle",
        g::RTA_STATE_CAPTURING => "capturing",
        g::RTA_STATE_TRANSFORMING => "transforming",
        _ => "unknown",
    };
    // RTA_CH_NONE when idle (rta.h:44, 116).
    let channel = (s.channel as u16 != g::RTA_CH_NONE).then_some(s.channel);
    // 0xFFFF: never read (rta.h:112-129).
    let idle = (s.idle_ms != 0xFFFF).then_some(s.idle_ms);
    // 0xFF: this sample rate is not supported.
    let first_band = (s.first_band != 0xFF).then_some(s.first_band);
    Readout {
        json: json!({
            "state": state,
            "tap": tap_name(s.tap),
            "channel": channel,
            "live_count": s.live_count,
            "live_mask": s.live_mask,
            "frames_per_s": s.frames_per_s,
            "busy_us_per_s": s.busy_us_per_s,
            "last_frame_us": s.last_frame_us,
            "idle_ms": idle,
            "sample_rate_hz": s.sample_rate_hz,
            "first_band": first_band,
            "bass_busy_us_per_s": s.bass_busy_us_per_s,
        }),
        ..table(vec![
            ("state".into(), state.into(), Json::Null),
            ("tap".into(), tap_name(s.tap).into(), Json::Null),
            (
                "channel".into(),
                channel.map_or("none".into(), |c| tap_channel(s.tap, c)),
                Json::Null,
            ),
            (
                "live".into(),
                format!("{} ({})", s.live_count, mask_channels(s.tap, s.live_mask)),
                Json::Null,
            ),
            (
                "frames".into(),
                format!("{} per second", s.frames_per_s),
                Json::Null,
            ),
            (
                "busy".into(),
                format!(
                    "{} us/s ({:.1} %), bass {} us/s",
                    s.busy_us_per_s,
                    s.busy_us_per_s as f32 / 10_000.0,
                    s.bass_busy_us_per_s
                ),
                Json::Null,
            ),
            (
                "last frame".into(),
                format!("{} us", s.last_frame_us),
                Json::Null,
            ),
            (
                "idle".into(),
                idle.map_or("never read".into(), |ms| format!("{ms} ms")),
                Json::Null,
            ),
            (
                "sample rate".into(),
                format!("{} Hz", s.sample_rate_hz),
                Json::Null,
            ),
            (
                "first band".into(),
                first_band.map_or("unsupported rate".into(), |b| b.to_string()),
                Json::Null,
            ),
        ])
    }
}

/// Levels as dBFS to one place, space separated.
fn level_row(levels: &[u8], level_zero: u8) -> String {
    levels
        .iter()
        .map(|l| format!("{:.1}", rta_level_dbfs(*l, level_zero)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn level_json(levels: &[u8], level_zero: u8) -> Json {
    json!(
        levels
            .iter()
            .map(|l| rta_level_dbfs(*l, level_zero))
            .collect::<Vec<_>>()
    )
}

/// Band frames name their own channel, but not their tap; frames are shown
/// by channel number, as `rta.bands` takes it. One channel's read is one
/// object, every channel's an array of them.
fn rta_bands(frames: &[RtaBandFrame], level_zero: u8, single: bool) -> Readout {
    if frames.is_empty() {
        return Readout {
            lines: vec!["no live channel".into()],
            json: json!([]),
        };
    }
    let mut lines = Vec::new();
    let mut out = Vec::new();
    for f in frames {
        let age = (f.age_ms != RtaBandFrame::NEVER).then_some(f.age_ms);
        lines.push(format!(
            "channel {}  seq {}  {} bands  {}",
            f.channel,
            f.seq,
            f.n_bands,
            age.map_or("no frame yet".into(), |a| format!("{a} ms old")),
        ));
        lines.push(format!("  avg   {}", level_row(f.avg_levels(), level_zero)));
        lines.push(format!(
            "  peak  {}",
            level_row(f.peak_levels(), level_zero)
        ));
        out.push(json!({
            "channel": f.channel,
            "seq": f.seq,
            "n_bands": f.n_bands,
            "age_ms": age,
            "avg_dbfs": level_json(f.avg_levels(), level_zero),
            "peak_dbfs": level_json(f.peak_levels(), level_zero),
        }));
    }
    Readout {
        lines,
        json: if single && out.len() == 1 {
            out.remove(0)
        } else {
            Json::Array(out)
        },
    }
}

fn rta_bins(f: &RtaBinFrame, level_zero: u8) -> Readout {
    let h = &f.header;
    let peak = f
        .levels
        .iter()
        .enumerate()
        .max_by_key(|(_, l)| **l)
        .map(|(i, l)| (i, rta_level_dbfs(*l, level_zero)));
    let mut lines = vec![
        format!("channel      {}", h.channel),
        format!("seq          {}", h.seq),
        format!(
            "fft          order {} ({} points)",
            h.fft_order,
            1u32 << h.fft_order.min(31)
        ),
        format!("sample rate  {} Hz", h.sample_rate_hz),
        format!("bins         {}", h.n_bins),
    ];
    if let Some((i, db)) = peak {
        lines.push(format!("loudest      bin {i}, {db:.1} dBFS"));
    }
    Readout {
        lines,
        json: json!({
            "channel": h.channel,
            "seq": h.seq,
            "fft_order": h.fft_order,
            "sample_rate_hz": h.sample_rate_hz,
            "n_bins": h.n_bins,
            "levels_dbfs": level_json(&f.levels, level_zero),
        }),
    }
}

// ------------------------------------------------------------ aux and meters

fn cs_aux(a: &CsAuxStates) -> Readout {
    let rows = (0..CsAuxStates::SLOTS)
        .map(|i| {
            let on = a.is_on(i);
            let pct = a.level_percent(i);
            (
                format!("slot {i}"),
                format!("{}  {pct:.1} %", if on { "on " } else { "off" }),
                json!({ "on": on, "level_pct": pct }),
            )
        })
        .collect::<Vec<_>>();
    Readout {
        json: json!({ "slots": rows.iter().map(|r| r.2.clone()).collect::<Vec<_>>() }),
        ..table(rows)
    }
}

fn meters(s: &SystemStatus, caps: &Capabilities) -> Readout {
    let mut rows: Vec<(String, String, Json)> = s
        .peaks
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let name = caps
                .channels
                .get(i)
                .map_or_else(|| format!("ch {i}"), |c| c.slug.clone());
            let mut text = peak_dbfs(*p).map_or("silent".to_string(), |db| format!("{db:.1} dBFS"));
            if s.clipped(i) {
                text.push_str("  clipped");
            }
            (name, text, Json::Null)
        })
        .collect();
    rows.push((
        "cpu".into(),
        format!("{} %, {} %", s.cpu0, s.cpu1),
        Json::Null,
    ));
    Readout {
        json: json!({
            "peak": s.peaks,
            "clip_flags": s.clip_flags,
            "cpu0": s.cpu0,
            "cpu1": s.cpu1,
            "active_inputs": s.active_inputs,
        }),
        ..table(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dspi_proto::Platform;
    use dspi_proto::generated::limiter;
    use dspi_proto::generated::wire::WIRE_FORMAT_VERSION;
    use dspi_transport::MockTransport;
    use dspi_transport::mock::Reply;

    fn caps() -> Capabilities {
        Capabilities {
            serial: "TEST".into(),
            platform: Platform::Rp2350,
            firmware: "1.1.6 beta 4".into(),
            firmware_version: dspi_proto::packets::FirmwareVersion::new(1, 1, 6, 4),
            build_info: None,
            wire_format: WIRE_FORMAT_VERSION as u8,
            num_channels: 17,
            num_inputs: 8,
            num_outputs: 9,
            max_bands: 10,
            band_storage: 12,
            channels: Vec::new(),
            features: Vec::new(),
            cs: None,
            siggen: None,
            active_preset: None,
        }
    }

    fn session(mock: MockTransport) -> Session {
        Session::new(Box::new(mock), caps()).unwrap()
    }

    fn get(mock: MockTransport, path: &str, indices: &[u8]) -> Readout {
        session(mock).read_whole(path, indices).unwrap().unwrap()
    }

    fn u16s(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// 18 bytes on an RP2350, 0.01 dB each, and 12000 for an output whose
    /// limiter has never run (limiter.c:209-219).
    #[test]
    fn the_limiter_meter_reads_every_output() {
        let mock =
            MockTransport::new().data(op::REQ_LIMITER, u16s(&[0, 150, 12000, 320, 0, 0, 0, 0, 0]));
        let log = mock.log_handle();
        let r = get(mock, "limit.meter", &[]);
        assert_eq!(r.lines.len(), 9);
        assert_eq!(r.lines[0], "out 1  0.0 dB");
        assert_eq!(r.lines[1], "out 2  1.5 dB");
        assert_eq!(r.lines[2], "out 3  not engaged");
        assert_eq!(r.lines[3], "out 4  3.2 dB");
        assert_eq!(r.json["reduction_db"][1], json!(1.5));
        assert_eq!(r.json["reduction_db"][2], Json::Null);
        assert_eq!(log.lock().unwrap()[0].value, limiter::LIMITER_GET_METER);
    }

    #[test]
    fn the_limiter_status_reads_four_bytes() {
        let mock = MockTransport::new().data(op::REQ_LIMITER, vec![1, 32, 16, 9]);
        let log = mock.log_handle();
        let r = get(mock, "limit.status", &[]);
        assert_eq!(
            r.lines,
            [
                "engaged    yes",
                "lookahead  32 samples",
                "block      16 samples",
                "outputs    9"
            ]
        );
        assert_eq!(r.json["engaged"], json!(true));
        assert_eq!(r.json["lookahead_samples"], json!(32));
        assert_eq!(log.lock().unwrap()[0].value, limiter::LIMITER_GET_STATUS);
    }

    #[test]
    fn the_sub_headroom_is_a_float_in_db() {
        let mock =
            MockTransport::new().data(op::REQ_GET_SUBHARM_HEADROOM, 4.2f32.to_le_bytes().to_vec());
        let r = get(mock, "sub.headroom", &[]);
        assert_eq!(r.lines, ["+4.2 dB"]);
        assert!((r.json["headroom_db"].as_f64().unwrap() - 4.2).abs() < 1e-6);

        let mock =
            MockTransport::new().data(op::REQ_GET_SUBHARM_HEADROOM, 0f32.to_le_bytes().to_vec());
        assert_eq!(get(mock, "sub.headroom", &[]).lines, ["none"]);
    }

    #[test]
    fn the_sub_meter_reads_a_peak_per_output() {
        let mock = MockTransport::new().data(
            op::REQ_GET_SUBHARM_METER,
            u16s(&[32767, 16384, 0, 0, 0, 0, 0, 0, 0]),
        );
        let r = get(mock, "sub.meter", &[]);
        assert_eq!(r.lines.len(), 9);
        assert_eq!(r.lines[0], "out 1  0.0 dBFS");
        assert_eq!(r.lines[1], "out 2  -6.0 dBFS");
        assert_eq!(r.lines[2], "out 3  silent");
        assert_eq!(r.json["peak"][0], json!(32767));
    }

    fn rta_caps_bytes() -> Vec<u8> {
        RtaCaps {
            input_channels: 8,
            output_channels: 9,
            order_min: 8,
            order_max: 10,
            order_default: 10,
            bass_bands: 7,
            max_bands: 41,
            level_zero: 200,
            dynamic_range_db: 100,
            idle_timeout_ms: 5000,
            max_bin_frame: 529,
            bass_dynamic_range_db: 90,
        }
        .encode()
        .to_vec()
    }

    #[test]
    fn the_analyser_caps_read_sixteen_bytes() {
        let mock = MockTransport::new().data(op::REQ_RTA_GET_CAPS, rta_caps_bytes());
        let r = get(mock, "rta.caps", &[]);
        assert_eq!(r.lines[0], "channels       8 in, 9 out");
        assert_eq!(r.lines[1], "fft order      8 to 10, default 10");
        assert_eq!(r.json["max_bands"], json!(41));
        assert_eq!(r.json["idle_timeout_ms"], json!(5000));
    }

    #[test]
    fn the_analyser_status_reads_twenty_four_bytes() {
        let s = RtaStatus {
            state: g::RTA_STATE_CAPTURING as u8,
            tap: g::RTA_TAP_OUTPUT as u8,
            channel: 1,
            live_count: 2,
            live_mask: 0b11,
            frames_per_s: 30,
            busy_us_per_s: 12_000,
            last_frame_us: 400,
            idle_ms: 0xFFFF,
            sample_rate_hz: 48_000,
            first_band: 0,
            bass_busy_us_per_s: 900,
        };
        let mock = MockTransport::new().data(op::REQ_RTA_GET_STATUS, s.encode().to_vec());
        let r = get(mock, "rta.status", &[]);
        assert!(r.lines.contains(&"state        capturing".to_string()));
        assert!(r.lines.contains(&"channel      out 2".to_string()));
        assert!(
            r.lines
                .contains(&"live         2 (out 1, out 2)".to_string())
        );
        assert!(r.lines.contains(&"idle         never read".to_string()));
        assert!(r.lines.contains(&"sample rate  48000 Hz".to_string()));
        assert_eq!(r.json["state"], json!("capturing"));
        assert_eq!(r.json["idle_ms"], Json::Null);
        assert_eq!(r.json["sample_rate_hz"], json!(48_000));
    }

    #[test]
    fn the_analyser_config_reads_twelve_bytes() {
        let c = RtaConfig {
            tap: g::RTA_TAP_INPUT as u8,
            channel_mask: 0b101,
            fft_order: 10,
            avg_ms: 0,
            peak_decay_db_s: 20,
            flags: 0,
        };
        let mock = MockTransport::new().data(op::REQ_RTA_GET_CONFIG, c.encode().to_vec());
        let r = get(mock, "rta.config", &[]);
        assert_eq!(
            r.lines,
            [
                "tap         input",
                "channels    in 1, in 3",
                "fft         order 10 (1024 points)",
                "averaging   off",
                "peak decay  20 dB/s",
                "run state   automatic",
            ]
        );
        assert_eq!(r.json["channel_mask"], json!(5));
    }

    fn frame(channel: u8) -> RtaBandFrame {
        let mut avg = [0u8; RtaBandFrame::MAX_BANDS];
        let mut peak = [0u8; RtaBandFrame::MAX_BANDS];
        avg[0] = 180;
        peak[0] = 200;
        RtaBandFrame {
            channel,
            seq: 3,
            n_bands: 2,
            age_ms: 12,
            avg,
            peak,
        }
    }

    /// 82 bytes for one channel, levels against the caps' zero.
    #[test]
    fn the_analyser_bands_read_one_frame() {
        let mock = MockTransport::new()
            .data(op::REQ_RTA_GET_BANDS, frame(1).encode().to_vec())
            .data(op::REQ_RTA_GET_CAPS, rta_caps_bytes());
        let log = mock.log_handle();
        let r = get(mock, "rta.bands", &[1]);
        assert_eq!(
            r.lines,
            [
                "channel 1  seq 3  2 bands  12 ms old",
                "  avg   -10.0 -100.0",
                "  peak  0.0 -100.0",
            ]
        );
        assert_eq!(r.json["avg_dbfs"], json!([-10.0, -100.0]));
        assert_eq!(
            log.lock().unwrap()[0].value,
            1,
            "the channel rides in wValue"
        );
    }

    #[test]
    fn the_analyser_bands_for_every_channel_are_sized_by_the_device() {
        let mut both = frame(0).encode().to_vec();
        both.extend(frame(2).encode());
        let mock = MockTransport::new()
            .data(op::REQ_RTA_GET_BANDS_ALL, both)
            .data(op::REQ_RTA_GET_CAPS, rta_caps_bytes());
        let r = get(mock, "rta.bands.all", &[]);
        assert_eq!(r.lines.len(), 6);
        assert_eq!(r.lines[3], "channel 2  seq 3  2 bands  12 ms old");
        assert_eq!(r.json.as_array().unwrap().len(), 2);

        let mock = MockTransport::new().data(op::REQ_RTA_GET_BANDS_ALL, Vec::new());
        assert_eq!(get(mock, "rta.bands.all", &[]).lines, ["no live channel"]);
    }

    /// The bin frame is read from byte offsets until whole.
    #[test]
    fn the_analyser_bins_read_a_whole_frame() {
        let f = RtaBinFrame {
            header: RtaBinHeader {
                channel: 0,
                seq: 7,
                fft_order: 8,
                sample_rate_hz: 48_000,
                n_bins: 128,
            },
            levels: (0..128u8).collect(),
        };
        let mock = MockTransport::new()
            .reply(op::REQ_RTA_GET_BINS, Reply::Window(f.encode()))
            .data(op::REQ_RTA_GET_CAPS, rta_caps_bytes());
        let r = get(mock, "rta.bins", &[]);
        assert!(r.lines.contains(&"bins         128".to_string()));
        assert!(
            r.lines
                .contains(&"loudest      bin 127, -36.5 dBFS".to_string())
        );
        assert_eq!(r.json["levels_dbfs"].as_array().unwrap().len(), 128);

        let mock = MockTransport::new().reply(op::REQ_RTA_GET_BINS, Reply::Stall);
        assert_eq!(get(mock, "rta.bins", &[]).lines, ["no frame yet"]);
    }

    /// 48 bytes: 16 states, then 16 levels in 8.8 percent (config.h:138-140).
    #[test]
    fn the_aux_table_reads_every_slot() {
        let mut block = CsAuxStates::default();
        block.state[2] = 1;
        block.level_q8[2] = 50 * 256;
        let mock = MockTransport::new().data(op::REQ_GET_CS_AUX_STATE, block.encode().to_vec());
        let log = mock.log_handle();
        let r = get(mock, "cs.aux.all", &[]);
        assert_eq!(r.lines.len(), 16);
        assert_eq!(r.lines[0], "slot 0   off  0.0 %");
        assert_eq!(r.lines[2], "slot 2   on   50.0 %");
        assert_eq!(r.json["slots"][2], json!({ "on": true, "level_pct": 50.0 }));
        assert_eq!(log.lock().unwrap()[0].value, 0xFFFF);
    }

    /// The older status rows answer more than the one byte a scalar read asks
    /// for, too.
    #[test]
    fn an_older_status_row_is_read_whole() {
        let s = AdatStatus {
            enabled: true,
            active: false,
            pin: 12,
            rate_ok: true,
            resync_count: 3,
            slip_count: 0,
        };
        let mock = MockTransport::new().data(op::REQ_GET_ADAT_STATUS, s.encode().to_vec());
        let r = get(mock, "adat.status", &[]);
        assert!(r.lines.contains(&"pin           12".to_string()));
        assert_eq!(r.json["enabled"], json!(true));
        assert_eq!(r.json["resync_count"], json!(3));
    }

    #[test]
    fn a_scalar_row_is_left_to_read() {
        let mut s = session(MockTransport::new());
        assert_eq!(s.read_whole("vol.user", &[]).unwrap(), None);
    }

    #[test]
    fn a_reply_the_codec_refuses_is_shown_as_bytes() {
        let r = describe("rta.status", &[], &[9; 24], &caps(), 200);
        assert!(r.lines[0].starts_with("24 bytes: 09 09"));
    }

    #[test]
    fn debug_fields_split_only_at_the_top_level() {
        assert_eq!(
            split_top("a: [1, 2], b: \"x, y\", c: 3"),
            ["a: [1, 2]", " b: \"x, y\"", " c: 3"]
        );
    }
}
