//! Request/reply transactions over the headset's vendor HID collection.
//!
//! Only opening a session enumerates devices. Every write passes the read /
//! live / save allowlist, or the settings allowlist, with explicit write
//! capabilities. HID calls are
//! synchronous: the timeout bounds polling but cannot interrupt a stalled
//! `HidD_GetInputReport`, so the app runs sessions in a worker process.

use crate::balance::Settings;
use crate::controls::Controls;
use crate::device::Device;
use crate::json::object;
use crate::protocol::{
    InputStream, battery_command, check_packet, control_matches, firmware_command, frame, live_command, parse_battery,
    parse_coefficients, parse_control, parse_firmware, parse_index, parse_live_ack, parse_query_ack, parse_save_ack,
    parse_setting, parse_storage_space, parse_ui, read_coefficients_command, read_control_command, read_index_command,
    read_setting_command, read_ui_command, save_coefficients_command, storage_space_command, validate_control,
    validate_request, validate_setting_write, write_control_command, write_setting_command,
};
use crate::{Error, Result};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub const VENDOR_ID: u16 = 0x3329;
pub const USAGE_PAGE: u16 = 0xFF13;

pub const BUSY: &str = "The headset control interface is busy. Fully quit Audeze HQ (including its tray icon) and other Maxwell tools, then retry. No EQ was sent.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Usb,
    Dongle,
}

impl Transport {
    pub fn parse(name: &str) -> Option<Transport> {
        match name {
            "usb" => Some(Transport::Usb),
            "dongle" => Some(Transport::Dongle),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Transport::Usb => "usb",
            Transport::Dongle => "dongle",
        }
    }

    pub fn product_id(self) -> u16 {
        match self {
            Transport::Usb => 0x4B1A,
            Transport::Dongle => 0x4B19,
        }
    }
}

/// Raw report I/O on one opened collection.
pub trait ReportIo {
    /// One input report (report ID 7), `input_size` bytes long.
    fn input(&mut self) -> Result<Vec<u8>>;
    /// Write one output report; false when the write failed or was short.
    fn write(&mut self, report: &[u8]) -> Result<bool>;
}

#[derive(Debug, Clone)]
pub struct Interface {
    pub path: String,
    pub input_size: usize,
    pub output_size: usize,
    pub device_version: u16,
}

pub struct Session<T: ReportIo> {
    pub io: T,
    pub transport: Transport,
    pub interface: Interface,
    pub allow_live: bool,
    pub allow_persistent: bool,
    /// Channel gain and source-profile writes. Never set with the EQ permissions.
    pub allow_settings: bool,
    /// Control writes (chat mix, sidetone, preset index...). Never set with the EQ permissions.
    pub allow_controls: bool,
    /// Pause between polls; tests replace it.
    pub sleep: fn(Duration),
}

impl<T: ReportIo> Session<T> {
    pub fn new(io: T, transport: Transport, interface: Interface, allow_live: bool, allow_persistent: bool) -> Self {
        Session {
            io,
            transport,
            interface,
            allow_live,
            allow_persistent,
            allow_settings: false,
            allow_controls: false,
            sleep: std::thread::sleep,
        }
    }

    fn check(&self, packet: &[u8]) -> Result<()> {
        if self.allow_settings && validate_setting_write(packet).is_ok() {
            return Ok(());
        }
        if validate_control(packet, self.allow_controls).is_ok() {
            return Ok(());
        }
        validate_request(packet, self.allow_live, self.allow_persistent)
    }

    /// Validate, empty old input, and write every report of one request.
    fn send(&mut self, packet: &[u8]) -> Result<()> {
        self.send_flagged(packet, self.transport == Transport::Dongle)
    }

    /// [`Self::send`] with an explicit dongle flag: clearing it on the dongle
    /// addresses the dongle instead of the headset.
    fn send_flagged(&mut self, packet: &[u8], dongle: bool) -> Result<()> {
        self.check(packet)?;
        let reports = frame(packet, dongle, self.interface.output_size)?;
        // Empty old responses before writing. Failure to empty aborts the write.
        self.drain()?;
        for report in reports {
            if !self.io.write(&report)? {
                return Err(Error::other("Incomplete HID write; live state may be uncertain"));
            }
        }
        Ok(())
    }

    fn default_timeout(&self) -> f64 {
        if self.transport == Transport::Dongle { 6.0 } else { 3.0 }
    }

    /// The first reply to `packet` before `timeout` seconds pass. 0x0901
    /// replies must also echo the sub-command, so a late gain reply cannot
    /// answer a different read.
    fn reply(&mut self, packet: &[u8], timeout: f64) -> Result<Option<Vec<u8>>> {
        let (command, _) = check_packet(packet, 0x5A)?;
        let mut stream = InputStream::default();
        let deadline = Instant::now() + Duration::from_secs_f64(timeout);
        while Instant::now() < deadline {
            (self.sleep)(Duration::from_millis(10));
            for message in stream.feed(&self.io.input()?)? {
                if message[..2] != [0x05, 0x5B] || check_packet(&message, 0x5B)?.0 != command {
                    continue;
                }
                if command == 0x0901 && message.get(6) != packet.get(6) {
                    continue;
                }
                // 0x2C83 replies put a status first, then the sub-command.
                if command == 0x2C83 && message.get(7) != packet.get(6) {
                    continue;
                }
                return Ok(Some(message));
            }
        }
        Ok(None)
    }

    /// Send a query answered by an acknowledgement and a 0x5D notification.
    /// Both can arrive in one input report, in either order.
    fn query(&mut self, packet: &[u8], dongle: bool, timeout: f64) -> Result<Vec<u8>> {
        let (command, _) = check_packet(packet, 0x5A)?;
        self.send_flagged(packet, dongle)?;
        let mut stream = InputStream::default();
        let (mut ack, mut notice) = (None, None);
        let deadline = Instant::now() + Duration::from_secs_f64(timeout);
        while Instant::now() < deadline {
            (self.sleep)(Duration::from_millis(10));
            for message in stream.feed(&self.io.input()?)? {
                let kind = message[1];
                if !matches!(kind, 0x5B | 0x5D) || check_packet(&message, kind)?.0 != command {
                    continue;
                }
                if kind == 0x5B {
                    ack.get_or_insert(message);
                } else {
                    notice.get_or_insert(message);
                }
            }
            if let (Some(ack), Some(notice)) = (&ack, &notice) {
                parse_query_ack(ack, command)?;
                return Ok(notice.clone());
            }
        }
        match ack {
            Some(ack) => {
                parse_query_ack(&ack, command)?;
                Err(Error::other("The headset acknowledged the query but sent no answer"))
            }
            None => Err(Error::other("No answer to the query")),
        }
    }

    /// Reconnects can leave delayed fragments and notifications queued, and one
    /// empty poll does not show that an old stream has ended.
    pub fn drain(&mut self) -> Result<()> {
        let mut empty_polls = 0;
        for _ in 0..100 {
            let report = self.io.input()?;
            if report.len() < 2 || report[0] != 7 || report[1] > 59 {
                return Err(Error::value("Malformed report while draining input"));
            }
            empty_polls = if report[1] == 0 { empty_polls + 1 } else { 0 };
            if empty_polls >= 6 {
                return Ok(());
            }
            (self.sleep)(Duration::from_millis(10));
        }
        Err(Error::value("Input queue did not drain; no command sent"))
    }

    /// Send one allowed request and return its matching, parsed reply.
    pub fn transact(&mut self, packet: &[u8], timeout: Option<f64>) -> Result<Vec<u8>> {
        let timeout = timeout.unwrap_or(self.default_timeout());
        if !(0.1..=30.0).contains(&timeout) {
            return Err(Error::value("Timeout must be 0.1..30 seconds"));
        }
        let (command, _) = check_packet(packet, 0x5A)?;
        self.send(packet)?;
        let message = self
            .reply(packet, timeout)?
            .ok_or_else(|| Error::other("No matching command response; live state may be uncertain"))?;
        match command {
            0x0E03 => parse_live_ack(&message)?,
            0x0A03 => {
                parse_storage_space(&message)?;
            }
            0x0A0D => parse_save_ack(&message, (u16::from_le_bytes([packet[6], packet[7]]) - 0xE42C) as u8)?,
            0x0901 => {
                parse_setting(&message, packet[6])?;
            }
            0x0900 | 0x2C82 => {}
            0x2C83 => {
                parse_control(&message, packet[6])?;
            }
            _ if packet[7] == 0xEF => {
                parse_ui(&message, packet[6])?;
            }
            _ => {
                parse_coefficients(&message, packet[6] - 0x2C)?;
            }
        }
        Ok(message)
    }
}

/// Reads right after a write can return a stale value, so a setting counts
/// only when two reads in a row agree.
const READ_ROUNDS: usize = 6;
const VERIFY_ATTEMPTS: usize = 4;

impl<T: ReportIo> Settings for Session<T> {
    fn read_setting(&mut self, sub: u8) -> Result<u8> {
        let packet = read_setting_command(sub)?;
        let mut previous = None;
        for _ in 0..READ_ROUNDS {
            let value = parse_setting(&self.transact(&packet, None)?, sub)?;
            if previous == Some(value) {
                return Ok(value);
            }
            previous = Some(value);
            (self.sleep)(Duration::from_millis(120));
        }
        Err(Error::other("The headset kept returning different values. Try reading again."))
    }

    fn write_setting(&mut self, sub: u8, value: u8) -> Result<Option<Vec<u8>>> {
        let packet = write_setting_command(sub, value)?;
        if !self.allow_settings {
            return Err(Error::value("Settings writes are not enabled for this session"));
        }
        self.send(&packet)?;
        // The 0x0900 reply format is undocumented; keep it for diagnosis only.
        let reply = self.reply(&packet, 1.0)?;
        for attempt in 0..VERIFY_ATTEMPTS {
            (self.sleep)(Duration::from_millis(if attempt == 0 { 300 } else { 600 }));
            if self.read_setting(sub)? == value {
                return Ok(reply);
            }
        }
        Err(Error::other(format!(
            "The headset did not keep the new value (setting 0x{sub:02X}). Read again to see what it holds now."
        )))
    }
}

impl<T: ReportIo> Session<T> {
    /// A control value that two reads in a row agree on.
    fn stable_control(&mut self, sub: u8) -> Result<u16> {
        let packet = read_control_command(sub)?;
        let mut previous = None;
        for _ in 0..READ_ROUNDS {
            let value = parse_control(&self.transact(&packet, None)?, sub)?;
            if previous == Some(value) {
                return Ok(value);
            }
            previous = Some(value);
            (self.sleep)(Duration::from_millis(120));
        }
        Err(Error::other("The headset kept returning different values. Try reading again."))
    }
}

impl<T: ReportIo> Controls for Session<T> {
    fn read_control(&mut self, sub: u8) -> Result<u16> {
        parse_control(&self.transact(&read_control_command(sub)?, None)?, sub)
    }

    fn write_control(&mut self, sub: u8, value: u16) -> Result<Option<Vec<u8>>> {
        let packet = write_control_command(sub, value)?;
        if !self.allow_controls {
            return Err(Error::value("Control writes are not enabled for this session"));
        }
        self.send(&packet)?;
        // Write replies are kept for diagnosis only; the read-back decides.
        let reply = self.reply(&packet, 1.0)?;
        for attempt in 0..VERIFY_ATTEMPTS {
            (self.sleep)(Duration::from_millis(if attempt == 0 { 300 } else { 600 }));
            if control_matches(sub, value, self.stable_control(sub)?) {
                return Ok(reply);
            }
        }
        Err(Error::other(format!(
            "The headset did not keep the new value (control 0x{sub:02X}). Read again to see what it holds now."
        )))
    }

    fn battery(&mut self) -> Result<u8> {
        let dongle = self.transport == Transport::Dongle;
        parse_battery(&self.query(&battery_command(), dongle, 2.0)?)
    }

    fn firmware(&mut self, dongle: bool) -> Result<String> {
        if dongle && self.transport != Transport::Dongle {
            return Err(Error::value("The dongle's firmware can only be read through the dongle"));
        }
        // Flags 0x80 address the headset through the dongle; 0 addresses the dongle.
        let flag = self.transport == Transport::Dongle && !dongle;
        parse_firmware(&self.query(&firmware_command(), flag, 2.0)?)
    }
}

impl<T: ReportIo> Device for Session<T> {
    /// Read-only snapshot. UI data exists only for the four custom slots.
    fn read_state(&mut self) -> Result<Value> {
        let raw_index = self.transact(&read_index_command(), None)?;
        let index = parse_index(&raw_index)?;
        let slot = (7..=10).contains(&index).then(|| index - 7);
        let interface = &self.interface;
        let mut state = object([
            (
                "device",
                object([
                    ("vid", json!(VENDOR_ID)),
                    ("pid", json!(self.transport.product_id())),
                    ("usage_page", json!(USAGE_PAGE)),
                    ("transport", json!(self.transport.name())),
                    ("path", json!(interface.path)),
                    ("bcd_device", json!(interface.device_version)),
                    ("input_report_bytes", json!(interface.input_size)),
                    ("output_report_bytes", json!(interface.output_size)),
                ]),
            ),
            ("firmware", Value::Null),
            ("firmware_note", json!("Firmware command is outside this minimal allowlist")),
            ("index", json!(index)),
            ("slot", json!(slot)),
            ("raw_index_hex", json!(hex::encode(&raw_index))),
            ("bands", Value::Null),
            ("raw_ui_hex", Value::Null),
        ]);
        if let Some(slot) = slot {
            let preset = parse_ui(&self.transact(&read_ui_command(slot)?, None)?, slot)?;
            state["raw_ui_hex"] = json!(hex::encode(&preset.raw_payload));
            state["raw_ui_response_hex"] = json!(hex::encode(&preset.raw_response));
            state["ui_header_hex"] = json!(hex::encode(&preset.header));
            state["bands"] = Value::Array(preset.bands.iter().map(|band| band.to_json()).collect());
            let reply = self.transact(&read_coefficients_command(slot)?, None)?;
            state["saved_coefficients_hex"] = json!(hex::encode(parse_coefficients(&reply, slot)?));
            state["raw_coefficient_response_hex"] = json!(hex::encode(&reply));
            if parse_index(&self.transact(&read_index_command(), None)?)? != index {
                return Err(Error::value("Preset changed while taking the backup; retry read-only snapshot"));
            }
        }
        Ok(state)
    }

    fn has_storage_space(&mut self) -> Result<bool> {
        parse_storage_space(&self.transact(&storage_space_command(), None)?)
    }

    fn save_coefficients(&mut self, slot: u8, coefficients: &[u8]) -> Result<Value> {
        let ack = self.transact(&save_coefficients_command(slot, coefficients)?, Some(8.0))?;
        let actual = parse_coefficients(&self.transact(&read_coefficients_command(slot)?, None)?, slot)?;
        if actual != coefficients {
            return Err(Error::value(
                "Saved coefficient read-back differs from the requested profile. Restore the saved backup.",
            ));
        }
        Ok(object([
            ("save_ack_hex", json!(hex::encode(&ack))),
            ("saved_verified", json!(true)),
            ("persistence_write_sent", json!(true)),
        ]))
    }

    /// One live update; callers enforce a prior backup and filter validation.
    fn apply_live(&mut self, coefficients: &[u8]) -> Result<Value> {
        let ack = self.transact(&live_command(coefficients)?, None)?;
        Ok(object([
            ("accepted", json!(true)),
            ("ack_hex", json!(hex::encode(&ack))),
            ("persistence_write_sent", json!(false)),
        ]))
    }
}

#[cfg(windows)]
pub use windows::{WinHid, open, open_controls, open_exclusive, open_settings};

#[cfg(not(windows))]
pub use hidapi_backend::{open, open_controls, open_settings};

/// hidapi, for macOS. The `hidapi` feature builds it on Windows too, so it
/// can be checked there; Windows itself uses the native module below.
#[cfg(any(not(windows), feature = "hidapi"))]
pub mod hidapi_backend {
    use super::{BUSY, Interface, ReportIo, Session, Transport, USAGE_PAGE, VENDOR_ID};
    use crate::balance::Settings;
    use crate::controls::Controls;
    use crate::descriptor::report_sizes;
    use crate::device::Device;
    use crate::{Error, Result};
    use hidapi::{HidApi, HidDevice};

    /// An opened control collection. hidapi opens it exclusively on macOS.
    pub struct HidapiIo {
        device: HidDevice,
        input_size: usize,
    }

    impl ReportIo for HidapiIo {
        fn input(&mut self) -> Result<Vec<u8>> {
            // Same shape as HidD_GetInputReport: full length, report ID first.
            let mut report = vec![0u8; self.input_size];
            report[0] = 7;
            self.device.get_input_report(&mut report).map_err(|error| Error::other(error.to_string()))?;
            Ok(report)
        }

        fn write(&mut self, report: &[u8]) -> Result<bool> {
            Ok(self.device.write(report).is_ok_and(|written| written == report.len()))
        }
    }

    fn session(transport: Transport, allow_live: bool, allow_persistent: bool) -> Result<Session<HidapiIo>> {
        let api = HidApi::new().map_err(|error| Error::other(format!("Could not start HID access: {error}")))?;
        let found: Vec<_> = api
            .device_list()
            .filter(|info| {
                info.vendor_id() == VENDOR_ID
                    && info.product_id() == transport.product_id()
                    && info.usage_page() == USAGE_PAGE
            })
            .collect();
        if found.len() != 1 {
            return Err(Error::other(format!(
                "Expected one original Maxwell PlayStation {} interface, found {}",
                transport.name(),
                found.len()
            )));
        }
        let info = found[0];
        let device = info.open_device(&api).map_err(|_| Error::other(BUSY))?;
        let mut descriptor = vec![0u8; 4096];
        let length = device.get_report_descriptor(&mut descriptor).map_err(|error| Error::other(error.to_string()))?;
        let (input_size, output_size) = report_sizes(&descriptor[..length], USAGE_PAGE)
            .filter(|&(input, output)| (62..=1024).contains(&input) && (62..=1024).contains(&output))
            .ok_or_else(|| Error::other("The headset's HID report layout was not recognized. No EQ was sent."))?;
        let interface = Interface {
            path: info.path().to_string_lossy().into_owned(),
            input_size,
            output_size,
            device_version: info.release_number(),
        };
        Ok(Session::new(HidapiIo { device, input_size }, transport, interface, allow_live, allow_persistent))
    }

    pub fn open(transport: Transport, allow_live: bool, allow_persistent: bool) -> Result<Box<dyn Device>> {
        Ok(Box::new(session(transport, allow_live, allow_persistent)?))
    }

    /// A session that can read settings and, with `allow_write`, change them. No EQ writes.
    pub fn open_settings(transport: Transport, allow_write: bool) -> Result<Box<dyn Settings>> {
        let mut session = session(transport, false, false)?;
        session.allow_settings = allow_write;
        Ok(Box::new(session))
    }

    /// A session that can read controls and, with `allow_write`, change them. No EQ writes.
    pub fn open_controls(transport: Transport, allow_write: bool) -> Result<Box<dyn Controls>> {
        let mut session = session(transport, false, false)?;
        session.allow_controls = allow_write;
        Ok(Box::new(session))
    }
}

#[cfg(windows)]
mod windows {
    use super::{BUSY, Interface, ReportIo, Session, Transport, USAGE_PAGE, VENDOR_ID};
    use crate::balance::Settings;
    use crate::controls::Controls;
    use crate::device::Device;
    use crate::{Error, Result};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SetupDiDestroyDeviceInfoList,
        SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
    };
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDD_ATTRIBUTES, HIDP_CAPS, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetHidGuid, HidD_GetInputReport,
        HidD_GetPreparsedData, HidP_GetCaps,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING, WriteFile};
    use windows_sys::core::GUID;

    const ERROR_SHARING_VIOLATION: u32 = 32;
    const ERROR_NO_MORE_ITEMS: u32 = 259;

    fn os_error(code: u32) -> Error {
        Error::other(std::io::Error::from_raw_os_error(code as i32).to_string())
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// An opened control collection. The handle closes on drop.
    pub struct WinHid {
        handle: HANDLE,
        input_size: usize,
    }

    // The handle is owned by one session and only used from one thread at a time.
    unsafe impl Send for WinHid {}

    impl Drop for WinHid {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
        }
    }

    impl ReportIo for WinHid {
        fn input(&mut self) -> Result<Vec<u8>> {
            let mut report = vec![0u8; self.input_size];
            report[0] = 7;
            if !unsafe { HidD_GetInputReport(self.handle, report.as_mut_ptr().cast(), report.len() as u32) } {
                return Err(os_error(unsafe { GetLastError() }));
            }
            Ok(report)
        }

        fn write(&mut self, report: &[u8]) -> Result<bool> {
            let mut written = 0u32;
            let ok = unsafe { WriteFile(self.handle, report.as_ptr(), report.len() as u32, &mut written, null_mut()) };
            Ok(ok != 0 && written as usize == report.len())
        }
    }

    /// `CreateFileW(path, access, share, null, OPEN_EXISTING, 0, null)`, returning
    /// the handle or the last error. Tests substitute this.
    pub type Create<'a> = dyn Fn(&str, u32, u32) -> std::result::Result<HANDLE, u32> + 'a;

    pub fn create_file(path: &str, access: u32, share: u32) -> std::result::Result<HANDLE, u32> {
        let path = wide(path);
        let handle = unsafe { CreateFileW(path.as_ptr(), access, share, null(), OPEN_EXISTING, 0, null_mut()) };
        if handle == INVALID_HANDLE_VALUE { Err(unsafe { GetLastError() }) } else { Ok(handle) }
    }

    /// RACE replies are a device-wide byte stream: a shared handle lets HQ or
    /// another tool consume fragments, so the whole transaction owns the collection.
    pub fn open_exclusive(path: &str, create: &Create<'_>) -> Result<HANDLE> {
        match create(path, 0xC000_0000, 0) {
            Ok(handle) => Ok(handle),
            Err(ERROR_SHARING_VIOLATION) => Err(Error::other(BUSY)),
            Err(code) => Err(os_error(code)),
        }
    }

    fn probe(path: &str, product_id: u16) -> Option<Interface> {
        let handle = create_file(path, 0, 3).ok()?;
        let result = (|| {
            let mut attributes =
                HIDD_ATTRIBUTES { Size: size_of::<HIDD_ATTRIBUTES>() as u32, ..unsafe { std::mem::zeroed() } };
            if !unsafe { HidD_GetAttributes(handle, &mut attributes) }
                || (attributes.VendorID, attributes.ProductID) != (VENDOR_ID, product_id)
            {
                return None;
            }
            let mut preparsed = 0;
            if !unsafe { HidD_GetPreparsedData(handle, &mut preparsed) } {
                return None;
            }
            let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
            let status = unsafe { HidP_GetCaps(preparsed, &mut caps) };
            unsafe { HidD_FreePreparsedData(preparsed) };
            let (input, output) = (usize::from(caps.InputReportByteLength), usize::from(caps.OutputReportByteLength));
            (status >= 0
                && caps.UsagePage == USAGE_PAGE
                && (62..=1024).contains(&input)
                && (62..=1024).contains(&output))
            .then(|| Interface {
                path: path.to_owned(),
                input_size: input,
                output_size: output,
                device_version: attributes.VersionNumber,
            })
        })();
        unsafe { CloseHandle(handle) };
        result
    }

    fn interfaces(product_id: u16) -> Result<Vec<Interface>> {
        let mut guid: GUID = unsafe { std::mem::zeroed() };
        unsafe { HidD_GetHidGuid(&mut guid) };
        let devices = unsafe { SetupDiGetClassDevsW(&guid, null(), null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) };
        if devices == -1 {
            return Err(os_error(unsafe { GetLastError() }));
        }
        let mut found = Vec::new();
        let result = (|| {
            for index in 0.. {
                let mut entry: SP_DEVICE_INTERFACE_DATA = unsafe { std::mem::zeroed() };
                entry.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
                if unsafe { SetupDiEnumDeviceInterfaces(devices, null(), &guid, index, &mut entry) } == 0 {
                    let code = unsafe { GetLastError() };
                    return if code == ERROR_NO_MORE_ITEMS { Ok(()) } else { Err(os_error(code)) };
                }
                let mut need = 0u32;
                unsafe { SetupDiGetDeviceInterfaceDetailW(devices, &entry, null_mut(), 0, &mut need, null_mut()) };
                if need < 8 {
                    continue;
                }
                // u32-aligned buffer: cbSize, then the NUL-terminated path at offset 4.
                let mut buffer = vec![0u32; (need as usize).div_ceil(4)];
                buffer[0] = if size_of::<usize>() == 8 { 8 } else { 6 };
                let ok = unsafe {
                    SetupDiGetDeviceInterfaceDetailW(
                        devices,
                        &entry,
                        buffer.as_mut_ptr().cast(),
                        need,
                        &mut need,
                        null_mut(),
                    )
                };
                if ok == 0 {
                    continue;
                }
                let units = unsafe {
                    std::slice::from_raw_parts(buffer.as_ptr().cast::<u16>().add(2), (need as usize - 4) / 2)
                };
                let path =
                    String::from_utf16_lossy(&units[..units.iter().position(|&u| u == 0).unwrap_or(units.len())]);
                found.extend(probe(&path, product_id));
            }
            Ok(())
        })();
        unsafe { SetupDiDestroyDeviceInfoList(devices) };
        result.map(|()| found)
    }

    /// Find exactly one Maxwell PlayStation control collection and open it exclusively.
    fn session(transport: Transport, allow_live: bool, allow_persistent: bool) -> Result<Session<WinHid>> {
        let mut found = interfaces(transport.product_id())?;
        if found.len() != 1 {
            return Err(Error::other(format!(
                "Expected one original Maxwell PlayStation {} interface, found {}",
                transport.name(),
                found.len()
            )));
        }
        let interface = found.remove(0);
        let handle = open_exclusive(&interface.path, &create_file)?;
        let io = WinHid { handle, input_size: interface.input_size };
        Ok(Session::new(io, transport, interface, allow_live, allow_persistent))
    }

    pub fn open(transport: Transport, allow_live: bool, allow_persistent: bool) -> Result<Box<dyn Device>> {
        Ok(Box::new(session(transport, allow_live, allow_persistent)?))
    }

    /// A session that can read settings and, with `allow_write`, change them. No EQ writes.
    pub fn open_settings(transport: Transport, allow_write: bool) -> Result<Box<dyn Settings>> {
        let mut session = session(transport, false, false)?;
        session.allow_settings = allow_write;
        Ok(Box::new(session))
    }

    /// A session that can read controls and, with `allow_write`, change them. No EQ writes.
    pub fn open_controls(transport: Transport, allow_write: bool) -> Result<Box<dyn Controls>> {
        let mut session = session(transport, false, false)?;
        session.allow_controls = allow_write;
        Ok(Box::new(session))
    }
}
