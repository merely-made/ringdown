//! Bluetooth transport for [`antinode`].
//!
//! This is the I/O half of the sans-io split: it owns the GATT connection and
//! the timing, and defers every question about what the bytes mean to the
//! protocol core.
//!
//! ```no_run
//! # async fn run() -> Result<(), antinode_ble::TransportError> {
//! use antinode::rpc::{Method, params};
//!
//! let found = antinode_ble::discover(std::time::Duration::from_secs(10)).await?;
//! let mut guitar = antinode_ble::Guitar::connect(&found[0]).await?;
//! println!("{:?}", guitar.banner().await?);
//! println!("{:?}", guitar.status().await?);
//! # Ok(())
//! # }
//! ```

use std::time::Duration;

use antinode::{
    handshake::Banner,
    llt::{self, Ack, LltCode},
    llt2,
    rpc::{self, Method, RequestIds, Response, Status},
};
use btleplug::api::{
    Central, CentralEvent, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use futures::{Stream, StreamExt};
use serde_json::Value;
use uuid::Uuid;

/// How long to wait for the device to answer one request.
///
/// Matches the vendor client's own timeout, which is the only evidence we have
/// about what the device considers a reasonable wait.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait for a single LLT frame to be acknowledged.
const ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// How many times to attempt a connection before giving up.
const CONNECT_ATTEMPTS: u32 = 3;

/// How long to wait between connection attempts.
const CONNECT_BACKOFF: Duration = Duration::from_millis(800);

/// What can go wrong talking to the instrument.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The Bluetooth stack itself failed.
    #[error("bluetooth error: {0}")]
    Bluetooth(#[from] btleplug::Error),

    /// No Bluetooth adapter is available.
    #[error("no bluetooth adapter found")]
    NoAdapter,

    /// Scanning finished without seeing a guitar.
    #[error("no guitar found while scanning for {0:?}")]
    NotFound(Duration),

    /// The peripheral connected but does not expose the expected GATT surface.
    #[error("connected device is missing the {0} characteristic")]
    MissingCharacteristic(&'static str),

    /// The device did not answer in time.
    ///
    /// Carries everything that *did* arrive while waiting. Without this a
    /// reply the client failed to recognise is indistinguishable from silence,
    /// and those two failures have opposite fixes.
    #[error("{}", timeout_message(.waited, .heard))]
    Timeout {
        /// How long was spent waiting.
        waited: Duration,
        /// Every notification received while waiting, as lossy UTF-8.
        heard: Vec<String>,
    },

    /// The device rejected a chunk of a split message.
    #[error("device rejected frame {frame} of a split message: {code:?}")]
    ChunkRejected {
        /// Which frame was rejected.
        frame: u32,
        /// The status the device returned.
        code: LltCode,
    },

    /// The protocol core refused to frame a message.
    #[error("framing failed: {0}")]
    Framing(#[from] llt::LltError),

    /// The compressed transport refused to prepare a message.
    #[error("LLT2 framing failed: {0}")]
    Framing2(#[from] llt2::Llt2Error),

    /// The RPC layer failed, including errors reported by the device.
    #[error("rpc failed: {0}")]
    Rpc(#[from] rpc::RpcError),

    /// The connect-time version banner could not be parsed.
    #[error("device sent an unrecognised version banner: {0:?}")]
    BadBanner(String),
}

/// Why a scanned device was taken to be a guitar.
///
/// Worth surfacing rather than collapsing to a boolean: a device matched only
/// by name is a weaker identification than one advertising the service, and a
/// caller staring at a failed connection deserves to know which it had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchedBy {
    /// The advertisement carried the guitar service UUID. Strongest signal.
    AdvertisedService,
    /// The advertised name looks like a HyVibe. Weaker, but many BLE devices
    /// never advertise their service UUIDs, so this is not a fallback to be
    /// embarrassed about.
    Name,
}

/// A guitar seen while scanning.
#[derive(Clone)]
pub struct Found {
    peripheral: Peripheral,
    /// The advertised local name, when the device provided one.
    pub name: Option<String>,
    /// The peripheral's address, as the platform reports it.
    pub address: String,
    /// How this device was identified.
    pub matched_by: MatchedBy,
}

impl std::fmt::Debug for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Found")
            .field("name", &self.name)
            .field("address", &self.address)
            .field("matched_by", &self.matched_by)
            .finish()
    }
}

/// Whether an advertised name looks like a HyVibe system.
///
/// The vendor's System Menu calls this the "BT ID" and the manual's example is
/// `H2-SE614`, so the model prefix plus a unit suffix is the shape to expect.
/// The bare product name is also accepted, since older units and other models
/// may not use the `H2-` prefix.
fn name_looks_like_a_guitar(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower.starts_with("h2-") || lower.contains("hyvibe") || lower.starts_with("lag")
}

fn service_uuid() -> Uuid {
    // The constants are strings in the core so it can stay dependency-light;
    // they are fixed and valid, so a parse failure here would be a build-time
    // typo rather than a runtime condition.
    Uuid::parse_str(antinode::GUITAR_SERVICE).expect("service uuid constant is malformed")
}

fn request_uuid() -> Uuid {
    Uuid::parse_str(antinode::GUITAR_CHARACTERISTIC_REQUEST)
        .expect("request characteristic uuid constant is malformed")
}

fn response_uuid() -> Uuid {
    Uuid::parse_str(antinode::GUITAR_CHARACTERISTIC_RESPONSE)
        .expect("response characteristic uuid constant is malformed")
}

/// Scan for guitars.
///
/// Scans for the whole `timeout` rather than returning on the first hit, so a
/// caller with more than one instrument in range sees all of them.
///
/// # Why the scan is unfiltered
///
/// The obvious implementation asks the adapter to filter on the guitar's
/// service UUID. That is wrong here, and wrong in a way that would be easy to
/// misread: a great many BLE peripherals do not put their service UUIDs in the
/// advertisement at all, exposing them only on service discovery after a
/// connection. Filtering on the service would then return nothing, and
/// "no guitar found" would look like evidence against the recovered protocol
/// map when it was only evidence about advertising behaviour.
///
/// So the scan is unfiltered and the matching happens here, on either the
/// service UUID or the advertised name, with [`Found::matched_by`] recording
/// which. A device is better identified imperfectly than missed silently.
pub async fn discover(timeout: Duration) -> Result<Vec<Found>, TransportError> {
    let manager = Manager::new().await?;
    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or(TransportError::NoAdapter)?;

    adapter.start_scan(ScanFilter::default()).await?;

    // Watch scan events rather than sleeping blind, so a caller sees devices as
    // the adapter reports them.
    let mut events = adapter.events().await?;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut seen = Vec::new();

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, events.next()).await {
            Ok(Some(CentralEvent::DeviceDiscovered(id)))
            | Ok(Some(CentralEvent::DeviceUpdated(id))) => {
                if seen.iter().any(|f: &Found| f.peripheral.id() == id) {
                    continue;
                }
                if let Ok(peripheral) = adapter.peripheral(&id).await {
                    let props = peripheral.properties().await.ok().flatten();
                    let name = props.as_ref().and_then(|p| p.local_name.clone());

                    let matched_by = if props
                        .as_ref()
                        .map(|p| p.services.contains(&service_uuid()))
                        .unwrap_or(false)
                    {
                        Some(MatchedBy::AdvertisedService)
                    } else if name.as_deref().is_some_and(name_looks_like_a_guitar) {
                        Some(MatchedBy::Name)
                    } else {
                        None
                    };

                    if let Some(matched_by) = matched_by {
                        seen.push(Found {
                            name,
                            address: props
                                .as_ref()
                                .map(|p| p.address.to_string())
                                .unwrap_or_default(),
                            peripheral,
                            matched_by,
                        });
                    }
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => break,
        }
    }

    let _ = adapter.stop_scan().await;

    if seen.is_empty() {
        return Err(TransportError::NotFound(timeout));
    }
    Ok(seen)
}

/// A connected guitar.
pub struct Guitar {
    peripheral: Peripheral,
    request: Characteristic,
    response: Characteristic,
    notifications: std::pin::Pin<Box<dyn Stream<Item = btleplug::api::ValueNotification> + Send>>,
    ids: RequestIds,
    write_len: usize,
    request_timeout: Duration,
    trace: bool,
    transport: Transport,
}

/// Which of the two transports this instrument speaks.
///
/// Chosen from the firmware versions in the connect-time banner, exactly as
/// the vendor's client chooses: both processors at 1.2.2 or newer means LLT2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// JSON messages in JSON frames.
    Llt,
    /// Compressed messages in binary frames.
    Llt2,
}

impl Guitar {
    /// Connect, set up notifications, and learn the usable write length.
    ///
    /// Follows the connect order the vendor's client uses: discover, subscribe,
    /// then read the version banner. The one step it cannot follow is
    /// requesting an MTU — see [`Guitar::write_len`].
    pub async fn connect(found: &Found) -> Result<Guitar, TransportError> {
        Guitar::connect_with_retries(found, CONNECT_ATTEMPTS).await
    }

    /// Connect, retrying transient failures.
    ///
    /// A BLE connect fails for reasons that have nothing to do with the peer
    /// being wrong: the platform hands out a peripheral handle that has gone
    /// stale since the scan, the device is mid-advertisement-interval, or the
    /// adapter is still tearing down a previous session. "Not connected"
    /// arriving from `connect` itself is the usual shape. One attempt turns
    /// those into a failed run and an investigation of the wrong thing.
    pub async fn connect_with_retries(
        found: &Found,
        attempts: u32,
    ) -> Result<Guitar, TransportError> {
        let mut last = None;
        for attempt in 1..=attempts.max(1) {
            match Guitar::connect_once(found).await {
                Ok(guitar) => return Ok(guitar),
                Err(e) => {
                    if attempt < attempts {
                        eprintln!(
                            "      connect attempt {attempt} failed ({e}); retrying in {:?}",
                            CONNECT_BACKOFF
                        );
                        tokio::time::sleep(CONNECT_BACKOFF).await;
                    }
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or(TransportError::NoAdapter))
    }

    async fn connect_once(found: &Found) -> Result<Guitar, TransportError> {
        let peripheral = found.peripheral.clone();
        if !peripheral.is_connected().await? {
            peripheral.connect().await?;
        }
        peripheral.discover_services().await?;

        let characteristics = peripheral.characteristics();
        let request = characteristics
            .iter()
            .find(|c| c.uuid == request_uuid())
            .cloned()
            .ok_or(TransportError::MissingCharacteristic("request"))?;
        let response = characteristics
            .iter()
            .find(|c| c.uuid == response_uuid())
            .cloned()
            .ok_or(TransportError::MissingCharacteristic("response"))?;

        // Subscribe before anything is sent, so no reply can be missed.
        peripheral.subscribe(&response).await?;
        let notifications = peripheral.notifications().await?;

        // Read the banner here rather than leaving it to the caller, because
        // the firmware versions in it decide which transport to speak, and
        // that has to be settled before the first message goes out. A banner
        // that will not parse leaves the older transport in force: small
        // messages are wire-identical either way, so the conservative choice
        // costs nothing until a large message needs sending.
        let transport = match peripheral.read(&response).await {
            Ok(raw) => Banner::parse(&String::from_utf8_lossy(&raw))
                .filter(|b| llt2::selects_llt2(b.stm, b.esp))
                .map(|_| Transport::Llt2)
                .unwrap_or(Transport::Llt),
            Err(_) => Transport::Llt,
        };

        Ok(Guitar {
            write_len: ASSUMED_WRITE_LEN,
            request_timeout: REQUEST_TIMEOUT,
            trace: false,
            transport,
            peripheral,
            request,
            response,
            notifications,
            ids: RequestIds::new(),
        })
    }

    /// The write length in use. See [`ASSUMED_WRITE_LEN`] for why it is
    /// assumed rather than negotiated.
    pub fn write_len(&self) -> usize {
        self.write_len
    }

    /// Print every notification received to stderr as it arrives.
    ///
    /// The client only surfaces messages it recognises, so when a request goes
    /// unanswered this is what distinguishes a device that said nothing from a
    /// device that said something unexpected.
    pub fn set_trace(&mut self, on: bool) {
        self.trace = on;
    }

    /// Subscribe and print traffic for a while without sending anything.
    ///
    /// Answers the narrower question of whether the device ever emits a
    /// notification unprompted, which separates "notifications are not working"
    /// from "the request was not understood".
    pub async fn listen(&mut self, duration: Duration) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + duration;
        let mut heard = Vec::new();
        while let Some(bytes) = self.next_notification(deadline).await {
            heard.push(render(&bytes));
        }
        heard
    }

    /// Write a raw message to the request characteristic, bypassing framing.
    ///
    /// For probing only: it is how an alternative encoding gets tested without
    /// the rest of the stack insisting on the recovered one.
    pub async fn write_raw(&self, bytes: &[u8], with_response: bool) -> Result<(), TransportError> {
        let kind = if with_response {
            WriteType::WithResponse
        } else {
            WriteType::WithoutResponse
        };
        self.peripheral.write(&self.request, bytes, kind).await?;
        Ok(())
    }

    /// How long to wait for a reply.
    ///
    /// The default matches the vendor client's, but that is evidence about
    /// what the app tolerates rather than about how slow the device can be: a
    /// large read may have to gather state from flash and compress it before
    /// it can answer at all.
    pub fn set_request_timeout(&mut self, timeout: Duration) {
        self.request_timeout = timeout;
    }

    /// Which transport this connection is using.
    pub fn transport(&self) -> Transport {
        self.transport
    }

    /// Force a transport, overriding what the banner implied.
    ///
    /// The version rule is read from the vendor's client rather than stated by
    /// the device, so being able to contradict it is what makes it testable.
    pub fn set_transport(&mut self, transport: Transport) {
        self.transport = transport;
    }

    /// Override the write length.
    ///
    /// Present because the negotiated MTU is not always discoverable, and
    /// finding the largest value the device actually accepts may be empirical.
    pub fn set_write_len(&mut self, len: usize) {
        self.write_len = len;
    }

    /// Read the connect-time version banner.
    ///
    /// This is a GATT *read* of the response characteristic, not a
    /// notification, and it happens before any RPC.
    pub async fn banner(&self) -> Result<Banner, TransportError> {
        let raw = self.peripheral.read(&self.response).await?;
        let text = String::from_utf8_lossy(&raw).into_owned();
        Banner::parse(&text).ok_or(TransportError::BadBanner(text))
    }

    /// Call a method and return its result.
    pub async fn call(&mut self, method: Method, params: Value) -> Result<Value, TransportError> {
        self.call_named(method.wire_name(), params).await
    }

    /// Call a method by its wire name, including one antinode has no
    /// [`Method`] variant for.
    ///
    /// The compressor's keyword dictionary names methods the vendor's own app
    /// never calls, and the only way to learn whether they are callable, and
    /// what they want, is to ask the instrument. This is how.
    pub async fn call_named(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, TransportError> {
        let id = self.ids.next_id();
        let encoded = serde_json::to_string(&serde_json::json!({
            "jsonrpc": antinode::rpc::JSONRPC_VERSION,
            "id": id,
            "method": method,
            "params": params,
        }))
        .map_err(|e| rpc::RpcError::Encode(e.to_string()))?;

        match self.transport {
            Transport::Llt => self.send_llt(&encoded, id).await?,
            Transport::Llt2 => self.send_llt2(&encoded, id).await?,
        }

        let response = self.await_response(id).await?;
        Ok(response.into_result()?)
    }

    /// Send via the older transport: JSON frames, acknowledged as JSON.
    async fn send_llt(&mut self, encoded: &str, id: i64) -> Result<(), TransportError> {
        let outbound = llt::frame_message(encoded, id, self.write_len)?;
        let chunked = outbound.is_chunked();
        let frames: Vec<Vec<u8>> = outbound
            .frames()
            .iter()
            .map(|f| f.as_bytes().to_vec())
            .collect();

        for (index, frame) in frames.iter().enumerate() {
            self.write_frame(frame).await?;
            // Only split messages are acknowledged frame by frame; an unsplit
            // one is answered directly by its RPC reply.
            if chunked {
                let frame_no = (index + 1) as u32;
                let ack = self.await_llt_ack(id, frame_no).await?;
                if !ack.code.is_continue() && !ack.code.is_terminal_success() {
                    return Err(TransportError::ChunkRejected {
                        frame: frame_no,
                        code: ack.code,
                    });
                }
            }
        }
        Ok(())
    }

    /// Send via LLT2: compressed, in binary frames acknowledged as six bytes.
    async fn send_llt2(&mut self, encoded: &str, id: i64) -> Result<(), TransportError> {
        let object_id = id as u8;
        let outbound = llt2::prepare(encoded, object_id, self.write_len)?;
        let framed = outbound.is_framed();
        let frames = outbound.frames().to_vec();

        for (index, frame) in frames.iter().enumerate() {
            self.write_frame(frame).await?;
            if framed {
                let frame_no = (index + 1) as u16;
                let ack = self.await_llt2_ack(object_id, frame_no).await?;
                if !ack.code.is_continue() && !ack.code.is_terminal_success() {
                    return Err(TransportError::ChunkRejected {
                        frame: u32::from(frame_no),
                        code: ack.code,
                    });
                }
            }
        }
        Ok(())
    }

    async fn write_frame(&self, bytes: &[u8]) -> Result<(), TransportError> {
        if self.trace {
            eprintln!("      -> {} bytes", bytes.len());
        }
        self.peripheral
            .write(&self.request, bytes, WriteType::WithResponse)
            .await?;
        Ok(())
    }

    /// Drain any notifications that arrive within `window` after a call.
    ///
    /// A reply is not necessarily the whole answer. `call` returns on the first
    /// message matching the request id and stops listening, so a device that
    /// acknowledges first and sends data afterwards would have its data
    /// discarded, the same shape of mistake that made a working `GetStatus`
    /// look like silence. This is how to check rather than assume.
    pub async fn drain(&mut self, window: Duration) -> Vec<String> {
        self.listen(window).await
    }

    /// Read the instrument's status.
    ///
    /// This is the control run that matters: if it answers, the recovered
    /// protocol map is confirmed against hardware.
    pub async fn status(&mut self) -> Result<Status, TransportError> {
        let value = self.call(Method::GetStatus, rpc::params::none()).await?;
        Ok(serde_json::from_value(value).map_err(|e| rpc::RpcError::Decode(e.to_string()))?)
    }

    /// Read the full configuration, including the live effect catalog.
    pub async fn read_config(&mut self) -> Result<Value, TransportError> {
        self.call(Method::ReadConfig, rpc::params::none()).await
    }

    /// Disconnect cleanly.
    pub async fn disconnect(&self) -> Result<(), TransportError> {
        self.peripheral.disconnect().await?;
        Ok(())
    }

    async fn await_llt_ack(&mut self, object_id: i64, frame: u32) -> Result<Ack, TransportError> {
        let deadline = tokio::time::Instant::now() + ACK_TIMEOUT;
        let mut heard = Vec::new();
        loop {
            let Some(bytes) = self.next_notification(deadline).await else {
                return Err(TransportError::Timeout {
                    waited: ACK_TIMEOUT,
                    heard,
                });
            };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if let Some(ack) = Ack::parse(&text) {
                // Acks for other transfers, or for frames already past, are
                // noise rather than errors.
                if ack.object_id == object_id && ack.message_id == frame {
                    return Ok(ack);
                }
            }
            heard.push(render(&bytes));
        }
    }

    async fn await_llt2_ack(
        &mut self,
        object_id: u8,
        frame: u16,
    ) -> Result<llt2::Ack2, TransportError> {
        let deadline = tokio::time::Instant::now() + ACK_TIMEOUT;
        let mut heard = Vec::new();
        loop {
            let Some(bytes) = self.next_notification(deadline).await else {
                return Err(TransportError::Timeout {
                    waited: ACK_TIMEOUT,
                    heard,
                });
            };
            if let Some(ack) = llt2::Ack2::parse(&bytes)
                && ack.answers(object_id, frame)
            {
                return Ok(ack);
            }
            heard.push(render(&bytes));
        }
    }

    async fn await_response(&mut self, id: i64) -> Result<Response, TransportError> {
        let deadline = tokio::time::Instant::now() + self.request_timeout;
        let mut heard = Vec::new();
        loop {
            let Some(bytes) = self.next_notification(deadline).await else {
                return Err(TransportError::Timeout {
                    waited: self.request_timeout,
                    heard,
                });
            };

            // A reply may arrive compressed or as plain JSON. Try decompressing
            // first: the start-nibble check makes that a cheap, unambiguous
            // test rather than a guess, and plain JSON simply fails it.
            let candidate = match antinode::compress::decode(&bytes) {
                Some(json) => Some(json),
                None => core::str::from_utf8(&bytes).ok().map(String::from),
            };

            if let Some(text) = candidate {
                // The response characteristic multiplexes acknowledgements with
                // replies, so anything that parses as an ack is not our answer.
                if Ack::parse(&text).is_none()
                    && let Ok(response) = Response::decode(&text)
                    && response.answers(id)
                {
                    return Ok(response);
                }
            }
            heard.push(render(&bytes));
        }
    }

    /// The next notification's raw bytes, or `None` once `deadline` passes.
    ///
    /// Deliberately **not** decoded to text here. A compressed reply is binary,
    /// and a lossy UTF-8 conversion would replace every byte outside ASCII with
    /// a replacement character, destroying the payload before anything had a
    /// chance to decompress it. Every notification passes through this one
    /// place so that tracing sees the traffic as it actually arrived.
    async fn next_notification(&mut self, deadline: tokio::time::Instant) -> Option<Vec<u8>> {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let note = tokio::time::timeout(remaining, self.notifications.next())
            .await
            .ok()
            .flatten()?;
        if self.trace {
            eprintln!(
                "      <- {} bytes: {}",
                note.value.len(),
                render(&note.value)
            );
        }
        Some(note.value)
    }
}

/// Present a notification readably, decompressing it when it is compressed.
///
/// Used for tracing and for the evidence a timeout carries, so that a
/// compressed frame reads as JSON rather than as a wall of replacement
/// characters.
fn render(bytes: &[u8]) -> String {
    if let Some(json) = antinode::compress::decode(bytes) {
        return format!("[compressed] {json}");
    }
    match core::str::from_utf8(bytes) {
        Ok(text) => format!("{text:?}"),
        Err(_) => {
            let hex: Vec<String> = bytes.iter().take(32).map(|b| format!("{b:02x}")).collect();
            format!(
                "[binary] {}{}",
                hex.join(" "),
                if bytes.len() > 32 { " ..." } else { "" }
            )
        }
    }
}

/// Render a timeout together with whatever was overheard.
///
/// The difference between "nothing arrived" and "something arrived that we
/// failed to recognise" is the entire diagnosis, and those two have opposite
/// fixes, so the error carries the evidence rather than discarding it.
fn timeout_message(waited: &Duration, heard: &[String]) -> String {
    if heard.is_empty() {
        format!("timed out after {waited:?}; the device sent nothing at all")
    } else {
        format!(
            "timed out after {waited:?}; the device sent {} message(s), none of which was the              expected reply: {heard:?}",
            heard.len()
        )
    }
}

/// The write length assumed when the platform will not say what it negotiated.
///
/// btleplug 0.11 exposes no MTU accessor at all — not to request one, not even
/// to read what the platform agreed (deviceplug/btleplug#246 is still open). So
/// this cannot be discovered the way the vendor's client discovers it, and a
/// number has to be chosen.
///
/// The choice is 514, matching what the vendor's client gets after asking for
/// an MTU of 517, on the reasoning that the device is built to accept it and
/// modern platform stacks negotiate high MTUs unprompted. That is an inference,
/// not a measurement, and it is the assumption most likely to be wrong on this
/// page.
///
/// The two failure modes are not symmetric, which is why the optimistic value
/// wins: assuming the 20-byte floor would make every message fail, since 20 is
/// too small to carry even one LLT frame, whereas assuming too much fails
/// visibly on the write that overruns. Neither corrupts anything. If writes are
/// rejected, lower it with [`Guitar::set_write_len`] until they are not — and
/// record the value that worked.
pub const ASSUMED_WRITE_LEN: usize = 514;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uuid_constants_parse() {
        // These are `expect`ed at runtime, so a typo must fail a test rather
        // than a user's connection.
        let _ = service_uuid();
        let _ = request_uuid();
        let _ = response_uuid();
    }

    #[test]
    fn the_uuids_are_distinct() {
        assert_ne!(request_uuid(), response_uuid());
        assert_ne!(service_uuid(), request_uuid());
    }

    #[test]
    fn the_manuals_bt_id_example_is_recognised() {
        // The H2 manual's System Menu screenshot shows "BT ID: H2-SE614".
        assert!(name_looks_like_a_guitar("H2-SE614"));
        assert!(name_looks_like_a_guitar("h2-se614"));
        assert!(name_looks_like_a_guitar("  H2-ABC123  "));
    }

    #[test]
    fn other_product_spellings_are_recognised() {
        assert!(name_looks_like_a_guitar("HyVibe"));
        assert!(name_looks_like_a_guitar("LAG HyVibe"));
        assert!(name_looks_like_a_guitar("Lag-Guitar"));
    }

    #[test]
    fn unrelated_devices_are_not_matched() {
        for name in ["AirPods", "Galaxy Buds", "MX Master 3", "", "Bose QC45"] {
            assert!(
                !name_looks_like_a_guitar(name),
                "{name} should not look like a guitar"
            );
        }
    }
}
