//! Drive the interface's editing path against a real device, then put the value
//! back. Proves the keystroke path writes, not just that the write path does.

use dspi_session::{Session, probe};
use dspi_transport::UsbTransport;
use dspi_tui::app::{App, BandField, Focus, Panel};
use dspi_tui::theme::{ColorDepth, Glyphs, Palette, Theme};

fn main() {
    let mut t = UsbTransport::open_only().expect("no device");
    let caps = probe(&mut t).expect("probe failed");
    let mut session = Session::new(Box::new(t), caps).expect("session");

    let theme = Theme::new(Palette::Amber, ColorDepth::detect(), Glyphs::Braille);
    let mut app = App::from_session(theme, &session);

    // Load channel 0's bands, as the real startup does.
    for b in 0..app.channels[0].bands.len() {
        if let Ok(p) = session.read_band(0, b as u8) {
            app.channels[0].bands[b] = dspi_proto::dsp::Band {
                filter_type: p.filter_type,
                freq: p.freq,
                q: p.q,
                gain_db: p.gain_db,
                bypass: p.bypass,
            };
        }
    }

    app.panel = Panel::Filters;
    app.focus = Focus::Content;
    app.selected_channel = 0;
    app.selected_band = 0;
    app.selected_field_col = BandField::Freq;

    let before = app.channels[0].bands[0].freq;
    println!("band 1 frequency before: {before} Hz");

    // One notch up, exactly as pressing '+' does.
    app.edit_band(&mut session, true, false);
    let after = session.read_band(0, 0).unwrap().freq;
    println!("after one step up:       {after} Hz   (echo: {})", app.echo);
    assert!(after > before, "the edit did not reach the device");

    // And back down, so the device is left as it was found.
    app.edit_band(&mut session, false, false);
    let restored = session.read_band(0, 0).unwrap().freq;
    println!("after one step down:     {restored} Hz");
    assert!(
        (restored - before).abs() < 0.01,
        "failed to restore: {restored} vs {before}"
    );

    println!("\nediting works, device restored");
}
