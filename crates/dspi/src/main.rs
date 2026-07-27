//! `dspi` - terminal control for DSPi audio processors.
//!
//! M1 scope: connect, discover, and dump state. The TUI and the one-shot command
//! grammar arrive in later milestones; this binary exists now so that every
//! layer below it is exercised against real hardware rather than only the mock.

use std::process::ExitCode;

use dspi_session::probe;
use dspi_transport::{TransportError, UsbTransport, list_devices};

/// Exit codes are part of the interface: scripts branch on these.
mod exit {
    pub const OK: u8 = 0;
    pub const USAGE: u8 = 1;
    pub const NO_DEVICE: u8 = 2;
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
    dspi --version           show app and protocol versions

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
