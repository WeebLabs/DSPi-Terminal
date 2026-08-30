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
    let palette = match flag_value(&flags, "--theme") {
        Some(name) => match dspi_tui::theme::Palette::parse(name) {
            Some(p) => p,
            None => {
                eprintln!(
                    "dspi: no theme called `{name}`; try one of: {}",
                    dspi_tui::theme::Palette::NAMES.join(", ")
                );
                return ExitCode::from(exit::USAGE);
            }
        },
        None => dspi_tui::theme::Palette::Console,
    };

    // A script beats every other reading of the arguments: `-f` is explicit,
    // and a non-terminal standard input with nothing else to do can only mean
    // someone is piping commands in.
    let script = flag_value(&flags, "-f").or_else(|| flag_value(&flags, "--file"));
    if script.is_some() {
        return ExitCode::from(cmd_script(serial, script, &flags, json));
    }
    if wants_stdin_script(&flags, std::io::IsTerminal::is_terminal(&std::io::stdin())) {
        return ExitCode::from(cmd_script(serial, None, &flags, json));
    }

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
        Some("raw") => cmd_raw(serial, &positional(&flags), json),
        Some("autoeq") => cmd_autoeq(serial, &positional(&flags), &flags),
        Some("--install-udev") => doctor::install_udev(),
        Some("screenshot") => cmd_screenshot(serial, &positional(&flags), &flags),
        Some("export") => cmd_export(serial, &positional(&flags)),
        Some("import") => cmd_import(serial, &positional(&flags), &flags),
        Some("dump") => cmd_dump(serial, json),
        Some("watch") => cmd_watch(serial, json),
        // No arguments opens the interface; arguments run one command and exit.
        None => cmd_tui(
            serial,
            flags.contains(&"--lite"),
            flags.contains(&"--no-animation"),
            palette,
        ),
        Some(other) if other.starts_with("--") => cmd_tui(
            serial,
            flags.contains(&"--lite"),
            flags.contains(&"--no-animation"),
            palette,
        ),
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
    dspi screenshot [w] [h] [screen]
                             render one frame of the interface as text
                             (--ansi for colour; --fixture, or no device,
                             draws the built-in example device)
    dspi watch               print the device's notifications as they arrive
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
    dspi undo                put back the value the last change replaced
    dspi redo                re-apply the change undo reversed
    dspi raw <op> <len> [v]  read any vendor opcode and hex-dump the reply
    dspi raw out <op> <v> <hex...>
                             write any vendor opcode with a payload
    dspi raw war <op> <v>    issue a write-as-read and print its status byte
    dspi autoeq search <q>   find a headphone correction profile
    dspi autoeq apply <id>   apply one to the input channels
    dspi -f <script>         run one command per line, `#` starts a comment
    dspi < script            the same, from standard input
    dspi --install-udev      install the Linux udev rule (needs root)
    dspi --version           show app and protocol versions

EXAMPLES:
    dspi get vol.user
    dspi set vol.user -18
    dspi get eq.freq 8 3
    dspi set bass.drive 12
    dspi eq usb.1 3 peak 2856 3.58 -8.6
    dspi ch.delay i2s.1.l 5
    dspi raw out 0x42 0 08 03 01 00
    printf 'vol.user -18\\nbass.on on\\n' | dspi

OPTIONS:
    --device <serial>        target a specific device
    --json                   machine-readable output
    --no-animation           no connect reveal or easing
    --lite                   reduce redraw rate, for a Pi or a slow link
    --theme <name>           console (default), amber, dark or mono
    --dry-run                report what would be written, write nothing
    --quiet                  do not echo each change
    --keep-going             in a script, carry on past a failing line
    --force                  agree to a change that needs a decision, such as
                             an output enable that turns another one off
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

    println!();
    for line in report.lines(session.dry_run) {
        println!("{line}");
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

fn cmd_screenshot(serial: Option<&str>, args: &[&str], flags: &[&str]) -> u8 {
    let width: u16 = args.first().and_then(|a| a.parse().ok()).unwrap_or(120);
    let height: u16 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(40);
    let screen = args.get(2).copied().unwrap_or("overview");
    let ansi = flags.contains(&"--ansi");

    // Without a device (or with --fixture) the frame comes from the same
    // fixture the gallery and the tests use, so documentation needs no
    // hardware.
    let fixture = flags.contains(&"--fixture") || open(serial).is_err();
    let (mut session, state) = if fixture {
        let state = if screen == "settings" {
            dspi_tui::settings::demo::state()
        } else {
            dspi_tui::shell::fixture::state()
        };
        let mock = dspi_transport::MockTransport::new().answering_everything(vec![0u8; 64]);
        let Some(session) = Session::new(Box::new(mock), state.caps.clone()) else {
            return exit::TRANSPORT;
        };
        (session, state)
    } else {
        let mut session = match connect(serial) {
            Ok(s) => s,
            Err(c) => return c,
        };
        let bulk = match session.snapshot() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("dspi: could not read the device state: {e}");
                return exit::TRANSPORT;
            }
        };
        let state = dspi_session::DeviceState::new(session.capabilities().clone(), bulk);
        (session, state)
    };
    let theme = dspi_tui::Theme::new(
        dspi_tui::theme::Palette::Console,
        if ansi {
            dspi_tui::theme::ColorDepth::detect()
        } else {
            dspi_tui::theme::ColorDepth::TrueColor
        },
        dspi_tui::perf::detect_glyphs(),
    );
    let mut live = dspi_tui::live::Live::new(
        state,
        theme,
        dspi_tui::perf::Performance::lite(),
        Box::new(dspi_tui::live::ConsoleScreens::new()),
    );
    live.refresh_presets(&mut session);
    live.tick(&mut session, None);
    if !live.show(&mut session, screen) {
        eprintln!("dspi: unknown screen {screen}; see `dspi screenshot --help`");
        return exit::USAGE;
    }
    live.tick(&mut session, None);
    let frame = if ansi {
        dspi_tui::render_frame_ansi(width, height, |area, buf| live.draw(area, buf))
    } else {
        dspi_tui::render_frame(width, height, |area, buf| live.draw(area, buf))
    };
    println!("{frame}");
    exit::OK
}

fn cmd_tui(
    serial: Option<&str>,
    lite: bool,
    no_animation: bool,
    palette: dspi_tui::theme::Palette,
) -> u8 {
    let mut session = match connect(serial) {
        Ok(s) => s,
        Err(c) => return c,
    };

    let theme = dspi_tui::Theme::new(
        palette,
        dspi_tui::theme::ColorDepth::detect(),
        dspi_tui::perf::detect_glyphs(),
    );
    let mut perf = if lite {
        dspi_tui::perf::Performance::lite()
    } else {
        dspi_tui::perf::Performance::detect()
    };
    if no_animation {
        perf.animate = false;
    }

    // One chunked read seeds everything the bulk packet covers, so the first
    // frame shows the person's tuning rather than a flat line.
    let bulk = match session.snapshot() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("dspi: could not read the device state: {e}");
            return exit::TRANSPORT;
        }
    };
    let state = dspi_session::DeviceState::new(session.capabilities().clone(), bulk);
    let live = dspi_tui::live::Live::new(
        state,
        theme,
        perf,
        Box::new(dspi_tui::live::ConsoleScreens::new()),
    );

    match dspi_tui::live::run(live, &mut session) {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("dspi: {e}");
            exit::TRANSPORT
        }
    }
}

/// Print notifications as the device pushes them, until interrupted.
///
/// This is the Console's Interrupt Monitor as a one-shot: turn the OS volume
/// slider and watch `user_volume` arrive with source `UAC1`. With `--json`,
/// one object per line.
fn cmd_watch(serial: Option<&str>, json: bool) -> u8 {
    let mut session = match connect(serial) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let source = match session.with_transport(|t| Ok(t.notifications())) {
        Ok(Some(s)) => s,
        _ => {
            eprintln!("dspi: this transport has no notification endpoint");
            return exit::TRANSPORT;
        }
    };
    let notes = dspi_session::Notifications::start(source);
    if !json {
        println!(
            "watching {} (Ctrl-C to stop)",
            session.capabilities().serial
        );
    }
    let started = std::time::Instant::now();
    loop {
        if notes.is_disconnected() {
            eprintln!("dspi: device disconnected");
            return exit::TRANSPORT;
        }
        let Some(n) = notes.next(std::time::Duration::from_millis(500)) else {
            continue;
        };
        let t = started.elapsed().as_secs_f64();
        if json {
            let (name, detail) = describe_event(&n.event);
            println!(
                "{{\"t\":{t:.3},\"seq\":{},\"lost\":{},\"event\":\"{name}\",\"detail\":\"{}\"}}",
                n.seq,
                n.lost,
                detail.replace('"', "'")
            );
        } else {
            let (name, detail) = describe_event(&n.event);
            let lost = if n.lost { "  (packets lost)" } else { "" };
            println!("{t:8.3}  #{:<3}  {name:<18} {detail}{lost}", n.seq);
        }
    }
}

/// A one-line description of a notification, naming the bulk field it
/// touched where it can.
fn describe_event(e: &dspi_session::Event) -> (&'static str, String) {
    use dspi_session::Event;
    match e {
        Event::Idle => ("idle", String::new()),
        Event::MasterVolume(db) => ("master_volume", format!("{db:.1} dB (legacy)")),
        Event::ParamChanged {
            offset,
            source,
            bytes,
        } => {
            let section = dspi_proto::wire::BulkPacket::section_at(*offset as usize).unwrap_or("?");
            let value = match bytes.len() {
                4 => format!(
                    "{}",
                    f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                ),
                1 => format!("{}", bytes[0]),
                _ => hex_string(bytes),
            };
            (
                "param_changed",
                format!(
                    "{section}+{} = {value} [{}]",
                    *offset as usize - section_offset(section),
                    source.describe()
                ),
            )
        }
        Event::BulkInvalidated { source } => ("bulk_invalidated", source.describe().to_string()),
        Event::PresetLoaded { slot } => ("preset_loaded", format!("slot {}", slot + 1)),
        Event::InputFormat { channels } => ("input_format", format!("{channels} channels")),
        Event::SiggenState {
            state,
            reason,
            signal_type,
            channel,
        } => (
            "siggen_state",
            format!("state {state} reason {reason} type {signal_type} channel {channel}"),
        ),
        Event::AdatState {
            enabled,
            active,
            pin,
        } => (
            "adat_state",
            format!("enabled {enabled} active {active} pin {pin}"),
        ),
        Event::I2sSlaveState { state, rate_hz } => {
            ("i2s_slave_state", format!("state {state} {rate_hz} Hz"))
        }
        Event::IrLearn {
            state,
            protocol,
            code,
        } => (
            "cs_ir_learn",
            format!("state {state} protocol {protocol} code 0x{code:08X}"),
        ),
        Event::AdatInputState {
            state,
            rate_hz,
            clock_mode,
        } => (
            "adat_input_state",
            format!("state {state} {rate_hz} Hz clock {clock_mode}"),
        ),
        Event::Unknown { id, bytes } => ("unknown", format!("0x{id:02X} {}", hex_string(bytes))),
    }
}

fn section_offset(name: &str) -> usize {
    dspi_proto::generated::SECTIONS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, o, _)| *o)
        .unwrap_or(0)
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
/// All three directions the protocol actually uses. An IN read is the default
/// because it is the safe one; `out` and `war` are spelled out because they
/// change the device.
fn cmd_raw(serial: Option<&str>, args: &[&str], json: bool) -> u8 {
    match args.first().copied() {
        Some("out") => raw_out(serial, &args[1..], json),
        Some("war") => raw_war(serial, &args[1..], json),
        _ => raw_in(serial, args, json),
    }
}

fn parse_num(s: &str) -> Option<u32> {
    s.strip_prefix("0x")
        .and_then(|h| u32::from_str_radix(h, 16).ok())
        .or_else(|| s.parse().ok())
}

fn raw_in(serial: Option<&str>, args: &[&str], json: bool) -> u8 {
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
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "opcode": opcode,
                        "wValue": value,
                        "length": d.len(),
                        "bytes": hex_string(&d),
                    })
                );
                return exit::OK;
            }
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

/// `dspi raw out <op> <wValue> <hex bytes...>`
fn raw_out(serial: Option<&str>, args: &[&str], json: bool) -> u8 {
    let (Some(op_s), Some(val_s)) = (args.first(), args.get(1)) else {
        eprintln!("dspi: raw out needs an opcode, a wValue and the payload,");
        eprintln!("      e.g. `dspi raw out 0x42 0 08 03 01 00`");
        return exit::USAGE;
    };
    let (Some(opcode), Some(value)) = (parse_num(op_s), parse_num(val_s)) else {
        eprintln!("dspi: opcode and wValue must be numbers");
        return exit::USAGE;
    };
    let payload = match parse_hex(&args[2..]) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("dspi: {e}");
            return exit::USAGE;
        }
    };

    let mut t = match open(serial) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };

    match t.control_out(opcode as u8, value as u16, &payload) {
        Ok(()) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "opcode": opcode,
                        "wValue": value,
                        "sent": payload.len(),
                        "bytes": hex_string(&payload),
                    })
                );
            } else {
                println!("Sent {} bytes to 0x{opcode:02X}.", payload.len());
            }
            exit::OK
        }
        Err(e) => {
            eprintln!("dspi: {e}");
            exit::TRANSPORT
        }
    }
}

/// `dspi raw war <op> <wValue>`
///
/// Write-as-read: a mutation issued as a one-byte IN with the parameters in
/// `wValue`. Several of the pin and clock setters are only reachable this way,
/// and the byte that comes back is the status code, not data.
fn raw_war(serial: Option<&str>, args: &[&str], json: bool) -> u8 {
    let Some(opcode) = args.first().and_then(|s| parse_num(s)) else {
        eprintln!("dspi: raw war needs an opcode and a wValue, e.g. `dspi raw war 0xC2 0x010E`");
        return exit::USAGE;
    };
    let value = args.get(1).and_then(|v| parse_num(v)).unwrap_or(0) as u16;

    let mut t = match open(serial) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };

    match t.control_in(opcode as u8, value, 1) {
        Ok(d) => {
            let status = d.first().copied().unwrap_or(0);
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "opcode": opcode,
                        "wValue": value,
                        "status": status,
                    })
                );
            } else {
                println!("status 0x{status:02X}");
            }
            // The status byte is the device's verdict, so a non-zero one is a
            // refusal and a script needs to see it as one.
            if status == 0 {
                exit::OK
            } else {
                exit::REJECTED
            }
        }
        Err(e) => {
            eprintln!("dspi: {e}");
            exit::TRANSPORT
        }
    }
}

fn hex_string(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join("")
}

/// Hex bytes, however the user chose to space them: `DE AD BE EF`, `deadbeef`,
/// `0xDE,0xAD` all mean the same four or two bytes.
fn parse_hex(args: &[&str]) -> Result<Vec<u8>, String> {
    let joined: String = args
        .join("")
        .chars()
        .filter(|c| !matches!(c, ',' | ':' | '-' | '_' | ' '))
        .collect();
    let joined = joined.replace("0x", "").replace("0X", "");

    if joined.is_empty() {
        return Err("raw out needs at least one payload byte".into());
    }
    if !joined.len().is_multiple_of(2) {
        return Err(format!(
            "`{joined}` is {} hex digits; bytes come in pairs",
            joined.len()
        ));
    }
    (0..joined.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&joined[i..i + 2], 16)
                .map_err(|_| format!("`{}` is not a hex byte", &joined[i..i + 2]))
        })
        .collect()
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

    run(&mut s, cmd, json, flags.contains(&"--quiet"), force(flags))
}

fn force(flags: &[&str]) -> bool {
    flags.contains(&"--force")
}

// ---------------------------------------------------------------------------
// Batching
// ---------------------------------------------------------------------------

/// Run a file, or standard input, one command per line.
///
/// The device is opened once for the whole script rather than once per line,
/// which is the point: a hundred `eq` lines against a fresh connection each
/// time would spend all of its time probing.
fn cmd_script(serial: Option<&str>, path: Option<&str>, flags: &[&str], json: bool) -> u8 {
    let text = match path {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("dspi: could not read {p}: {e}");
                return exit::USAGE;
            }
        },
        None => {
            let mut buf = String::new();
            if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf) {
                eprintln!("dspi: could not read standard input: {e}");
                return exit::USAGE;
            }
            buf
        }
    };

    let lines = script_lines(&text);
    if lines.is_empty() {
        return exit::OK;
    }

    let mut s = match connect(serial) {
        Ok(s) => s,
        Err(c) => return c,
    };
    if flags.contains(&"--dry-run") {
        s.dry_run = true;
    }

    let ctx = context_for(&s);
    let quiet = flags.contains(&"--quiet");
    let keep_going = flags.contains(&"--keep-going");
    let force = force(flags);
    let mut first_failure = exit::OK;

    for (n, line) in lines.iter().enumerate() {
        let toks = dspi_cmd::tokenize(line);
        let refs: Vec<&str> = toks.iter().map(String::as_str).collect();
        let code = match dspi_cmd::parse(&refs, &ctx) {
            Ok(cmd) => run(&mut s, cmd, json, quiet, force),
            Err(e) => {
                eprintln!("dspi: line {}: {e}", n + 1);
                exit::USAGE
            }
        };

        if code != exit::OK {
            // The first failure is the one worth reporting: everything after it
            // may only have failed because of it.
            if first_failure == exit::OK {
                first_failure = code;
            }
            if !keep_going {
                eprintln!("dspi: stopped at line {} (`{line}`)", n + 1);
                return first_failure;
            }
        }
    }
    first_failure
}

/// The runnable lines of a script: comments and blank lines dropped.
fn script_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(strip_comment)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Everything before an unquoted `#`.
///
/// Quotes matter: a preset called "Studio #2" is a name, not a comment.
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Flags that mean "do something other than run commands", so that a piped
/// standard input does not turn `dspi --version` into an empty script.
const NOT_A_SCRIPT: &[&str] = &[
    "--help",
    "-h",
    "--version",
    "-V",
    "--doctor",
    "--install-udev",
    "--lite",
];

/// Should this invocation read commands from standard input?
///
/// Only when there is nothing else to do and stdin is not a terminal, so
/// `dspi < script.dspi` and `generate | dspi` work while an interactive `dspi`
/// still opens the interface.
fn wants_stdin_script(flags: &[&str], stdin_is_terminal: bool) -> bool {
    !stdin_is_terminal
        && positional_all(flags).is_empty()
        && !flags.iter().any(|f| NOT_A_SCRIPT.contains(f))
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

fn run(s: &mut Session, cmd: Command, json: bool, quiet: bool, force: bool) -> u8 {
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

            // Turning an output on can need the other side of Core 1 turned
            // off, which is a decision rather than a retry, so it is only made
            // when the user says so.
            if force && path == "out.enable" && value.as_bool() == Some(true) {
                return match s.enable_output_confirmed(indices[0]) {
                    Ok(dspi_session::EnableOutcome::Rejected) => {
                        eprintln!("dspi: output {} did not come on.", indices[0]);
                        exit::REJECTED
                    }
                    Ok(_) => {
                        if !quiet {
                            println!("{path} {} on", indices[0]);
                        }
                        exit::OK
                    }
                    Err(e) => {
                        eprintln!("dspi: {e}");
                        write_exit(&e)
                    }
                };
            }

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
                    if let dspi_session::WriteError::Core1Conflict { confirm, .. } = &e {
                        eprintln!("      Re-run with --force to {confirm}.");
                    }
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
            qp,
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
                qp,
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

        // `dspi undo` on a fresh process has nothing to undo, and says so:
        // the journal lives with the connection. The interface is where these
        // earn their keep; the CLI form exists so a script can walk one back
        // inside a batch.
        Command::Verb { name, .. } if name == "undo" => match s.undo() {
            Ok(None) => {
                println!("Nothing to undo.");
                exit::OK
            }
            Ok(Some(undone)) => {
                for reason in &undone.skipped {
                    println!("Skipped: {reason}.");
                }
                match undone.command {
                    Some(line) => println!("{line}"),
                    None => println!("Nothing to undo."),
                }
                exit::OK
            }
            Err(e) => {
                eprintln!("dspi: {e}");
                write_exit(&e)
            }
        },

        Command::Verb { name, .. } if name == "redo" => match s.redo() {
            Ok(None) => {
                println!("Nothing to redo.");
                exit::OK
            }
            Ok(Some(done)) => {
                if let Some(line) = done.command {
                    println!("{line}");
                }
                exit::OK
            }
            Err(e) => {
                eprintln!("dspi: {e}");
                write_exit(&e)
            }
        },

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
        | W::PlatformRange { .. }
        | W::Unavailable { .. } => exit::USAGE,
        // Nothing was written and the old value stands, which is the same
        // thing a script needs to know about a silent rejection.
        W::Core1Conflict { .. } => exit::REJECTED,
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

#[cfg(test)]
mod tests {
    use super::*;

    // -- scripts ------------------------------------------------------------

    #[test]
    fn a_script_drops_blank_lines_and_comments() {
        let text = "\
# a tuning for the small room
vol.user -18

  bass.on on   # and a little weight
";
        assert_eq!(script_lines(text), vec!["vol.user -18", "bass.on on"]);
    }

    /// A `#` inside quotes is part of a name, not the start of a comment; a
    /// preset called "Studio #2" must survive.
    #[test]
    fn a_hash_inside_quotes_is_not_a_comment() {
        assert_eq!(
            strip_comment(r#"preset.name 3 "Studio #2""#).trim(),
            r#"preset.name 3 "Studio #2""#
        );
        assert_eq!(strip_comment("vol.user -18 # loud").trim(), "vol.user -18");
        assert_eq!(strip_comment("# nothing here").trim(), "");
    }

    #[test]
    fn a_script_of_nothing_but_comments_has_no_lines() {
        assert!(script_lines("# one\n\n   # two\n").is_empty());
    }

    /// Every line has to be a line the parser accepts, or a script silently
    /// does something other than what it reads as.
    #[test]
    fn every_script_line_parses_with_the_shared_grammar() {
        let ctx = dspi_cmd::Context {
            channel_slugs: (0..17).map(|i| format!("ch.{i}")).collect(),
            num_inputs: 8,
            num_outputs: 9,
            max_bands: 10,
        };
        let text = "\
# set up the sub
vol.user -18
eq ch.0 3 peak 2856 3.58 -8.6
get out.gain 0
undo
";
        let lines = script_lines(text);
        assert_eq!(lines.len(), 4);
        for line in &lines {
            let toks = dspi_cmd::tokenize(line);
            let refs: Vec<&str> = toks.iter().map(String::as_str).collect();
            dspi_cmd::parse(&refs, &ctx).unwrap_or_else(|e| panic!("`{line}` does not parse: {e}"));
        }
    }

    /// Piping commands in is a script; asking for the version is not, even
    /// with standard input redirected from a file.
    #[test]
    fn only_an_otherwise_empty_invocation_reads_standard_input() {
        assert!(wants_stdin_script(&[], false));
        assert!(wants_stdin_script(&["--json"], false));
        assert!(wants_stdin_script(&["--device", "ABC"], false));

        // A terminal on standard input means a person, so open the interface.
        assert!(!wants_stdin_script(&[], true));
        // Anything with something else to do keeps doing it.
        assert!(!wants_stdin_script(&["--version"], false));
        assert!(!wants_stdin_script(&["--help"], false));
        assert!(!wants_stdin_script(&["--lite"], false));
        assert!(!wants_stdin_script(&["dump"], false));
        assert!(!wants_stdin_script(&["vol.user", "-18"], false));
    }

    // -- raw ----------------------------------------------------------------

    /// However the user spaced the bytes, they mean the same payload.
    #[test]
    fn hex_payloads_are_read_however_they_are_spaced() {
        let want = vec![0xDE, 0xAD, 0xBE, 0xEF];
        assert_eq!(parse_hex(&["DE", "AD", "BE", "EF"]).unwrap(), want);
        assert_eq!(parse_hex(&["deadbeef"]).unwrap(), want);
        assert_eq!(parse_hex(&["0xDE,0xAD", "0xBE:0xEF"]).unwrap(), want);
        assert_eq!(parse_hex(&["08", "00"]).unwrap(), vec![8, 0]);
    }

    /// A half-typed byte is a mistake worth naming, not something to pad.
    #[test]
    fn an_odd_number_of_hex_digits_is_refused() {
        let e = parse_hex(&["DEA"]).unwrap_err();
        assert!(e.contains("pairs"), "{e}");
        assert!(parse_hex(&[]).is_err());
        assert!(parse_hex(&["ZZ"]).is_err());
    }

    #[test]
    fn opcodes_are_accepted_in_hex_or_decimal() {
        assert_eq!(parse_num("0x42"), Some(0x42));
        assert_eq!(parse_num("66"), Some(66));
        assert_eq!(parse_num("0xFFFF"), Some(0xFFFF));
        assert_eq!(parse_num("nonsense"), None);
    }

    #[test]
    fn bytes_render_back_as_the_hex_they_came_from() {
        assert_eq!(hex_string(&[0xDE, 0xAD]), "DEAD");
        assert_eq!(hex_string(&[]), "");
    }

    // -- flags --------------------------------------------------------------

    #[test]
    fn flag_values_are_read_and_their_values_are_not_positional() {
        let flags = ["set", "vol.user", "-18", "--device", "ABC123", "--json"];
        assert_eq!(flag_value(&flags, "--device"), Some("ABC123"));
        assert_eq!(flag_value(&flags, "--nothing"), None);
        assert_eq!(positional_all(&flags), vec!["set", "vol.user", "-18"]);
    }

    #[test]
    fn force_is_off_unless_it_is_asked_for() {
        assert!(!force(&["out.enable", "8", "on"]));
        assert!(force(&["out.enable", "8", "on", "--force"]));
    }
}
