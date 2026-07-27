//! Render a representative frame without a device, for design review.
//!
//! `dspi screenshot` needs hardware and the vendor interface is exclusive, so
//! this exists to look at a layout while the interface itself is running.

use dspi_proto::dsp;
use dspi_tui::app::{App, ChannelView, Focus, Panel};
use dspi_tui::theme::{ColorDepth, Glyphs, Palette, Theme};

fn main() {
    let palette = std::env::args()
        .nth(1)
        .and_then(|a| Palette::parse(&a))
        .unwrap_or(Palette::Amber);

    // Detect rather than assume truecolor. Hardcoding it meant this previewed a
    // palette the person running it could not actually see: Apple Terminal is
    // 256-colour only, and that branch went untuned for exactly that reason.
    let depth = ColorDepth::detect();
    eprintln!("rendering at {depth:?}");
    let mut a = App::new(Theme::new(palette, depth, Glyphs::Braille));
    a.platform = "RP2350".into();
    a.firmware = "1.1.5".into();
    a.preset = "Preset 3".into();
    a.ctx.num_inputs = 8;
    a.ctx.num_outputs = 9;
    a.cpu = (14, 2);

    let tuning = [
        (dspi_proto::FilterType::LowShelf, 105.0, 0.707, 8.8),
        (dspi_proto::FilterType::Peaking, 64.0, 0.30, -6.2),
        (dspi_proto::FilterType::Peaking, 2856.0, 3.58, -8.6),
        (dspi_proto::FilterType::Peaking, 1880.0, 1.69, 3.6),
        (dspi_proto::FilterType::Peaking, 6749.0, 4.74, 4.0),
    ];

    a.channels = (0..17)
        .map(|i| {
            let is_output = i >= 8;
            let bands: Vec<dsp::Band> = if i < 2 {
                tuning
                    .iter()
                    .map(|(t, f, q, g)| dsp::Band {
                        filter_type: *t,
                        freq: *f,
                        q: *q,
                        gain_db: *g,
                        bypass: false,
                    })
                    .collect()
            } else {
                vec![dsp::Band::default(); 5]
            };
            ChannelView {
                name: if is_output {
                    let n = i - 8;
                    if n == 8 {
                        "PDM".into()
                    } else {
                        format!("SPDIF {} {}", n / 2 + 1, if n % 2 == 0 { "L" } else { "R" })
                    }
                } else {
                    format!("USB {}", i + 1)
                },
                slug: format!("ch.{i}"),
                is_output,
                curve: dsp::curve(&bands, 0.0),
                bands,
                peak: match i {
                    0 => 0.62,
                    1 => 0.48,
                    8 | 9 => 0.31,
                    16 => 0.72,
                    _ => 0.0,
                },
                clipped: i == 16,
            }
        })
        .collect();
    a.visible = vec![true; 17];

    a.panel = Panel::Filters;
    a.focus = Focus::Content;
    a.selected_band = 2;

    // Colours are the point of a theme review, so emit them.
    // Size and selection from the command line, so a short window and a
    // scrolled list can be eyeballed without a device attached.
    let arg = |n: usize, default: u16| -> u16 {
        std::env::args()
            .nth(n)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let (w, h) = (arg(2, 104), arg(3, 28));
    if let Some(ch) = std::env::args()
        .nth(4)
        .and_then(|v| v.parse::<usize>().ok())
    {
        a.selected_channel = ch.min(a.channels.len().saturating_sub(1));
        a.panel = dspi_tui::app::Panel::Filters;
    }
    print!("{}", dspi_tui::render_to_ansi(&a, w, h));
}
