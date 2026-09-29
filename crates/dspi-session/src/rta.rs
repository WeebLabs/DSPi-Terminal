//! The spectrum analyser's host half: the Console's `RtaEngine`
//! (`SpectrumAnalyser.swift:380-890`) for a single-threaded loop.
//!
//! The device has one FFT engine, polled over EP0 and never pushed. It is off
//! at boot, forgets its configuration at every power cycle, starts on the
//! first band or bin read and stops itself 5 s after the last one (rta.h:13,
//! rta.h:28). So the host owns everything: the configuration it wants, the
//! polling that keeps the analyser alive, the time average of the bins, and
//! the STOP when nobody is looking.
//!
//! Views (the analyser panel today; the response-graph overlay and the bars
//! strip later) subscribe with a [`Request`] and release it when they go
//! away. The engine folds every request into one device configuration, as the
//! Console's does, so a second view adds a subscription rather than a change
//! here.
//!
//! This is the only feature that adds continuous USB traffic, and the
//! Terminal does its USB synchronously on the thread that reads the keyboard.
//! [`RtaEngine::tick`] therefore takes a [`Budget`]: one band read per tick,
//! at most one bin chunk, and no optional read once the tick has used its
//! share of time, so a parameter write is never queued behind a frame read.
//! Every tick's transfers are timed ([`Timing`]) so the share can be checked
//! against a real device.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use dspi_proto::generated::opcodes as op;
use dspi_proto::generated::rta as g;
use dspi_proto::packets::{
    PacketError, RtaBandFrame, RtaBinFrame, RtaBinHeader, RtaCaps, RtaConfig, RtaStatus,
};
use dspi_transport::{Transport, TransportError};

/// `RTA_TAP_INPUT`: after the per-input PEQ, before the matrix (rta.h:30).
pub const TAP_INPUT: u8 = g::RTA_TAP_INPUT as u8;
/// `RTA_TAP_OUTPUT`: after output gain and delay (rta.h:31).
pub const TAP_OUTPUT: u8 = g::RTA_TAP_OUTPUT as u8;
/// `RTA_STATE_IDLE` (rta.h:36).
pub const STATE_IDLE: u8 = g::RTA_STATE_IDLE as u8;
/// `RTA_CTL_STOP`, the `wValue` of 0x0E that stops the analyser (rta.h:40).
const CTL_STOP: u16 = g::RTA_CTL_STOP;
/// `RTA_ORDER_MIN` and `RTA_ORDER_MAX` (rta_fft.h:39-40): 256 to 1024 points.
pub const ORDER_MIN: u8 = g::RTA_ORDER_MIN as u8;
pub const ORDER_MAX: u8 = g::RTA_ORDER_MAX as u8;
/// `RTA_MAX_BANDS` (rta_fft.h:42).
pub const MAX_BANDS: usize = g::RTA_MAX_BANDS as usize;
/// `RTA_LEVEL_ZERO_DBFS` (rta_fft.h:45), used only until the caps say.
const LEVEL_ZERO: u8 = g::RTA_LEVEL_ZERO_DBFS as u8;

/// Status and the applied configuration are re-read this often
/// (`RTA_STATUS_INTERVAL`, Constants.swift:128). Status is not a data read, so
/// it neither starts the analyser nor keeps it alive (rta.h:147, config.h:152).
pub const STATUS_INTERVAL: Duration = Duration::from_millis(500);
/// Bins are never read faster than this, nor faster than a frame is published
/// (`RTA_MIN_BIN_INTERVAL`, Constants.swift:127).
pub const MIN_BIN_INTERVAL: Duration = Duration::from_millis(50);
/// A frame older than the larger of this and four refresh intervals is shown
/// as silence (`SpectrumAnalyser.swift:806-808`).
pub const STALE_MIN: Duration = Duration::from_millis(500);
/// Pushes of one configuration before giving up on it
/// (`SpectrumAnalyser.swift:637-653`).
pub const MAX_PUSH_ATTEMPTS: u8 = 3;
/// A SET is applied later from the device's main loop and never STALLs over
/// USB (vendor_commands.c:1784-1790, rta.h:135-137), so a push is confirmed by
/// reading 0x09 back this long afterwards rather than at once.
pub const CONFIRM_DELAY: Duration = Duration::from_millis(100);
/// Width each bin is averaged over on the host (`rtaBinSmoothingOctaves`,
/// GraphSpectrumOverlay.swift:483).
pub const BIN_SMOOTHING_OCTAVES: f64 = 1.0 / 6.0;
/// Room for a frame per channel in one 0x0F read: the stride is fixed at 82
/// bytes and at most 16 channels exist at either tap (rta.h:62).
const MAX_FRAMES: usize = 16;

/// The three settings the device owns but forgets: the Console keeps them in
/// its preferences and pushes them whenever it starts watching
/// (`RtaOptions`, SpectrumAnalyser.swift:353-364).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Transform size as an FFT order; `None` is the caps default.
    pub fft_order: Option<u8>,
    /// Averaging time constant in ms; 0 turns the extra averaging off.
    pub avg_ms: u16,
    /// Peak-hold decay in dB per second; 0 holds peaks off.
    pub peak_decay_db_s: u8,
}

impl Default for Options {
    /// The Console's defaults (`DSPi_ConsoleApp.swift:119-121`).
    fn default() -> Self {
        Self {
            fft_order: None,
            avg_ms: 300,
            peak_decay_db_s: 12,
        }
    }
}

/// What one view wants to see (`RtaRequest`, SpectrumAnalyser.swift:257-265).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub tap: u8,
    /// Bit i is channel i at `tap`.
    pub mask: u16,
    /// The bins belong to whichever channel was transformed last, so a view
    /// that wants them is asking for a one-channel selection too.
    pub wants_bins: bool,
}

/// A subscription, handed back to [`RtaEngine::release`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ViewId(u64);

/// How much of one loop tick the analyser may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Bin chunks per tick. A USB transfer carries a whole 529-byte frame, so
    /// one chunk is one frame there; a smaller transport assembles the frame
    /// over several ticks and a torn one is discarded.
    pub max_bin_chunks: usize,
    /// Once the tick's analyser transfers have taken this long, the optional
    /// reads (bins) wait for the next tick. The band read and the twice-a-
    /// second status read always go.
    pub max_time: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_bin_chunks: 1,
            max_time: Duration::from_millis(8),
        }
    }
}

/// What one tick did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TickReport {
    pub transfers: u8,
    /// Wall time spent in this tick's transfers.
    pub elapsed: Duration,
    /// The control path reported the device gone.
    pub disconnected: bool,
}

/// Running totals of the analyser's share of the loop, for checking the
/// budget against a device (HW-2b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timing {
    /// Ticks that issued at least one transfer.
    pub ticks: u64,
    pub transfers: u64,
    pub total: Duration,
    pub last: Duration,
    pub max: Duration,
}

impl Timing {
    /// Mean time per busy tick.
    pub fn mean(&self) -> Duration {
        if self.ticks == 0 {
            Duration::ZERO
        } else {
            self.total / self.ticks as u32
        }
    }
}

/// The latest bins, averaged over time and smoothed across frequency.
#[derive(Debug, Clone, PartialEq)]
pub struct Bins {
    pub channel: u8,
    pub fft_order: u8,
    pub sample_rate_hz: u32,
    /// dBFS per bin after the host's time average.
    pub levels_db: Vec<f64>,
    /// The same after 1/6-octave smoothing, which is what is drawn.
    pub smoothed_db: Vec<f64>,
}

impl Bins {
    /// Hz of bin `k`: the frame holds N/2 bins across half the sample rate
    /// (rta.h:101-110).
    pub fn frequency(&self, k: usize) -> f64 {
        if self.levels_db.is_empty() {
            return 0.0;
        }
        k as f64 * self.sample_rate_hz as f64 / (2 * self.levels_db.len()) as f64
    }

    /// The smoothed level at `hz`, interpolated between bins; `None` below
    /// bin 1 (DC belongs to no band) and above the last bin.
    pub fn level_at(&self, hz: f64) -> Option<f64> {
        let n = self.smoothed_db.len();
        if n < 2 || self.sample_rate_hz == 0 {
            return None;
        }
        let pos = hz * (2 * n) as f64 / self.sample_rate_hz as f64;
        if pos < 1.0 || pos > (n - 1) as f64 {
            return None;
        }
        let i = pos.floor() as usize;
        let frac = pos - i as f64;
        let a = self.smoothed_db[i];
        let b = self.smoothed_db[(i + 1).min(n - 1)];
        Some(a + (b - a) * frac)
    }
}

/// A band frame as held, with when it arrived.
#[derive(Debug, Clone, PartialEq)]
struct Held {
    frame: RtaBandFrame,
    received: Instant,
    stale: bool,
}

/// The bins averaged in power the way the device averages its bands
/// (`RtaBinAverage`, SpectrumAnalyser.swift:267-306): each new frame moves the
/// average by `dt / (avg + dt)`. The device never averages the bins it
/// publishes, so without this the Averaging setting would stop at the bass.
#[derive(Debug, Clone, Default, PartialEq)]
struct BinAverage {
    key: Option<(u8, u8, u8, u32, usize)>,
    seq: Option<u8>,
    time: Option<Instant>,
    power: Vec<f64>,
}

impl BinAverage {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Averaged dBFS per bin, or `None` for a frame already counted.
    fn add(
        &mut self,
        frame: &RtaBinFrame,
        tap: u8,
        now: Instant,
        avg_ms: u16,
        level_zero: u8,
    ) -> Option<Vec<f64>> {
        let h = &frame.header;
        let key = (
            tap,
            h.channel,
            h.fft_order,
            h.sample_rate_hz,
            frame.levels.len(),
        );
        if self.key == Some(key) && self.seq == Some(h.seq) {
            return None;
        }
        let fresh: Vec<f64> = frame
            .levels
            .iter()
            .map(|v| 10f64.powf(level_db(*v, level_zero) / 10.0))
            .collect();
        match self.time {
            Some(then) if self.key == Some(key) && avg_ms > 0 => {
                let dt = now.saturating_duration_since(then).as_secs_f64();
                let a = dt / (avg_ms as f64 / 1000.0 + dt);
                for (p, f) in self.power.iter_mut().zip(&fresh) {
                    *p += (f - *p) * a;
                }
            }
            _ => self.power = fresh,
        }
        self.key = Some(key);
        self.seq = Some(h.seq);
        self.time = Some(now);
        Some(
            self.power
                .iter()
                .map(|p| 10.0 * p.max(1e-30).log10())
                .collect(),
        )
    }
}

/// dBFS of one level byte: 0.5 dB steps from `level_zero`, which the caps
/// report (rta_fft.h:44-45, rta.h:81).
pub fn level_db(v: u8, level_zero: u8) -> f64 {
    (v as f64 - level_zero as f64) * 0.5
}

/// Each bin's level averaged in power over `octaves` centred on it
/// (`rtaSmoothBins`, GraphSpectrumOverlay.swift:488-504). Where the window is
/// narrower than a bin, as across the low end, the bin is left alone.
pub fn smooth_bins(levels_db: &[f64], octaves: f64) -> Vec<f64> {
    let n = levels_db.len();
    if octaves <= 0.0 || n <= 2 {
        return levels_db.to_vec();
    }
    let mut prefix = vec![0.0f64; n + 1];
    for k in 0..n {
        prefix[k + 1] = prefix[k] + 10f64.powf(levels_db[k] / 10.0);
    }
    // Bin frequency is proportional to its index, so the window is a ratio.
    let half = 2f64.powf(octaves / 2.0);
    let mut out = levels_db.to_vec();
    for (k, o) in out.iter_mut().enumerate().skip(1) {
        let lo = ((k as f64 / half).ceil() as usize).max(1);
        let hi = ((k as f64 * half).floor() as usize).min(n - 1);
        if hi <= lo {
            continue;
        }
        let mean = (prefix[hi + 1] - prefix[lo]) / (hi - lo + 1) as f64;
        *o = 10.0 * mean.max(1e-30).log10();
    }
    out
}

/// Whether band `i` has a reading at this transform size and rate
/// (`rtaBandIsPopulated`, SpectrumAnalyserView.swift:222-233). The bass bands
/// come from a continuous filter bank and always do (rta.h:79); above them the
/// firmware's FFT geometry applies: base-10 centres `1000 * 10^((i - 20) / 10)`
/// with edges at `10^(+/-0.05)`, DC and Nyquist excluded.
pub fn band_populated(i: usize, sample_rate_hz: f64, fft_order: u8, bass_bands: usize) -> bool {
    if i < bass_bands {
        return true;
    }
    if sample_rate_hz <= 0.0 || fft_order == 0 {
        return true;
    }
    let fc = 1000.0 * 10f64.powf((i as f64 - 20.0) / 10.0);
    let (lo, hi) = (fc * 10f64.powf(-0.05), fc * 10f64.powf(0.05));
    let n = (1u32 << fft_order) as f64;
    let bin_hz = sample_rate_hz / n;
    let first = (lo / bin_hz).ceil().max(1.0);
    let last = (hi / bin_hz).floor().min(n / 2.0 - 1.0);
    first <= last
}

/// The device side of the analyser, as the host sees it.
#[derive(Debug, Clone)]
pub struct RtaEngine {
    caps: Option<RtaCaps>,
    /// Nominal band centres from the caps table, so axis and bands agree.
    centres: Vec<u16>,
    options: Options,
    views: BTreeMap<ViewId, (u64, Request)>,
    next_view: u64,
    request_seq: u64,
    /// What we believe the device has applied; `None` means push again.
    applied: Option<RtaConfig>,
    /// The last configuration sent, so asking for a different one starts the
    /// give-up count again instead of inheriting it.
    last_attempted: Option<RtaConfig>,
    attempts: u8,
    rejected: bool,
    /// When a pushed configuration should be read back.
    confirm_at: Option<Instant>,
    stop_pending: bool,
    /// The tap the held frames were taken at: channel 0 means a different
    /// thing on the other side of the matrix.
    tap: u8,
    held: BTreeMap<u8, Held>,
    bins: Option<Bins>,
    bin_average: BinAverage,
    /// A bin frame being assembled over several chunks.
    partial: Vec<u8>,
    status: Option<RtaStatus>,
    last_bin_read: Option<Instant>,
    last_status_read: Option<Instant>,
    timing: Timing,
    skipped: u64,
    torn: u64,
}

impl Default for RtaEngine {
    fn default() -> Self {
        Self {
            caps: None,
            centres: Vec::new(),
            options: Options::default(),
            views: BTreeMap::new(),
            next_view: 0,
            request_seq: 0,
            applied: None,
            last_attempted: None,
            attempts: 0,
            rejected: false,
            confirm_at: None,
            stop_pending: false,
            tap: TAP_OUTPUT,
            held: BTreeMap::new(),
            bins: None,
            bin_average: BinAverage::default(),
            partial: Vec::new(),
            status: None,
            last_bin_read: None,
            last_status_read: None,
            timing: Timing::default(),
            skipped: 0,
            torn: 0,
        }
    }
}

impl RtaEngine {
    pub fn new() -> Self {
        Self::default()
    }

    // ---------------------------------------------------------------- caps

    /// Read the caps and the band-centre table, once per connect. A STALL, a
    /// short reply, a protocol other than version 3 (rta.h:27) or an
    /// incomplete table leaves the feature absent, and every view shows its
    /// unavailable notice. Returns whether the analyser is usable.
    pub fn connect(&mut self, t: &mut dyn Transport) -> bool {
        self.disconnect();
        let Ok(bytes) = t.control_in(op::REQ_RTA_GET_CAPS, 0, RtaCaps::SIZE as u16) else {
            return false;
        };
        let Ok(caps) = RtaCaps::decode(&bytes) else {
            return false;
        };
        // Centres arrive 32 to a chunk from wValue 1; the table ends at a
        // short chunk or a STALL (rta.h:140-142, rta.c:393-405).
        let mut centres = Vec::new();
        for chunk in caps.centre_chunks() {
            let want = (RtaCaps::CENTRES_PER_CHUNK * 2) as u16;
            match t.control_in_upto(op::REQ_RTA_GET_CAPS, chunk, want) {
                Ok(c) if c.len() >= 2 => {
                    centres.extend(dspi_proto::packets::decode_rta_centres(&c));
                    if c.len() < want as usize {
                        break;
                    }
                }
                _ => break,
            }
        }
        self.load(caps, centres)
    }

    /// Adopt caps and centres read elsewhere (a fixture, or `connect`).
    pub fn load(&mut self, caps: RtaCaps, centres: Vec<u16>) -> bool {
        // The Console's sanity checks (SpectrumAnalyser.swift:85-87): a caps
        // record that does not describe a transform this protocol knows is no
        // analyser at all.
        let sane = caps.order_min >= ORDER_MIN
            && caps.order_max <= ORDER_MAX
            && caps.order_min <= caps.order_default
            && caps.order_default <= caps.order_max
            && caps.max_bands > 0
            && caps.max_bands as usize <= MAX_BANDS
            && caps.bass_bands <= caps.max_bands;
        if !sane || centres.len() < caps.max_bands as usize {
            self.caps = None;
            self.centres.clear();
            return false;
        }
        self.centres = centres[..caps.max_bands as usize].to_vec();
        self.caps = Some(caps);
        self.forget_push();
        true
    }

    /// Forget the device: a new one knows nothing of this one's configuration.
    /// The views stay subscribed, since they belong to the interface.
    pub fn disconnect(&mut self) {
        self.caps = None;
        self.centres.clear();
        self.forget_push();
        self.stop_pending = false;
        self.clear_snapshot();
    }

    pub fn supported(&self) -> bool {
        self.caps.is_some()
    }

    pub fn caps(&self) -> Option<&RtaCaps> {
        self.caps.as_ref()
    }

    pub fn centres(&self) -> &[u16] {
        &self.centres
    }

    /// The bass bank's band count (rta.h:79), or the header's when unread.
    pub fn bass_bands(&self) -> usize {
        self.caps
            .map(|c| c.bass_bands as usize)
            .unwrap_or(g::RTA_BASS_BANDS as usize)
    }

    /// dBFS of a level byte against this device's zero.
    pub fn level_db(&self, v: u8) -> f64 {
        let zero = self
            .caps
            .map(|c| c.level_zero)
            .filter(|z| *z != 0)
            .unwrap_or(LEVEL_ZERO);
        level_db(v, zero)
    }

    // ------------------------------------------------------------- options

    pub fn options(&self) -> Options {
        self.options
    }

    /// Adopt new device-side options; they go out on the next tick.
    pub fn set_options(&mut self, o: Options) {
        if o == self.options {
            return;
        }
        self.options = o;
        self.forget_push();
    }

    /// The transform order that will be pushed: the preference when this
    /// device offers it, otherwise the device's default, since a size outside
    /// its range would be refused on every push (SpectrumAnalyser.swift:573-579).
    pub fn fft_order(&self) -> u8 {
        let Some(c) = self.caps else {
            return self.options.fft_order.unwrap_or(ORDER_MAX);
        };
        match self.options.fft_order {
            Some(o) if (c.order_min..=c.order_max).contains(&o) => o,
            _ => c.order_default,
        }
    }

    // ------------------------------------------------------- subscriptions

    /// Start watching. Hand the id back to [`Self::release`].
    pub fn subscribe(&mut self, request: Request) -> ViewId {
        let id = ViewId(self.next_view);
        self.next_view += 1;
        self.update(id, request);
        id
    }

    /// Change what a subscription wants. An unchanged request keeps its place,
    /// so a view that re-publishes the same thing does not steal the tap.
    pub fn update(&mut self, id: ViewId, request: Request) {
        if self.views.get(&id).map(|(_, r)| *r) == Some(request) {
            return;
        }
        self.request_seq += 1;
        self.views.insert(id, (self.request_seq, request));
        self.stop_pending = false;
    }

    /// Stop watching. When the last view goes, the next tick sends STOP: the
    /// device would stop by itself 5 s later, and stopping at once hands its
    /// processor time back (SpectrumAnalyser.swift:463-488).
    pub fn release(&mut self, id: ViewId) {
        if self.views.remove(&id).is_none() {
            return;
        }
        if self.views.is_empty() {
            self.forget_push();
            self.stop_pending = true;
        }
    }

    pub fn watching(&self) -> bool {
        !self.views.is_empty()
    }

    /// The configuration every view together asks for
    /// (`SpectrumAnalyser.swift:615-636`). The latest request is primary: it
    /// sets the tap, and a view that wants bins narrows the mask to its own.
    /// `None` when nobody is watching or the mask is empty, which the device
    /// would STALL (rta.h:62).
    pub fn wanted(&self) -> Option<RtaConfig> {
        let (_, primary) = self.views.values().max_by_key(|(seq, _)| *seq)?;
        let mask = if primary.wants_bins {
            primary.mask
        } else {
            self.views
                .values()
                .filter(|(_, r)| r.tap == primary.tap)
                .fold(0, |m, (_, r)| m | r.mask)
        };
        if mask == 0 {
            return None;
        }
        Some(RtaConfig {
            tap: primary.tap,
            channel_mask: mask,
            fft_order: self.fft_order(),
            avg_ms: self.options.avg_ms,
            peak_decay_db_s: self.options.peak_decay_db_s,
            flags: 0,
        })
    }

    fn wants_bins(&self) -> bool {
        self.views.values().any(|(_, r)| r.wants_bins)
    }

    fn forget_push(&mut self) {
        self.applied = None;
        self.last_attempted = None;
        self.attempts = 0;
        self.rejected = false;
        self.confirm_at = None;
    }

    fn clear_snapshot(&mut self) {
        self.held.clear();
        self.bins = None;
        self.bin_average.reset();
        self.partial.clear();
        self.status = None;
        self.last_bin_read = None;
        self.last_status_read = None;
    }

    // ---------------------------------------------------------------- tick

    /// One poll, from the loop's tick. Each product has its own cadence in
    /// elapsed time, so a slower loop changes how often things are read but
    /// not what is read (`tick`, SpectrumAnalyser.swift:612-773).
    pub fn tick(&mut self, t: &mut dyn Transport, now: Instant, budget: Budget) -> TickReport {
        let started = Instant::now();
        let mut report = TickReport::default();
        self.run(t, now, budget, started, &mut report);
        report.elapsed = started.elapsed();
        if report.transfers > 0 {
            self.timing.ticks += 1;
            self.timing.transfers += report.transfers as u64;
            self.timing.total += report.elapsed;
            self.timing.last = report.elapsed;
            self.timing.max = self.timing.max.max(report.elapsed);
        }
        report
    }

    fn run(
        &mut self,
        t: &mut dyn Transport,
        now: Instant,
        budget: Budget,
        started: Instant,
        report: &mut TickReport,
    ) {
        // Nobody is watching: stop the device once, and forget the picture.
        if !self.watching() {
            if self.stop_pending {
                self.stop_pending = false;
                report.transfers += 1;
                let r = t.control_in(op::REQ_RTA_CONTROL, CTL_STOP, 1);
                report.disconnected |= matches!(r, Err(TransportError::Disconnected));
                self.clear_snapshot();
            }
            return;
        }
        if !self.supported() {
            return;
        }
        let Some(want) = self.wanted() else {
            return;
        };

        // A tap change invalidates every frame held: they are indexed by
        // channel, and channel 0 is a different thing at the other tap.
        if want.tap != self.tap {
            self.tap = want.tap;
            self.held.clear();
            self.bins = None;
            self.bin_average.reset();
            self.partial.clear();
        }

        let mut pushed = false;
        if !self.agrees(self.applied, &want) {
            if !self.agrees(self.last_attempted, &want) {
                self.last_attempted = Some(want);
                self.attempts = 0;
                self.rejected = false;
            }
            if self.attempts < MAX_PUSH_ATTEMPTS {
                self.attempts += 1;
                self.applied = Some(want);
                report.transfers += 1;
                let r = t.control_out(op::REQ_RTA_SET_CONFIG, 0, &want.encode());
                if self.gone(&r, report) {
                    return;
                }
                self.confirm_at = Some(now + CONFIRM_DELAY);
                pushed = true;
            }
        }

        // Band frames: one transfer for every channel, or one for the one.
        report.transfers += 1;
        let n = want.channel_mask.count_ones() as usize;
        let frames = if n > 1 {
            // 0x0F takes the bulk lock, so it STALLs while a chunked bulk
            // session is open (vendor_commands.c:168-172, 4334-4337): that is
            // a skipped frame, not an error.
            let len = (RtaBandFrame::SIZE * n.min(MAX_FRAMES)) as u16;
            t.control_in_upto(op::REQ_RTA_GET_BANDS_ALL, 0, len)
                .map(|b| decode_frames(&b))
        } else {
            let ch = want.channel_mask.trailing_zeros() as u16;
            t.control_in(op::REQ_RTA_GET_BANDS, ch, RtaBandFrame::SIZE as u16)
                .map(|b| RtaBandFrame::decode(&b).into_iter().collect())
        };
        match frames {
            Ok(f) => self.accept_frames(want.tap, f, now),
            Err(e) => {
                if self.gone(&Err::<(), _>(e), report) {
                    return;
                }
                self.skipped += 1;
            }
        }

        // Status and the applied configuration, twice a second, and soon
        // after a push to confirm it. Not on the tick that pushed: the device
        // applies a staged configuration from its main loop.
        let status_due = self
            .last_status_read
            .is_none_or(|t0| now.saturating_duration_since(t0) >= STATUS_INTERVAL)
            || self.confirm_at.is_some_and(|at| now >= at);
        if status_due && !pushed {
            self.last_status_read = Some(now);
            self.confirm_at = None;
            report.transfers += 2;
            match t.control_in(op::REQ_RTA_GET_STATUS, 0, RtaStatus::SIZE as u16) {
                Ok(b) => {
                    if let Ok(s) = RtaStatus::decode(&b) {
                        self.accept_status(s);
                    }
                }
                Err(e) => {
                    if self.gone(&Err::<(), _>(e), report) {
                        return;
                    }
                }
            }
            match t.control_in(op::REQ_RTA_GET_CONFIG, 0, RtaConfig::SIZE as u16) {
                Ok(b) => {
                    if let Ok(applied) = RtaConfig::decode(&b) {
                        self.confirm(applied, &want);
                    }
                }
                Err(e) => {
                    if self.gone(&Err::<(), _>(e), report) {
                        return;
                    }
                }
            }
        }

        // Bins, for a one-channel view only, no faster than a frame appears.
        if self.wants_bins() && n == 1 {
            let interval = self.frame_interval().max(MIN_BIN_INTERVAL);
            let due = !self.partial.is_empty()
                || self
                    .last_bin_read
                    .is_none_or(|t0| now.saturating_duration_since(t0) >= interval);
            for _ in 0..budget.max_bin_chunks {
                if !due || started.elapsed() >= budget.max_time {
                    break;
                }
                if self.partial.is_empty() {
                    self.last_bin_read = Some(now);
                }
                report.transfers += 1;
                match self.read_bin_chunk(t, want.tap, now) {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(e) => {
                        self.partial.clear();
                        self.gone(&Err::<(), _>(e), report);
                        break;
                    }
                }
            }
        }
        self.age(now);
    }

    /// Whether `r` says the device went away; records it in the report.
    fn gone<T>(&self, r: &Result<T, TransportError>, report: &mut TickReport) -> bool {
        let d = matches!(r, Err(TransportError::Disconnected));
        report.disconnected |= d;
        d
    }

    /// The device clamps averaging and decay rather than refusing them, so a
    /// configuration agrees on the fields it either takes or refuses: tap,
    /// mask and size (SpectrumAnalyser.swift:720-725).
    fn agrees(&self, a: Option<RtaConfig>, want: &RtaConfig) -> bool {
        a.is_some_and(|a| {
            a.tap == want.tap
                && a.channel_mask == want.channel_mask
                && a.fft_order == want.fft_order
                && a.avg_ms == want.avg_ms
                && a.peak_decay_db_s == want.peak_decay_db_s
        })
    }

    /// Compare what the device applied with what was pushed.
    fn confirm(&mut self, applied: RtaConfig, want: &RtaConfig) {
        let agrees = applied.tap == want.tap
            && applied.channel_mask == want.channel_mask
            && applied.fft_order == want.fft_order;
        if agrees {
            // Keep our own copy as applied: taking the device's clamped
            // averaging would make the next tick's want differ and push again.
            self.attempts = 0;
            self.applied = Some(*want);
            self.rejected = false;
        } else if self.attempts >= MAX_PUSH_ATTEMPTS {
            self.rejected = true;
        } else {
            // Not applied yet, or refused: push again next tick.
            self.applied = None;
        }
    }

    /// Read the next piece of the bin frame. `Ok(true)` means more is needed.
    ///
    /// 0x0C answers from a byte offset with no lock (vendor_commands.c:4297-
    /// 4311), and a reply may be shorter than asked: the frame is 16 + N/2 + 1
    /// bytes, less than the caps' ceiling at every size but the largest.
    fn read_bin_chunk(
        &mut self,
        t: &mut dyn Transport,
        tap: u8,
        now: Instant,
    ) -> Result<bool, TransportError> {
        let ceiling = self
            .caps
            .map(|c| c.max_bin_frame as usize)
            .filter(|n| *n > RtaBinHeader::SIZE)
            .unwrap_or(RtaBinHeader::SIZE + (1usize << ORDER_MAX) / 2 + 1);
        let target = RtaBinHeader::decode(&self.partial)
            .map(|h| h.frame_len())
            .unwrap_or(ceiling);
        let offset = self.partial.len();
        let len = target
            .saturating_sub(offset)
            .min(t.max_transfer())
            .min(u16::MAX as usize);
        let chunk = match t.control_in_upto(op::REQ_RTA_GET_BINS, offset as u16, len as u16) {
            Ok(c) => c,
            // No frame published yet, or the offset ran past it.
            Err(TransportError::Stalled { .. }) => {
                self.partial.clear();
                return Ok(false);
            }
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            self.partial.clear();
            return Ok(false);
        }
        self.partial.extend(chunk);
        let Ok(header) = RtaBinHeader::decode(&self.partial) else {
            if self.partial.len() >= RtaBinHeader::SIZE {
                // A header that does not parse is not going to.
                self.partial.clear();
                return Ok(false);
            }
            return Ok(true);
        };
        if self.partial.len() < header.frame_len() {
            return Ok(true);
        }
        let bytes = std::mem::take(&mut self.partial);
        match RtaBinFrame::decode(&bytes) {
            Ok(frame) => self.accept_bins(tap, &frame, now),
            // The device republished mid-read: discard rather than draw
            // half of each (rta.c:128-131, 157-161).
            Err(PacketError::Torn { .. }) => self.torn += 1,
            Err(_) => {}
        }
        Ok(false)
    }

    // ------------------------------------------------------------ accepting

    /// Take band frames read at `tap`. Frames name their own channel.
    pub fn accept_frames(&mut self, tap: u8, frames: Vec<RtaBandFrame>, now: Instant) {
        if tap != self.tap {
            self.tap = tap;
            self.held.clear();
            self.bins = None;
            self.bin_average.reset();
        }
        for frame in frames {
            self.held.insert(
                frame.channel,
                Held {
                    frame,
                    received: now,
                    stale: false,
                },
            );
        }
        self.age(now);
    }

    /// Take a whole bin frame, time-averaging it into what is drawn.
    pub fn accept_bins(&mut self, tap: u8, frame: &RtaBinFrame, now: Instant) {
        let zero = self
            .caps
            .map(|c| c.level_zero)
            .filter(|z| *z != 0)
            .unwrap_or(LEVEL_ZERO);
        if let Some(levels) = self
            .bin_average
            .add(frame, tap, now, self.options.avg_ms, zero)
        {
            let smoothed = smooth_bins(&levels, BIN_SMOOTHING_OCTAVES);
            self.bins = Some(Bins {
                channel: frame.header.channel,
                fft_order: frame.header.fft_order,
                sample_rate_hz: frame.header.sample_rate_hz,
                levels_db: levels,
                smoothed_db: smoothed,
            });
        }
    }

    pub fn accept_status(&mut self, s: RtaStatus) {
        self.status = Some(s);
    }

    /// Silence every held frame that has stopped updating.
    ///
    /// The device publishes only while audio arrives. When the host stops
    /// streaming, each channel keeps its last levels part way down a fade,
    /// and only its age grows (`silencingStale`, SpectrumAnalyser.swift:
    /// 811-829). A frame is as old as the device said when it was read plus
    /// the time since, so a stream this host stopped reading goes quiet too.
    fn age(&mut self, now: Instant) {
        let limit = self.stale_after();
        let mut stale_channels = BTreeSet::new();
        for (ch, h) in self.held.iter_mut() {
            if h.frame.age_ms == RtaBandFrame::NEVER {
                continue;
            }
            let age = Duration::from_millis(h.frame.age_ms as u64)
                + now.saturating_duration_since(h.received);
            if age > limit && !h.stale {
                h.stale = true;
                h.frame.avg = [0; RtaBandFrame::MAX_BANDS];
                h.frame.peak = [0; RtaBandFrame::MAX_BANDS];
            }
            if h.stale {
                stale_channels.insert(*ch);
            }
        }
        // Bins carry no age of their own; they go with their channel.
        if self
            .bins
            .as_ref()
            .is_some_and(|b| stale_channels.contains(&b.channel))
        {
            self.bins = None;
            self.bin_average.reset();
        }
    }

    // --------------------------------------------------------------- reading

    /// The tap the held frames were taken at.
    pub fn tap(&self) -> u8 {
        self.tap
    }

    /// The latest frame for `channel`, only when it was taken at `tap` and
    /// has data: a view at the other tap draws nothing rather than somebody
    /// else's channel (SpectrumAnalyserView.swift:554-561).
    pub fn frame(&self, tap: u8, channel: u8) -> Option<&RtaBandFrame> {
        if tap != self.tap {
            return None;
        }
        self.held
            .get(&channel)
            .map(|h| &h.frame)
            .filter(|f| f.age_ms != RtaBandFrame::NEVER)
    }

    /// Whether `channel`'s frame has gone stale and is shown as silence.
    pub fn is_stale(&self, channel: u8) -> bool {
        self.held.get(&channel).is_some_and(|h| h.stale)
    }

    /// The latest bins, when they belong to `channel` at `tap`.
    pub fn bins(&self, tap: u8, channel: u8) -> Option<&Bins> {
        self.bins
            .as_ref()
            .filter(|b| tap == self.tap && b.channel == channel)
    }

    pub fn status(&self) -> Option<&RtaStatus> {
        self.status.as_ref()
    }

    pub fn running(&self) -> bool {
        self.status.is_some_and(|s| s.state != STATE_IDLE)
    }

    /// Set once the device has refused the wanted configuration three times.
    pub fn rejected(&self) -> bool {
        self.rejected
    }

    pub fn timing(&self) -> Timing {
        self.timing
    }

    /// Band reads that were refused (a STALL on 0x0F during a bulk session).
    pub fn skipped_frames(&self) -> u64 {
        self.skipped
    }

    /// Bin frames discarded because they changed under a chunked read.
    pub fn torn_frames(&self) -> u64 {
        self.torn
    }

    /// How long one channel waits between frames: its capture time times the
    /// channels sharing the rotation, from the device's own frame rate when it
    /// reports one (`channelRefreshInterval`, SpectrumAnalyserView.swift:
    /// 595-603).
    pub fn refresh_interval(&self) -> Duration {
        let s = self.status.unwrap_or_default();
        let live = s.live_count.max(1) as f64;
        let secs = if s.frames_per_s > 0 {
            live / s.frames_per_s as f64
        } else {
            let rate = if s.sample_rate_hz > 0 {
                s.sample_rate_hz as f64
            } else {
                48_000.0
            };
            (1u32 << self.fft_order()) as f64 / rate * live
        };
        Duration::from_secs_f64(secs)
    }

    /// The bin cadence follows the device's frame time, so a 1024-point
    /// transform is not read four times a frame (SpectrumAnalyser.swift:
    /// 870-876).
    fn frame_interval(&self) -> Duration {
        self.refresh_interval()
    }

    /// How old a frame may get before it is silence: a few refresh intervals,
    /// so a slow rotation through many channels is not taken for a stopped
    /// stream.
    pub fn stale_after(&self) -> Duration {
        STALE_MIN.max(self.refresh_interval() * 4)
    }

    /// "idle", or how often each channel refreshes
    /// (`refreshDescription`, SpectrumAnalyserView.swift:565-572).
    pub fn refresh_description(&self) -> String {
        let Some(s) = self
            .status
            .filter(|s| s.state != STATE_IDLE && s.live_count > 0)
        else {
            return "idle".into();
        };
        let ms = (self.refresh_interval().as_secs_f64() * 1000.0).round() as u64;
        let channels = if s.live_count == 1 {
            "1 channel".to_string()
        } else {
            format!("{} channels", s.live_count)
        };
        format!("{channels}, each refreshed every {ms} ms")
    }

    /// The sample rate the analyser runs at, or 48 kHz before it says.
    pub fn sample_rate_hz(&self) -> f64 {
        self.status
            .map(|s| s.sample_rate_hz)
            .filter(|r| *r > 0)
            .map(|r| r as f64)
            .unwrap_or(48_000.0)
    }

    /// Whether band `i` is measured at the current size and rate.
    pub fn band_populated(&self, i: usize) -> bool {
        band_populated(
            i,
            self.sample_rate_hz(),
            self.fft_order(),
            self.bass_bands(),
        )
    }

    /// The centre of the lowest band this configuration can measure, for the
    /// note that says what a larger transform would buy
    /// (`lowestMeasurableCentreHz`, SpectrumAnalyserView.swift:584-590). The
    /// status reports 0xFF for an unsupported rate (rta.h:125).
    pub fn lowest_measurable_centre(&self) -> Option<u16> {
        let first = self.status.map(|s| {
            if s.first_band == 0xFF {
                MAX_BANDS
            } else {
                s.first_band as usize
            }
        })?;
        if first == 0 || first >= self.centres.len() {
            return None;
        }
        Some(self.centres[first])
    }
}

/// The frames of a 0x0F reply: 82-byte strides, one per live channel. A
/// channel the device could not fill is zeroed (vendor_commands.c:4344-4347),
/// which fails the version check and is skipped rather than failing the rest.
fn decode_frames(b: &[u8]) -> Vec<RtaBandFrame> {
    b.chunks_exact(RtaBandFrame::SIZE)
        .filter_map(|c| RtaBandFrame::decode(c).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dspi_transport::MockTransport;
    use dspi_transport::mock::{Direction, LogHandle, Reply};

    /// The RP2350's caps (survey-firmware-beta4 3.2).
    fn caps() -> RtaCaps {
        RtaCaps {
            input_channels: 8,
            output_channels: 9,
            order_min: 8,
            order_max: 10,
            order_default: 10,
            bass_bands: 14,
            max_bands: 37,
            level_zero: 243,
            dynamic_range_db: 120,
            idle_timeout_ms: 5000,
            max_bin_frame: 529,
            bass_dynamic_range_db: 70,
        }
    }

    /// `rta_band_centre_tab` (rta_tables.h:818-822).
    const CENTRES: [u16; 37] = [
        10, 13, 16, 20, 25, 32, 40, 50, 63, 80, 100, 125, 160, 200, 250, 315, 400, 500, 630, 800,
        1000, 1250, 1600, 2000, 2500, 3150, 4000, 5000, 6300, 8000, 10000, 12500, 16000, 20000,
        25000, 31500, 40000,
    ];

    fn chunk(range: std::ops::Range<usize>) -> Vec<u8> {
        CENTRES[range]
            .iter()
            .flat_map(|c| c.to_le_bytes())
            .collect()
    }

    /// The caps opcode answers the header, then the two centre chunks, in
    /// the order the engine asks for them.
    fn caps_reply() -> Reply {
        Reply::Sequence(vec![
            Reply::Data(caps().encode().to_vec()),
            Reply::Data(chunk(0..32)),
            Reply::Data(chunk(32..37)),
        ])
    }

    fn frame(channel: u8, level: u8, age_ms: u16) -> RtaBandFrame {
        RtaBandFrame {
            channel,
            seq: 1,
            n_bands: 34,
            age_ms,
            avg: [level; RtaBandFrame::MAX_BANDS],
            peak: [level.saturating_add(6); RtaBandFrame::MAX_BANDS],
        }
    }

    fn config(tap: u8, mask: u16, order: u8) -> RtaConfig {
        RtaConfig {
            tap,
            channel_mask: mask,
            fft_order: order,
            avg_ms: 300,
            peak_decay_db_s: 12,
            flags: 0,
        }
    }

    fn bin_frame(seq: u8, level: u8) -> RtaBinFrame {
        RtaBinFrame {
            header: RtaBinHeader {
                channel: 0,
                seq,
                fft_order: 10,
                sample_rate_hz: 48_000,
                n_bins: 512,
            },
            levels: vec![level; 512],
        }
    }

    /// A device whose analyser applies whatever it is sent.
    fn device(mock: MockTransport) -> (RtaEngine, MockTransport, LogHandle) {
        let mut mock = mock.reply(op::REQ_RTA_GET_CAPS, caps_reply());
        let log = mock.log_handle();
        let mut e = RtaEngine::new();
        assert!(e.connect(&mut mock), "caps and centres read");
        log.lock().unwrap().clear();
        (e, mock, log)
    }

    fn sent(log: &LogHandle, opcode: u8) -> Vec<(Direction, u16, Vec<u8>)> {
        log.lock()
            .unwrap()
            .iter()
            .filter(|x| x.opcode == opcode)
            .map(|x| (x.direction, x.value, x.payload.clone()))
            .collect()
    }

    fn one(tap: u8, ch: u8) -> Request {
        Request {
            tap,
            mask: 1 << ch,
            wants_bins: true,
        }
    }

    #[test]
    fn connect_reads_the_caps_and_the_band_centres() {
        let mut mock = MockTransport::new().reply(op::REQ_RTA_GET_CAPS, caps_reply());
        let log = mock.log_handle();
        let mut e = RtaEngine::new();
        assert!(e.connect(&mut mock));
        assert_eq!(e.caps(), Some(&caps()));
        assert_eq!(e.centres(), &CENTRES[..]);
        // wValue 0 is the header, 1 and 2 the chunks (rta.h:140-142).
        let values: Vec<u16> = sent(&log, op::REQ_RTA_GET_CAPS)
            .iter()
            .map(|(_, v, _)| *v)
            .collect();
        assert_eq!(values, vec![0, 1, 2]);
        assert_eq!(e.fft_order(), 10, "the caps default");
    }

    #[test]
    fn no_caps_means_no_analyser() {
        // A firmware without the feature STALLs the caps read.
        let mut e = RtaEngine::new();
        assert!(!e.connect(&mut MockTransport::new()));
        assert!(!e.supported());

        // Version 2 is incompatible with 3 and is refused by name.
        let mut old = caps().encode();
        old[0] = 2;
        let mut mock = MockTransport::new().data(op::REQ_RTA_GET_CAPS, old.to_vec());
        assert!(!e.connect(&mut mock));

        // A table that stops short is not an analyser either.
        let mut mock = MockTransport::new().reply(
            op::REQ_RTA_GET_CAPS,
            Reply::Sequence(vec![
                Reply::Data(caps().encode().to_vec()),
                Reply::Data(chunk(0..20)),
            ]),
        );
        assert!(!e.connect(&mut mock));

        // And nothing is polled for a view without an analyser.
        let mut mock = MockTransport::new();
        let log = mock.log_handle();
        e.subscribe(one(TAP_OUTPUT, 0));
        e.tick(&mut mock, Instant::now(), Budget::default());
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn a_preference_the_device_cannot_do_falls_back_to_its_default() {
        let mut e = RtaEngine::new();
        e.set_options(Options {
            fft_order: Some(9),
            ..Options::default()
        });
        let mut c = caps();
        c.order_max = 9;
        c.order_default = 9;
        assert!(e.load(c, CENTRES.to_vec()));
        assert_eq!(e.fft_order(), 9);
        e.set_options(Options {
            fft_order: Some(10),
            ..Options::default()
        });
        assert_eq!(e.fft_order(), 9, "1024 points is past this device");
    }

    #[test]
    fn the_configuration_is_pushed_once_and_confirmed() {
        let want = config(TAP_OUTPUT, 1 << 2, 10);
        let (mut e, mut mock, log) = device(
            MockTransport::new()
                .data(op::REQ_RTA_GET_CONFIG, want.encode().to_vec())
                .data(
                    op::REQ_RTA_GET_STATUS,
                    RtaStatus::default().encode().to_vec(),
                )
                .data(op::REQ_RTA_GET_BANDS, frame(2, 200, 10).encode().to_vec()),
        );
        e.subscribe(Request {
            tap: TAP_OUTPUT,
            mask: 1 << 2,
            wants_bins: false,
        });
        let t0 = Instant::now();
        e.tick(&mut mock, t0, Budget::default());
        let pushes = sent(&log, op::REQ_RTA_SET_CONFIG);
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].2, want.encode().to_vec());
        // Not read back on the tick that pushed...
        assert!(sent(&log, op::REQ_RTA_GET_CONFIG).is_empty());
        // ...but soon after, and then it is settled.
        e.tick(&mut mock, t0 + CONFIRM_DELAY, Budget::default());
        assert_eq!(sent(&log, op::REQ_RTA_GET_CONFIG).len(), 1);
        for i in 2..20 {
            e.tick(
                &mut mock,
                t0 + Duration::from_millis(50 * i),
                Budget::default(),
            );
        }
        assert_eq!(sent(&log, op::REQ_RTA_SET_CONFIG).len(), 1, "never again");
        assert!(!e.rejected());
        // Twice a second after that.
        assert_eq!(sent(&log, op::REQ_RTA_GET_STATUS).len(), 2);

        // A new preference goes out on the next tick.
        e.set_options(Options {
            avg_ms: 1000,
            ..Options::default()
        });
        e.tick(&mut mock, t0 + Duration::from_secs(2), Budget::default());
        let pushes = sent(&log, op::REQ_RTA_SET_CONFIG);
        assert_eq!(pushes.len(), 2);
        assert_eq!(RtaConfig::decode(&pushes[1].2).unwrap().avg_ms, 1000);
    }

    #[test]
    fn a_refused_configuration_is_tried_three_times_then_reported() {
        // The device keeps answering a 512-point configuration.
        let other = config(TAP_OUTPUT, 1, 9);
        let (mut e, mut mock, log) = device(
            MockTransport::new()
                .data(op::REQ_RTA_GET_CONFIG, other.encode().to_vec())
                .data(
                    op::REQ_RTA_GET_STATUS,
                    RtaStatus::default().encode().to_vec(),
                )
                .data(op::REQ_RTA_GET_BANDS, frame(0, 200, 10).encode().to_vec()),
        );
        e.subscribe(Request {
            tap: TAP_OUTPUT,
            mask: 1,
            wants_bins: false,
        });
        let t0 = Instant::now();
        for i in 0..40 {
            e.tick(
                &mut mock,
                t0 + Duration::from_millis(100 * i),
                Budget::default(),
            );
        }
        assert_eq!(sent(&log, op::REQ_RTA_SET_CONFIG).len(), 3);
        assert!(e.rejected(), "Device refused this configuration");

        // Asking for something different starts the count again.
        e.subscribe(Request {
            tap: TAP_OUTPUT,
            mask: 0b11,
            wants_bins: false,
        });
        e.tick(&mut mock, t0 + Duration::from_secs(5), Budget::default());
        assert_eq!(sent(&log, op::REQ_RTA_SET_CONFIG).len(), 4);
    }

    #[test]
    fn one_channel_reads_its_bands_with_0x0b() {
        let (mut e, mut mock, log) = device(
            MockTransport::new()
                .data(op::REQ_RTA_GET_BANDS, frame(3, 223, 10).encode().to_vec())
                .answering_everything(RtaStatus::default().encode().to_vec()),
        );
        e.subscribe(Request {
            tap: TAP_INPUT,
            mask: 1 << 3,
            wants_bins: false,
        });
        e.tick(&mut mock, Instant::now(), Budget::default());
        let reads = sent(&log, op::REQ_RTA_GET_BANDS);
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].1, 3, "wValue is the channel");
        assert!(sent(&log, op::REQ_RTA_GET_BANDS_ALL).is_empty());
        let f = e.frame(TAP_INPUT, 3).expect("held");
        assert_eq!(e.level_db(f.avg[0]), -10.0);
        assert!(
            e.frame(TAP_OUTPUT, 3).is_none(),
            "the other tap has nothing"
        );
    }

    #[test]
    fn several_channels_read_every_frame_with_0x0f() {
        let mut all = frame(0, 200, 10).encode().to_vec();
        // A channel the device could not fill comes back zeroed and is skipped.
        all.extend([0u8; RtaBandFrame::SIZE]);
        all.extend(frame(4, 180, 10).encode());
        let (mut e, mut mock, log) = device(
            MockTransport::new()
                .data(op::REQ_RTA_GET_BANDS_ALL, all)
                .answering_everything(RtaStatus::default().encode().to_vec()),
        );
        e.subscribe(Request {
            tap: TAP_OUTPUT,
            mask: 0b1_0011,
            wants_bins: false,
        });
        e.tick(&mut mock, Instant::now(), Budget::default());
        assert_eq!(sent(&log, op::REQ_RTA_GET_BANDS_ALL).len(), 1);
        assert!(sent(&log, op::REQ_RTA_GET_BANDS).is_empty());
        assert!(e.frame(TAP_OUTPUT, 0).is_some());
        assert!(e.frame(TAP_OUTPUT, 1).is_none());
        assert!(e.frame(TAP_OUTPUT, 4).is_some());
        // Several channels rotate, so nobody reads bins.
        assert!(sent(&log, op::REQ_RTA_GET_BINS).is_empty());
    }

    #[test]
    fn a_stall_on_0x0f_skips_a_frame_and_keeps_the_picture() {
        let (mut e, mut mock, _log) = device(
            MockTransport::new()
                .reply(
                    op::REQ_RTA_GET_BANDS_ALL,
                    Reply::Sequence(vec![
                        Reply::Data(frame(1, 200, 10).encode().to_vec()),
                        // A bulk session holds the lock (vendor_commands.c:168-172).
                        Reply::Stall,
                        Reply::Data(frame(1, 210, 10).encode().to_vec()),
                    ]),
                )
                .answering_everything(RtaStatus::default().encode().to_vec()),
        );
        e.subscribe(Request {
            tap: TAP_OUTPUT,
            mask: 0b11,
            wants_bins: false,
        });
        let t0 = Instant::now();
        e.tick(&mut mock, t0, Budget::default());
        let r = e.tick(&mut mock, t0 + Duration::from_millis(50), Budget::default());
        assert!(!r.disconnected);
        assert_eq!(e.skipped_frames(), 1);
        assert_eq!(e.frame(TAP_OUTPUT, 1).unwrap().avg[0], 200, "kept");
        e.tick(
            &mut mock,
            t0 + Duration::from_millis(100),
            Budget::default(),
        );
        assert_eq!(e.frame(TAP_OUTPUT, 1).unwrap().avg[0], 210);
    }

    #[test]
    fn bins_are_read_for_one_channel_and_averaged_and_smoothed() {
        let (mut e, mut mock, log) = device(
            MockTransport::new()
                .window(op::REQ_RTA_GET_BINS, bin_frame(5, 223).encode())
                .data(op::REQ_RTA_GET_BANDS, frame(0, 200, 10).encode().to_vec())
                .answering_everything(RtaStatus::default().encode().to_vec()),
        );
        e.subscribe(one(TAP_OUTPUT, 0));
        let t0 = Instant::now();
        e.tick(&mut mock, t0, Budget::default());
        let b = e.bins(TAP_OUTPUT, 0).expect("bins");
        assert_eq!(b.levels_db.len(), 512);
        assert!((b.smoothed_db[100] - -10.0).abs() < 1e-9, "flat stays flat");
        assert!((b.frequency(512) - 24_000.0).abs() < 1e-9);
        // Not again until a frame interval has passed.
        e.tick(&mut mock, t0 + Duration::from_millis(10), Budget::default());
        assert_eq!(sent(&log, op::REQ_RTA_GET_BINS).len(), 1);
        e.tick(&mut mock, t0 + Duration::from_millis(60), Budget::default());
        assert_eq!(sent(&log, op::REQ_RTA_GET_BINS).len(), 2);
    }

    #[test]
    fn a_torn_bin_frame_is_discarded() {
        // Read in 64-byte chunks, the frame is republished part way through:
        // the head says sequence 5 and the tail sequence 6.
        let a = bin_frame(5, 223).encode();
        let b = bin_frame(6, 200).encode();
        let (mut e, mut mock, _log) = device(
            MockTransport::new()
                .with_max_transfer(64)
                .reply(
                    op::REQ_RTA_GET_BINS,
                    Reply::Sequence(
                        std::iter::repeat_n(Reply::Window(a.clone()), 8)
                            .chain(std::iter::repeat_n(Reply::Window(b), 1))
                            .chain(std::iter::once(Reply::Window(a)))
                            .collect(),
                    ),
                )
                .data(op::REQ_RTA_GET_BANDS, frame(0, 200, 10).encode().to_vec())
                .answering_everything(RtaStatus::default().encode().to_vec()),
        );
        e.subscribe(one(TAP_OUTPUT, 0));
        let t0 = Instant::now();
        // One chunk a tick: nine ticks for 529 bytes.
        for i in 0..9 {
            e.tick(&mut mock, t0 + Duration::from_millis(i), Budget::default());
        }
        assert_eq!(e.torn_frames(), 1);
        assert!(e.bins(TAP_OUTPUT, 0).is_none(), "half of each is not drawn");

        // An in-progress tail (0xFF) is torn too, whatever the head says.
        let mut tail = bin_frame(7, 223).encode();
        *tail.last_mut().unwrap() = 0xFF;
        assert!(matches!(
            RtaBinFrame::decode(&tail),
            Err(PacketError::Torn { .. })
        ));
    }

    #[test]
    fn a_stale_frame_is_shown_as_silence() {
        let mut e = RtaEngine::new();
        assert!(e.load(caps(), CENTRES.to_vec()));
        let t0 = Instant::now();
        // The device says this channel last published 600 ms ago.
        e.accept_frames(TAP_OUTPUT, vec![frame(0, 200, 600), frame(1, 200, 10)], t0);
        assert!(e.is_stale(0));
        assert_eq!(e.frame(TAP_OUTPUT, 0).unwrap().avg[0], 0, "silence");
        assert_eq!(e.frame(TAP_OUTPUT, 0).unwrap().peak[0], 0);
        assert!(!e.is_stale(1));

        // A fresh frame that is never replaced goes quiet as time passes.
        e.accept_frames(TAP_OUTPUT, vec![], t0 + Duration::from_millis(600));
        assert!(e.is_stale(1));

        // Several channels rotating slowly are allowed four refresh intervals.
        e.accept_status(RtaStatus {
            state: 1,
            live_count: 9,
            frames_per_s: 10,
            ..RtaStatus::default()
        });
        assert_eq!(e.stale_after(), Duration::from_millis(3600));
        assert_eq!(
            e.refresh_description(),
            "9 channels, each refreshed every 900 ms"
        );
    }

    #[test]
    fn closing_the_last_view_sends_stop() {
        let (mut e, mut mock, log) = device(
            MockTransport::new()
                .data(op::REQ_RTA_GET_BANDS, frame(0, 200, 10).encode().to_vec())
                .answering_everything(vec![1; 24]),
        );
        let a = e.subscribe(one(TAP_OUTPUT, 0));
        let b = e.subscribe(one(TAP_OUTPUT, 0));
        let t0 = Instant::now();
        e.tick(&mut mock, t0, Budget::default());
        e.release(a);
        e.tick(&mut mock, t0 + Duration::from_millis(50), Budget::default());
        assert!(sent(&log, op::REQ_RTA_CONTROL).is_empty(), "one view left");
        e.release(b);
        e.tick(
            &mut mock,
            t0 + Duration::from_millis(100),
            Budget::default(),
        );
        let stops = sent(&log, op::REQ_RTA_CONTROL);
        assert_eq!(stops, vec![(Direction::In, 0, vec![])], "0x0E wValue 0");
        assert!(e.frame(TAP_OUTPUT, 0).is_none(), "the picture is forgotten");
        // Once.
        e.tick(
            &mut mock,
            t0 + Duration::from_millis(150),
            Budget::default(),
        );
        assert_eq!(sent(&log, op::REQ_RTA_CONTROL).len(), 1);

        // A view that comes back before the tick cancels the stop.
        let c = e.subscribe(one(TAP_OUTPUT, 0));
        e.release(c);
        e.subscribe(one(TAP_OUTPUT, 0));
        e.tick(
            &mut mock,
            t0 + Duration::from_millis(200),
            Budget::default(),
        );
        assert_eq!(sent(&log, op::REQ_RTA_CONTROL).len(), 1);
    }

    #[test]
    fn the_latest_request_sets_the_tap_and_bins_narrow_the_mask() {
        let mut e = RtaEngine::new();
        assert!(e.load(caps(), CENTRES.to_vec()));
        let strip = e.subscribe(Request {
            tap: TAP_OUTPUT,
            mask: 0b0110,
            wants_bins: false,
        });
        e.subscribe(Request {
            tap: TAP_OUTPUT,
            mask: 0b1000,
            wants_bins: false,
        });
        assert_eq!(e.wanted().unwrap().channel_mask, 0b1110, "one tap, merged");
        let panel = e.subscribe(one(TAP_INPUT, 1));
        let w = e.wanted().unwrap();
        assert_eq!((w.tap, w.channel_mask), (TAP_INPUT, 0b10));
        e.release(panel);
        // Re-publishing an unchanged request does not steal the tap.
        e.update(strip, e.views[&strip].1);
        assert_eq!(e.wanted().unwrap().channel_mask, 0b1110);
    }

    #[test]
    fn the_bin_average_moves_by_dt_over_avg_plus_dt() {
        let mut avg = BinAverage::default();
        let t0 = Instant::now();
        let first = avg.add(&bin_frame(1, 243), 1, t0, 300, 243).unwrap();
        assert!(first[10].abs() < 1e-9);
        assert!(
            avg.add(&bin_frame(1, 243), 1, t0, 300, 243).is_none(),
            "seen"
        );
        // 20 dB down, 300 ms later with a 300 ms average: halfway in power.
        let next = avg
            .add(
                &bin_frame(2, 203),
                1,
                t0 + Duration::from_millis(300),
                300,
                243,
            )
            .unwrap();
        let want = 10.0 * ((1.0 + 0.01) / 2.0f64).log10();
        assert!((next[10] - want).abs() < 1e-9, "{} vs {want}", next[10]);
    }

    #[test]
    fn smoothing_spreads_a_tone_across_a_sixth_of_an_octave() {
        let mut levels = vec![-120.0; 512];
        levels[400] = 0.0;
        let s = smooth_bins(&levels, BIN_SMOOTHING_OCTAVES);
        assert!(s[400] < -1.0, "a pure tone high up reads low: {}", s[400]);
        assert!(s[390] > -60.0, "and spreads to its neighbours: {}", s[390]);
        // Across the low end the window is narrower than a bin.
        levels[3] = 0.0;
        assert_eq!(smooth_bins(&levels, BIN_SMOOTHING_OCTAVES)[3], 0.0);
    }

    #[test]
    fn bass_bands_are_always_populated_and_small_transforms_leave_gaps() {
        assert!(band_populated(0, 48_000.0, 8, 14));
        assert!(band_populated(13, 48_000.0, 8, 14));
        // 256 points at 48 kHz are 187.5 Hz apart: the 250 Hz band has none.
        assert!(!band_populated(14, 48_000.0, 8, 14));
        assert!(band_populated(14, 48_000.0, 10, 14));
        assert!(band_populated(30, 48_000.0, 8, 14));
    }

    /// The analyser's share of a tick against the mock, which has no bus:
    /// the numbers are the engine's own cost. HW-2b measures the real ones.
    #[test]
    fn a_tick_stays_inside_its_budget() {
        let (mut e, mut mock, _log) = device(
            MockTransport::new()
                .window(op::REQ_RTA_GET_BINS, bin_frame(5, 223).encode())
                .data(op::REQ_RTA_GET_BANDS, frame(0, 200, 10).encode().to_vec())
                .data(
                    op::REQ_RTA_GET_CONFIG,
                    config(TAP_OUTPUT, 1, 10).encode().to_vec(),
                )
                .answering_everything(RtaStatus::default().encode().to_vec()),
        );
        e.subscribe(one(TAP_OUTPUT, 0));
        let t0 = Instant::now();
        let mut most = 0u8;
        for i in 0..200 {
            let r = e.tick(
                &mut mock,
                t0 + Duration::from_millis(50 * i),
                Budget::default(),
            );
            most = most.max(r.transfers);
        }
        let timing = e.timing();
        eprintln!(
            "mock: {} busy ticks, {} transfers, mean {:?}, max {:?}",
            timing.ticks,
            timing.transfers,
            timing.mean(),
            timing.max
        );
        // A push, a band read, status and config, and one bin chunk at most.
        assert!(most <= 5, "{most}");
        assert!(timing.mean() < Duration::from_millis(5), "{timing:?}");
    }

    #[test]
    fn the_budget_holds_the_bins_back_once_the_tick_is_spent() {
        let (mut e, mut mock, log) = device(
            MockTransport::new()
                .window(op::REQ_RTA_GET_BINS, bin_frame(5, 223).encode())
                .data(op::REQ_RTA_GET_BANDS, frame(0, 200, 10).encode().to_vec())
                .answering_everything(RtaStatus::default().encode().to_vec()),
        );
        e.subscribe(one(TAP_OUTPUT, 0));
        let spent = Budget {
            max_bin_chunks: 1,
            max_time: Duration::ZERO,
        };
        e.tick(&mut mock, Instant::now(), spent);
        assert_eq!(
            sent(&log, op::REQ_RTA_GET_BANDS).len(),
            1,
            "bands always go"
        );
        assert!(sent(&log, op::REQ_RTA_GET_BINS).is_empty());
    }

    #[test]
    fn a_disconnect_is_reported() {
        let (mut e, mut mock, _log) =
            device(MockTransport::new().reply(op::REQ_RTA_GET_BANDS, Reply::Disconnect));
        e.subscribe(one(TAP_OUTPUT, 0));
        let r = e.tick(&mut mock, Instant::now(), Budget::default());
        assert!(r.disconnected);
    }
}
