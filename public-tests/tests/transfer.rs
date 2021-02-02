use assignment_2_solution::{deserialize_register_command, serialize_register_command, ClientCommandHeader, ClientRegisterCommand, ClientRegisterCommandContent, RegisterCommand, SectorVec, SECTOR_VEC_SIZE, SystemRegisterCommand, SystemCommandHeader, SystemRegisterCommandContent, MAGIC_NUMBER};
use ntest::timeout;
use uuid::Uuid;

#[test]
#[timeout(200)]
fn serialize_deserialize_unknown_msg_type() {
    // given
    let mut sink = MAGIC_NUMBER.to_vec();
    let mut unknown_msg_type = vec![0, 0, 0, 0x12];
    sink.append(&mut unknown_msg_type);
    let mut slice: &[u8] = &sink[..];
    let data_read: &mut dyn std::io::Read = &mut slice;

    // when
    let res = deserialize_register_command(data_read);

    // then
    res.err().expect("Should be error");
}

#[test]
#[timeout(200)]
fn serialize_deserialize_is_identity_client_read() {
    // given
    let request_identifier = 7;
    let sector_idx = 8;
    let register_cmd = RegisterCommand::Client(ClientRegisterCommand {
        header: ClientCommandHeader {
            request_identifier,
            sector_idx,
        },
        content: ClientRegisterCommandContent::Read,
    });
    let mut sink: Vec<u8> = vec![];

    // when
    serialize_register_command(&register_cmd, &mut sink).expect("Could not serialize?");
    let mut slice: &[u8] = &sink[..];
    let data_read: &mut dyn std::io::Read = &mut slice;
    let deserialized_cmd = deserialize_register_command(data_read).expect("Could not deserialize");

    // then
    match deserialized_cmd {
        RegisterCommand::Client(ClientRegisterCommand {
            header,
            content: ClientRegisterCommandContent::Read,
        }) => {
            assert_eq!(header.sector_idx, sector_idx);
            assert_eq!(header.request_identifier, request_identifier);
        }
        _ => panic!("Expected Read command"),
    }
}

#[test]
#[timeout(200)]
fn serialize_deserialize_is_identity_client_write() {
    // given
    let request_identifier = 7;
    let sector_idx = 8;
    let content_data = SectorVec(vec![2; SECTOR_VEC_SIZE]);
    let register_cmd = RegisterCommand::Client(ClientRegisterCommand {
        header: ClientCommandHeader {
            request_identifier,
            sector_idx,
        },
        content: ClientRegisterCommandContent::Write {
            data: content_data.clone(),
        },
    });
    let mut sink: Vec<u8> = vec![];

    // when
    serialize_register_command(&register_cmd, &mut sink).expect("Could not serialize?");
    let mut slice: &[u8] = &sink[..];
    let data_read: &mut dyn std::io::Read = &mut slice;
    let deserialized_cmd = deserialize_register_command(data_read).expect("Could not deserialize");

    // then
    match deserialized_cmd {
        RegisterCommand::Client(ClientRegisterCommand {
            header,
            content: ClientRegisterCommandContent::Write { data },
        }) => {
            assert_eq!(header.sector_idx, sector_idx);
            assert_eq!(header.request_identifier, request_identifier);
            assert_eq!(data, content_data);
        }
        _ => panic!("Expected Read command"),
    }
}

#[test]
#[timeout(200)]
fn serialize_deserialize_is_identity_system_read_proc() {
    // given
    let process_identifier = 7;
    let msg_ident = Uuid::new_v4();
    let read_ident = 123;
    let sector_idx = 8;

    let register_cmd = RegisterCommand::System(SystemRegisterCommand{
        header: SystemCommandHeader {
            process_identifier,
            msg_ident,
            read_ident,
            sector_idx,
        },
        content: SystemRegisterCommandContent::ReadProc
    });
    let mut sink = vec![];

    // when
    serialize_register_command(&register_cmd, &mut sink).expect("Could not serialize?");
    let mut slice: &[u8] = &sink[..];
    let data_read: &mut dyn std::io::Read = &mut slice;
    let deserialized_cmd = deserialize_register_command(data_read).expect("Could not deserialize");

    // then
    match deserialized_cmd {
        RegisterCommand::System(SystemRegisterCommand {
            header,
            content: SystemRegisterCommandContent::ReadProc,
        }) => {
            assert_eq!(header.process_identifier, process_identifier);
            assert_eq!(header.msg_ident, msg_ident);
            assert_eq!(header.read_ident, read_ident);
            assert_eq!(header.sector_idx, sector_idx);
        }
        _ => panic!("Expected Read command"),
    }
}

#[test]
#[timeout(200)]
fn serialize_deserialize_is_identity_system_value() {
    // given
    let process_identifier = 7;
    let msg_ident = Uuid::new_v4();
    let read_ident = 123;
    let sector_idx = 8;

    let timestamp_ = 12;
    let write_rank_ = 198;
    let sector_data_ = SectorVec(vec![2; SECTOR_VEC_SIZE]);

    let register_cmd = RegisterCommand::System(SystemRegisterCommand{
        header: SystemCommandHeader {
            process_identifier,
            msg_ident,
            read_ident,
            sector_idx,
        },
        content: SystemRegisterCommandContent::Value {
            timestamp: timestamp_,
            write_rank: write_rank_,
            sector_data: sector_data_.clone(),
        },
    });
    let mut sink = vec![];

    // when
    serialize_register_command(&register_cmd, &mut sink).expect("Could not serialize?");
    let mut slice: &[u8] = &sink[..];
    let data_read: &mut dyn std::io::Read = &mut slice;
    let deserialized_cmd = deserialize_register_command(data_read).expect("Could not deserialize");

    // then
    match deserialized_cmd {
        RegisterCommand::System(SystemRegisterCommand {
            header,
            content: SystemRegisterCommandContent::Value {
                timestamp, write_rank, sector_data
            },
        }) => {
            assert_eq!(header.process_identifier, process_identifier);
            assert_eq!(header.msg_ident, msg_ident);
            assert_eq!(header.read_ident, read_ident);
            assert_eq!(header.sector_idx, sector_idx);

            assert_eq!(timestamp, timestamp_);
            assert_eq!(write_rank, write_rank_);
            assert_eq!(sector_data, sector_data_);
        }
        _ => panic!("Expected Read command"),
    }
}

#[test]
#[timeout(200)]
fn serialize_deserialize_is_identity_system_write_proc() {
    // given
    let process_identifier = 7;
    let msg_ident = Uuid::new_v4();
    let read_ident = 123;
    let sector_idx = 8;

    let timestamp_ = 12;
    let write_rank_ = 198;
    let data_to_write_ = SectorVec(vec![2; SECTOR_VEC_SIZE]);

    let register_cmd = RegisterCommand::System(SystemRegisterCommand{
        header: SystemCommandHeader {
            process_identifier,
            msg_ident,
            read_ident,
            sector_idx,
        },
        content: SystemRegisterCommandContent::WriteProc {
            timestamp: timestamp_,
            write_rank: write_rank_,
            data_to_write: data_to_write_.clone(),
        },
    });
    let mut sink = vec![];

    // when
    serialize_register_command(&register_cmd, &mut sink).expect("Could not serialize?");
    let mut slice: &[u8] = &sink[..];
    let data_read: &mut dyn std::io::Read = &mut slice;
    let deserialized_cmd = deserialize_register_command(data_read).expect("Could not deserialize");

    // then
    match deserialized_cmd {
        RegisterCommand::System(SystemRegisterCommand {
            header,
            content: SystemRegisterCommandContent::WriteProc {
                timestamp, write_rank, data_to_write
            },
        }) => {
            assert_eq!(header.process_identifier, process_identifier);
            assert_eq!(header.msg_ident, msg_ident);
            assert_eq!(header.read_ident, read_ident);
            assert_eq!(header.sector_idx, sector_idx);

            assert_eq!(timestamp, timestamp_);
            assert_eq!(write_rank, write_rank_);
            assert_eq!(data_to_write, data_to_write_);
        }
        _ => panic!("Expected Read command"),
    }
}


