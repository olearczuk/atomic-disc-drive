pub mod atomic_register {
    use crate::ClientRegisterCommandContent::{Read, Write};
    use crate::SystemRegisterCommandContent::{Ack, ReadProc, Value, WriteProc};
    use crate::{
        AtomicRegister, Broadcast, ClientCommandHeader, ClientRegisterCommand, OperationComplete,
        OperationReturn, ReadReturn, RegisterClient, SectorVec, SectorsManager,
        StableStorage, StatusCode, SystemCommandHeader, SystemRegisterCommand,
        SystemRegisterCommandContent,
    };
    use serde::{Deserialize, Serialize};
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use uuid::Uuid;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct DataWithRequestIdentifier {
        request_identifier: u64,
        data: SectorVec,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct DataWithHeader {
        header: ClientCommandHeader,
        data: SectorVec,
    }

    const ATOMIC_REGISTER_STATE_KEY: &str = "state";

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct AtomicRegisterStorageState {
        read_ident: u64,
        writing: bool,
        // TODO: is option necessary?
        writeval: Option<DataWithHeader>,
    }

    pub struct AtomicRegisterImplementation {
        stable_storage: Box<dyn StableStorage>,
        register_client: Arc<dyn RegisterClient>,
        sectors_manager: Arc<dyn SectorsManager>,

        self_ident: u8,
        processes_count: usize,
        readlist: HashMap<u8, (u64, u8, SectorVec)>,
        acks: HashSet<u8>,
        reading: bool,
        // TODO: is option necessary?
        readval: Option<DataWithRequestIdentifier>,
        state: AtomicRegisterStorageState,
        callback: Option<Box<dyn FnOnce(OperationComplete) + Send + Sync>>,
    }

    impl AtomicRegisterImplementation {
        pub async fn new(
            self_ident: u8,
            stable_storage: Box<dyn StableStorage>,
            register_client: Arc<dyn RegisterClient>,
            sectors_manager: Arc<dyn SectorsManager>,
            processes_count: usize,
        ) -> (Box<dyn AtomicRegister>, Option<ClientRegisterCommand>) {
            let state_val = stable_storage.get(ATOMIC_REGISTER_STATE_KEY).await;
            let state = match state_val {
                None => AtomicRegisterStorageState {
                    read_ident: 0,
                    writing: false,
                    writeval: None,
                },
                Some(vec) => bincode::deserialize(&vec[..]).unwrap(),
            };

            let mut register = Box::new(AtomicRegisterImplementation {
                stable_storage,
                register_client,
                sectors_manager,
                self_ident,
                processes_count,
                readlist: HashMap::new(),
                acks: HashSet::new(),
                reading: false,
                state,
                readval: None,
                callback: None,
            });

            if register.state.writing {
                let writeval = register.state.writeval.clone().unwrap();
                register.state.writing = false;
                register.state.writeval = None;
                let cmd = ClientRegisterCommand {
                    header: writeval.header,
                    content: Write {
                        data: writeval.data,
                    },
                };
                return (register, Some(cmd));
            }
            (register, None)
        }

        async fn store_state(&mut self) {
            let encoded = bincode::serialize(&self.state).unwrap();
            self.stable_storage
                .put(ATOMIC_REGISTER_STATE_KEY, encoded.as_slice())
                .await.unwrap();
        }
    }

    #[async_trait::async_trait]
    impl AtomicRegister for AtomicRegisterImplementation {
        async fn client_command(
            &mut self,
            cmd: ClientRegisterCommand,
            operation_complete: Box<dyn FnOnce(OperationComplete) + Send + Sync>,
        ) {
            self.callback = Some(operation_complete);
            self.state.read_ident += 1;
            self.acks.clear();
            self.readlist.clear();
            match &cmd.content {
                Read => {
                    self.readval = Some(DataWithRequestIdentifier {
                        request_identifier: cmd.header.request_identifier,
                        data: SectorVec(vec![]),
                    });
                    self.reading = true;
                }
                Write { data } => {
                    self.state.writeval = Some(DataWithHeader {
                        header: cmd.header,
                        data: data.clone(),
                    });
                    self.state.writing = true;
                }
            }
            self.store_state().await;

            let header = SystemCommandHeader::new(
                self.self_ident,
                Uuid::from_u128(cmd.header.request_identifier as u128),
                self.state.read_ident,
                cmd.header.sector_idx,
            );
            let broadcast = Broadcast {
                cmd: Arc::new(SystemRegisterCommand {
                    header,
                    content: ReadProc,
                }),
            };
            self.register_client.broadcast(broadcast).await;
        }

        async fn system_command(&mut self, cmd: SystemRegisterCommand) {
            let sector_idx = cmd.header.sector_idx;
            match cmd.content {
                ReadProc => {
                    let value = self.sectors_manager.read_data(sector_idx).await;
                    let (timestamp, write_rank) =
                        self.sectors_manager.read_metadata(sector_idx).await;

                    let header = SystemCommandHeader::new(
                        self.self_ident,
                        cmd.header.msg_ident,
                        cmd.header.read_ident,
                        sector_idx,
                    );
                    let content =
                        SystemRegisterCommandContent::new_value(timestamp, write_rank, value);
                    let msg = crate::Send {
                        cmd: Arc::new(SystemRegisterCommand { header, content }),
                        target: usize::from(cmd.header.process_identifier),
                    };
                    self.register_client.send(msg).await;
                }
                Value {
                    timestamp,
                    write_rank,
                    sector_data,
                } => {
                    if cmd.header.read_ident != self.state.read_ident {
                        return;
                    }
                    self.readlist.insert(
                        cmd.header.process_identifier,
                        (timestamp, write_rank, sector_data),
                    );
                    if (self.reading || self.state.writing)
                        && 2 * self.readlist.len() > self.processes_count
                    {
                        let readlist_cloned = self.readlist.clone();
                        let (_, (maxts, maxrank, maxdata)) = readlist_cloned
                            .iter()
                            .max_by_key(|(_, (ts, wr, _))| (ts, wr))
                            .unwrap();
                        self.readlist.clear();
                        self.acks.clear();

                        let content = if self.reading {
                            self.readval = Some(DataWithRequestIdentifier {
                                request_identifier: self.readval.clone().unwrap().request_identifier,
                                data: maxdata.clone(),
                            });

                            SystemRegisterCommandContent::new_write_proc(
                                *maxts,
                                *maxrank,
                                self.readval.clone().unwrap().data,
                            )
                        } else {
                            SystemRegisterCommandContent::new_write_proc(
                                *maxts + 1,
                                self.self_ident,
                                self.state.writeval.clone().unwrap().data,
                            )
                        };
                        self.state.read_ident += 1;
                        self.store_state().await;

                        let header = SystemCommandHeader::new(
                            self.self_ident,
                            cmd.header.msg_ident,
                            self.state.read_ident,
                            sector_idx,
                        );

                        let broadcast = Broadcast {
                            cmd: Arc::new(SystemRegisterCommand { header, content }),
                        };
                        self.register_client.broadcast(broadcast).await;
                    }
                }
                WriteProc {
                    timestamp,
                    write_rank,
                    data_to_write,
                } => {
                    let (ts, wr) = self.sectors_manager.read_metadata(sector_idx).await;
                    if (timestamp, write_rank) > (ts, wr) {
                        self.sectors_manager
                            .write(sector_idx, &(data_to_write, timestamp, write_rank))
                            .await;
                    }

                    let header = SystemCommandHeader::new(
                        self.self_ident,
                        cmd.header.msg_ident,
                        cmd.header.read_ident,
                        sector_idx,
                    );
                    let msg = crate::Send {
                        cmd: Arc::new(SystemRegisterCommand {
                            header,
                            content: Ack,
                        }),
                        target: usize::from(cmd.header.process_identifier),
                    };
                    self.register_client.send(msg).await;
                }
                Ack => {
                    if cmd.header.read_ident != self.state.read_ident {
                        return;
                    }

                    self.acks.insert(cmd.header.process_identifier);
                    if (self.reading || self.state.writing)
                        && 2 * self.acks.len() > self.processes_count
                    {
                        self.acks.clear();
                        let (request_identifier, op_return) = if self.reading {
                            self.reading = false;
                            (
                                self.readval.clone().unwrap().request_identifier,
                                OperationReturn::Read(ReadReturn {
                                    read_data: Some(self.readval.clone().unwrap().data),
                                }),
                            )
                        } else {
                            self.state.writing = false;
                            self.store_state().await;
                            (
                                self.state
                                    .writeval
                                    .clone()
                                    .unwrap()
                                    .header
                                    .request_identifier,
                                OperationReturn::Write,
                            )
                        };
                        let op = OperationComplete {
                            status_code: StatusCode::Ok,
                            request_identifier,
                            op_return,
                        };
                        if self.callback.is_some() {
                            (self.callback.take().unwrap())(op);
                        }
                        self.callback = None;
                    }
                }
            }
        }
    }
}
