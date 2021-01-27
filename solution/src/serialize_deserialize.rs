pub mod serialize_deserialize {
    use std::io::{Error, Read, ErrorKind, Write};
    use crate::{ClientCommandHeader, ClientRegisterCommand, SectorVec, ClientRegisterCommandContent, RegisterCommand, SystemRegisterCommand, SystemRegisterCommandContent, SystemCommandHeader, SECTOR_VEC_SIZE};
    use crate::RegisterCommand::{Client, System};
    use uuid::Uuid;
    use std::convert::TryInto;

    pub const READ: u8 = 0x01;
    pub const WRITE: u8 =0x02;

    pub fn serialize_client_command(cmd: &ClientRegisterCommand, writer: &mut dyn Write) -> Result<(), Error> {
        let mut content = vec![0, 0, 0];
        let msg_type = match &cmd.content {
            ClientRegisterCommandContent::Read => READ,
            ClientRegisterCommandContent::Write {..} => WRITE,
        };
        content.push(msg_type);

        let mut request_identifier = cmd.header.request_identifier.to_be_bytes().to_vec();
        let mut sector_idx = cmd.header.sector_idx.to_be_bytes().to_vec();

        content.append(&mut request_identifier);
        content.append(&mut sector_idx);

        if let ClientRegisterCommandContent::Write { data } = &cmd.content {
            let mut data_cloned = data.0.clone();
            content.append(&mut data_cloned);
        }
        writer.write_all(&content)
    }

    pub fn deserialize_client_command(data: &mut dyn Read, msg_type: u8) -> Option<Result<RegisterCommand, Error>> {
        if msg_type != READ && msg_type != WRITE {
            return None;
        }

        let mut buffer: [u8; 16] = [0; 16];
        if let Err(err) = data.read_exact(&mut buffer) {
            return Some(Err(err));
        }

        let request_identifier = u64::from_be_bytes(buffer[..8].try_into().expect("request_identifier"));
        let sector_idx = u64::from_be_bytes(buffer[8..].try_into().expect("sector_idx"));
        let header = ClientCommandHeader { request_identifier, sector_idx };
        match msg_type {
            READ => {
                Some(Ok(Client(ClientRegisterCommand {
                    header,
                    content: ClientRegisterCommandContent::Read,
                })))
            },
            WRITE => {
                let mut content = SectorVec::new();
                if let Err(err) = data.read_exact(content.0.as_mut_slice()) {
                    return Some(Err(err));
                }
                Some(Ok(Client(ClientRegisterCommand {
                    header,
                    content: ClientRegisterCommandContent::Write { data: content, }
                })))
            },
            _ => {
                // this should never happen because of if in the beginning
                Some(Err(Error::new(ErrorKind::Other, format!("[UNEXPECTED] unknown client msg_type: {}", msg_type))))
            }
        }
    }

    const READ_PROC: u8 = 0x03;
    const VALUE: u8 = 0x04;
    const WRITE_PROC: u8 = 0x05;
    const ACK: u8 = 0x06;

    pub fn serialize_system_command(cmd: &SystemRegisterCommand, writer: &mut dyn Write) -> Result<(), Error> {
        let mut content = vec![0, 0, cmd.header.process_identifier];
        let msg_type = match &cmd.content {
            SystemRegisterCommandContent::ReadProc => READ_PROC,
            SystemRegisterCommandContent::Value{..} => VALUE,
            SystemRegisterCommandContent::WriteProc {..} => WRITE_PROC,
            SystemRegisterCommandContent::Ack => ACK,
        };
        content.push(msg_type);

        let mut msg_ident_vec = cmd.header.msg_ident.as_bytes().to_vec();
        let mut read_ident_vec = cmd.header.read_ident.to_be_bytes().to_vec();
        let mut sector_idx_vec = cmd.header.sector_idx.to_be_bytes().to_vec();

        content.append(&mut msg_ident_vec);
        content.append(&mut read_ident_vec);
        content.append(&mut sector_idx_vec);

        match &cmd.content {
            SystemRegisterCommandContent::ReadProc => {},
            SystemRegisterCommandContent::Value { timestamp, write_rank, sector_data } => {
                let mut timestamp_vec = timestamp.to_be_bytes().to_vec();
                let mut padding = vec![0; 7];
                let mut sector_data_vec = sector_data.0.clone();

                content.append(&mut timestamp_vec);
                content.append(&mut padding);
                content.push(*write_rank);
                content.append(&mut sector_data_vec);
            },
            SystemRegisterCommandContent::WriteProc { timestamp, write_rank, data_to_write} => {
                let mut timestamp_vec = timestamp.to_be_bytes().to_vec();
                let mut padding = vec![0; 7];
                let mut data_to_write_vec = data_to_write.0.clone();

                content.append(&mut timestamp_vec);
                content.append(&mut padding);
                content.push(*write_rank);
                content.append(&mut data_to_write_vec);

            },
            SystemRegisterCommandContent::Ack => {},
        }
        writer.write_all(&content)
    }

    pub fn deserialize_system_command(data: &mut dyn Read, process_identifier: u8, msg_type: u8) -> Result<RegisterCommand, Error> {
        if msg_type != READ_PROC && msg_type != VALUE && msg_type != WRITE_PROC && msg_type != ACK {
            return Err(Error::new(ErrorKind::Other, format!("unknown system msg_type: {}", msg_type)));
        }

        let mut buffer: [u8; 32] = [0; 32];
        if let Err(err) = data.read_exact(&mut buffer) {
            return Err(err);
        }

        let msg_ident = Uuid::from_bytes(buffer[..16].try_into().expect("msg_ident"));
        let read_ident = u64::from_be_bytes(buffer[16..24].try_into().expect("read_ident"));
        let sector_idx = u64::from_be_bytes(buffer[24..].try_into().expect("sector_idx"));
        let header = SystemCommandHeader { process_identifier, msg_ident, read_ident, sector_idx, };
        match msg_type {
            READ_PROC => {
                Ok(System(SystemRegisterCommand{
                    header,
                    content: SystemRegisterCommandContent::ReadProc,
                }))
            },
            VALUE => {
                let mut buffer: [u8; 16 + SECTOR_VEC_SIZE] = [0; 16 + SECTOR_VEC_SIZE];
                if let Err(err) = data.read_exact(&mut buffer) {
                    return Err(err);
                }
                let timestamp = u64::from_be_bytes(buffer[..8].try_into().expect("timestamp"));
                let write_rank = buffer[15];
                let sector_data = SectorVec(buffer[16..].to_vec());

                Ok(System(SystemRegisterCommand{
                    header,
                    content: SystemRegisterCommandContent::Value {
                        timestamp,
                        write_rank,
                        sector_data
                    }
                }))
            },
            WRITE_PROC => {
                let mut buffer: [u8; 16 + SECTOR_VEC_SIZE] = [0; 16 + SECTOR_VEC_SIZE];
                if let Err(err) = data.read_exact(&mut buffer) {
                    return Err(err);
                }
                let timestamp = u64::from_be_bytes(buffer[..8].try_into().expect("timestamp"));
                let write_rank = buffer[15];
                let data_to_write = SectorVec(buffer[16..].to_vec());

                Ok(System(SystemRegisterCommand{
                    header,
                    content: SystemRegisterCommandContent::WriteProc {
                        timestamp,
                        write_rank,
                        data_to_write,
                    }
                }))
            },
            ACK => {
                Ok(System(SystemRegisterCommand{
                    header,
                    content: SystemRegisterCommandContent::Ack,
                }))
            },
            _ => {
                // this should never happen because of if in the beginning
                Err(Error::new(ErrorKind::Other, format!("[UNEXPECTED] unknown system msg_type: {}", msg_type)))
            }
        }
    }
}