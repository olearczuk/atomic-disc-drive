use assignment_2_solution::{build_atomic_register, build_sectors_manager, Broadcast, ClientCommandHeader, ClientRegisterCommand, ClientRegisterCommandContent, RegisterClient, StableStorage, SystemRegisterCommand, SystemCommandHeader, SystemRegisterCommandContent, SectorVec, build_stable_storage, AtomicRegister, OperationComplete, StatusCode, OperationReturn, SectorIdx};
use async_channel::{unbounded, Sender, Receiver};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use uuid::Uuid;
use std::path::PathBuf;

async fn get_register(path: PathBuf, processes_count: usize) -> (Box<dyn AtomicRegister>, Receiver<ClientMsg>, Option<ClientRegisterCommand>) {
    let sectors_manager = build_sectors_manager(path.clone());
    let storage = build_stable_storage(path.clone()).await;
    let (tx, rx) = unbounded();
    let (register, pending_cmd) = build_atomic_register(
        1,
        storage,
        Arc::new(DummyRegisterClient::new(tx)),
        sectors_manager,
        processes_count,
    ).await;
    (register, rx, pending_cmd)
}

#[tokio::test]
async fn test_read_whole_procedure() {
    let path = tempdir().unwrap().into_path();
    let (register, rx, cmd) = get_register(path.clone(), 3).await;
    assert!(matches!(cmd, None));

    let operation_complete = Box::new(|op_complete| {
        match op_complete {
            OperationComplete { status_code, request_identifier, op_return } => {
                assert_eq!(status_code, StatusCode::Ok);
                assert_eq!(request_identifier, 7);
                match op_return {
                    OperationReturn::Read(read_return) => {
                        assert_eq!(read_return.read_data.unwrap().0, vec![3; 4096]);
                        // way to announce callback was called
                        std::fs::write(PathBuf::from("./some_test_file"), [1, 2, 3]).unwrap();
                    }
                    OperationReturn::Write => panic!("not matching")
                };
            },
        };
    });
    let sector_idx = 0;
    let request_identifier = 7;

    let register = make_read(register, rx.clone(), 1, request_identifier,
        request_identifier, operation_complete).await;

    {
        // testing Read side effects - updating read_ident
        let (register, rx, cmd)  = get_register(path.clone(), 1).await;
        assert!(matches!(cmd, None));

        // expected_read_ident = 2, because make_read changed from 0 to 1 and make_read adds 1 more
        make_read(register, rx.clone(), 2, request_identifier, sector_idx, Box::new(|_|{})).await;
    }

    let register = make_values(register, rx, request_identifier, sector_idx,
        1, 2, 6, vec![3; 4096]).await;

    {
        // testing Values side effects - updating ident
        // expected_read_ident = 3, because make_values changed from 1 to 2 and make_read adds 1 more
        let (register, rx, cmd) = get_register(path.clone(), 3).await;

        // cmd unchanged
        assert!(matches!(cmd, None));
        make_read(register, rx.clone(), 3, request_identifier, sector_idx, Box::new(|_|{})).await;
    }

    make_acks(register, 2).await;
    // checking if callback was called
    assert_eq!(std::fs::read(PathBuf::from("./some_test_file")).unwrap(), [1, 2, 3]);
    std::fs::remove_file("./some_test_file").unwrap();
}

#[tokio::test]
async fn test_write_whole_procedure() {
    let path = tempdir().unwrap().into_path();
    let (register, rx, cmd) = get_register(path.clone(), 3).await;
    assert!(matches!(cmd, None));

    let operation_complete = Box::new(|op_complete| {
        match op_complete {
            OperationComplete { status_code, request_identifier, op_return } => {
                assert_eq!(status_code, StatusCode::Ok);
                assert_eq!(request_identifier, 7);
                assert!(matches!(op_return, OperationReturn::Write));
                // way to announce callback was called
                std::fs::write(PathBuf::from("./some_test_file"), [1, 2, 3]).unwrap();
            },
        };
    });
    let sector_idx = 0;
    let request_identifier = 7;

    let register = make_write(register, rx.clone(), request_identifier, sector_idx,
        1, operation_complete).await;
    {
        // testing Write side effects - updating registers State
        let (register, rx, cmd)  = get_register(path.clone(), 1).await;
        let cmd = cmd.unwrap();
        assert_eq!(cmd.header.sector_idx, sector_idx);
        assert_eq!(cmd.header.request_identifier, request_identifier);
        match cmd.content {
            ClientRegisterCommandContent::Write { data } => {
                assert_eq!(data.0, vec![2; 4096]);
            },
            ClientRegisterCommandContent::Read => panic!("not matching"),
        }

        // expected_read_ident = 2, because make_wrote changed from 0 to 1 and make_read adds 1 more
        make_read(register, rx.clone(), 2, request_identifier, sector_idx, Box::new(|_|{})).await;
    }

    let register = make_values(register, rx, request_identifier, sector_idx,
        1, 3, 1, vec![2; 4096]).await;

    {
        // testing Values side effects - updating ident
        let (register, rx, cmd) = get_register(path.clone(), 3).await;

        // cmd unchanged
        let cmd = cmd.unwrap();
        assert_eq!(cmd.header.sector_idx, sector_idx);
        assert_eq!(cmd.header.request_identifier, request_identifier);
        match cmd.content {
            ClientRegisterCommandContent::Write { data } => {
                assert_eq!(data.0, vec![2; 4096]);
            },
            ClientRegisterCommandContent::Read => panic!("not matching"),
        }

        // expected_read_ident = 3, because make_values changed from 1 to 2 and make_read adds 1 more
        make_read(register, rx.clone(), 3, request_identifier, sector_idx, Box::new(|_|{})).await;
    }

    make_acks(register, 2).await;
    {
        // testing ACKs side effects - writing = false (so creating register results in cmd = None)
        let (_, _, cmd) = get_register(path.clone(), 3).await;
        assert!(matches!(cmd, None));
    }

    // checking if callback was called
    assert_eq!(std::fs::read(PathBuf::from("./some_test_file")).unwrap(), [1, 2, 3]);
    std::fs::remove_file("./some_test_file").unwrap();
}

#[tokio::test]
async fn test_write_proc_side_effects() {
    let path = tempdir().unwrap().into_path();
    let (register, rx, cmd) = get_register(path.clone(), 1).await;
    assert!(matches!(cmd, None));

    make_write_proc(register, rx.clone()).await;

    // check if WRITE_PROC properly updated and stored data/metadata and if READ_PROC properly read it
    let (register, rx, cmd) = get_register(path.clone(), 1).await;
    assert!(matches!(cmd, None));
    make_read_proc(register, rx.clone(), 1, 2, vec![5; 4096]).await;

    // check if WRITE_PROC did not change ident using read
    // testing Values side effects - updating ident
    let (register, rx, cmd) = get_register(path.clone(), 3).await;

    // cmd unchanged
    assert!(matches!(cmd, None));

    // expected_read_ident = 0, because read_ident has not been changed and make_read adds 1 to it
    make_read(register, rx.clone(), 1, 0, 0, Box::new(|_|{})).await;
}

async fn make_read(mut register: Box<dyn AtomicRegister>, rx: Receiver<ClientMsg>, expected_read_ident: u64,
                   request_identifier: u64, sector_idx: SectorIdx,
                   operation_complete: Box<dyn FnOnce(OperationComplete) + Send + Sync>) -> Box<dyn AtomicRegister> {
    // when
    register.client_command(
        ClientRegisterCommand {
            header: ClientCommandHeader {
                request_identifier,
                sector_idx,
            },
            content: ClientRegisterCommandContent::Read,
        }, operation_complete, ).await;

    // then
    match rx.recv().await {
        Ok(ClientMsg::Broadcast(Broadcast{cmd})) => {
            assert_eq!(cmd.header.process_identifier, 1);
            assert_eq!(cmd.header.msg_ident, Uuid::from_u128(request_identifier as u128));
            assert_eq!(cmd.header.read_ident, expected_read_ident);
            assert_eq!(cmd.header.sector_idx, sector_idx);
            assert!(matches!(cmd.content, SystemRegisterCommandContent::ReadProc));
        },
        _ => panic!("not matching"),
    };
    register
}

async fn make_read_proc(mut register: Box<dyn AtomicRegister>, rx: Receiver<ClientMsg>, exp_timestamp: u64,
                        exp_write_rank: u8, exp_sector_data: Vec<u8>) -> Box<dyn AtomicRegister>{
    // when
    register.system_command(
        SystemRegisterCommand {
            header: SystemCommandHeader {
                process_identifier: 0,
                msg_ident: Uuid::from_u128(7),
                read_ident: 12,
                sector_idx: 0,
            },
            content: SystemRegisterCommandContent::ReadProc,
        }
    ).await;

    // then
    match rx.recv().await {
        Ok(ClientMsg::Send(assignment_2_solution::Send{cmd, target})) => {
            assert_eq!(cmd.header.process_identifier, 1);
            assert_eq!(cmd.header.msg_ident, Uuid::from_u128(7));
            assert_eq!(cmd.header.read_ident, 12);
            assert_eq!(cmd.header.sector_idx, 0);
            assert_eq!(target, 0);
            match cmd.content.clone() {
                SystemRegisterCommandContent::Value { timestamp, write_rank, sector_data } => {
                    assert_eq!(timestamp, exp_timestamp);
                    assert_eq!(write_rank, exp_write_rank);
                    assert_eq!(sector_data.0, exp_sector_data);
                },
                _ => panic!("not matching"),
            }
        },
        _ => panic!("not matching"),
    };
    register
}

async fn make_write(mut register: Box<dyn AtomicRegister>, rx: Receiver<ClientMsg>, request_identifier: u64, sector_idx: u64,
                    expected_read_ident: u64, operation_complete: Box<dyn FnOnce(OperationComplete) + Send + Sync>) -> Box<dyn AtomicRegister> {
    // when
    register.client_command(
        ClientRegisterCommand {
            header: ClientCommandHeader {
                request_identifier,
                sector_idx,
            },
            content: ClientRegisterCommandContent::Write {data: SectorVec(vec![2; 4096])},
        }, operation_complete).await;

    // then
    match rx.recv().await {
        Ok(ClientMsg::Broadcast(Broadcast{cmd})) => {
            assert_eq!(cmd.header.process_identifier, 1);
            assert_eq!(cmd.header.msg_ident, Uuid::from_u128(7));
            assert_eq!(cmd.header.read_ident, expected_read_ident);
            assert_eq!(cmd.header.sector_idx, 0);
            assert!(matches!(cmd.content, SystemRegisterCommandContent::ReadProc));
        },
        _ => panic!("not matching"),
    };
    register
}

async fn make_values(mut register: Box<dyn AtomicRegister>, rx: Receiver<ClientMsg>, request_identifier: u64, sector_idx: SectorIdx,
                     read_ident: u64, exp_timestamp: u64, exp_write_rank: u8, exp_sector_data: Vec<u8>) -> Box<dyn AtomicRegister>{
    // when
    register.system_command(SystemRegisterCommand {
        header: SystemCommandHeader {
            process_identifier: 2,
            msg_ident: Uuid::from_u128(request_identifier as u128),
            read_ident,
            sector_idx,
        },
        content: SystemRegisterCommandContent::Value {
            timestamp: 2,
            write_rank: 4,
            sector_data: SectorVec(vec![2; 4096])
        }
    }).await;

    register.system_command(SystemRegisterCommand {
        header: SystemCommandHeader {
            process_identifier: 3,
            msg_ident: Uuid::from_u128(7),
            read_ident,
            sector_idx,
        },
        content: SystemRegisterCommandContent::Value {
            timestamp: 2,
            write_rank: 6,
            sector_data: SectorVec(vec![3; 4096])
        }
    }).await;

    //then
    match rx.recv().await {
        Ok(ClientMsg::Broadcast(Broadcast{cmd})) => {
            assert_eq!(cmd.header.process_identifier, 1);
            assert_eq!(cmd.header.sector_idx, sector_idx);
            assert_eq!(cmd.header.read_ident, read_ident + 1);
            assert_eq!(cmd.header.msg_ident, Uuid::from_u128(7));
            match cmd.content.clone() {
                SystemRegisterCommandContent::WriteProc { timestamp, write_rank, data_to_write } => {
                    assert_eq!(timestamp, exp_timestamp);
                    assert_eq!(write_rank, exp_write_rank);
                    assert_eq!(data_to_write.0, exp_sector_data);
                },
                _ => panic!("not matching"),
            }
        },
        _ => panic!("not matching"),
    };
    register
}

async fn make_write_proc(mut register: Box<dyn AtomicRegister>, rx: Receiver<ClientMsg>)
    -> Box<dyn AtomicRegister> {
    // when
    register.system_command(
        SystemRegisterCommand {
            header: SystemCommandHeader {
                process_identifier: 0,
                msg_ident: Uuid::from_u128(7),
                read_ident: 12,
                sector_idx: 0,
            },
            content: SystemRegisterCommandContent::WriteProc {
                timestamp: 1,
                write_rank: 2,
                data_to_write: SectorVec(vec![5; 4096])
            }
        }
    ).await;

    // then
    match rx.recv().await {
        Ok(ClientMsg::Send(assignment_2_solution::Send{cmd, target})) => {
            assert_eq!(cmd.header.process_identifier, 1);
            assert_eq!(cmd.header.msg_ident, Uuid::from_u128(7));
            assert_eq!(cmd.header.read_ident, 12);
            assert_eq!(cmd.header.sector_idx, 0);
            assert_eq!(target, 0);
            assert!(matches!(cmd.content, SystemRegisterCommandContent::Ack));
        },
        _ => panic!("not matching"),
    };
    register
}

async fn make_acks(mut register: Box<dyn AtomicRegister>, read_ident: u64) {
    // when
    register.system_command(SystemRegisterCommand {
        header: SystemCommandHeader {
            process_identifier: 2,
            msg_ident: Uuid::from_u128(7),
            read_ident,
            sector_idx: 0,
        },
        content: SystemRegisterCommandContent:: Ack,
    }).await;

    register.system_command(SystemRegisterCommand {
        header: SystemCommandHeader {
            process_identifier: 3,
            msg_ident: Uuid::from_u128(7),
            read_ident,
            sector_idx: 0,
        },
        content: SystemRegisterCommandContent::Ack,
    }).await;
}

enum ClientMsg {
    Send(assignment_2_solution::Send),
    Broadcast(Broadcast),
}

struct DummyRegisterClient {
    tx: Sender<ClientMsg>,
}

impl DummyRegisterClient {
    fn new(tx: Sender<ClientMsg>) -> Self {
        Self { tx }
    }
}

#[async_trait::async_trait]
impl RegisterClient for DummyRegisterClient {
    async fn send(&self, msg: assignment_2_solution::Send) {
        self.tx.send(ClientMsg::Send(msg)).await.unwrap();
    }

    async fn broadcast(&self, msg: Broadcast) {
        self.tx.send(ClientMsg::Broadcast(msg)).await.unwrap();
    }
}

#[derive(Clone, Default)]
struct RamStableStorage {
    map: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

#[async_trait::async_trait]
impl StableStorage for RamStableStorage {
    async fn put(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
        let mut map = self.map.lock().unwrap();
        map.insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    async fn get(&self, key: &str) -> Option<Vec<u8>> {
        let map = self.map.lock().unwrap();
        map.get(key).map(Clone::clone)
    }
}
