//! `dspi` - terminal control for DSPi audio processors.
//!
//! M1 scope: connect, discover, and dump state. The TUI and the one-shot command
//! grammar arrive in later milestones; this binary exists now so that every
//! layer below it is exercised against real hardware rather than only the mock.

mod doctor;

use std::process::ExitCode;

use dspi_cmd::{Command, Context};
use dspi_proto::registry::{Kind, REGISTRY, by_path};
use dspi_proto::value::Value;

use dspi_session::{Outcome, Session, probe};
use dspi_transport::{Transport, TransportError, UsbTransport, list_devices};

/// Exit codes are part of the interface: scripts branch on these.
mod exit {
    pub const OK: u8 = 0;
    pub const USAGE: u8 = 1;
    pub const NO_DEVICE: u8 = 2;
    /// The device accepted the write and then kept a different value.
    pub const REJECTED: u8 = 3;
    pub const TRANSPORT: u8 = 4;
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags: Vec<&str> = args.iter().map(String::as_str).collect();

    let json = flags.contains(&"--json");
    let serial = flag_value(&flags, "--device");

    let code = match flags.first().copied() {
        Some("--help" | "-h") => {
            usage();
            exit::OK
        }
        Some("--version" | "-V") => {
            version();
            exit::OK
        }
        Some("list") => cmd_list(json),
        Some("params") => cmd_params(),
        Some("completions") => cmd_completions(&positional(&flags)),
        Some("doctor") | Some("--doctor") => doctor::run(),
        Some("raw") => cmd_raw(serial, &positional(&flags)),
        Some("autoeq") => cmd_autoeq(serial, &positional(&flags), &flags),
        Some("--install-udev") => doctor::install_udev(),
        Some("screenshot") => cmd_screenshot(serial, &positional(&flags)),
        Some("export") => cmd_export(serial, &positional(&flags)),
        Some("import") => cmd_import(serial, &positional(&flags), &flags),
        Some("dump") => cmd_dump(serial, json),
        // No arguments opens the interface; arguments run one command and exit.
        None => cmd_tui(serial, flags.contains(&"--lite")),
        Some(other) if other.starts_with("--") => cmd_tui(serial, flags.contains(&"--lite")),
        // Everything else is a command in the shared grammar, so the CLI and the
        // TUI's ':' line accept exactly the same syntax.
        Some(_) => cmd_run(serial, &flags, json),
    };

    ExitCode::from(code)
}

fn flag_value<'a>(flags: &'a [&'a str], name: &str) -> Option<&'a str> {
    flags
        .iter()
        .position(|f| *f == name)
        .and_then(|i| flags.get(i + 1))
        .copied()
}

fn version() {
    println!(
        "dspi {} (protocol from DSPi {} @ {})",
        env!("CARGO_PKG_VERSION"),
        dspi_proto::generated::provenance::BRANCH,
        dspi_proto::generated::provenance::SHORT,
    );
    println!(
        "wire format V{}, {} vendor opcodes",
        dspi_proto::generated::wire::WIRE_FORMAT_VERSION,
        dspi_proto::generated::ALL_OPCODES.len(),
    );
}

fn usage() {
    println!(
        "\
dspi - terminal control for DSPi audio processors

USAGE:
    dspi                     open the interface
    dspi dump                connect and print the full device state
    dspi list                list every connected DSPi
    dspi params              list every parameter this build knows
    dspi get <path> [i..]    read one parameter
    dspi set <path> [i..] v  write one parameter, and confirm it
    dspi <path> [i..] v      the same, without the `set`
    dspi eq <ch> <band> <type> [freq] [q] [gain]
                             set a whole filter band in one transfer
    dspi export <file>       write all filters to a Console-compatible file
    dspi import <file>       apply a filter file or .dspipreset
                             (--volume, --hardware to include those blocks)
                             (--map-legacy for pre-2026 channel names)
    dspi completions <shell> generate shell completions
    dspi doctor              diagnose connection problems
    dspi raw <op> <len> [v]  issue any vendor opcode and hex-dump the reply
    dspi autoeq search <q>   find a headphone correction profile
    dspi autoeq apply <id>   apply one to the input channels
    dspi --install-udev      install the Linux udev rule (needs root)
    dspi --version           show app and protocol versions

EXAMPLES:
    dspi get vol.user
    dspi set vol.user -18
    dspi get eq.freq 8 3
    dspi set bass.drive 12
    dspi eq usb.1 3 peak 2856 3.58 -8.6
    dspi ch.delay i2s.1.l 5

OPTIONS:
    --device <serial>        target a specific device
    --json                   machine-readable output
    --lite                   reduce redraw rate, for a Pi or a slow link
"
    );
}

fn cmd_list(json: bool) -> u8 {
    let devices = match list_devices() {
        Ok(d) => d,
        Err(e) => return fail(e),
    };

    if devices.is_empty() {
        if json {
            println!("[]");
        } else {
            eprintln!("No DSPi found.");
        }
        return exit::NO_DEVICE;
    }

    if json {
        let arr: Vec<_> = devices
            .iter()
            .map(|d| {
                serde_json::json!({
                    "serial": d.serial,
                    "short": d.short_name(),
                    "bus": d.bus_id,
                    "address": d.address,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr).unwrap());
    } else {
        for d in &devices {
            println!("{}  bus {} addr {}", d.short_name(), d.bus_id, d.address);
        }
    }
    exit::OK
}

fn cmd_dump(serial: Option<&str>, json: bool) -> u8 {
    let mut t = match open(serial) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };

    let caps = match probe(&mut t) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };

    if json {
        match serde_json::to_string_pretty(&caps) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("dspi: could not serialise state: {e}");
                return exit::TRANSPORT;
            }
        }
        return exit::OK;
    }

    println!(
        "{}  {}  firmware {}  wire V{}",
        caps.serial,
        caps.platform.name(),
        caps.firmware,
        caps.wire_format
    );
    println!(
        "{} channels: {} in, {} out, {} bands each",
        caps.num_channels, caps.num_inputs, caps.num_outputs, caps.max_bands
    );
    if let Some(p) = caps.active_preset {
        println!("active preset: {p}");
    }

    println!("\nchannels");
    for c in &caps.channels {
        println!(
            "  {:>2}  {:<12}  {:<16}  {}",
            c.index,
            c.slug,
            c.name,
            if c.is_output { "output" } else { "input" }
        );
    }

    println!("\nfeatures");
    for f in &caps.features {
        println!(
            "  {}  {:<22}  {}",
            if f.present { "yes" } else { " no" },
            f.name,
            f.evidence
        );
    }

    if let Some(cs) = &caps.cs {
        println!(
            "\ncontrol surfaces: caps v{}, {} slots, {} nouns, {} IR commands",
            cs.caps_version, cs.max_bindings, cs.noun_count, cs.max_ir_commands
        );
    }
    if let Some(sg) = &caps.siggen {
        println!(
            "test signals: {} types, {} outputs, multitone max {}",
            sg.type_count, sg.output_channels, sg.multitone_max
        );
    }

    exit::OK
}

/// Render one frame of the interface to stdout.
///
/// Useful for documentation and for checking a layout at a size you do not have
/// a terminal for, such as the 80x24 floor.
/// Write every channel's filters to a Console-compatible file.
fn cmd_export(serial: Option<&str>, args: &[&str]) -> u8 {
    let Some(path) = args.first() else {
        eprintln!("dspi: export needs a file name, e.g. `dspi export tuning.txt`");
        return exit::USAGE;
    };

    // The extension picks the format: a filter bank, or a whole-device document.
    if path.ends_with(dspi_session::preset_file::FILE_EXTENSION) {
        return export_preset(serial, path);
    }

    let mut session = match connect(serial) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let caps = session.capabilities().clone();

    let mut file = dspi_session::filterfile::FilterFile {
        format_version: dspi_session::filterfile::FORMAT_VERSION,
        channels: Vec::new(),
    };

    for c in &caps.channels {
        let mut bank = dspi_session::filterfile::ChannelBank {
            header: format!(
                "{} {}: {}",
                if c.is_output { "Output" } else { "Input" },
                if c.is_output {
                    c.index - caps.num_inputs
                } else {
                    c.index
                },
                c.name
            ),
            index: Some(if c.is_output {
                c.index - caps.num_inputs
            } else {
                c.index
            }),
            is_output: c.is_output,
            ..Default::default()
        };

        if !c.is_output {
            bank.preamp_db = session
                .read("pre", &[c.index])
                .ok()
                .and_then(|v| v.as_f32());
        }

        for b in 0..caps.max_bands {
            if let Ok(p) = session.read_band(c.index, b) {
                bank.peq.push(dspi_proto::dsp::Band {
                    filter_type: p.filter_type,
                    freq: p.freq,
                    q: p.q,
                    gain_db: p.gain_db,
                    bypass: p.bypass,
                });
            }
        }
        // Crossover bands live at wire indices 20-23 and only on outputs.
        if c.is_output {
            for b in 20..24 {
                if let Ok(p) = session.read_band(c.index, b) {
                    bank.crossover.push(dspi_proto::dsp::Band {
                        filter_type: p.filter_type,
                        freq: p.freq,
                        q: p.q,
                        gain_db: p.gain_db,
                        bypass: p.bypass,
                    });
                }
            }
        }
        file.channels.push(bank);
    }

    let stamp = std::process::Command::new("date")
        .arg("+%Y-%m-%d %H:%M:%S")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let text = dspi_session::filterfile::write(&file, &stamp);
    match std::fs::write(path, &text) {
        Ok(()) => {
            println!(
                "Wrote {path}: {} channels, {} bands",
                file.channels.len(),
                file.channels.iter().map(|c| c.peq.len()).sum::<usize>()
            );
            exit::OK
        }
        Err(e) => {
            eprintln!("dspi: could not write {path}: {e}");
            exit::TRANSPORT
        }
    }
}

/// Capture everything the device is doing into a `.dspipreset`.
fn export_preset(serial: Option<&str>, path: &str) -> u8 {
    let mut session = match connect(serial) {
        Ok(s) => s,
        Err(c) => return c,
    };

    let name = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    let doc = dspi_session::preset_file::capture(&mut session, name);

    match std::fs::write(path, dspi_session::preset_file::write(&doc)) {
        Ok(()) => {
            println!(
                "Wrote {path}: {} channels, {} crosspoints",
                doc.channels.len(),
                doc.matrix.len()
            );
            exit::OK
        }
        Err(e) => {
            eprintln!("dspi: could not write {path}: {e}");
            exit::TRANSPORT
        }
    }
}

/// Read a filter file and apply it.
///
/// Applying goes band by band through the ordinary write path, so every value
/// gets the same clamping, gating and readback as a typed command. Anything the
/// file asks for that this device cannot do is reported rather than skipped
/// quietly.
fn cmd_import(serial: Option<&str>, args: &[&str], flags: &[&str]) -> u8 {
    let Some(path) = args.first() else {
        eprintln!("dspi: import needs a file name");
        return exit::USAGE;
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("dspi: could not read {path}: {e}");
            return exit::USAGE;
        }
    };
    // A .dspipreset is a whole-device document rather than a filter bank, so it
    // takes a different path; the extension picks, and a mis-typed name gets a
    // clear answer rather than a confusing parse error.
    if path.ends_with(dspi_session::preset_file::FILE_EXTENSION) {
        return import_preset(&text, path, flags);
    }

    let file = match dspi_session::filterfile::parse(&text) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("dspi: {e}");
            return exit::USAGE;
        }
    };

    let mut session = match connect(serial) {
        Ok(s) => s,
        Err(c) => return c,
    };
    if flags.contains(&"--dry-run") {
        session.dry_run = true;
    }
    let caps = session.capabilities().clone();

    let map_legacy = flags.contains(&"--map-legacy");
    let mut applied = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    let mut mapped: Vec<String> = Vec::new();

    for bank in &file.channels {
        // A file written for a different device shape will name channels this
        // one does not have; say so rather than silently dropping them.
        let Some(channel) = resolve_channel(bank, &caps, map_legacy) else {
            skipped.push(bank.header.clone());
            continue;
        };

        if bank.index.is_none()
            && !caps
                .channels
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case(bank.header.trim()))
        {
            mapped.push(format!(
                "{} → {}",
                bank.header,
                caps.channels
                    .get(channel as usize)
                    .map(|c| c.name.as_str())
                    .unwrap_or("?")
            ));
        }

        for (i, b) in bank.peq.iter().enumerate() {
            if i as u8 >= caps.max_bands {
                break;
            }
            let packet = dspi_proto::value::EqParamPacket {
                channel,
                band: i as u8,
                filter_type: b.filter_type,
                bypass: b.bypass,
                freq: b.freq,
                q: b.q,
                gain_db: b.gain_db,
                qp: None,
            };
            if session.write_band(&packet).is_ok() {
                applied += 1;
            }
        }
    }

    println!(
        "{} {applied} bands from {path}",
        if session.dry_run {
            "Would apply"
        } else {
            "Applied"
        }
    );
    if !mapped.is_empty() {
        println!("Mapped older channel names: {}", mapped.join(", "));
    }
    if !skipped.is_empty() {
        println!("Skipped, no matching channel: {}", skipped.join(", "));
        if !map_legacy
            && skipped
                .iter()
                .any(|s| legacy_channel(&s.to_ascii_lowercase(), &caps).is_some())
        {
            println!(
                "  These look like pre-2026 channel names. Re-run with --map-legacy \
                 to place them on the current channels."
            );
        }
    }
    exit::OK
}

/// Inspect a whole-device document and report what it would do.
///
/// Applying every block is still to come; reading, validating and reporting is
/// what makes a file trustworthy before anyone acts on it, so that lands first.
fn import_preset(text: &str, path: &str, flags: &[&str]) -> u8 {
    use dspi_session::preset_file;

    let doc = match preset_file::parse(text) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("dspi: {e}");
            return exit::USAGE;
        }
    };

    println!(
        "{path}: {}{}, {} channels",
        doc.meta.name.as_deref().unwrap_or("unnamed"),
        doc.meta
            .platform
            .as_deref()
            .map(|p| format!(" from {p}"))
            .unwrap_or_default(),
        doc.channels.len()
    );

    let features: Vec<&str> = [
        doc.psybass.is_some().then_some("psychoacoustic bass"),
        doc.upmix.is_some().then_some("upmixer"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !features.is_empty() {
        println!("Carries: {}", features.join(", "));
    }

    let mut session = match connect(flag_value(flags, "--device")) {
        Ok(s) => s,
        Err(c) => return c,
    };
    if flags.contains(&"--dry-run") {
        session.dry_run = true;
    }

    // Volume and wiring are opt-in: they describe a room and a board, and
    // changing either without being asked is the kind of surprise that loses
    // trust in a tool.
    let options = preset_file::ApplyOptions {
        audio_processing: true,
        volume_levels: flags.contains(&"--volume"),
        hardware_io: flags.contains(&"--hardware"),
    };

    let report = preset_file::apply(&mut session, &doc, options);

    println!(
        "\n{} {} channels, {} bands, {} crossover bands, {} crosspoints",
        if session.dry_run {
            "Would apply"
        } else {
            "Applied"
        },
        report.channels_applied,
        report.bands_applied,
        report.crossover_bands_applied,
        report.crosspoints_applied
    );
    if !report.skipped.is_empty() {
        println!("Skipped: {}", report.skipped.join("; "));
    }
    exit::OK
}

/// Match a file's channel section to a channel on this device.
///
/// Index-keyed headers are authoritative. A name-keyed header is matched
/// against the device's own channel names.
///
/// Files written before the firmware's unified channel model use names like
/// "Master L" and "Sub" for a topology that no longer exists: there is no master
/// bus now, and inputs are first-class channels. Mapping those onto the current
/// model is a guess, right for a stereo setup and wrong for an eight-channel
/// one, so it happens only when the user asks for it with `--map-legacy`.
fn resolve_channel(
    bank: &dspi_session::filterfile::ChannelBank,
    caps: &dspi_session::Capabilities,
    map_legacy: bool,
) -> Option<u8> {
    if let Some(ix) = bank.index {
        let ch = if bank.is_output {
            caps.num_inputs.checked_add(ix)?
        } else {
            ix
        };
        return (ch < caps.num_channels).then_some(ch);
    }

    let want = bank.header.trim().to_ascii_lowercase();
    if let Some(c) = caps
        .channels
        .iter()
        .find(|c| c.name.to_ascii_lowercase() == want)
    {
        return Some(c.index);
    }

    if !map_legacy {
        return None;
    }
    legacy_channel(&want, caps)
}

/// The pre-V16 names, mapped onto the current model on request.
///
/// The old master pair becomes the first two inputs, and the old outputs become
/// the first outputs in order. Stated explicitly on import so the user can see
/// what was assumed.
fn legacy_channel(name: &str, caps: &dspi_session::Capabilities) -> Option<u8> {
    let out = |n: u8| {
        caps.num_inputs
            .checked_add(n)
            .filter(|c| *c < caps.num_channels)
    };
    match name {
        "master l" | "master left" => (caps.num_inputs > 0).then_some(0),
        "master r" | "master right" => (caps.num_inputs > 1).then_some(1),
        "out l" | "out left" => out(0),
        "out r" | "out right" => out(1),
        // The subwoofer was always the last output.
        "sub" | "pdm" | "pdm sub" => caps.num_channels.checked_sub(1),
        _ => None,
    }
}

fn cmd_screenshot(serial: Option<&str>, args: &[&str]) -> u8 {
    let width: u16 = args.first().and_then(|a| a.parse().ok()).unwrap_or(120);
    let height: u16 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(40);
    let panel = args.get(2).copied().unwrap_or("dashboard");

    let session = match connect(serial) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let theme = dspi_tui::Theme::dark(
        dspi_tui::theme::ColorDepth::TrueColor,
        dspi_tui::app::detect_glyphs(),
    );
    let mut app = dspi_tui::App::from_session(theme, &session);
    let mut session = session;
    load_bands(&mut session, &mut app);
    if let Ok(m) = session.meters() {
        app.apply_meters(&m);
    }
    app.rebuild_fields();
    app.load_fields(&mut session);
    app.load_matrix(&mut session);
    app.load_surfaces(&mut session);

    app.panel = match panel {
        "cursor" => {
            // Park the cursor on 1 kHz so the readout is exercised.
            app.cursor = Some(dspi_proto::dsp::POINTS / 2 + 20);
            dspi_tui::app::Panel::Filters
        }
        "grid" => {
            app.grid_mode = true;
            dspi_tui::app::Panel::Dashboard
        }
        "surfaces" => dspi_tui::app::Panel::Surfaces,
        "matrix" => dspi_tui::app::Panel::Matrix,
        "input" => dspi_tui::app::Panel::Input,
        "dynamics" => dspi_tui::app::Panel::Dynamics,
        "spatial" => dspi_tui::app::Panel::Spatial,
        "system" => dspi_tui::app::Panel::System,
        "presets" => dspi_tui::app::Panel::Presets,
        "filters" => dspi_tui::app::Panel::Filters,
        "meters" => {
            app.meters_expanded = true;
            dspi_tui::app::Panel::Dashboard
        }
        _ => dspi_tui::app::Panel::Dashboard,
    };

    // The panel changed after the initial load, so refresh for the new one.
    app.rebuild_fields();
    app.load_fields(&mut session);

    println!("{}", dspi_tui::render_to_string(&app, width, height));
    exit::OK
}

fn cmd_tui(serial: Option<&str>, lite: bool) -> u8 {
    let session = match connect(serial) {
        Ok(s) => s,
        Err(c) => return c,
    };

    let theme = dspi_tui::Theme::dark(
        dspi_tui::theme::ColorDepth::detect(),
        dspi_tui::app::detect_glyphs(),
    );
    let mut app = dspi_tui::App::from_session(theme, &session);
    if lite {
        app.perf = dspi_tui::app::Performance::lite();
    }

    // Seed the curves from what the device is actually doing, so the first frame
    // shows the user's tuning rather than a flat line.
    let mut session = session;
    load_bands(&mut session, &mut app);

    match dspi_tui::run(app, &mut session) {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("dspi: {e}");
            exit::TRANSPORT
        }
    }
}

/// Read every band on every channel.
///
/// There is no full-packet EQ read in the protocol, so this is five transfers
/// per band. It is the slowest thing the app does at startup, which is why it
/// happens once here rather than lazily per panel.
fn load_bands(session: &mut Session, app: &mut dspi_tui::App) {
    let channels = app.channels.len();
    for ch in 0..channels {
        let bands = app.channels[ch].bands.len();
        for band in 0..bands {
            if let Ok(p) = session.read_band(ch as u8, band as u8) {
                app.channels[ch].bands[band] = dspi_proto::dsp::Band {
                    filter_type: p.filter_type,
                    freq: p.freq,
                    q: p.q,
                    gain_db: p.gain_db,
                    bypass: p.bypass,
                };
            }
        }
        app.recompute(ch);
    }
}

/// Search the AutoEQ database, or apply a profile.
fn cmd_autoeq(serial: Option<&str>, args: &[&str], flags: &[&str]) -> u8 {
    use dspi_session::autoeq;

    let db = match autoeq::load() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("dspi: {e}");
            return exit::USAGE;
        }
    };

    match args.first().copied() {
        Some("search") => {
            let query = args[1..].join(" ");
            if query.is_empty() {
                eprintln!("dspi: autoeq search needs something to look for");
                return exit::USAGE;
            }
            let hits = db.search(&query);
            if hits.is_empty() {
                println!("Nothing matching \"{query}\".");
                return exit::OK;
            }
            for e in hits.iter().take(20) {
                println!(
                    "{:<44}{} bands, preamp {:+.1} dB",
                    e.id,
                    e.filters.len(),
                    e.preamp
                );
            }
            if hits.len() > 20 {
                println!("... and {} more; narrow the search.", hits.len() - 20);
            }
            exit::OK
        }

        Some("apply") => {
            let Some(id) = args.get(1) else {
                eprintln!("dspi: autoeq apply needs a profile id from `autoeq search`");
                return exit::USAGE;
            };
            let Some(entry) = db.get(id) else {
                eprintln!("dspi: no profile called `{id}`");
                return exit::USAGE;
            };

            let mut session = match connect(serial) {
                Ok(s) => s,
                Err(c) => return c,
            };
            if flags.contains(&"--dry-run") {
                session.dry_run = true;
            }

            // Default to every input: a headphone correction belongs on what is
            // being listened to, and applying it to one side only would be worse
            // than not applying it.
            let caps = session.capabilities().clone();
            let channels: Vec<u8> = match args.get(2) {
                Some(name) => {
                    match caps
                        .channels
                        .iter()
                        .find(|c| c.slug == *name || c.name == *name)
                    {
                        Some(c) => vec![c.index],
                        None => {
                            eprintln!("dspi: no channel called `{name}`");
                            return exit::USAGE;
                        }
                    }
                }
                None => (0..caps.num_inputs).collect(),
            };

            let mut applied = 0usize;
            let mut unsupported: Vec<&str> = Vec::new();

            for ch in &channels {
                let _ = session.write("pre", &[*ch], dspi_proto::value::Value::Float(entry.preamp));

                for (i, f) in entry.filters.iter().enumerate() {
                    if i as u8 >= caps.max_bands {
                        break;
                    }
                    let Some(kind) = autoeq::filter_type(&f.kind) else {
                        if !unsupported.contains(&f.kind.as_str()) {
                            unsupported.push(&f.kind);
                        }
                        continue;
                    };
                    let packet = dspi_proto::value::EqParamPacket {
                        channel: *ch,
                        band: i as u8,
                        filter_type: kind,
                        bypass: false,
                        freq: f.freq,
                        q: f.q,
                        gain_db: f.gain,
                        qp: None,
                    };
                    if session.write_band(&packet).is_ok() {
                        applied += 1;
                    }
                }

                // Bands the profile does not use must be cleared, or whatever
                // was there before survives underneath the correction.
                for i in entry.filters.len()..caps.max_bands as usize {
                    let packet = dspi_proto::value::EqParamPacket {
                        channel: *ch,
                        band: i as u8,
                        filter_type: dspi_proto::FilterType::Flat,
                        bypass: false,
                        freq: 1000.0,
                        q: 0.707,
                        gain_db: 0.0,
                        qp: None,
                    };
                    let _ = session.write_band(&packet);
                }
            }

            println!(
                "{} {} to {} channel(s): {applied} bands, preamp {:+.1} dB",
                if session.dry_run {
                    "Would apply"
                } else {
                    "Applied"
                },
                entry.id,
                channels.len(),
                entry.preamp
            );
            if entry.filters.len() > caps.max_bands as usize {
                println!(
                    "Note: the profile has {} bands, this device has {}; the rest were dropped.",
                    entry.filters.len(),
                    caps.max_bands
                );
            }
            if !unsupported.is_empty() {
                println!(
                    "Shapes this build does not know: {}",
                    unsupported.join(", ")
                );
            }
            exit::OK
        }

        _ => {
            println!(
                "AutoEQ database: {} profiles, {} copy{}",
                db.entries.len(),
                match db.origin {
                    autoeq::Origin::User => "your",
                    autoeq::Origin::Bundled => "the built-in",
                },
                if db.generated_at.is_empty() {
                    String::new()
                } else {
                    format!(", generated {}", db.generated_at)
                }
            );
            println!("\n  dspi autoeq search <words>");
            println!("  dspi autoeq apply <id> [channel]");
            exit::OK
        }
    }
}

/// Issue any vendor opcode directly.
///
/// This is the escape hatch that means a firmware feature shipping before app
/// support is still reachable on the day it lands, and it is the fastest way to
/// see what a device actually returns when a decode looks wrong.
fn cmd_raw(serial: Option<&str>, args: &[&str]) -> u8 {
    let parse_num = |s: &str| -> Option<u32> {
        s.strip_prefix("0x")
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .or_else(|| s.parse().ok())
    };

    let (Some(op_s), Some(len_s)) = (args.first(), args.get(1)) else {
        eprintln!("dspi: raw needs an opcode and a length, e.g. `dspi raw 0x87 32`");
        eprintln!("      an optional third argument is wValue.");
        return exit::USAGE;
    };
    let (Some(opcode), Some(len)) = (parse_num(op_s), parse_num(len_s)) else {
        eprintln!("dspi: opcode and length must be numbers");
        return exit::USAGE;
    };
    let value = args.get(2).and_then(|v| parse_num(v)).unwrap_or(0) as u16;

    let mut t = match open(serial) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };

    match t.control_in(opcode as u8, value, len as u16) {
        Ok(d) => {
            for (i, chunk) in d.chunks(16).enumerate() {
                let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
                let ascii: String = chunk
                    .iter()
                    .map(|b| {
                        if b.is_ascii_graphic() {
                            *b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                println!("{:04X}  {:<48}  {ascii}", i * 16, hex.join(" "));
            }
            println!("\n{} bytes", d.len());
            exit::OK
        }
        Err(e) => {
            eprintln!("dspi: {e}");
            exit::TRANSPORT
        }
    }
}

fn cmd_completions(args: &[&str]) -> u8 {
    let Some(shell) = args.first() else {
        eprintln!(
            "dspi: which shell? one of: {}",
            dspi_cmd::shell::SHELLS.join(", ")
        );
        return exit::USAGE;
    };
    match dspi_cmd::shell::generate(shell) {
        Some(script) => {
            print!("{script}");
            exit::OK
        }
        None => {
            eprintln!(
                "dspi: no completions for `{shell}`; try one of: {}",
                dspi_cmd::shell::SHELLS.join(", ")
            );
            exit::USAGE
        }
    }
}

/// Parse with the shared grammar, then run it.
///
/// Parsing needs the device's channel names, so this connects first. That costs
/// a little latency on a bad command, but it means `dspi ch.delay i2s.1.l 5`
/// works with the names the device actually reports.
fn cmd_run(serial: Option<&str>, flags: &[&str], json: bool) -> u8 {
    let args = positional_all(flags);
    let mut s = match connect(serial) {
        Ok(s) => s,
        Err(c) => return c,
    };
    if flags.contains(&"--dry-run") {
        s.dry_run = true;
    }

    let ctx = context_for(&s);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let cmd = match dspi_cmd::parse(&refs, &ctx) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("dspi: {e}");
            return exit::USAGE;
        }
    };

    run(&mut s, cmd, json, flags.contains(&"--quiet"))
}

fn context_for(s: &Session) -> Context {
    let caps = s.capabilities();
    Context {
        channel_slugs: caps.channels.iter().map(|c| c.slug.clone()).collect(),
        num_inputs: caps.num_inputs,
        num_outputs: caps.num_outputs,
        max_bands: caps.max_bands,
    }
}

fn run(s: &mut Session, cmd: Command, json: bool, quiet: bool) -> u8 {
    match cmd {
        Command::Get { path, indices } => {
            let d = by_path(path).expect("the parser only returns known paths");
            match s.read(path, &indices) {
                Ok(v) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::json!({ "path": path, "value": show(d.kind, &v) })
                        );
                    } else {
                        println!("{}", show(d.kind, &v));
                    }
                    exit::OK
                }
                Err(e) => {
                    eprintln!("dspi: {e}");
                    write_exit(&e)
                }
            }
        }

        Command::Set {
            path,
            indices,
            value,
        } => {
            let d = by_path(path).expect("the parser only returns known paths");
            match s.write(path, &indices, value.clone()) {
                Ok(Outcome::Rejected { sent, actual }) => {
                    eprintln!(
                        "dspi: {path} was not applied. Asked for {}, device kept {}.",
                        show(d.kind, &sent),
                        show(d.kind, &actual)
                    );
                    exit::REJECTED
                }
                Ok(outcome) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::json!({
                                "path": path,
                                "value": show(d.kind, &value),
                                "outcome": format!("{outcome:?}"),
                            })
                        );
                    } else if !quiet {
                        println!("{path} = {}", show(d.kind, &value));
                    }
                    exit::OK
                }
                Err(e) => {
                    eprintln!("dspi: {e}");
                    write_exit(&e)
                }
            }
        }

        Command::SetBand {
            channel,
            band,
            filter_type,
            freq,
            q,
            gain,
        } => {
            // One transfer for the whole band, which is how the firmware stores
            // it; setting fields one at a time would be six round trips.
            let packet = dspi_proto::value::EqParamPacket {
                channel,
                band,
                filter_type: dspi_proto::FilterType::from_raw(filter_type),
                bypass: false,
                freq,
                q,
                gain_db: gain,
                qp: None,
            };
            match s.write_band(&packet) {
                Ok(Outcome::Rejected { .. }) => {
                    eprintln!("dspi: the band was not applied as sent");
                    exit::REJECTED
                }
                Ok(_) => {
                    if !quiet {
                        println!(
                            "band {} on channel {channel} = {} {freq} Hz Q {q} {gain:+} dB",
                            band + 1,
                            dspi_proto::FilterType::from_raw(filter_type).label()
                        );
                    }
                    exit::OK
                }
                Err(e) => {
                    eprintln!("dspi: {e}");
                    write_exit(&e)
                }
            }
        }

        Command::Verb { name, .. } => {
            eprintln!("dspi: `{name}` is not usable here");
            exit::USAGE
        }
    }
}

fn cmd_params() -> u8 {
    for d in REGISTRY {
        let arity = d.target.arity();
        let args = match arity {
            0 => String::new(),
            1 => " <index>".into(),
            _ => " <index> <index>".into(),
        };
        let range = match d.kind {
            Kind::Float { unit, min, max } => {
                format!("{min} to {max}{}", unit.suffix())
            }
            Kind::Int { unit, min, max } => format!("{min} to {max}{}", unit.suffix()),
            Kind::Bool => "on | off".into(),
            Kind::Choice(v) => v.iter().map(|(_, n)| *n).collect::<Vec<_>>().join(" | "),
            Kind::Trigger => "(action)".into(),
            _ => String::new(),
        };
        println!(
            "{:<22}{:<18}{}",
            format!("{}{args}", d.path),
            range,
            d.plain
        );
    }
    exit::OK
}

/// Every positional token, including the first, for the shared grammar.
fn positional_all(flags: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for f in flags {
        if skip {
            skip = false;
            continue;
        }
        if *f == "--device" {
            skip = true;
            continue;
        }
        if f.starts_with("--") {
            continue;
        }
        out.push((*f).to_string());
    }
    out
}

/// Positional arguments after the command, excluding flags and their values.
fn positional<'a>(flags: &'a [&'a str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut skip = false;
    for (i, f) in flags.iter().enumerate().skip(1) {
        if skip {
            skip = false;
            continue;
        }
        if *f == "--device" {
            skip = true;
            continue;
        }
        if f.starts_with("--") {
            continue;
        }
        let _ = i;
        out.push(*f);
    }
    out
}

fn connect(serial: Option<&str>) -> Result<Session, u8> {
    let mut t = open(serial).map_err(fail)?;
    let caps = probe(&mut t).map_err(fail)?;
    Session::new(Box::new(t), caps).ok_or_else(|| {
        eprintln!("dspi: the device reported an impossible channel map");
        exit::TRANSPORT
    })
}

/// Distinguish "you typed something wrong" from "the device is unhappy".
///
/// A script needs to tell these apart: bad input is worth reporting to a human,
/// a transport failure is worth retrying.
fn write_exit(e: &dspi_session::WriteError) -> u8 {
    use dspi_session::WriteError as W;
    match e {
        W::Value(_)
        | W::UnknownParam(_)
        | W::ReadOnly { .. }
        | W::WrongArity { .. }
        | W::BadTarget(..)
        | W::Unavailable { .. } => exit::USAGE,
        W::Transport(_) => exit::TRANSPORT,
    }
}

/// Render a value using its parameter's kind, so a choice shows its name rather
/// than the wire number behind it.
fn show(kind: Kind, v: &Value) -> String {
    if let (Kind::Choice(variants), Some(n)) = (kind, v.as_u8())
        && let Some((_, name)) = variants.iter().find(|(raw, _)| *raw == n)
    {
        return (*name).to_string();
    }
    v.display(kind.unit())
}

fn open(serial: Option<&str>) -> dspi_transport::Result<UsbTransport> {
    match serial {
        Some(s) => UsbTransport::open_serial(s),
        None => UsbTransport::open_only(),
    }
}

fn fail(e: TransportError) -> u8 {
    eprintln!("dspi: {e}");
    match e {
        TransportError::NotFound | TransportError::SerialNotFound(_) => exit::NO_DEVICE,
        _ => exit::TRANSPORT,
    }
}
