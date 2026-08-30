//! Render the shell with fixture data, for design review without a device.
//!
//!   gallery [width] [height] [console|amber|dark|mono] [rp2350|rp2040]
//!           [--screen overview|input|output|matrix|crossfeed|loudness
//!                     |leveller|psybass|upmixer|signals] [--settings <page>]
//!           [--expand <n>] [--ansi]
//!
//! Prints the frame as text, or as ANSI escapes with `--ansi` so the colours
//! can be looked at by piping to a terminal. `--screen` picks what fills the
//! pane: one of the Console's detail screens, or one of its tool panels, which
//! replace the graph as well. The default is the input page. `--settings`
//! opens Settings on one of its pages, as `,` does: about, advanced, graphing,
//! overview, inputs, outputs, i2s, global, surfaces, interfaces, groups,
//! macros. `--expand n` opens the nth card on one of the three Control pages,
//! whose bodies are otherwise behind a collapsed header.

use dspi_tui::screens::{
    CrossfeedPanel, InputPage, LevellerPanel, LoudnessPanel, MatrixPanel, OutputPage, Overview,
    PsybassPanel, SignalsPanel, UpmixerPanel, shared,
};
use dspi_tui::settings::{AppConfig, SettingsScreen};
use dspi_tui::shell::{Focus, Screen, Selection, Shell, Tool, fixture};
use dspi_tui::theme::{ColorDepth, Glyphs, Palette, Theme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let ansi = raw.iter().any(|a| a == "--ansi");
    let screen = raw
        .iter()
        .position(|a| a == "--screen")
        .and_then(|i| raw.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "input".into());
    let settings = raw
        .iter()
        .position(|a| a == "--settings")
        .and_then(|i| raw.get(i + 1))
        .cloned();
    // The Control pages are lists of collapsed cards, so a card body is only
    // reachable by opening one. `--expand n` puts the cursor on the nth
    // focusable row and activates it, which is what a reviewer would do.
    let expand: Option<usize> = raw
        .iter()
        .position(|a| a == "--expand")
        .and_then(|i| raw.get(i + 1))
        .and_then(|a| a.parse().ok());
    // Positional arguments, with the flags and their values taken out.
    let mut args: Vec<&String> = Vec::new();
    let mut skip = false;
    for a in &raw {
        if std::mem::take(&mut skip) || a == "--ansi" {
            continue;
        }
        if a == "--screen" || a == "--settings" || a == "--expand" {
            skip = true;
            continue;
        }
        args.push(a);
    }
    let num = |i: usize, d: u16| args.get(i).and_then(|a| a.parse().ok()).unwrap_or(d);
    let (w, h) = (num(0, 120), num(1, 40));
    let palette = args
        .get(2)
        .and_then(|a| Palette::parse(a))
        .unwrap_or(Palette::Console);
    let depth = if ansi {
        ColorDepth::detect()
    } else {
        ColorDepth::TrueColor
    };
    let theme = Theme::new(palette, depth, Glyphs::Braille);
    let mut model = match args.get(3).map(|s| s.as_str()) {
        Some("rp2040") => fixture::rp2040(&theme),
        _ => fixture::rp2350(&theme),
    };

    // The tool panels are gated on device features, so the gallery's fixture
    // reports them present; without that every panel would draw its "requires
    // newer firmware" banner and there would be nothing to review.
    let mut state = fixture::state();
    for name in ["psychoacoustic_bass", "upmixer", "test_signals"] {
        state.caps.features.push(dspi_session::probe::Feature {
            name: name.into(),
            present: true,
            evidence: "fixture".into(),
        });
    }
    state.caps.siggen = Some(dspi_session::probe::SiggenCaps {
        version: 1,
        type_count: 15,
        output_channels: 9,
        multitone_max: 16,
        valid_channel_mask: 0x1FF,
        types: Vec::new(),
    });
    // The shell fixture leaves every DSP block at zero, which makes a panel a
    // page of flat sliders and an empty graph. Give each one the Console's own
    // defaults so a design review sees a working panel.
    let section = |name: &str| {
        dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, o, _)| *o)
            .expect("section")
    };
    let g = section("global");
    state.bulk.patch(g + 5, &[1]);
    state.bulk.patch(g + 6, &0x01FFu16.to_le_bytes());
    state.bulk.patch(g + 8, &80.0f32.to_le_bytes());
    state.bulk.patch(g + 12, &100.0f32.to_le_bytes());
    let cf = section("crossfeed");
    state.bulk.patch(cf, &[1, 0, 1, 0x03]);
    state.bulk.patch(cf + 4, &700.0f32.to_le_bytes());
    state.bulk.patch(cf + 8, &4.5f32.to_le_bytes());
    let lev = section("leveller");
    state.bulk.patch(lev, &[1, 1, 0, 0]);
    state.bulk.patch(lev + 4, &50.0f32.to_le_bytes());
    state.bulk.patch(lev + 8, &12.0f32.to_le_bytes());
    state.bulk.patch(lev + 12, &(-60.0f32).to_le_bytes());
    state.bulk.patch(lev + 16, &[0x03, 0x03]);
    let bass = section("psybass");
    state.bulk.patch(bass, &[1, 0]);
    state.bulk.patch(bass + 2, &0x00FFu16.to_le_bytes());
    state.bulk.patch(bass + 4, &80.0f32.to_le_bytes());
    state.bulk.patch(bass + 12, &6.0f32.to_le_bytes());
    state.bulk.patch(bass + 16, &50.0f32.to_le_bytes());
    let up = section("upmix");
    state.bulk.patch(up, &[1, 1, 1, 0]);
    state.bulk.patch(up + 4, &70.0f32.to_le_bytes());
    state.bulk.patch(up + 12, &30.0f32.to_le_bytes());
    state.bulk.patch(up + 16, &20.0f32.to_le_bytes());
    state.bulk.patch(up + 20, &200.0f32.to_le_bytes());
    state.bulk.patch(up + 24, &120.0f32.to_le_bytes());
    state.bulk.patch(up + 28, &10.0f32.to_le_bytes());
    state.bulk.patch(up + 32, &100.0f32.to_le_bytes());
    state.bulk.patch(up + 36, &7000.0f32.to_le_bytes());
    state.upmix_status = Some(dspi_proto::packets::UpmixStatus {
        active: true,
        parked_reason: 0,
        corr_q14: 11_000,
        balance_q14: 8_192,
        center_gain_q15: 20_000,
        ls_gain_q15: 9_000,
        rs_gain_q15: 12_000,
    });
    // The Settings pages show wiring, so they get a device with some. The
    // three Control pages are entirely caps-driven, so theirs additionally
    // reports a control-surface capability table and the records built on it.
    let state = if settings.is_some() {
        dspi_tui::settings::cs_model::demo::state()
    } else {
        state
    };
    let shared = shared();
    let tool = match screen.as_str() {
        "matrix" => Some((
            Tool::Matrix,
            Box::new(MatrixPanel::new(shared.clone())) as Box<dyn Screen>,
        )),
        "crossfeed" => Some((
            Tool::Crossfeed,
            Box::new(CrossfeedPanel::new()) as Box<dyn Screen>,
        )),
        "loudness" => Some((
            Tool::Loudness,
            Box::new(LoudnessPanel::new()) as Box<dyn Screen>,
        )),
        "leveller" => Some((
            Tool::Leveller,
            Box::new(LevellerPanel::new()) as Box<dyn Screen>,
        )),
        "psybass" => Some((
            Tool::Psybass,
            Box::new(PsybassPanel::new()) as Box<dyn Screen>,
        )),
        "upmixer" => Some((
            Tool::Upmixer,
            Box::new(UpmixerPanel::new()) as Box<dyn Screen>,
        )),
        "signals" => Some((
            Tool::Signals,
            Box::new(SignalsPanel::new()) as Box<dyn Screen>,
        )),
        _ => None,
    };
    let (detail, selection): (Box<dyn Screen>, Selection) = match screen.as_str() {
        "overview" | "matrix" => (Box::new(Overview::new(shared.clone())), Selection::Overview),
        "output" => (
            Box::new(OutputPage::new(0, shared.clone(), &state)),
            Selection::Output(0),
        ),
        _ => (
            Box::new(InputPage::new(0, shared.clone(), &state)),
            Selection::Input(0),
        ),
    };
    model.selection = selection;
    let mut shell = Shell::new(model, theme, detail);
    shell.focus = Focus::Screen;
    if let Some((tool, panel)) = tool {
        shell.open_tool(tool, panel);
    }
    if let Some(name) = &settings {
        let page = SettingsScreen::page_from_name(name).unwrap_or_else(|| {
            eprintln!("unknown settings page {name}");
            std::process::exit(2);
        });
        let mut s = SettingsScreen::new(
            &state,
            dspi_tui::settings::cs_model::demo::settings_data(),
            AppConfig::default(),
        )
        .open(page, &state);
        if let Some(n) = expand {
            use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
            let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
            s.handle(key(KeyCode::Tab), &state);
            for _ in 0..n {
                s.handle(key(KeyCode::Down), &state);
            }
            s.handle(key(KeyCode::Enter), &state);
        }
        shell.open_settings(Box::new(s));
    }

    let mut term = Terminal::new(TestBackend::new(w, h)).expect("backend");
    term.draw(|f| shell.draw(f.area(), f.buffer_mut(), &state))
        .expect("draw");
    let buf = term.backend().buffer();

    fn sgr(c: Color, fg: bool) -> String {
        let base = if fg { 38 } else { 48 };
        match c {
            Color::Rgb(r, g, b) => format!("\x1b[{base};2;{r};{g};{b}m"),
            Color::Indexed(i) => format!("\x1b[{base};5;{i}m"),
            Color::Reset => format!("\x1b[{}m", if fg { 39 } else { 49 }),
            _ => String::new(),
        }
    }

    for y in 0..h {
        let mut line = String::new();
        for x in 0..w {
            let cell = &buf[(x, y)];
            if ansi {
                line.push_str("\x1b[0m");
                line.push_str(&sgr(cell.fg, true));
                line.push_str(&sgr(cell.bg, false));
                if cell.modifier.contains(Modifier::REVERSED) {
                    line.push_str("\x1b[7m");
                }
                if cell.modifier.contains(Modifier::BOLD) {
                    line.push_str("\x1b[1m");
                }
            }
            line.push_str(cell.symbol());
        }
        if ansi {
            line.push_str("\x1b[0m");
        }
        println!("{}", line.trim_end());
    }
}
