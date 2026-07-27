//! `dspi` - terminal control for DSPi audio processors.
//!
//! M1 scope: connect, discover, and dump state. The TUI and the one-shot command
//! grammar arrive in later milestones; this binary exists now so that every
//! layer below it is exercised against real hardware rather than only the mock.

use std::process::ExitCode;

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
        Some("get") => cmd_get(serial, &flags, json),
        Some("set") => cmd_set(serial, &flags, json),
        Some("dump") | None => cmd_dump(serial, json),
        Some(other) if other.starts_with("--") => cmd_dump(serial, json),
        Some(other) => {
            eprintln!("dspi: unknown command `{other}`\n");
            usage();
            exit::USAGE
        }
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
    dspi [dump]              connect and print the full device state
    dspi list                list every connected DSPi
    dspi params              list every parameter this build knows
    dspi get <path> [i..]    read one parameter
    dspi set <path> [i..] v  write one parameter, and confirm it
    dspi --version           show app and protocol versions

EXAMPLES:
    dspi get vol.user
    dspi set vol.user -18
    dspi get eq.freq 8 3
    dspi set bass.drive 12

OPTIONS:
    --device <serial>        target a specific device
    --json                   machine-readable output
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

fn cmd_get(serial: Option<&str>, flags: &[&str], json: bool) -> u8 {
    let args = positional(flags);
    let Some(path) = args.first() else {
        eprintln!("dspi: get needs a parameter path, e.g. `dspi get vol.user`");
        return exit::USAGE;
    };
    let Some(d) = by_path(path) else {
        eprintln!("dspi: no parameter called `{path}`. Try `dspi params`.");
        return exit::USAGE;
    };
    let indices: Vec<u8> = args[1..].iter().filter_map(|a| a.parse().ok()).collect();

    let mut s = match connect(serial) {
        Ok(s) => s,
        Err(c) => return c,
    };

    match s.read(path, &indices) {
        Ok(v) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "path": path, "value": show(d.kind, &v) })
                );
            } else {
                // Bare value, so it captures cleanly in a shell.
                println!("{}", show(d.kind, &v));
            }
            exit::OK
        }
        Err(e) => {
            eprintln!("dspi: {e}");
            exit::TRANSPORT
        }
    }
}

fn cmd_set(serial: Option<&str>, flags: &[&str], json: bool) -> u8 {
    let args = positional(flags);
    if args.len() < 2 {
        eprintln!("dspi: set needs a path and a value, e.g. `dspi set vol.user -18`");
        return exit::USAGE;
    }
    let path = args[0];
    let Some(d) = by_path(path) else {
        eprintln!("dspi: no parameter called `{path}`. Try `dspi params`.");
        return exit::USAGE;
    };

    // Everything between the path and the final value is an index.
    let raw_value = *args.last().unwrap();
    let indices: Vec<u8> = args[1..args.len() - 1]
        .iter()
        .filter_map(|a| a.parse().ok())
        .collect();

    let value = match parse_value(d.kind, raw_value) {
        Some(v) => v,
        None => {
            eprintln!("dspi: `{raw_value}` is not a valid value for {path}");
            return exit::USAGE;
        }
    };

    let mut s = match connect(serial) {
        Ok(s) => s,
        Err(c) => return c,
    };
    if flags.contains(&"--dry-run") {
        s.dry_run = true;
    }

    match s.write(path, &indices, value.clone()) {
        Ok(Outcome::Rejected { sent, actual }) => {
            // The device accepted the request and then did something else. This
            // must not exit zero: a script needs to notice.
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
            } else if !flags.contains(&"--quiet") {
                println!("{} = {}", path, show(d.kind, &value));
            }
            exit::OK
        }
        Err(e) => {
            eprintln!("dspi: {e}");
            write_exit(&e)
        }
    }
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

fn parse_value(kind: Kind, raw: &str) -> Option<Value> {
    Some(match kind {
        Kind::Bool => match raw.to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" => Value::Bool(true),
            "off" | "false" | "no" | "0" => Value::Bool(false),
            _ => return None,
        },
        Kind::Trigger => Value::Trigger,
        Kind::Choice(variants) => {
            let lower = raw.to_ascii_lowercase();
            let by_name = variants.iter().find(|(_, n)| *n == lower);
            match by_name {
                Some((v, _)) => Value::Choice(*v),
                None => Value::Choice(raw.parse().ok()?),
            }
        }
        Kind::Text { .. } => Value::Text(raw.to_string()),
        Kind::Int { .. } => Value::Int(raw.parse().ok()?),
        Kind::Mask => Value::Mask(if let Some(hex) = raw.strip_prefix("0x") {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            raw.parse().ok()?
        }),
        _ => Value::Float(raw.parse().ok()?),
    })
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
