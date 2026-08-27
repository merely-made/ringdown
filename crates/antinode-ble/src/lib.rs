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
    rpc::{self, Method, Request, RequestIds, Response, Status},
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
    #[error("timed out after {0:?} waiting for the device")]
    Timeout(Duration),

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

    /// The RPC layer failed, including errors reported by the device.
    #[error("rpc failed: {0}")]
    Rpc(#[from] rpc::RpcError),

    /// The connect-time version banner could not be parsed.
    #[error("device sent an unrecognised version banner: {0:?}")]
    BadBanner(String),
}

/// A guitar seen while scanning.
#[derive(Clone)]
pub struct Found {
    peripheral: Peripheral,
    /// The advertised local name, when the device provided one.
    pub name: Option<String>,
    /// The peripheral's address, as the platform reports it.
    pub address: String,
}

impl std::fmt::Debug for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Found")
            .field("name", &self.name)
            .field("address", &self.address)
            .finish()
    }
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

/// Scan for guitars advertising the expected service.
///
/// Scans for the whole `timeout` rather than returning on the first hit, so a
/// caller with more than one instrument in range sees all of them.
pub async fn discover(timeout: Duration) -> Result<Vec<Found>, TransportError> {
    let manager = Manager::new().await?;
    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or(TransportError::NoAdapter)?;

    adapter
        .start_scan(ScanFilter {
            services: vec![service_uuid()],
        })
        .await?;

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
                    let advertises = props
                        .as_ref()
                        .map(|p| p.services.contains(&service_uuid()))
                        .unwrap_or(false);
                    if advertises {
                        seen.push(Found {
                            name: props.as_ref().and_then(|p| p.local_name.clone()),
                            address: props
                                .as_ref()
                                .map(|p| p.address.to_string())
                                .unwrap_or_default(),
                            peripheral,
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
}

impl Guitar {
    /// Connect, set up notifications, and learn the usable write length.
    ///
    /// Follows the connect order the vendor's client uses: discover, subscribe,
    /// then read the version banner. The one step it cannot follow is
    /// requesting an MTU — see [`Guitar::write_len`].
    pub async fn connect(found: &Found) -> Result<Guitar, TransportError> {
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

        Ok(Guitar {
            write_len: ASSUMED_WRITE_LEN,
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
        let id = self.ids.next_id();
        let request = Request::new(id, method, params);
        let encoded = request.encode()?;

        let outbound = llt::frame_message(&encoded, id, self.write_len)?;
        let chunked = outbound.is_chunked();
        let frames = outbound.frames().to_vec();

        for (index, frame) in frames.iter().enumerate() {
            self.peripheral
                .write(&self.request, frame.as_bytes(), WriteType::WithResponse)
                .await?;

            // Only split messages are acknowledged frame by frame; an unsplit
            // one is answered directly by its RPC reply.
            if chunked {
                let frame_no = (index + 1) as u32;
                let ack = self.await_ack(id, frame_no).await?;
                if !ack.code.is_continue() && !ack.code.is_terminal_success() {
                    return Err(TransportError::ChunkRejected {
                        frame: frame_no,
                        code: ack.code,
                    });
                }
            }
        }

        let response = self.await_response(id).await?;
        Ok(response.into_result()?)
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

    async fn await_ack(&mut self, object_id: i64, frame: u32) -> Result<Ack, TransportError> {
        let deadline = tokio::time::Instant::now() + ACK_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(TransportError::Timeout(ACK_TIMEOUT));
            }
            let Ok(Some(note)) = tokio::time::timeout(remaining, self.notifications.next()).await
            else {
                return Err(TransportError::Timeout(ACK_TIMEOUT));
            };
            let text = String::from_utf8_lossy(&note.value);
            if let Some(ack) = Ack::parse(&text) {
                // Acks for other transfers, or for frames already past, are
                // noise rather than errors.
                if ack.object_id == object_id && ack.message_id == frame {
                    return Ok(ack);
                }
            }
        }
    }

    async fn await_response(&mut self, id: i64) -> Result<Response, TransportError> {
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(TransportError::Timeout(REQUEST_TIMEOUT));
            }
            let Ok(Some(note)) = tokio::time::timeout(remaining, self.notifications.next()).await
            else {
                return Err(TransportError::Timeout(REQUEST_TIMEOUT));
            };
            let text = String::from_utf8_lossy(&note.value);

            // The response characteristic multiplexes acknowledgements with
            // replies, so anything that parses as an ack is not our answer.
            if Ack::parse(&text).is_some() {
                continue;
            }
            if let Ok(response) = Response::decode(&text)
                && response.answers(id)
            {
                return Ok(response);
            }
        }
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
}
