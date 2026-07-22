use core::convert::TryFrom;

use super::{
    FrameDecodeError, FrameFlags, ManagementBackend, ManagementCommand, ManagementFrame,
    ManagementHeader, ManagementStatus, ProfileWriteOptions, RebootMode, WireReader,
    WireWriteError, WireWriter, MANAGEMENT_HEADER_SIZE,
};

const RESPONSE_STATUS_SIZE: usize = 2;
const RESPONSE_PREFIX_SIZE: usize = MANAGEMENT_HEADER_SIZE + RESPONSE_STATUS_SIZE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagementServiceError {
    Frame(FrameDecodeError),
    UnexpectedResponse,
    ResponseBufferTooSmall,
}

impl From<FrameDecodeError> for ManagementServiceError {
    fn from(value: FrameDecodeError) -> Self {
        Self::Frame(value)
    }
}

/// Transport-independent KWMP request dispatcher.
pub struct ManagementService<const RESPONSE_CAPACITY: usize> {
    response: [u8; RESPONSE_CAPACITY],
}

impl<const RESPONSE_CAPACITY: usize> ManagementService<RESPONSE_CAPACITY> {
    pub const fn new() -> Self {
        Self {
            response: [0; RESPONSE_CAPACITY],
        }
    }

    pub fn handle_frame<'a, B: ManagementBackend>(
        &'a mut self,
        backend: &mut B,
        input: &[u8],
    ) -> Result<&'a [u8], ManagementServiceError> {
        if RESPONSE_CAPACITY < RESPONSE_PREFIX_SIZE {
            return Err(ManagementServiceError::ResponseBufferTooSmall);
        }

        let frame = ManagementFrame::decode(input)?;
        if frame.header.flags.contains(FrameFlags::RESPONSE) {
            return Err(ManagementServiceError::UnexpectedResponse);
        }

        let data_capacity = RESPONSE_CAPACITY - RESPONSE_PREFIX_SIZE;
        let mut writer = WireWriter::new(
            &mut self.response[RESPONSE_PREFIX_SIZE..RESPONSE_PREFIX_SIZE + data_capacity],
        );

        let status = match ManagementCommand::try_from(frame.header.command) {
            Ok(command) => Self::dispatch(backend, command, frame.payload, &mut writer),
            Err(()) => ManagementStatus::UnknownCommand,
        };

        let data_length = if status.is_ok() { writer.len() } else { 0 };
        self.response[MANAGEMENT_HEADER_SIZE..RESPONSE_PREFIX_SIZE]
            .copy_from_slice(&status.value().to_le_bytes());

        let payload_length = RESPONSE_STATUS_SIZE
            .checked_add(data_length)
            .ok_or(ManagementServiceError::ResponseBufferTooSmall)?;
        let payload_length_u32 = u32::try_from(payload_length)
            .map_err(|_| ManagementServiceError::ResponseBufferTooSmall)?;

        ManagementHeader::response(
            frame.header.command,
            frame.header.request_id,
            payload_length_u32,
            !status.is_ok(),
        )
        .encode(&mut self.response[..MANAGEMENT_HEADER_SIZE])
        .map_err(|_| ManagementServiceError::ResponseBufferTooSmall)?;

        Ok(&self.response[..RESPONSE_PREFIX_SIZE + data_length])
    }

    fn dispatch<B: ManagementBackend>(
        backend: &mut B,
        command: ManagementCommand,
        payload: &[u8],
        writer: &mut WireWriter<'_>,
    ) -> ManagementStatus {
        match command {
            ManagementCommand::GetDeviceInfo => {
                if !payload.is_empty() {
                    return ManagementStatus::InvalidPayload;
                }

                match backend.get_device_info() {
                    Ok(info) => Self::write_result(info.encode(writer)),
                    Err(status) => status,
                }
            }

            ManagementCommand::GetStatus => {
                if !payload.is_empty() {
                    return ManagementStatus::InvalidPayload;
                }

                match backend.get_status() {
                    Ok(status) => Self::write_result(status.encode(writer)),
                    Err(status) => status,
                }
            }

            ManagementCommand::Reboot => {
                let mut reader = WireReader::new(payload);
                let mode = match reader.read_u8().ok().and_then(RebootMode::from_u8) {
                    Some(mode) => mode,
                    None => return ManagementStatus::InvalidPayload,
                };

                if reader.finish().is_err() {
                    return ManagementStatus::InvalidPayload;
                }

                match backend.reboot(mode) {
                    Ok(()) => ManagementStatus::Ok,
                    Err(status) => status,
                }
            }

            ManagementCommand::GetActiveProfile => {
                if !payload.is_empty() {
                    return ManagementStatus::InvalidPayload;
                }

                match backend.get_active_profile() {
                    Ok(profile) => Self::write_result(writer.write_u8(profile)),
                    Err(status) => status,
                }
            }

            ManagementCommand::SetActiveProfile => {
                let mut reader = WireReader::new(payload);
                let profile = match reader.read_u8() {
                    Ok(profile) => profile,
                    Err(_) => return ManagementStatus::InvalidPayload,
                };

                if reader.finish().is_err() {
                    return ManagementStatus::InvalidPayload;
                }

                match backend.set_active_profile(profile) {
                    Ok(()) => ManagementStatus::Ok,
                    Err(status) => status,
                }
            }

            ManagementCommand::ReadProfile => {
                let mut reader = WireReader::new(payload);
                let profile = match reader.read_u8() {
                    Ok(value) => value,
                    Err(_) => return ManagementStatus::InvalidPayload,
                };
                let offset = match reader.read_u32() {
                    Ok(value) => value,
                    Err(_) => return ManagementStatus::InvalidPayload,
                };
                let requested_length = match reader.read_u16() {
                    Ok(value) => value as usize,
                    Err(_) => return ManagementStatus::InvalidPayload,
                };

                if reader.finish().is_err() || requested_length > writer.remaining().saturating_sub(2)
                {
                    return ManagementStatus::InvalidPayload;
                }

                let output = &mut writer.remaining_mut()[2..2 + requested_length];
                let length = match backend.read_profile(profile, offset, output) {
                    Ok(length) if length <= requested_length => length,
                    Ok(_) => return ManagementStatus::InternalError,
                    Err(status) => return status,
                };

                if writer.write_u16(length as u16).is_err() || writer.advance(length).is_err() {
                    return ManagementStatus::InternalError;
                }

                ManagementStatus::Ok
            }

            ManagementCommand::WriteProfile => {
                let mut reader = WireReader::new(payload);
                let profile = match reader.read_u8() {
                    Ok(value) => value,
                    Err(_) => return ManagementStatus::InvalidPayload,
                };
                let offset = match reader.read_u32() {
                    Ok(value) => value,
                    Err(_) => return ManagementStatus::InvalidPayload,
                };
                let options = match reader
                    .read_u8()
                    .ok()
                    .and_then(ProfileWriteOptions::from_bits)
                {
                    Some(value) => value,
                    None => return ManagementStatus::InvalidPayload,
                };
                let data_length = reader.remaining();
                let data = match reader.read_bytes(data_length) {
                    Ok(data) => data,
                    Err(_) => return ManagementStatus::InvalidPayload,
                };

                match backend.write_profile(profile, offset, options, data) {
                    Ok(()) => ManagementStatus::Ok,
                    Err(status) => status,
                }
            }

            _ => ManagementStatus::Unsupported,
        }
    }

    fn write_result(result: Result<(), WireWriteError>) -> ManagementStatus {
        match result {
            Ok(()) => ManagementStatus::Ok,
            Err(_) => ManagementStatus::InternalError,
        }
    }
}

impl<const RESPONSE_CAPACITY: usize> Default for ManagementService<RESPONSE_CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}
