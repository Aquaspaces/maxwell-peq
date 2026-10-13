//! RACE framing and the HID session, without opening hardware.
//! Ports tests/test_protocol.py, test_storage_protocol.py, test_input_drain.py
//! and test_device_access.py.

use maxwell_core::device::Device;
use maxwell_core::hid::{Interface, Model, PRODUCTS, ReportIo, Session, Transport, label, select};
use maxwell_core::protocol::*;
use maxwell_core::{Error, Result};
use std::collections::VecDeque;

fn response(command: u16, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x05, 0x5B];
    packet.extend(((payload.len() + 2) as u16).to_le_bytes());
    packet.extend(command.to_le_bytes());
    packet.extend(payload);
    packet
}

fn reports(data: &[u8]) -> Vec<Vec<u8>> {
    data.chunks(59)
        .map(|chunk| {
            let mut report = vec![7, chunk.len() as u8, 0x80];
            report.extend(chunk);
            report.resize(62, 0);
            report
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

fn unhex(text: &str) -> Vec<u8> {
    hex::decode(text).unwrap()
}

#[test]
fn vendor_request_vectors() {
    assert_eq!(hex(&read_index_command()), "055a040001090000");
    assert_eq!(hex(&read_ui_command(2).unwrap()), "055a06000c0a02efe803");
    assert_eq!(hex(&live_command(&unhex("12345678")).unwrap()), "055a0700030e0012345678");
    let first = &output_reports(&read_index_command(), false, 62, false, false).unwrap()[0];
    assert_eq!(hex(&first[..11]), "060800055a040001090000");
}

#[test]
fn fragmentation_and_routing() {
    let blob: Vec<u8> = (0..3).flat_map(|_| 0..=255u8).collect();
    let packet = live_command(&blob).unwrap();
    let chunks = output_reports(&packet, true, 63, true, false).unwrap();
    assert!(chunks.iter().all(|r| r.len() == 63 && r[0] == 6 && r[2] == 0x80));
    assert!(chunks[..chunks.len() - 1].iter().all(|r| r[1] == 59));
    let joined: Vec<u8> = chunks.iter().flat_map(|r| r[3..3 + usize::from(r[1])].to_vec()).collect();
    assert_eq!(joined, packet);
}

#[test]
fn allowlist() {
    for packet in
        [response(0x0E03, &[0]), unhex("055a05000009000007"), unhex("055a06000c0a30e4e803"), unhex("055a04000d0a2ce4")]
    {
        assert!(validate_request(&packet, true, false).is_err(), "{}", hex(&packet));
    }
    assert!(output_reports(&live_command(&[0, 0]).unwrap(), false, 62, false, false).is_err());
    assert!(read_ui_command(4).is_err());
}

#[test]
fn saved_coefficient_read() {
    let packet = read_coefficients_command(0).unwrap();
    assert_eq!(hex(&packet), "055a06000c0a2ce4e803");
    validate_request(&packet, false, false).unwrap();
    let raw = vec![0u8; 844];
    let reply = response(0x0A0C, &[unhex("002ce44c03"), raw.clone()].concat());
    assert_eq!(parse_coefficients(&reply, 0).unwrap(), raw);
    assert!(parse_coefficients(&reply, 1).is_err());
    assert!(parse_coefficients(&response(0x0A0C, &[unhex("002ce44b03"), raw].concat()), 0).is_err());
}

#[test]
fn a_preset_that_was_never_saved_reads_as_none() {
    let missing = response(0x0A0C, &unhex("052de4"));
    assert_eq!(parse_stored_coefficients(&missing, 1).unwrap(), None);
    assert!(parse_coefficients(&missing, 1).unwrap_err().to_string().contains("Preset 2 has no saved EQ"));
    // A missing key that is not the one asked for is still a wrong reply.
    assert!(parse_stored_coefficients(&missing, 2).is_err());
}

#[test]
fn preset_select_matches_the_audeze_app() {
    assert_eq!(hex(&select_preset_command(9).unwrap()), "055a05000009000009");
    for index in [0, 6, 11] {
        assert!(select_preset_command(index).is_err(), "{index}");
    }
    validate_select(&select_preset_command(7).unwrap()).unwrap();
    for packet in
        [unhex("055a05000009000003"), unhex("055a05000009290064"), write_setting_command(LEFT_GAIN, 100).unwrap()]
    {
        assert!(validate_select(&packet).is_err(), "{}", hex(&packet));
    }
    // The EQ allowlist only takes a reselect with save access, for the Maxwell 2 reload.
    assert!(validate_request(&select_preset_command(7).unwrap(), true, false).is_err());
    parse_select_ack(&unhex("055b0500000900000a")).unwrap();
    assert_eq!(
        parse_select_ack(&unhex("055b0500000903000a")).unwrap_err().to_string(),
        "Headset rejected the preset change (status 3)"
    );
    assert!(parse_select_ack(&unhex("055b0500000900290a")).is_err());
}

#[test]
fn reassembly_split_and_multiple() {
    let packet = response(0x0A0C, &(0..200).map(|i| i as u8).collect::<Vec<_>>());
    let ack = response(0x0E03, &[0]);
    let mut stream = InputStream::default();
    let mut results = Vec::new();
    for report in reports(&[packet.clone(), ack.clone()].concat()) {
        results.extend(stream.feed(&report).unwrap());
    }
    assert_eq!(results, vec![packet, ack]);
    // Nothing left over: a fresh ACK parses on its own.
    assert_eq!(stream.feed(&reports(&response(0x0E03, &[0]))[0]).unwrap().len(), 1);
}

#[test]
fn bounds() {
    assert!(live_command(&[0; 1024]).is_err());
    assert!(InputStream::default().feed(&[&[7, 60, 0][..], &[0; 59]].concat()).is_err());
    assert!(InputStream::default().feed(&reports(&unhex("055bffff"))[0]).is_err());
    assert!(check_packet(&[response(0x0E03, &[0]), vec![0]].concat(), 0x5B).is_err());
}

#[test]
fn ack_failures() {
    parse_live_ack(&response(0x0E03, &[0])).unwrap();
    for packet in [response(0x0E03, &[1]), response(0x0A0D, &[0]), response(0x0E03, &[]), response(0x0E03, &[0, 0])] {
        assert!(parse_live_ack(&packet).is_err(), "{}", hex(&packet));
    }
}

#[test]
fn index() {
    assert_eq!(parse_index(&response(0x0901, &[0, 0, 1, 7])).unwrap(), 7);
    assert!(parse_index(&response(0x0901, &[0x29, 0, 1, 7])).is_err());
}

#[test]
fn ui_payload_preserves_unknowns() {
    let mut band = 0x102u16.to_le_bytes().to_vec();
    for value in [100_000i32, -600, 12_500, 77] {
        band.extend(value.to_le_bytes());
    }
    let raw = [unhex("0102030405"), band.repeat(10)].concat();
    let packet = response(0x0A0C, &[&[0, 0x02, 0xEF][..], &(raw.len() as u16).to_le_bytes(), &raw].concat());
    let preset = parse_ui(&packet, 2).unwrap();
    assert_eq!(preset.raw_response, packet);
    assert_eq!(preset.header, unhex("0102030405"));
    assert_eq!(
        preset.bands[0],
        UiBand {
            tag: 0x102,
            frequency_hundredths: 100_000,
            gain_hundredths: -600,
            width_hundredths: 12_500,
            unknown: 77
        }
    );
    assert_eq!(preset.bands.len(), 10);
    assert!(parse_ui(&packet, 1).is_err());
    assert!(parse_ui(&response(0x0A0C, &[&[0, 0x02, 0xEF, 0, 0][..], &raw].concat()), 2).is_err());
}

#[test]
fn save_vector_route_and_permission() {
    let packet = save_coefficients_command(3, &[0; 844]).unwrap();
    assert_eq!(hex(&packet[..8]), "055a50030d0a2fe4");
    assert_eq!(packet.len(), 852);
    assert!(validate_request(&packet, true, false).is_err());
    validate_request(&packet, false, true).unwrap();
    let chunks = output_reports(&packet, true, 62, false, true).unwrap();
    assert!(chunks.iter().all(|r| r[2] == 0x80 && r.len() == 62));
    let joined: Vec<u8> = chunks.iter().flat_map(|r| r[3..3 + usize::from(r[1])].to_vec()).collect();
    assert_eq!(joined, packet);
    let wrong_key = [&packet[..6], &unhex("00ef"), &packet[8..]].concat();
    assert!(validate_request(&wrong_key, false, true).is_err());
    assert!(save_coefficients_command(4, &[0; 844]).is_err());
    assert!(save_coefficients_command(0, &[0; 843]).is_err());
}

#[test]
fn capacity_is_a_boolean_not_zero_success_status() {
    assert_eq!(hex(&storage_space_command()), "055a0400030a4c03");
    validate_request(&storage_space_command(), false, false).unwrap();
    assert!(parse_storage_space(&response(0x0A03, &[1])).unwrap());
    assert!(!parse_storage_space(&response(0x0A03, &[0])).unwrap());
    assert!(parse_storage_space(&response(0x0A03, &[2])).is_err());
}

#[test]
fn save_ack_requires_success_and_the_same_key() {
    parse_save_ack(&unhex("055b05000d0a002fe4"), 3).unwrap();
    for packet in
        [response(0x0A0D, &unhex("012fe4")), response(0x0A0D, &unhex("002ee4")), response(0x0A0D, &unhex("002fe400"))]
    {
        assert!(parse_save_ack(&packet, 3).is_err(), "{}", hex(&packet));
    }
}

/// Scripted input reports; records writes.
struct Script {
    inputs: VecDeque<Vec<u8>>,
    repeat: Option<Vec<u8>>,
    reads: usize,
    writes: Vec<Vec<u8>>,
}

impl Script {
    fn new(inputs: Vec<Vec<u8>>, repeat: Option<Vec<u8>>) -> Self {
        Script { inputs: inputs.into(), repeat, reads: 0, writes: Vec::new() }
    }
}

impl ReportIo for Script {
    fn input(&mut self) -> Result<Vec<u8>> {
        self.reads += 1;
        Ok(self.inputs.pop_front().or_else(|| self.repeat.clone()).expect("script ran out of reports"))
    }

    fn write(&mut self, report: &[u8]) -> Result<bool> {
        self.writes.push(report.to_vec());
        Ok(true)
    }
}

fn session(io: Script, transport: Transport, allow_live: bool) -> Session<Script> {
    let interface = Interface { path: "test-hid-path".into(), input_size: 62, output_size: 62, device_version: 0x0100 };
    let mut session = Session::new(io, transport, interface, allow_live, false);
    session.sleep = |_| {};
    session
}

fn empty() -> Vec<u8> {
    [&[7, 0, 0][..], &[0; 59]].concat()
}

fn fragment() -> Vec<u8> {
    [&[7, 1, 0, 42][..], &[0; 58]].concat()
}

#[test]
fn delayed_fragment_restarts_quiet_period() {
    let mut inputs = vec![empty(), fragment()];
    inputs.extend(std::iter::repeat_n(empty(), 5));
    inputs.push(fragment());
    inputs.extend(std::iter::repeat_n(empty(), 6));
    let count = inputs.len();
    let mut session = session(Script::new(inputs, None), Transport::Usb, false);
    session.drain().unwrap();
    assert_eq!(session.io.reads, count);
}

#[test]
fn continuous_input_aborts_with_bounded_reads() {
    let mut session = session(Script::new(vec![], Some(fragment())), Transport::Usb, false);
    let error = session.drain().unwrap_err().to_string();
    assert!(error.contains("no command sent"), "{error}");
    assert_eq!(session.io.reads, 100);
}

#[test]
fn malformed_report_aborts() {
    let malformed = [&[7, 60, 0][..], &[0; 59]].concat();
    let mut session = session(Script::new(vec![], Some(malformed)), Transport::Usb, false);
    assert!(session.drain().unwrap_err().to_string().contains("Malformed"));
}

#[test]
fn transact_skips_unrelated_replies_and_returns_the_match() {
    let ack = response(0x0E03, &[0]);
    let mut inputs = vec![empty(); 6];
    // A stray notification arrives first, then the reply.
    inputs.extend(reports(&response(0x0901, &[0, 0, 1, 7])));
    inputs.extend(reports(&ack));
    let mut session = session(Script::new(inputs, Some(empty())), Transport::Dongle, true);
    let packet = live_command(&[1, 2, 3]).unwrap();
    assert_eq!(session.transact(&packet, None).unwrap(), ack);
    assert_eq!(session.io.writes.len(), 1);
    assert_eq!(session.io.writes[0][2], 0x80, "dongle routing flag");
}

#[test]
fn transact_refuses_live_eq_without_permission() {
    let mut session = session(Script::new(vec![], Some(empty())), Transport::Usb, false);
    assert!(session.transact(&live_command(&[1]).unwrap(), None).is_err());
    assert_eq!(session.io.reads, 0);
    assert!(session.io.writes.is_empty());
}

#[test]
fn transact_reports_a_rejected_live_ack() {
    let mut inputs = vec![empty(); 6];
    inputs.extend(reports(&response(0x0E03, &[5])));
    let mut session = session(Script::new(inputs, Some(empty())), Transport::Usb, true);
    let error = session.transact(&live_command(&[1]).unwrap(), None).unwrap_err();
    assert_eq!(error.to_string(), "Headset rejected live EQ (status 5)");
}

#[test]
fn transact_times_out_without_a_reply() {
    let mut session = session(Script::new(vec![], Some(empty())), Transport::Usb, false);
    let error = session.transact(&read_index_command(), Some(0.1)).unwrap_err();
    assert!(matches!(error, Error::Other(_)));
    assert_eq!(error.to_string(), "No matching command response; live state may be uncertain");
}

#[test]
fn select_waits_until_the_headset_reports_the_new_preset() {
    use maxwell_core::device::Device;
    let mut inputs = vec![empty(); 6];
    inputs.extend(reports(&unhex("055b05000009000009")));
    // The first read still shows the old preset; the next one has switched.
    for index in [10, 9] {
        inputs.extend(vec![empty(); 6]);
        inputs.extend(reports(&response(0x0901, &[0, 0, 1, index])));
    }
    let mut session = session(Script::new(inputs, Some(empty())), Transport::Dongle, false);
    assert!(session.select_preset(9).is_err(), "selecting needs its own permission");
    session.allow_select = true;
    let result = session.select_preset(9).unwrap();
    assert_eq!(result["select_ack_hex"], "055b05000009000009");
    assert_eq!(session.io.writes.len(), 3);
    assert_eq!(hex(&session.io.writes[0][3..12]), "055a05000009000009");
}

#[test]
fn product_ids_pick_the_model_for_the_transport() {
    let found = vec![(0x4B1A, "m1-usb"), (0x4B19, "m1-dongle"), (0x4B2A, "m2-usb"), (0x0001, "other")];
    // Two USB headsets at once is ambiguous.
    assert!(select(found.clone(), Transport::Usb).unwrap_err().to_string().contains("found 2"));
    assert_eq!(select(found, Transport::Dongle).unwrap(), (Model::Maxwell, 0x4B19, "m1-dongle"));
    assert_eq!(select(vec![(0x4B2A, ())], Transport::Usb).unwrap(), (Model::Maxwell2, 0x4B2A, ()));
    for dongle in [0x4B28, 0x4B29] {
        assert_eq!(select(vec![(dongle, ())], Transport::Dongle).unwrap().0, Model::Maxwell2);
        assert!(select(vec![(dongle, ())], Transport::Usb).is_err());
    }
}

#[test]
fn the_linux_udev_rule_covers_every_supported_id() {
    let rule = include_str!("../../../src-tauri/linux/70-maxwell-peq.rules");
    for (id, ..) in PRODUCTS {
        let line = format!("ATTRS{{idProduct}}==\"{id:04x}\"");
        assert!(rule.contains(&line), "the udev rule is missing {id:04x}");
    }
}

#[test]
fn xbox_ids_are_the_original_maxwell() {
    assert_eq!(select(vec![(0x4B1E, ())], Transport::Usb).unwrap(), (Model::Maxwell, 0x4B1E, ()));
    assert_eq!(select(vec![(0x4B18, ())], Transport::Dongle).unwrap(), (Model::Maxwell, 0x4B18, ()));
    assert!(select(vec![(0x4B18, ())], Transport::Usb).is_err());
    assert_eq!(label(Model::Maxwell, 0x4B1E), "Original Maxwell Xbox (untested)");
    assert_eq!(label(Model::Maxwell, 0x4B18), "Original Maxwell Xbox (untested)");
    assert_eq!(label(Model::Maxwell, 0x4B1A), "Original Maxwell PS");
}

#[test]
fn maxwell2_anc_is_refused_before_any_eq() {
    for id in [0x4BA8, 0x4BA9, 0x4BAA] {
        for transport in [Transport::Usb, Transport::Dongle] {
            let error = select(vec![(id, ())], transport).unwrap_err().to_string();
            assert!(error.contains("Maxwell 2 ANC") && error.ends_with("No EQ was sent."), "{error}");
        }
    }
}

#[test]
fn preset_reselect_needs_the_save_capability() {
    assert_eq!(hex(&select_preset_command(7).unwrap()), "055a05000009000007");
    assert_eq!(hex(&write_voice_prompt_command(0).unwrap()), "055a05000009200000");
    assert_eq!(hex(&read_voice_prompt_command()), "055a040001092000");
    assert!(select_preset_command(6).is_err() && select_preset_command(11).is_err());
    for packet in
        [select_preset_command(10).unwrap(), write_voice_prompt_command(9).unwrap(), read_voice_prompt_command()]
    {
        assert!(validate_request(&packet, true, false).is_err(), "{}", hex(&packet));
        validate_request(&packet, false, true).unwrap();
    }
    for other in ["055a05000009000003", "055a05000009290050", "055a050000092f000a"] {
        assert!(validate_request(&unhex(other), true, true).is_err(), "{other}");
    }
}

fn exchange(reply: &[u8]) -> Vec<Vec<u8>> {
    [vec![empty(); 6], reports(reply)].concat()
}

fn sent(session: &Session<Script>) -> Vec<String> {
    session.io.writes.iter().map(|report| hex(&report[3..3 + usize::from(report[1])])).collect()
}

fn persistent_session(inputs: Vec<Vec<u8>>, model: Model, transport: Transport) -> Session<Script> {
    let mut session = session(Script::new(inputs, Some(empty())), transport, true);
    session.allow_persistent = true;
    session.model = model;
    session
}

#[test]
fn maxwell2_usb_reselects_the_saved_preset_like_audeze_hq() {
    let prompt = response(0x0901, &[0x20, 0, 0, 5]);
    let written = response(0x0900, &[0]);
    let inputs = [
        exchange(&prompt),
        exchange(&prompt),
        exchange(&written),
        exchange(&written),
        exchange(&written),
        exchange(&response(0x0901, &[0, 0, 1, 9])),
    ]
    .concat();
    let mut session = persistent_session(inputs, Model::Maxwell2, Transport::Usb);
    assert!(session.reload_saved(9).unwrap());
    assert_eq!(
        sent(&session),
        [
            "055a040001092000",   // read the voice-prompt level
            "055a040001092000",   // twice, until it is stable
            "055a05000009200000", // mute prompts
            "055a05000009000009", // reselect preset 9
            "055a05000009200005", // restore the prompt level
            "055a040001090000",   // confirm the preset
        ]
    );
}

#[test]
fn only_maxwell2_usb_reselects() {
    for (model, transport) in
        [(Model::Maxwell, Transport::Usb), (Model::Maxwell, Transport::Dongle), (Model::Maxwell2, Transport::Dongle)]
    {
        let mut session = persistent_session(vec![], model, transport);
        assert!(!session.reload_saved(9).unwrap());
        assert!(session.io.writes.is_empty());
    }
}

#[cfg(windows)]
mod access {
    use maxwell_core::hid::{BUSY, open_exclusive};
    use std::cell::RefCell;

    #[test]
    fn control_handle_is_exclusive() {
        let calls = RefCell::new(Vec::new());
        let handle = open_exclusive("test-hid-path", &|path: &str, access, share| {
            calls.borrow_mut().push((path.to_owned(), access, share));
            Ok(1234 as _)
        })
        .unwrap();
        assert_eq!(handle as usize, 1234);
        assert_eq!(*calls.borrow(), vec![("test-hid-path".to_owned(), 0xC000_0000, 0)]);
    }

    #[test]
    fn busy_interface_explains_recovery_without_shared_retry() {
        let calls = RefCell::new(0);
        let error = open_exclusive("test-hid-path", &|_: &str, _, _| {
            *calls.borrow_mut() += 1;
            Err(32)
        })
        .unwrap_err();
        assert_eq!(error.to_string(), BUSY);
        assert!(BUSY.contains("Fully quit Audeze HQ") && BUSY.contains("No EQ was sent"));
        assert_eq!(*calls.borrow(), 1);
    }
}
