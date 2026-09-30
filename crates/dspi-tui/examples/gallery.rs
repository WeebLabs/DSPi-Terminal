//! Render the shell with fixture data, for design review without a device.
//!
//!   gallery [width] [height] [calm|console|amber|dark|mono] [rp2350|rp2040]
//!           [--screen overview|input|output|limiter|matrix|crossfeed|loudness
//!                     |leveller|psybass|upmixer|signals|stats|monitor
//!                     |autoeq|subharm|tube|spectrum|nodevice] [--settings <page>]
//!           [--expand n] [--busy|--full] [--channels 1,2,9] [--bars|--both]
//!           [--depth truecolor|256|16|mono] [--ansi]
//!
//! Prints the frame as text, or as ANSI escapes with `--ansi` so the colours
//! can be looked at by piping to a terminal. `--screen` picks what fills the
//! pane: one of the Console's detail screens, or one of its tool panels, which
//! replace the graph as well. The default is the input page. `--settings`
//! opens Settings on one of its pages, as `,` does: about, advanced, graphing,
//! spectrum, overview, inputs, outputs, i2s, global, surfaces, interfaces,
//! groups, macros, aux. `--channels` picks the outputs the spectrum panel
//! shows (one, with bins, by default) and `--bars` or `--both` how it draws
//! them. `--expand n` opens the nth card on one of the four card pages,
//! whose bodies are otherwise behind a collapsed header. `--busy` swaps in
//! the fixture with every channel tuned, which is what the overview grid
//! is for; `--full` the one where no two channels are alike.

use dspi_tui::live::pending_tool;
use dspi_tui::screens::{
    AutoEqPanel, CrossfeedPanel, InputPage, LevellerPanel, LoudnessPanel, MatrixPanel,
    MonitorPanel, OutputPage, Overview, PsybassPanel, SignalsPanel, SpectrumPanel, StatsPanel,
    UpmixerPanel, shared, spectrum,
};
use dspi_tui::settings::{AppConfig, SettingsScreen};
use dspi_tui::shell::{Focus, Screen, Selection, Shell, Tool, fixture};
use dspi_tui::theme::{ColorDepth, Glyphs, Palette, Theme};

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let ansi = raw.iter().any(|a| a == "--ansi");
    let busy = raw.iter().any(|a| a == "--busy");
    let full = raw.iter().any(|a| a == "--full");
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
    // `--quick "1 2 > 3"` opens the `;` bar prefilled, for review.
    let quick = raw
        .iter()
        .position(|a| a == "--quick")
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
        if std::mem::take(&mut skip)
            || ["--ansi", "--busy", "--full", "--bars", "--both"].contains(&a.as_str())
        {
            continue;
        }
        if [
            "--screen",
            "--settings",
            "--expand",
            "--quick",
            "--channels",
        ]
        .contains(&a.as_str())
        {
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
    let depth = match raw
        .iter()
        .position(|a| a == "--depth")
        .and_then(|i| raw.get(i + 1))
    {
        Some(d) => match d.as_str() {
            "256" => ColorDepth::Ansi256,
            "16" => ColorDepth::Ansi16,
            "mono" => ColorDepth::Mono,
            _ => ColorDepth::TrueColor,
        },
        None if ansi => ColorDepth::detect(),
        None => ColorDepth::TrueColor,
    };
    let glyphs = if ansi {
        dspi_tui::perf::detect_glyphs()
    } else {
        Glyphs::Braille
    };
    let theme = Theme::new(palette, depth, glyphs);
    let mut model = match args.get(3).map(|s| s.as_str()) {
        Some("rp2040") => fixture::rp2040(&theme),
        _ => fixture::rp2350(&theme),
    };

    // The tool panels are gated on device features, so the gallery's fixture
    // reports them present; without that every panel would draw its "requires
    // newer firmware" banner and there would be nothing to review.
    let section = |name: &str| {
        dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, o, _)| *o)
            .expect("section")
    };
    let mut state = if full {
        fixture::full_state()
    } else if busy {
        fixture::busy_state()
    } else {
        fixture::state()
    };
    for name in ["psychoacoustic_bass", "upmixer", "test_signals"] {
        state.caps.features.push(dspi_session::probe::Feature {
            name: name.into(),
            present: true,
            evidence: "fixture".into(),
        });
    }
    // The output limiter: output 1 and 2 on and linked, output 1 reducing,
    // so the output page shows the cell in its orange state.
    state = dspi_tui::screens::limiter::demo_state(state);
    state.limiter_meter = Some(dspi_proto::packets::LimiterMeter {
        centi_db: vec![320, 0, 0, 0, 0, 0, 0, 0, 0],
    });
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
    let mut state = if settings.is_some() {
        dspi_tui::settings::cs_model::demo::state()
    } else {
        state
    };
    let shared = shared();

    // The three panels of `DESIGN.md` 7.10 to 7.12 draw from the application's
    // own state rather than from the device, so the gallery has to fill that
    // in too: an empty stats snapshot is a page of dashes and an empty log is
    // an empty box, neither of which is a design to review.
    {
        let mut app = shared.borrow_mut();
        app.stats = dspi_tui::actions::Stats {
            read: true,
            clock_hz: 300_000_000,
            core_mv: 1150,
            sample_rate_hz: 48_000,
            temp_centi_c: 4231,
            pdm_ring_under: 3,
            usb_ring_over: 1,
            starvation_total: 7,
            starvation_delta: 2,
            starvation_per_instance: [4, 3, 0, 0],
            polls_since_starvation: Some(0),
            polls_between_starvations: Some(15),
            buffers: Some(dspi_proto::packets::BufferStatsPacket {
                num_spdif: 2,
                flags: dspi_proto::packets::BufferStatsPacket::FLAG_STREAMING
                    | dspi_proto::packets::BufferStatsPacket::FLAG_PDM_ACTIVE,
                sequence: 9,
                spdif: [
                    dspi_proto::packets::SpdifBufferStats {
                        consumer_fill_pct: 50,
                        consumer_min_fill_pct: 20,
                        consumer_max_fill_pct: 80,
                        ..Default::default()
                    },
                    dspi_proto::packets::SpdifBufferStats {
                        consumer_fill_pct: 92,
                        consumer_min_fill_pct: 40,
                        consumer_max_fill_pct: 100,
                        ..Default::default()
                    },
                    Default::default(),
                    Default::default(),
                ],
                pdm: dspi_proto::packets::PdmBufferStats {
                    dma_fill_pct: 20,
                    dma_min_fill_pct: 10,
                    dma_max_fill_pct: 35,
                    ring_fill_pct: 12,
                    ring_min_fill_pct: 8,
                    ring_max_fill_pct: 60,
                },
            }),
            spdif_rx: Some(dspi_proto::packets::SpdifRxStatusPacket {
                state: dspi_proto::enums::SpdifRxState::Locked,
                input_source: dspi_proto::enums::InputSource::from_raw(4),
                lock_count: 2,
                loss_count: 1,
                sample_rate: 44_100,
                fifo_fill_pct: 55,
                lib_state: 2,
                callback_counts: 0x21,
                ..Default::default()
            }),
            spdif_rx_pin: Some(20),
            lg: Some(dspi_proto::packets::LgSoundSyncStatus {
                enabled: true,
                present: true,
                volume: 42,
                muted: false,
            }),
            adat: Some(dspi_proto::packets::AdatStatus {
                enabled: true,
                active: true,
                pin: 6,
                rate_ok: true,
                resync_count: 1,
                slip_count: 0,
            }),
            ..Default::default()
        };

        app.log.active = true;
        let mut at = 0.125;
        for (seq, event) in [
            (7u8, dspi_session::Event::PresetLoaded { slot: 2 }),
            (
                8,
                dspi_session::Event::ParamChanged {
                    offset: section("user_volume") as u16,
                    source: dspi_session::Source::Uac1,
                    bytes: (-30.0f32).to_le_bytes().to_vec(),
                },
            ),
            (
                9,
                dspi_session::Event::ParamChanged {
                    offset: (section("outputs") + 12 + 4) as u16,
                    source: dspi_session::Source::Gpio,
                    bytes: (-3.5f32).to_le_bytes().to_vec(),
                },
            ),
            (
                10,
                dspi_session::Event::ParamChanged {
                    offset: (section("eq") + 16) as u16,
                    source: dspi_session::Source::HostSet,
                    bytes: {
                        let mut b = vec![0u8; 16];
                        b[0] = 1;
                        b[4..8].copy_from_slice(&2856.0f32.to_le_bytes());
                        b[8..12].copy_from_slice(&3.58f32.to_le_bytes());
                        b[12..16].copy_from_slice(&(-8.6f32).to_le_bytes());
                        b
                    },
                },
            ),
            (
                11,
                dspi_session::Event::I2sSlaveState {
                    state: 3,
                    rate_hz: 48_000,
                },
            ),
        ] {
            app.log.push(
                at,
                &dspi_session::Notification {
                    seq,
                    event,
                    lost: false,
                },
            );
            at += 0.734;
        }
    }

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
        "stats" => Some((
            Tool::Stats,
            Box::new(StatsPanel::new(shared.clone())) as Box<dyn Screen>,
        )),
        "monitor" => Some((
            Tool::Monitor,
            Box::new(MonitorPanel::new(shared.clone())) as Box<dyn Screen>,
        )),
        // The browser loads the real database, so this is the one panel whose
        // gallery frame depends on what is installed rather than on a fixture.
        "autoeq" => Some((
            Tool::AutoEq,
            Box::new(AutoEqPanel::searching(shared.clone(), "sennheiser")) as Box<dyn Screen>,
        )),
        // Beta4 tools whose panels have not landed: the fixture reports none
        // of their features, so each shows what the device lacks.
        "subharm" => Some((Tool::Subharm, pending_tool(&state, Tool::Subharm))),
        "tube" => Some((Tool::Tube, pending_tool(&state, Tool::Tube))),
        // The analyser draws from the shared engine, which the fixture fills
        // with a device's caps and a picture of the chosen outputs.
        "spectrum" => {
            let channels: Vec<u8> = raw
                .iter()
                .position(|a| a == "--channels")
                .and_then(|i| raw.get(i + 1))
                .map(|list| {
                    list.split(',')
                        .filter_map(|c| c.trim().parse::<u8>().ok())
                        .filter(|c| *c > 0)
                        .map(|c| c - 1)
                        .collect()
                })
                .unwrap_or_else(|| vec![0]);
            fixture::spectrum(&shared, &channels);
            let mode = if raw.iter().any(|a| a == "--both") {
                spectrum::Mode::Both
            } else if raw.iter().any(|a| a == "--bars") {
                spectrum::Mode::Bars
            } else {
                spectrum::Mode::Curves
            };
            Some((
                Tool::Spectrum,
                Box::new(SpectrumPanel::open(shared.clone(), &state).mode(mode)) as Box<dyn Screen>,
            ))
        }
        _ => None,
    };
    let (detail, selection): (Box<dyn Screen>, Selection) = match screen.as_str() {
        "overview" | "matrix" | "nodevice" => {
            (Box::new(Overview::new(shared.clone())), Selection::Overview)
        }
        "output" => (
            Box::new(OutputPage::new(0, shared.clone(), &state)),
            Selection::Output(0),
        ),
        // The output page with the limiter's settings open, as Enter on its
        // cell beside MUTE does.
        "limiter" => {
            use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
            let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
            let mut page = OutputPage::new(0, shared.clone(), &state);
            for _ in 0..4 {
                page.handle(key(KeyCode::Down), &state);
            }
            page.handle(key(KeyCode::Right), &state);
            page.handle(key(KeyCode::Enter), &state);
            (Box::new(page), Selection::Output(0))
        }
        _ => (
            Box::new(InputPage::new(0, shared.clone(), &state)),
            Selection::Input(0),
        ),
    };
    fixture::select(&mut model, &state, &theme, selection);
    if screen == "nodevice" {
        fixture::disconnect(&mut model, &mut state);
    }
    let mut shell = Shell::new(model, theme, detail);
    shell.focus = Focus::Screen;
    if let Some((tool, panel)) = tool {
        shell.open_tool(tool, panel);
    }
    if let Some(line) = &quick {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        shell.handle(key(KeyCode::Char(';')), &state);
        for c in line.chars() {
            shell.handle(key(KeyCode::Char(c)), &state);
        }
    }
    if let Some(name) = &settings {
        let page = SettingsScreen::page_from_name(name).unwrap_or_else(|| {
            eprintln!("unknown settings page {name}");
            std::process::exit(2);
        });
        let mut data = dspi_tui::settings::cs_model::demo::settings_data();
        data.rta = Some(spectrum::demo::caps());
        let mut s = SettingsScreen::new(&state, data, AppConfig::default()).open(page, &state);
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

    if ansi {
        print!(
            "{}",
            dspi_tui::render_frame_ansi(w, h, |area, buf| shell.draw(area, buf, &state))
        );
    } else {
        println!(
            "{}",
            dspi_tui::render_frame(w, h, |area, buf| shell.draw(area, buf, &state))
        );
    }
}
