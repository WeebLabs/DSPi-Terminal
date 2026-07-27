//! Where does startup time actually go, and does the fast path agree with the
//! slow one? Reads only; the device is untouched.
//!
//! `cargo run --release -p dspi-session --example timing`
use dspi_transport::Transport;
use std::time::Instant;

fn main() {
    let t0 = Instant::now();
    let mut tr = dspi_transport::usb::UsbTransport::open_only().expect("no device");
    println!("open                {:>9.1?}", t0.elapsed());

    let t = Instant::now();
    let caps = dspi_session::probe::probe(&mut tr).expect("probe");
    println!("probe (incl. bulk)  {:>9.1?}", t.elapsed());
    println!(
        "bands               {:>9}  live, {} slots on the wire",
        caps.max_bands, caps.band_storage
    );

    // The band probe deliberately provokes stalls. If they left the endpoint
    // halted, everything after it would fail.
    let t = Instant::now();
    for _ in 0..100 {
        tr.control_in(dspi_proto::generated::opcodes::REQ_GET_PLATFORM, 0, 4)
            .expect("endpoint unusable after the band probe");
    }
    println!(
        "one control_in      {:>9.1?}  (mean of 100)",
        t.elapsed() / 100
    );

    let n = caps.num_channels;
    let live = caps.max_bands;
    let mut s = dspi_session::Session::new(Box::new(tr), caps).expect("session");

    let t = Instant::now();
    let snap = s.snapshot().expect("snapshot");
    println!("snapshot            {:>9.1?}", t.elapsed());

    let t = Instant::now();
    let mut scalar = Vec::new();
    for ch in 0..n {
        for band in 0..live {
            scalar.push((ch, band, s.read_band(ch, band)));
        }
    }
    println!(
        "read_band x{:<8}{:>9.1?}",
        n as u32 * live as u32,
        t.elapsed()
    );

    // The point of the exercise: the fast path must report the same device.
    let (mut ok, mut bad, mut unread) = (0, 0, 0);
    for (ch, band, got) in &scalar {
        let Ok(want) = got else {
            unread += 1;
            println!(
                "  unreadable: ch{ch} band{band}  ({:?})",
                got.as_ref().err()
            );
            continue;
        };
        let have = snap.band(*ch, *band).expect("band in range");
        if have == *want {
            ok += 1;
        } else {
            bad += 1;
            if bad <= 6 {
                println!("  ch{ch} band{band}\n    scalar {want:?}\n    bulk   {have:?}");
            }
        }
    }
    println!("bands agree         {ok:>9}  ({bad} differ, {unread} unreadable)");
    println!("---                 {:>9.1?}", t0.elapsed());
}
