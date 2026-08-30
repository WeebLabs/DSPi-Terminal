//! `dspi doctor`: answer "why can't it see my device?".
//!
//! This is the single most common support question for any USB tool, and the
//! answer is completely different on each platform: a missing udev rule on
//! Linux, another application holding the interface on macOS, an unbound driver
//! on Windows. A generic "permission denied" helps nobody, so every check knows
//! what to suggest where.

use dspi_proto::{USB_PID, USB_VID};
use dspi_transport::{TransportError, UsbTransport, list_devices};

/// Where the Linux rule belongs.
const UDEV_PATH: &str = "/etc/udev/rules.d/70-dspi.rules";
const UDEV_RULES: &str = include_str!("../../../xtask/70-dspi.rules");

enum Status {
    Ok(String),
    Warn(String),
    Fail(String, String),
}

impl Status {
    fn print(&self) {
        match self {
            Status::Ok(m) => println!("  ok    {m}"),
            Status::Warn(m) => println!("  note  {m}"),
            Status::Fail(m, fix) => {
                println!("  FAIL  {m}");
                for line in fix.lines() {
                    println!("          {line}");
                }
            }
        }
    }

    fn is_fail(&self) -> bool {
        matches!(self, Status::Fail(..))
    }
}

pub fn run() -> u8 {
    println!(
        "dspi {} on {} {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    let mut checks = vec![terminal_check()];
    checks.extend(usb_checks());

    for c in &checks {
        c.print();
    }

    let failed = checks.iter().filter(|c| c.is_fail()).count();
    println!();
    if failed == 0 {
        println!("Everything looks fine.");
        0
    } else {
        println!("{failed} problem(s) found.");
        2
    }
}

fn terminal_check() -> Status {
    let (cols, rows) = dspi_tui::terminal_size().unwrap_or((0, 0));
    let depth = dspi_tui::theme::ColorDepth::detect();
    let glyphs = dspi_tui::perf::detect_glyphs();

    let msg = format!("terminal {cols}x{rows}, {depth:?} colour, {glyphs:?} glyphs");
    if cols < 60 || rows < 12 {
        Status::Warn(format!("{msg} (the interface needs at least 60x12)"))
    } else {
        Status::Ok(msg)
    }
}

fn usb_checks() -> Vec<Status> {
    let devices = match list_devices() {
        Ok(d) => d,
        Err(e) => {
            return vec![Status::Fail(
                format!("cannot enumerate USB: {e}"),
                "The USB subsystem is unreachable. On Linux check that usbfs is \
                 mounted; elsewhere this usually means a permissions problem."
                    .into(),
            )];
        }
    };

    let mut out = vec![Status::Ok("USB subsystem reachable".into())];

    if devices.is_empty() {
        out.push(Status::Fail(
            format!("no DSPi found (looking for {USB_VID:04X}:{USB_PID:04X})"),
            "Check the cable and that the device is powered. If it is plugged in \
             and still not listed, it may be in bootloader mode, which enumerates \
             as RPI-RP2 instead."
                .into(),
        ));
        return out;
    }

    out.push(Status::Ok(format!(
        "{} DSPi device(s): {}",
        devices.len(),
        devices
            .iter()
            .map(|d| d.short_name())
            .collect::<Vec<_>>()
            .join(", ")
    )));

    // Opening is the step that actually fails in the field, so try it rather
    // than inferring from enumeration.
    for d in &devices {
        match UsbTransport::open_serial(&d.serial) {
            Ok(mut t) => {
                out.push(Status::Ok(format!("{}: interface claimed", d.short_name())));
                match dspi_session::probe(&mut t) {
                    Ok(caps) => out.push(Status::Ok(format!(
                        "{}: {} firmware {}, wire V{}, {} channels",
                        d.short_name(),
                        caps.platform.name(),
                        caps.firmware,
                        caps.wire_format,
                        caps.num_channels
                    ))),
                    Err(e) => out.push(Status::Fail(
                        format!("{}: opened but would not answer: {e}", d.short_name()),
                        "The device enumerated but is not responding to vendor \
                         commands. Try unplugging and replugging it."
                            .into(),
                    )),
                }
            }
            Err(TransportError::PermissionDenied) => {
                out.push(Status::Fail(
                    format!("{}: cannot claim the vendor interface", d.short_name()),
                    permission_fix(),
                ));
            }
            Err(e) => out.push(Status::Fail(
                format!("{}: {e}", d.short_name()),
                "Unexpected failure opening the device.".into(),
            )),
        }
    }

    if cfg!(target_os = "linux") {
        out.push(udev_check());
    }

    out
}

/// The fix differs completely per platform, which is the whole point of this
/// command existing.
fn permission_fix() -> String {
    if cfg!(target_os = "linux") {
        format!(
            "Almost always a missing udev rule. Install it with:\n\
             sudo dspi --install-udev\n\
             sudo udevadm control --reload\n\
             then unplug and replug the device.\n\
             (The rule belongs at {UDEV_PATH}.)"
        )
    } else if cfg!(target_os = "macos") {
        "The vendor interface is exclusive on macOS, so only one application may \
         hold it at a time. Quit DSPi Console or any other DSPi tool and try again."
            .into()
    } else {
        "Windows binds WinUSB automatically via the device's MS OS 2.0 \
         descriptors, so this usually means another application is holding the \
         interface. Close any other DSPi tool and try again."
            .into()
    }
}

fn udev_check() -> Status {
    if std::path::Path::new(UDEV_PATH).exists() {
        Status::Ok(format!("udev rule installed at {UDEV_PATH}"))
    } else {
        Status::Warn(format!(
            "no udev rule at {UDEV_PATH}; non-root access needs one \
             (`sudo dspi --install-udev`)"
        ))
    }
}

/// Write the udev rule. Linux only, and needs root.
pub fn install_udev() -> u8 {
    if !cfg!(target_os = "linux") {
        eprintln!(
            "dspi: udev rules are a Linux thing; nothing to install on {}.",
            std::env::consts::OS
        );
        return 1;
    }

    match std::fs::write(UDEV_PATH, UDEV_RULES) {
        Ok(()) => {
            println!("Installed {UDEV_PATH}");
            println!("Now run:  sudo udevadm control --reload");
            println!("then unplug and replug the DSPi.");
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("dspi: need root to write {UDEV_PATH}. Try `sudo dspi --install-udev`.");
            1
        }
        Err(e) => {
            eprintln!("dspi: could not write {UDEV_PATH}: {e}");
            1
        }
    }
}
