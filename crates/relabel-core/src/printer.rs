//! Working out what is on the other end of the link.
//!
//! Every read here is allowed to fail on its own. Several models answer some of
//! these with "not supported", one answers the status query with a payload too
//! short to carry a version, and one answers a different command id than it
//! should. Identification therefore degrades rather than aborting: a printer
//! that will not say what it is can still print, using the conservative
//! fallback flow.

use crate::error::SessionError;
use crate::job::Timeouts;
use crate::session::Session;
use relabel_proto::{
    ModelSpec, PrintFlow,
    models::{self},
    parse::{self, ConnectResult, StatusData},
    payload::{self, HeartbeatKind, IDENTITY_READS, InfoField},
};

/// What the printer said about itself.
#[derive(Debug, Clone, Default)]
pub struct Identity {
    /// 0 to 5. Zero when the printer never answered, which is a working
    /// assumption rather than a fact.
    pub protocol: u8,
    /// True when the protocol level is a fallback rather than something the
    /// printer told us.
    pub protocol_assumed: bool,
    pub model_id: Option<u16>,
    /// The best match. `None` for a model this build's registry does not know.
    pub model: Option<&'static ModelSpec>,
    /// Every model that reports this id. More than one is not an error: a few
    /// ids name two different printers, and they differ in print head width, so
    /// the choice is offered rather than guessed.
    pub candidates: Vec<&'static ModelSpec>,
    pub serial: Option<String>,
    pub software: Option<String>,
    pub hardware: Option<String>,
    pub battery_percent: Option<u8>,
    pub density: Option<u8>,
    pub label_type: Option<u8>,
    pub auto_shutdown: Option<u8>,
    /// Present only when the printer answered the status query in full.
    pub status: Option<StatusData>,
    /// Reads that came back unsupported or timed out, for the device pane to
    /// explain rather than silently omit.
    pub missing: Vec<&'static str>,
}

impl Identity {
    /// Which print flow this printer wants.
    pub fn flow(&self) -> PrintFlow {
        relabel_proto::select_flow(self.model_id, self.protocol)
    }

    /// The print head width in dots, or zero when it is genuinely unknown.
    ///
    /// Zero is deliberate. It forces the image encoder onto its safe branch and
    /// makes "we do not know" visible, where a plausible guess would silently
    /// produce a truncated label.
    pub fn printhead_px(&self) -> u16 {
        self.model.map(|m| m.printhead_px).unwrap_or(0)
    }

    pub fn name(&self) -> &str {
        self.model.map(|m| m.name).unwrap_or("Unknown printer")
    }

    /// Whether the tag read is expected to fail on this firmware.
    ///
    /// The manufacturer's own registry lists, per model, the firmware versions
    /// where it does not work. Saying "no tag" on one of those would blame the
    /// roll for a firmware problem.
    pub fn rfid_broken(&self) -> bool {
        match (self.model, self.software.as_deref()) {
            (Some(m), Some(fw)) => m.rfid_broken_on(fw),
            _ => false,
        }
    }
}

/// One request the handshake sends, for saying what a connect is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeStep {
    Connect,
    /// Only sent to a printer that answered Connect at level 2 or better.
    Status,
    /// The `n`th of the identity reads, in [`IDENTITY_READS`] order, from 1.
    Info {
        field: InfoField,
        n: u8,
    },
    Heartbeat,
}

impl HandshakeStep {
    /// What relabel is doing at this step, to follow "while relabel was" or
    /// "Connecting to D110_M-GB28010920: ".
    pub fn says(&self) -> String {
        match self {
            HandshakeStep::Connect => "asking it to connect".into(),
            HandshakeStep::Status => "asking which protocol it speaks".into(),
            HandshakeStep::Info { field, n } => format!(
                "reading its {} ({n} of {})",
                field_name(*field),
                IDENTITY_READS.len()
            ),
            HandshakeStep::Heartbeat => "checking the link".into(),
        }
    }
}

/// Connect, negotiate the protocol level, and read identity.
///
/// Never fails on an individual read. The only hard error is a link that has
/// gone away: a transport failure, or a session that has already ended, at any
/// step, the heartbeat included.
pub async fn handshake(session: &Session, t: &Timeouts) -> Result<Identity, SessionError> {
    handshake_with(session, t, |_| {}).await
}

/// [`handshake`], calling `on_step` immediately before each request it sends.
///
/// A step is reported only when its request is actually sent, so a printer
/// below level 2 never reports [`HandshakeStep::Status`]. When the handshake
/// fails, the last step reported is the one the link died on.
pub async fn handshake_with(
    session: &Session,
    t: &Timeouts,
    mut on_step: impl FnMut(HandshakeStep),
) -> Result<Identity, SessionError> {
    let mut id = Identity::default();

    // 1. Connect. A missing reply is not fatal - several implementations never
    //    send this command at all and print successfully - but it is the only
    //    route to a protocol level, so it is worth asking.
    on_step(HandshakeStep::Connect);
    match session.request(payload::connect(), t.default).await {
        Ok(f) => match ConnectResult::parse(&f.data) {
            Ok(r) => match r.protocol_version() {
                Some(v) => id.protocol = v,
                // `ConnectedV3` means "level 2 or better", and **2 is the
                // baseline**, not 3. The status query below may raise it.
                //
                // The distinction is not academic. A D110 answers that query
                // with a payload too short to carry a version, so the upgrade
                // never happens and whatever is assumed here is what the job
                // engine uses - and 2 against 3 is the difference between the
                // two-byte and the one-byte `PrintStart`.
                None => {
                    id.protocol = 2;
                    id.protocol_assumed = true;
                }
            },
            Err(_) => id.missing.push("connect result"),
        },
        Err(e) if link_gone(&e) => return Err(e),
        Err(_) => {
            id.protocol_assumed = true;
            id.missing.push("connect");
        }
    }

    // 2. Try to raise the baseline to an exact level.
    if id.protocol >= 2 {
        on_step(HandshakeStep::Status);
        match session
            .request(payload::printer_status_data(), t.default)
            .await
        {
            Ok(f) => match StatusData::parse(&f.data) {
                Ok(s) => {
                    if let Some(v) = s.protocol_version {
                        id.protocol = v;
                        id.protocol_assumed = false;
                    } else {
                        // A payload too short to carry one. Known behaviour on
                        // at least one model; keep the level we have.
                        id.missing.push("protocol level");
                        id.protocol_assumed = true;
                    }
                    id.status = Some(s);
                }
                Err(_) => id.missing.push("printer status data"),
            },
            Err(e) if link_gone(&e) => return Err(e),
            Err(_) => {
                id.missing.push("printer status data");
                id.protocol_assumed = true;
            }
        }
    }

    // 3. Identity, model id first because everything downstream depends on it.
    for (n, field) in (1u8..).zip(IDENTITY_READS) {
        on_step(HandshakeStep::Info { field: *field, n });
        let reply = match session
            .request(payload::printer_info(*field), t.settings)
            .await
        {
            Ok(f) => f,
            Err(e) if link_gone(&e) => return Err(e),
            Err(_) => {
                id.missing.push(field_name(*field));
                continue;
            }
        };
        let d = &reply.data;
        match field {
            InfoField::ModelId => {
                if let Ok(m) = parse::model_id(d) {
                    id.model_id = Some(m);
                    id.candidates = models::by_wire_id(m).collect();
                    id.model = id.candidates.first().copied();
                }
            }
            InfoField::SerialNumber => id.serial = parse::serial_number(d),
            InfoField::SoftwareVersion => id.software = parse::version(d),
            InfoField::HardwareVersion => id.hardware = parse::version(d),
            InfoField::BatteryChargeLevel => id.battery_percent = parse::battery_percent(d),
            InfoField::Density => id.density = d.first().copied(),
            InfoField::LabelType => id.label_type = d.first().copied(),
            InfoField::AutoShutdownTime => id.auto_shutdown = d.first().copied(),
            _ => {}
        }
    }

    // 4. One heartbeat. Cheap, and it is the first thing that would fail if the
    //    link were only half up. A printer that declines it, or does not
    //    answer, is only missing it; a link that is gone here is gone.
    let kind = HeartbeatKind::for_protocol(id.protocol);
    on_step(HandshakeStep::Heartbeat);
    match session.request(payload::heartbeat(kind), t.default).await {
        Ok(_) => {}
        Err(e) if link_gone(&e) => return Err(e),
        Err(_) => id.missing.push("heartbeat"),
    }

    Ok(id)
}

/// Whether a failed request means the link itself is gone, rather than the
/// printer declining or not answering one request. A session whose loop has
/// ended answers every request with `Ended` at once, so without this a link
/// that went between two reads would be reported as a printer that declined
/// every read after it.
fn link_gone(e: &SessionError) -> bool {
    matches!(e, SessionError::Transport(_) | SessionError::Ended)
}

/// A printer property in words, for the list of reads that failed and for
/// [`HandshakeStep::says`].
pub fn field_name(f: InfoField) -> &'static str {
    match f {
        InfoField::ModelId => "model id",
        InfoField::SerialNumber => "serial number",
        InfoField::SoftwareVersion => "firmware version",
        InfoField::HardwareVersion => "hardware version",
        InfoField::BatteryChargeLevel => "battery level",
        InfoField::Density => "density",
        InfoField::LabelType => "label type",
        InfoField::AutoShutdownTime => "auto-shutdown time",
        _ => "a printer property",
    }
}
