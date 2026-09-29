//! USB backend, over `nusb`.
//!
//! `nusb` talks WinUSB, IOKit and Linux usbfs directly, with no libusb to
//! install. The firmware ships MS OS 2.0 descriptors, so Windows binds WinUSB to
//! the vendor interface automatically and no Zadig step is needed.
//!
//! All transfers go through a **claimed interface** rather than the device
//! handle: `nusb`'s device-level control transfers are unsupported on Windows,
//! and WinUSB additionally requires that the low byte of `wIndex` match the
//! claimed interface number. The protocol already requires `wIndex == 2`, so
//! these agree; this module simply never lets them disagree.

use std::time::Duration;

use dspi_proto::generated::usb as usb_gen;
use dspi_proto::{REQ_TYPE_IN, REQ_TYPE_OUT, USB_PID, USB_VID, VENDOR_INTERFACE};
use nusb::MaybeFuture;
use nusb::transfer::{Bulk, ControlIn, ControlOut, ControlType, In, Recipient, TransferError};

use crate::{
    DEFAULT_TIMEOUT, DeviceDescriptor, NotificationSource, Result, Transport, TransportError,
};

/// `VENDOR_EP_IN` (config.h): the notification endpoint. It is a bulk
/// endpoint despite the spec's name; an interrupt endpoint polled alongside
/// rapid EP0 traffic crashes the RP2xxx device controller.
const NOTIFY_EP: u8 = usb_gen::VENDOR_EP_IN as u8;
const NOTIFY_EP_SIZE: usize = usb_gen::VENDOR_EP_SIZE as usize;

/// The notification endpoint, opened on a clone of the claimed interface so
/// the control path and this reader can live on different threads.
pub struct UsbNotifications {
    endpoint: nusb::Endpoint<Bulk, In>,
}

impl NotificationSource for UsbNotifications {
    fn read(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let buf = self.endpoint.allocate(NOTIFY_EP_SIZE);
        let done = self.endpoint.transfer_blocking(buf, timeout);
        match done.status {
            Ok(()) => {
                let mut v = done.buffer.into_vec();
                v.truncate(done.actual_len);
                Ok(v)
            }
            Err(TransferError::Cancelled) => Ok(Vec::new()),
            Err(TransferError::Disconnected) => Err(TransportError::Disconnected),
            Err(TransferError::Stall) => Err(TransportError::Stalled { opcode: NOTIFY_EP }),
            Err(e) => Err(TransportError::Usb(e.to_string())),
        }
    }
}

/// The vendor interface number, as a `u8` for `claim_interface`.
const IFACE: u8 = VENDOR_INTERFACE as u8;

/// Every connected DSPi, in a stable order, identified by serial.
///
/// A device with no readable serial is skipped rather than guessed at: identity
/// is what per-device settings and reconnection are keyed on, so an ambiguous
/// device is worse than an absent one.
pub fn list_devices() -> Result<Vec<DeviceDescriptor>> {
    let devices = nusb::list_devices()
        .wait()
        .map_err(|e| TransportError::Usb(e.to_string()))?;

    let mut found: Vec<DeviceDescriptor> = devices
        .filter(|d| d.vendor_id() == USB_VID && d.product_id() == USB_PID)
        .filter_map(|d| {
            Some(DeviceDescriptor {
                serial: d.serial_number()?.to_string(),
                bus_id: d.bus_id().to_string(),
                address: d.device_address(),
            })
        })
        .collect();

    found.sort_by(|a, b| a.serial.cmp(&b.serial));
    Ok(found)
}

pub struct UsbTransport {
    interface: nusb::Interface,
    descriptor: DeviceDescriptor,
    timeout: Duration,
}

impl UsbTransport {
    /// Open the only connected DSPi, or fail if there is not exactly one.
    ///
    /// With several attached, the caller must choose: silently picking one would
    /// mean a script could reconfigure the wrong unit.
    pub fn open_only() -> Result<Self> {
        let mut devices = list_devices()?;
        match devices.len() {
            0 => Err(TransportError::NotFound),
            1 => Self::open_serial(&devices.remove(0).serial),
            _ => Err(TransportError::Usb(format!(
                "{} DSPi devices connected; select one with --device <serial>",
                devices.len()
            ))),
        }
    }

    pub fn open_serial(serial: &str) -> Result<Self> {
        let devices = nusb::list_devices()
            .wait()
            .map_err(|e| TransportError::Usb(e.to_string()))?;

        let info = devices
            .filter(|d| d.vendor_id() == USB_VID && d.product_id() == USB_PID)
            .find(|d| d.serial_number() == Some(serial))
            .ok_or_else(|| TransportError::SerialNotFound(serial.to_string()))?;

        let descriptor = DeviceDescriptor {
            serial: serial.to_string(),
            bus_id: info.bus_id().to_string(),
            address: info.device_address(),
        };

        let device = info.open().wait().map_err(map_open_error)?;
        let interface = device
            .claim_interface(IFACE)
            .wait()
            .map_err(map_open_error)?;

        Ok(Self {
            interface,
            descriptor,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Transport for UsbTransport {
    fn control_in(&mut self, opcode: u8, value: u16, len: u16) -> Result<Vec<u8>> {
        debug_assert_eq!(REQ_TYPE_IN, 0xC1, "IN is vendor|interface|device-to-host");

        let data = self
            .interface
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Interface,
                    request: opcode,
                    value,
                    // Never a parameter. The protocol reserves wIndex for the
                    // interface number, and WinUSB enforces the same thing.
                    index: VENDOR_INTERFACE,
                    length: len,
                },
                self.timeout,
            )
            .wait()
            .map_err(|e| map_transfer_error(e, opcode))?;

        // A short read means the device answered a different question than the
        // one we asked. Treating it as success would decode garbage.
        if data.len() < len as usize {
            return Err(TransportError::ShortRead {
                opcode,
                wanted: len as usize,
                got: data.len(),
            });
        }

        Ok(data)
    }

    fn control_in_upto(&mut self, opcode: u8, value: u16, max_len: u16) -> Result<Vec<u8>> {
        self.interface
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Interface,
                    request: opcode,
                    value,
                    index: VENDOR_INTERFACE,
                    length: max_len,
                },
                self.timeout,
            )
            .wait()
            .map_err(|e| map_transfer_error(e, opcode))
    }

    fn control_out(&mut self, opcode: u8, value: u16, data: &[u8]) -> Result<()> {
        debug_assert_eq!(REQ_TYPE_OUT, 0x41, "OUT is vendor|interface|host-to-device");

        self.interface
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Interface,
                    request: opcode,
                    value,
                    index: VENDOR_INTERFACE,
                    data,
                },
                self.timeout,
            )
            .wait()
            .map_err(|e| map_transfer_error(e, opcode))?;

        Ok(())
    }

    fn descriptor(&self) -> &DeviceDescriptor {
        &self.descriptor
    }

    fn notifications(&self) -> Option<Box<dyn NotificationSource>> {
        let endpoint = self
            .interface
            .clone()
            .endpoint::<Bulk, In>(NOTIFY_EP)
            .ok()?;
        Some(Box::new(UsbNotifications { endpoint }))
    }

    fn max_transfer(&self) -> usize {
        // WinUSB caps a single control transfer at 4 KB. The other backends
        // manage larger, but the bulk packet exceeds 4 KB everywhere, so the
        // chunked path is used unconditionally rather than only on Windows.
        // One code path gets exercised on every platform, which is the point.
        4096
    }
}

fn map_open_error(e: nusb::Error) -> TransportError {
    let msg = e.to_string();
    let lower = msg.to_lowercase();
    if lower.contains("permission") || lower.contains("access") || lower.contains("denied") {
        TransportError::PermissionDenied
    } else {
        // Keep the underlying text: "could not open" covers a dozen distinct
        // causes and discarding the detail makes them indistinguishable.
        TransportError::Usb(msg)
    }
}

fn map_transfer_error(e: TransferError, opcode: u8) -> TransportError {
    match e {
        // A stall is the firmware refusing the request: an unknown opcode, a bad
        // index, an oversized payload, or a handler that returned failure. It is
        // also what a transfer hitting a flash blackout looks like, which is why
        // the retry helper treats it as possibly transient.
        TransferError::Stall => TransportError::Stalled { opcode },
        TransferError::Disconnected => TransportError::Disconnected,
        TransferError::Cancelled => TransportError::TimedOut { opcode, retries: 0 },
        other => TransportError::Usb(other.to_string()),
    }
}
