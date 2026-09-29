//! Device state the bulk packet does not carry, read on demand.
//!
//! The bulk shadow is kept current by notifications, but a few values never
//! reach it: the subharmonic synthesizer's solo and headroom, the sub and
//! limiter meters, the limiter's engaged flag, the auxiliary outputs, and
//! the output-config mode. Each has its own GET, and each read here stores
//! its answer on [`DeviceState`], where the screens draw from.
//!
//! None of them is read at connect: a screen that shows one asks for it, at
//! its own rate, while it is showing it, as the runner already does for the
//! upmixer's status. A stall means the firmware lacks the feature, and the
//! field stays `None`.

use dspi_proto::generated::limiter;
use dspi_proto::generated::opcodes as op;
use dspi_proto::packets::{CsAuxStates, LimiterMeter, LimiterStatus, SubharmMeter};
use dspi_proto::value::Value;

use crate::state::DeviceState;
use crate::{Session, WriteError};

impl DeviceState {
    /// `REQ_GET_SUBHARM_SOLO` (config.h:201-202). Solo is runtime only and has
    /// no wire offset (bulk_params.h:382-384), so no notification carries it,
    /// while another host or a control surface can change it: a panel
    /// showing it polls this.
    pub fn refresh_subharm_solo(&mut self, session: &mut Session) -> Result<bool, WriteError> {
        let solo = session.read("sub.solo", &[])?.as_bool().unwrap_or(false);
        self.subharm_solo = Some(solo);
        Ok(solo)
    }

    /// `REQ_GET_SUBHARM_HEADROOM` (config.h:194): the worst-case gain the
    /// synthesizer adds, as an f32 in dB, 0 while it is off. The Console reads
    /// it again after every write that can move it.
    pub fn refresh_subharm_headroom(&mut self, session: &mut Session) -> Result<f32, WriteError> {
        let b = session.with_transport(|t| t.control_in(op::REQ_GET_SUBHARM_HEADROOM, 0, 4))?;
        let db = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        self.subharm_headroom_db = Some(db);
        Ok(db)
    }

    /// `REQ_GET_SUBHARM_METER` (config.h:200): one u16 per output, so its
    /// length follows the device (18 bytes on an RP2350, 10 on an RP2040).
    pub fn refresh_subharm_meter(&mut self, session: &mut Session) -> Result<(), WriteError> {
        let len = 2 * self.caps.num_outputs as u16;
        let b = session.with_transport(|t| t.control_in(op::REQ_GET_SUBHARM_METER, 0, len))?;
        self.subharm_meter = SubharmMeter::decode(&b).ok();
        Ok(())
    }

    /// `REQ_LIMITER` at `LIMITER_GET_METER` (limiter.h:19): gain reduction per
    /// output. The output byte of `wValue` is ignored for the read-only
    /// blocks, so it is sent as 0.
    pub fn refresh_limiter_meter(&mut self, session: &mut Session) -> Result<(), WriteError> {
        let len = 2 * self.caps.num_outputs as u16;
        let b = session
            .with_transport(|t| t.control_in(op::REQ_LIMITER, limiter::LIMITER_GET_METER, len))?;
        self.limiter_meter = LimiterMeter::decode(&b).ok();
        Ok(())
    }

    /// `REQ_LIMITER` at `LIMITER_GET_STATUS` (limiter.h:20): whether the
    /// lookahead delay is in the path, which is what the latency warning
    /// depends on.
    pub fn refresh_limiter_status(&mut self, session: &mut Session) -> Result<(), WriteError> {
        let b = session.with_transport(|t| {
            t.control_in(
                op::REQ_LIMITER,
                limiter::LIMITER_GET_STATUS,
                LimiterStatus::SIZE as u16,
            )
        })?;
        self.limiter_status = LimiterStatus::decode(&b).ok();
        Ok(())
    }

    /// `REQ_GET_CS_AUX_STATE` with `wValue = 0xFFFF` (config.h:138-140): every
    /// slot's state and level in one read. After this, `NOTIFY_EVT_CS_AUX`
    /// keeps the block current through [`DeviceState::apply`].
    pub fn refresh_cs_aux(&mut self, session: &mut Session) -> Result<(), WriteError> {
        // `0xFFFF` is the whole-table form; any other wValue is one slot.
        let b = session.with_transport(|t| {
            t.control_in(op::REQ_GET_CS_AUX_STATE, 0xFFFF, CsAuxStates::SIZE as u16)
        })?;
        self.cs_aux_states = CsAuxStates::decode(&b).ok();
        Ok(())
    }

    /// `REQ_GET_OUTPUT_CONFIG_MODE` (config.h:362). The preset directory
    /// carries the same byte, so a caller that has just read the directory
    /// can set [`DeviceState::output_config_mode`] from it instead.
    pub fn refresh_output_config_mode(&mut self, session: &mut Session) -> Result<u8, WriteError> {
        let mode = match session.read("preset.iomode", &[])? {
            Value::Choice(m) => m,
            other => other.as_u8().unwrap_or(self.output_config_mode),
        };
        self.output_config_mode = mode;
        Ok(mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::{Notification, evt};
    use dspi_proto::Platform;
    use dspi_proto::generated::{BULK_SIZE, wire::WIRE_FORMAT_VERSION};
    use dspi_proto::wire::BulkPacket;
    use dspi_transport::MockTransport;
    use dspi_transport::mock::Reply;

    fn caps(outputs: u8) -> crate::Capabilities {
        crate::Capabilities {
            serial: "TEST".into(),
            platform: if outputs == 9 {
                Platform::Rp2350
            } else {
                Platform::Rp2040
            },
            firmware: "1.1.6 beta 4".into(),
            firmware_version: dspi_proto::packets::FirmwareVersion::new(1, 1, 6, 4),
            build_info: None,
            wire_format: WIRE_FORMAT_VERSION as u8,
            num_channels: if outputs == 9 { 17 } else { 7 },
            num_inputs: if outputs == 9 { 8 } else { 2 },
            num_outputs: outputs,
            max_bands: 10,
            band_storage: 12,
            channels: Vec::new(),
            features: ["subharmonic_synth", "tube_preamp", "output_limiter"]
                .iter()
                .map(|n| crate::probe::Feature {
                    name: (*n).into(),
                    present: true,
                    evidence: "test".into(),
                })
                .collect(),
            cs: None,
            siggen: None,
            active_preset: None,
        }
    }

    fn packet() -> Vec<u8> {
        let mut b = vec![0u8; BULK_SIZE];
        b[0] = WIRE_FORMAT_VERSION as u8;
        b[6..8].copy_from_slice(&(BULK_SIZE as u16).to_le_bytes());
        b
    }

    fn rig(mock: MockTransport, outputs: u8) -> (DeviceState, Session) {
        let session = Session::new(Box::new(mock), caps(outputs)).unwrap();
        let state = DeviceState::new(caps(outputs), BulkPacket::decode(packet()).unwrap());
        (state, session)
    }

    #[test]
    fn solo_and_headroom_are_read_and_kept() {
        let mock = MockTransport::new()
            .data(op::REQ_GET_SUBHARM_SOLO, vec![1])
            .data(op::REQ_GET_SUBHARM_HEADROOM, 4.5f32.to_le_bytes().to_vec());
        let (mut st, mut s) = rig(mock, 9);
        assert_eq!(st.subharm_solo, None, "unknown until read");
        assert!(st.refresh_subharm_solo(&mut s).unwrap());
        assert_eq!(st.subharm_solo, Some(true));
        assert_eq!(st.refresh_subharm_headroom(&mut s).unwrap(), 4.5);
        assert_eq!(st.subharm_headroom_db, Some(4.5));
    }

    /// The meters are one u16 per output, so an RP2040 asks for 10 bytes and
    /// an RP2350 for 18 (survey-firmware-beta4 section 5).
    #[test]
    fn the_meters_are_sized_by_the_device() {
        let peaks: Vec<u8> = (0..9u16).flat_map(|i| (i * 1000).to_le_bytes()).collect();
        let mock = MockTransport::new()
            .data(op::REQ_GET_SUBHARM_METER, peaks.clone())
            .data(op::REQ_LIMITER, peaks);
        let log = mock.log_handle();
        let (mut st, mut s) = rig(mock, 5);
        st.refresh_subharm_meter(&mut s).unwrap();
        st.refresh_limiter_meter(&mut s).unwrap();
        assert_eq!(st.subharm_meter.as_ref().unwrap().peaks.len(), 5);
        assert_eq!(st.limiter_meter.as_ref().unwrap().reduction_db(4), 40.0);
        let log = log.lock().unwrap();
        assert_eq!(log[1].opcode, op::REQ_LIMITER);
        assert_eq!(log[1].value, limiter::LIMITER_GET_METER);
    }

    #[test]
    fn the_limiter_status_says_whether_the_delay_is_in() {
        let mock = MockTransport::new().data(op::REQ_LIMITER, vec![1, 32, 16, 9]);
        let (mut st, mut s) = rig(mock, 9);
        st.refresh_limiter_status(&mut s).unwrap();
        let status = st.limiter_status.unwrap();
        assert!(status.engaged);
        assert_eq!(status.lookahead, 32);
    }

    #[test]
    fn a_missing_feature_leaves_the_field_empty() {
        let mock = MockTransport::new().reply(op::REQ_GET_SUBHARM_HEADROOM, Reply::Stall);
        let (mut st, mut s) = rig(mock, 9);
        assert!(st.refresh_subharm_headroom(&mut s).is_err());
        assert_eq!(st.subharm_headroom_db, None);
    }

    #[test]
    fn the_output_config_mode_is_read_through_the_registry() {
        let mock = MockTransport::new().data(op::REQ_GET_OUTPUT_CONFIG_MODE, vec![0]);
        let (mut st, mut s) = rig(mock, 9);
        assert_eq!(st.output_config_mode, 1, "WITH_PRESET until read");
        assert_eq!(st.refresh_output_config_mode(&mut s).unwrap(), 0);
        assert_eq!(st.output_config_mode, 0);
    }

    /// The whole table is read once, then each `NOTIFY_EVT_CS_AUX` patches
    /// its slot, so a page showing the aux outputs never has to poll.
    #[test]
    fn the_aux_table_is_read_once_and_then_kept_by_notifications() {
        let mut block = CsAuxStates::default();
        block.state[2] = 1;
        block.level_q8[3] = 50 * 256;
        let mock = MockTransport::new().data(op::REQ_GET_CS_AUX_STATE, block.encode().to_vec());
        let log = mock.log_handle();
        let (mut st, mut s) = rig(mock, 9);

        // Before the read, an event is kept but there is no table to patch.
        let (_, e) = crate::notify::decode(&[2, evt::CS_AUX, 0, 1, 3, 1, 0, 0x19, 5]).unwrap();
        st.apply(&Notification {
            seq: 1,
            event: e,
            lost: false,
        });
        assert_eq!(st.cs_aux_states, None);

        st.refresh_cs_aux(&mut s).unwrap();
        assert_eq!(log.lock().unwrap()[0].value, 0xFFFF, "the whole-table form");
        let all = st.cs_aux_states.as_ref().unwrap();
        assert!(all.is_on(2));
        assert_eq!(all.level_percent(3), 50.0);

        // Slot 3 dims to 25 % from a control surface.
        let (_, e) = crate::notify::decode(&[2, evt::CS_AUX, 0, 2, 3, 1, 0x00, 0x19, 5]).unwrap();
        st.apply(&Notification {
            seq: 2,
            event: e,
            lost: false,
        });
        let all = st.cs_aux_states.as_ref().unwrap();
        assert!(all.is_on(3));
        assert_eq!(all.level_percent(3), 25.0);
        assert!(all.is_on(2), "the other slots are left alone");

        // A slot past the table is ignored rather than trusted.
        let (_, e) = crate::notify::decode(&[2, evt::CS_AUX, 0, 3, 40, 1, 0, 0, 5]).unwrap();
        st.apply(&Notification {
            seq: 3,
            event: e,
            lost: false,
        });
        assert_eq!(st.cs_aux_states.as_ref().unwrap().state.len(), 16);
    }
}
