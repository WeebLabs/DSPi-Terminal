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
use dspi_transport::{TransportError, UsbTransport, list_devices};

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
        Some("--install-udev") => doctor::install_udev(),
        Some("screenshot") => cmd_screenshot(serial, &positional(&flags)),
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
    dspi completions <shell> generate shell completions
    dspi doctor              diagnose connection problems
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
