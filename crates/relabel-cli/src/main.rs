//! Bring-up driver for NIIMBOT printers.
//!
//! Not a shipped binary. This is how the protocol stack gets pointed at real
//! hardware before there is a GUI, and how the questions that only a printer can
//! answer get answered: whether the counter bytes matter, whether pacing is
//! needed, whether a model silently caps a multi-copy job at one page.
//!
//! Every subcommand that talks to a printer accepts `--trace`, which dumps the
//! frames in both directions afterwards. That output is the single most useful
//! thing to attach to a bug report.

mod bitmap;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use relabel_core::job::{PrintOptions, Timeouts, print};
use relabel_core::printer::Identity;
use relabel_core::transport::ble::{self, BleTransport};
use relabel_core::{Session, handshake};
use relabel_proto::{
    Frame, HeadSpec, LabelType, PrintDirection, encode_page,
    image::CountsMode,
    parse::{self, TagRead},
    payload,
};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "relabel-cli", about = "Bring-up driver for NIIMBOT printers")]
struct Cli {
    /// Match a printer by name or id substring. Without it, the first printer
    /// that looks like one is used.
    #[arg(long, global = true)]
    device: Option<String>,

    /// How long to scan for.
    #[arg(long, global = true, default_value = "4")]
    scan_secs: u64,

    /// Dump the protocol log when the command finishes.
    #[arg(long, global = true)]
    trace: bool,

    /// Milliseconds to wait between writes. Zero disables pacing.
    #[arg(long, global = true)]
    pace_ms: Option<u64>,

    #[command(subcommand)]
    cmd: Cmd,
}

/// Mirrors [`PrintDirection`] so it can be a command-line value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Dir {
    Top,
    Left,
}

impl From<Dir> for PrintDirection {
    fn from(d: Dir) -> PrintDirection {
        match d {
            Dir::Top => PrintDirection::Top,
            Dir::Left => PrintDirection::Left,
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// List Bluetooth devices in range.
    Scan {
        /// Show everything, not just devices whose name looks like a printer.
        #[arg(long)]
        all: bool,
    },
    /// Connect and report what the printer says about itself.
    Info,
    /// Read the label tag.
    Rfid,
    /// Poll the print status once.
    Status,
    /// Encode an image and print it.
    Print {
        /// The image to print: PNG, JPEG, BMP or GIF.
        file: PathBuf,
        #[arg(long, default_value = "1")]
        copies: u16,
        /// Defaults to the model's own default.
        #[arg(long)]
        density: Option<u8>,
        #[arg(long, value_enum, default_value = "atkinson")]
        dither: bitmap::Dither,
        #[arg(long, default_value = "128")]
        threshold: u8,
        #[arg(long)]
        invert: bool,
        /// Encode and report, without connecting to anything.
        #[arg(long)]
        dry_run: bool,
        /// Assume this print head width for a dry run, in dots.
        #[arg(long, default_value = "96")]
        head: u16,
        /// Which axis lies across the head, for a dry run. `left` is what the
        /// D11/D110 family uses; a real print takes this from the model.
        #[arg(long, value_enum, default_value = "left")]
        direction: Dir,
        /// Send the three row counter bytes as zero, to test whether firmware
        /// checks them.
        #[arg(long)]
        zero_counters: bool,
        /// Print a text preview of the 1-bit result.
        #[arg(long)]
        preview: bool,
    },
    /// Print a millimetre ruler, to measure a label whose size is unknown.
    ///
    /// The printer never reports how long its labels are, so the label measures
    /// itself: the scale is rendered longer than the label can be, and wherever
    /// the paper runs out is the answer. Tick height encodes the scale and every
    /// centimetre is counted out in squares, so it reads without numerals.
    Ruler {
        /// How long a scale to render. Make it longer than the label.
        #[arg(long, default_value = "60")]
        length_mm: u16,
        /// Render and preview without printing.
        #[arg(long)]
        dry_run: bool,
        /// Assume this head width for a dry run, in dots.
        #[arg(long, default_value = "96")]
        head: u16,
        /// Which axis lies across the head, for a dry run.
        #[arg(long, value_enum, default_value = "left")]
        direction: Dir,
        #[arg(long)]
        preview: bool,
    },
    /// Send a frame by hand: command byte, then payload, as hex.
    Raw {
        /// Command byte first, then the payload. For example `40 08` asks for
        /// the model id. Spaces are optional.
        hex: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let t = Timeouts::default();

    // The one command that needs no printer at all.
    if let Cmd::Print {
        dry_run: true,
        file,
        head,
        direction,
        dither,
        threshold,
        invert,
        zero_counters,
        preview,
        ..
    } = &cli.cmd
    {
        return dry_run(
            file,
            &ImageOpts {
                head: *head,
                direction: (*direction).into(),
                dither: *dither,
                threshold: *threshold,
                invert: *invert,
                zero_counters: *zero_counters,
                preview: *preview,
            },
        );
    }

    if let Cmd::Ruler {
        dry_run: true,
        length_mm,
        head,
        direction,
        preview,
    } = &cli.cmd
    {
        let dir: PrintDirection = (*direction).into();
        let plane = build_ruler(*head, *length_mm, 203, dir)?;
        if *preview {
            println!("{}", bitmap::preview(&plane, 150));
        }
        let page = encode_page(&plane, &HeadSpec::new(*head, dir))?;
        println!(
            "{}x{} dots, {} frames, {} bytes on the wire.",
            page.cols,
            page.rows,
            page.frames.len(),
            page.wire_bytes()
        );
        return Ok(());
    }

    let adapter = ble::adapter().await?;

    if let Cmd::Scan { all } = cli.cmd {
        let found = ble::scan(&adapter, Duration::from_secs(cli.scan_secs)).await?;
        if found.is_empty() {
            println!("Nothing found. Is the printer switched on and not already connected?");
            return Ok(());
        }
        for d in &found {
            if !all && !d.likely_printer {
                continue;
            }
            let rssi = d.rssi.map(|r| format!("{r} dBm")).unwrap_or_default();
            let mark = if d.likely_printer { "*" } else { " " };
            println!(
                "{mark} {:<28} {:<10} {:?}",
                d.name.as_deref().unwrap_or("(no name)"),
                rssi,
                d.id
            );
        }
        if !all && !found.iter().any(|d| d.likely_printer) {
            println!("(no device looked like a printer; try --all)");
        }
        return Ok(());
    }

    // Everything else needs a connection.
    let found = ble::scan(&adapter, Duration::from_secs(cli.scan_secs)).await?;
    let want = cli.device.as_deref();
    let chosen = found
        .iter()
        .find(|d| match want {
            Some(w) => {
                let hay = format!("{} {:?}", d.name.as_deref().unwrap_or(""), d.id);
                hay.to_lowercase().contains(&w.to_lowercase())
            }
            None => d.likely_printer,
        })
        .ok_or_else(|| match want {
            Some(w) => anyhow::anyhow!("no device matched {w:?}. Try `relabel-cli scan --all`"),
            None => anyhow::anyhow!("no printer found. Try `relabel-cli scan --all`"),
        })?;

    println!(
        "Connecting to {}...",
        chosen.name.as_deref().unwrap_or("(no name)")
    );
    let peripheral = ble::find(&adapter, &chosen.id).await?;
    let mut transport = BleTransport::connect(peripheral).await?;
    if let Some(ms) = cli.pace_ms {
        transport.set_pace(Duration::from_millis(ms));
    }
    let mtu = transport.mtu();
    let session = Session::start(Box::new(transport));

    let id = handshake(&session, &t).await?;
    report_identity(&id, mtu);

    let result = run(&cli, &session, &id, &t).await;

    if cli.trace {
        println!("\n--- protocol log ---");
        for e in session.trace().await {
            let dir = if e.outbound { "->" } else { "<-" };
            let hex: String = e.bytes.iter().map(|b| format!("{b:02x}")).collect();
            println!("{:>6}ms {dir} {:<22} {hex}", e.at_ms, e.name);
        }
    }
    result
}

async fn run(cli: &Cli, session: &Session, id: &Identity, t: &Timeouts) -> Result<()> {
    match &cli.cmd {
        Cmd::Scan { .. } | Cmd::Print { dry_run: true, .. } | Cmd::Ruler { dry_run: true, .. } => {
            unreachable!("handled earlier")
        }

        Cmd::Ruler {
            length_mm, preview, ..
        } => {
            let model = id.model;
            let head = id.printhead_px();
            if head == 0 {
                bail!("this printer's head width is unknown, so a ruler cannot be sized for it");
            }
            let dir = model.map(|m| m.direction).unwrap_or(PrintDirection::Top);
            let dpi = model.map(|m| m.dpi).unwrap_or(203);
            let plane = build_ruler(head, *length_mm, dpi, dir)?;
            if *preview {
                println!("{}", bitmap::preview(&plane, 150));
            }
            let page = encode_page(&plane, &HeadSpec::new(head, dir))?;
            println!(
                "Printing a {length_mm} mm scale: {}x{} dots, {} frames.",
                page.cols,
                page.rows,
                page.frames.len()
            );
            println!("Read off where the paper runs out - that is the label length.");
            let opts = PrintOptions {
                density: model.map(|m| m.density.2).unwrap_or(2),
                label_type: model
                    .and_then(|m| m.label_types.first().copied())
                    .unwrap_or(LabelType::Gap),
                ..PrintOptions::default()
            }
            .for_model(id.model_id);
            print(session, id.flow(), &[page], &opts, |_| {}).await?;
            println!("Done.");
            Ok(())
        }

        Cmd::Info => Ok(()),

        Cmd::Rfid => {
            match session
                .request(payload::rfid_info(), Duration::from_millis(600))
                .await
            {
                Ok(f) => match parse::RfidTag::parse(&f.data)? {
                    TagRead::NoTag => println!("No tag in the printer."),
                    TagRead::Tag(tag) => {
                        println!("  uuid       {}", tag.uuid_hex());
                        println!("  barcode    {}", tag.barcode);
                        println!("  serial     {}", tag.serial);
                        println!("  used       {} of {}", tag.used, tag.total);
                        println!("  type       {}", tag.consumable_type);
                        if let Some(c) = tag.capacity {
                            println!("  capacity   {c}");
                        }
                    }
                },
                Err(e) => {
                    // Silence is a normal third state here, not a failure.
                    println!("No tag information ({e}).");
                    if id.rfid_broken() {
                        println!(
                            "This model's registry lists firmware {} as one where the tag read \
                             does not work, so this is the firmware rather than the roll.",
                            id.software.as_deref().unwrap_or("?")
                        );
                    }
                }
            }
            Ok(())
        }

        Cmd::Status => {
            let f = session.request(payload::print_status(), t.status).await?;
            let s = parse::PrintStatus::parse(&f.data)?;
            println!("  page       {}", s.page);
            println!("  print      {}%", s.print_progress);
            println!("  feed       {}%", s.feed_progress);
            println!("  undecoded  {:02x?}", s.undocumented);
            if let Some(e) = s.error {
                println!("  error      {e}");
            }
            Ok(())
        }

        Cmd::Raw { hex } => {
            let bytes = parse_hex(hex)?;
            let (id_byte, data) = bytes
                .split_first()
                .context("give at least a command byte")?;
            let frame = Frame::new(*id_byte, data);
            match session.request(frame, t.default).await {
                Ok(r) => println!(
                    "<- {:02x} {}",
                    r.id,
                    r.data
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                ),
                Err(e) => println!("no reply: {e}"),
            }
            Ok(())
        }

        Cmd::Print {
            file,
            copies,
            density,
            dither,
            threshold,
            invert,
            zero_counters,
            preview,
            ..
        } => {
            let model = id.model;
            let head = id.printhead_px();
            let direction = model.map(|m| m.direction).unwrap_or(PrintDirection::Top);
            let plane = bitmap::load(file, head, direction, *dither, *threshold, *invert)?;
            if *preview {
                println!("{}", bitmap::preview(&plane, 120));
            }

            let mut spec = HeadSpec::new(head, direction);
            if *zero_counters {
                spec.counts = CountsMode::ForceZero;
            }
            let page = encode_page(&plane, &spec)?;
            println!(
                "Encoding {}x{} dots into {} frames, {} bytes.",
                page.cols,
                page.rows,
                page.frames.len(),
                page.wire_bytes()
            );

            let opts = PrintOptions {
                density: density.or(model.map(|m| m.density.2)).unwrap_or(2),
                label_type: model
                    .and_then(|m| m.label_types.first().copied())
                    .unwrap_or(LabelType::Gap),
                copies: *copies,
                ..PrintOptions::default()
            }
            .for_model(id.model_id);
            if opts.copies_per_job == Some(1) && *copies > 1 {
                println!(
                    "This model prints one label per job, so {copies} copies will go as \
                     {copies} separate jobs."
                );
            }
            println!(
                "Printing at density {} on {}, flow {:?}.",
                opts.density,
                opts.label_type.label(),
                id.flow()
            );

            let mut last = 0;
            print(session, id.flow(), &[page], &opts, |p| {
                if p.page != last {
                    last = p.page;
                    println!("  page {} of {}", p.page, p.total);
                }
            })
            .await?;
            println!("Done.");
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// Render the calibration ruler to a 1-bit plane.
///
/// The scale is deliberately longer than the label: the whole point is that the
/// paper runs out part way through it.
fn build_ruler(
    head: u16,
    length_mm: u16,
    dpi: u16,
    dir: PrintDirection,
) -> Result<relabel_proto::BitPlane> {
    let length_px =
        relabel_render::Dpmm::for_dpi(dpi).dots_from_tenths_mm(i32::from(length_mm) * 10);
    let target = relabel_render::Target::for_label(head, length_px as u16, dpi, dir)?;
    let grey = relabel_render::calibration::ruler(&target, dir == PrintDirection::Left)?;
    Ok(grey.threshold(128))
}

/// How an image becomes dots. Bundled because these travel together everywhere,
/// and an eight-argument call is an eight-argument bug waiting to happen.
struct ImageOpts {
    head: u16,
    direction: PrintDirection,
    dither: bitmap::Dither,
    threshold: u8,
    invert: bool,
    zero_counters: bool,
    preview: bool,
}

fn dry_run(file: &std::path::Path, o: &ImageOpts) -> Result<()> {
    let plane = bitmap::load(file, o.head, o.direction, o.dither, o.threshold, o.invert)?;
    if o.preview {
        println!("{}", bitmap::preview(&plane, 120));
    }
    let (head, direction) = (o.head, o.direction);
    let mut spec = HeadSpec::new(head, direction);
    if o.zero_counters {
        spec.counts = CountsMode::ForceZero;
    }
    let page = encode_page(&plane, &spec)?;
    println!(
        "{}x{} dots, {} frames, {} bytes on the wire.",
        page.cols,
        page.rows,
        page.frames.len(),
        page.wire_bytes()
    );
    // Throughput is dominated by the write count times the pace, not by bytes.
    let secs = page.frames.len() as f64 * 0.012;
    println!("Roughly {secs:.1}s of row streaming at a 10 ms pace.");
    println!("(dry run: assumed a {head} dot head, direction {direction:?})");
    Ok(())
}

fn report_identity(id: &Identity, mtu: usize) {
    println!("  model      {}", id.name());
    if id.candidates.len() > 1 {
        let others: Vec<&str> = id.candidates.iter().skip(1).map(|m| m.name).collect();
        println!("             (this id also names {})", others.join(", "));
    }
    if let Some(m) = id.model_id {
        println!("  wire id    {m}");
    }
    println!(
        "  protocol   {}{}",
        id.protocol,
        if id.protocol_assumed {
            " (assumed)"
        } else {
            ""
        }
    );
    println!("  flow       {:?}", id.flow());
    if let Some(m) = id.model {
        println!("  head       {} dots at {} dpi", m.printhead_px, m.dpi);
        println!(
            "  density    {}..{}, default {}",
            m.density.0, m.density.1, m.density.2
        );
    }
    if let Some(s) = &id.serial {
        println!("  serial     {s}");
    }
    if let Some(s) = &id.software {
        println!("  firmware   {s}");
    }
    if let Some(s) = &id.hardware {
        println!("  hardware   {s}");
    }
    if let Some(b) = id.battery_percent {
        println!("  battery    about {b}%");
    }
    println!("  mtu        {mtu}");
    if let Some(st) = &id.status {
        // Surfaced rather than acted on: the exact meaning of these is not
        // confirmed, and clamping writes to a field we have guessed at would
        // break models this has never run against.
        println!(
            "  reported   packet<={} send<={} interval={}ms colour={}",
            st.single_packet_size, st.max_send_size, st.packet_interval, st.supports_color
        );
    }
    if !id.missing.is_empty() {
        println!("  unanswered {}", id.missing.join(", "));
    }
}

fn parse_hex(parts: &[String]) -> Result<Vec<u8>> {
    let joined: String = parts
        .join("")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if !joined.len().is_multiple_of(2) {
        bail!("hex needs an even number of digits");
    }
    (0..joined.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&joined[i..i + 2], 16)
                .with_context(|| format!("bad hex at offset {i}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_hex;

    #[test]
    fn hex_accepts_spaced_and_joined_forms() {
        let want = vec![0x40u8, 0x08];
        assert_eq!(parse_hex(&["40".into(), "08".into()]).unwrap(), want);
        assert_eq!(parse_hex(&["4008".into()]).unwrap(), want);
        assert_eq!(parse_hex(&["40 08".into()]).unwrap(), want);
    }

    #[test]
    fn hex_rejects_an_odd_digit_count() {
        assert!(parse_hex(&["400".into()]).is_err());
        assert!(parse_hex(&["zz".into()]).is_err());
    }
}
