//! Drive the interface's `:` line against a real device, then restore.

use dspi_session::{Session, probe};
use dspi_transport::UsbTransport;
use dspi_tui::app::App;
use dspi_tui::theme::{ColorDepth, Glyphs, Palette, Theme};

fn main() {
    let mut t = UsbTransport::open_only().expect("no device");
    let caps = probe(&mut t).expect("probe failed");
    let mut session = Session::new(Box::new(t), caps).expect("session");

    let theme = Theme::new(Palette::Amber, ColorDepth::TrueColor, Glyphs::Braille);
    let mut app = App::from_session(theme, &session);

    let before = session.read("bass.drive", &[]).unwrap();
    println!("bass.drive before: {before:?}");

    // A read, which should report rather than change anything.
    app.run_command(&mut session, "get bass.drive");
    println!(
        "get      -> {:?}",
        app.status.as_ref().map(|(m, _)| m.clone())
    );

    // A write.
    app.run_command(&mut session, "bass.drive 9");
    println!("set      -> echo: {}", app.echo);
    println!(
        "           device now: {:?}",
        session.read("bass.drive", &[]).unwrap()
    );

    // Something that cannot work, which should be reported, not swallowed.
    app.run_command(&mut session, "bass.drive 99");
    println!(
        "bad      -> {:?}",
        app.status.as_ref().map(|(m, _)| m.clone())
    );

    app.run_command(&mut session, "wobble 3");
    println!(
        "nonsense -> {:?}",
        app.status.as_ref().map(|(m, _)| m.clone())
    );

    // Put it back.
    let restore = format!("bass.drive {}", before.as_f32().unwrap());
    app.run_command(&mut session, &restore);
    let after = session.read("bass.drive", &[]).unwrap();
    println!("\nrestored to: {after:?}");
    assert_eq!(format!("{before:?}"), format!("{after:?}"), "not restored");
    println!("command line works, device restored");
}
