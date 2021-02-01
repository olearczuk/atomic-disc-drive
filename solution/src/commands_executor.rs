pub mod commands_executor {
    use std::sync::{Arc};
    use crate::{AtomicRegister, ClientRegisterCommand, SectorIdx, OperationComplete, SystemRegisterCommand, SystemRegisterCommandContent, RegisterCommand};
    use std::collections::{HashMap};
    use uuid::Uuid;
    use tokio::sync::{Mutex, MutexGuard, Notify};
    use tokio::sync::mpsc::{UnboundedSender, UnboundedReceiver, unbounded_channel};
    use std::mem::{size_of, size_of_val};

    pub struct CommandsExecutor {
        sector_notifiers: Vec<Arc<Notify>>,
        sector_counters: Mutex<HashMap<SectorIdx, (u64, u64)>>,
        ready_register_receiver: Mutex<UnboundedReceiver<usize>>,
        cmd_senders: Vec<Mutex<UnboundedSender<WorkerMessage>>>,
        msg_id_handler_sector_idx: Mutex<HashMap<Uuid, (usize, SectorIdx)>>,
    }

    enum WorkerMessage {
        System(SystemRegisterCommand),
        Client(ClientRegisterCommand, Box<dyn FnOnce(OperationComplete) + Send + Sync>)
    }

    async fn worker_thread(index: usize, registers: Arc<Vec<Mutex<Box<dyn AtomicRegister>>>>,
                           ready_sender: UnboundedSender<usize>, mut cmd_receiver: UnboundedReceiver<WorkerMessage>) {
        loop {
            ready_sender.send(index).unwrap();
            let msg = cmd_receiver.recv().await.unwrap();
            let mut register = registers[index].lock().await;
            match msg {
                WorkerMessage::Client(cmd, callback) =>
                    register.client_command(cmd.clone(), callback).await,
                WorkerMessage::System(cmd) => register.system_command(cmd.clone()).await,
            };
        }
    }

    impl CommandsExecutor {
        pub fn new(registers: Vec<Mutex<Box<dyn AtomicRegister>>>, max_sector: u64) -> Arc<Self> {
            let (ready_register_sender, ready_register_receiver) = unbounded_channel();

            let registers = Arc::new(registers);
            let mut cmd_senders = vec![];
            for index in 0..registers.clone().len(){
                let (cmd_sender, cmd_receiver) = unbounded_channel();
                cmd_senders.push(Mutex::new(cmd_sender));

                let ready_register_sender_cloned = ready_register_sender.clone();
                let registers_cloned = registers.clone();
                tokio::spawn(async move {
                    worker_thread(index, registers_cloned, ready_register_sender_cloned,
                        cmd_receiver).await;
                });
            }

            let mut sector_notifiers = vec![];
            for _ in 0..max_sector {
                sector_notifiers.push(Arc::new(Notify::new()));
            }

            Arc::new(CommandsExecutor {
                sector_notifiers,
                sector_counters: Mutex::new(HashMap::new()),
                ready_register_receiver: Mutex::new(ready_register_receiver),
                cmd_senders,
                msg_id_handler_sector_idx: Mutex::new(HashMap::new()),
            })
        }

        pub async fn execute_client_command(&self, cmd: ClientRegisterCommand,
                                        operation_complete: Box<dyn FnOnce(OperationComplete) + Send + Sync>) {
            let sector_idx = cmd.header.sector_idx;
            {
                let mut sector_counters = self.sector_counters.lock().await;
                if !sector_counters.contains_key(&sector_idx) {
                    sector_counters.insert(sector_idx, (0, 0));
                }
            }
            let mut sector_counters = self.sector_counters.lock().await;
            let (counter, total_finished) = sector_counters.get_mut(&sector_idx).unwrap();
            let my_counter = *counter;
            *counter = *counter + 1;
            drop(sector_counters);

            loop {
                let sector_counters = self.sector_counters.lock().await;
                let (_, total_finished) = sector_counters.get(&sector_idx).unwrap();
                if my_counter == *total_finished {
                    break;
                }
                drop(sector_counters);
                let notify = self.sector_notifiers[sector_idx as usize].clone();
                notify.notified().await;
            }

            let mut ready_register_receiver = self.ready_register_receiver.lock().await;
            let register_index = ready_register_receiver.recv().await.unwrap();
            drop(ready_register_receiver);


            let req_id = Uuid::from_u128(cmd.header.request_identifier as u128);
            let mut msg_id_handler_sector_idx = self.msg_id_handler_sector_idx.lock().await;
            msg_id_handler_sector_idx.insert(req_id, (register_index, sector_idx));
            drop(msg_id_handler_sector_idx);

            let cmd_sender = self.cmd_senders[register_index].lock().await;
            cmd_sender.send(WorkerMessage::Client(cmd, operation_complete)).unwrap_or_default();
        }

        pub async fn execute_system_command(&self, cmd: SystemRegisterCommand) {
            match cmd.content {
                SystemRegisterCommandContent::ReadProc | SystemRegisterCommandContent::WriteProc {..} => {
                    let mut ready_register_receiver = self.ready_register_receiver.lock().await;
                    let register_index = ready_register_receiver.recv().await.unwrap();
                    drop(ready_register_receiver);

                    let cmd_sender = self.cmd_senders[register_index].lock().await;
                    cmd_sender.send(WorkerMessage::System(cmd)).unwrap_or_default();
                }
                SystemRegisterCommandContent::Ack | SystemRegisterCommandContent::Value {..} => {
                    let msg_ident = cmd.header.msg_ident;
                    let msg_id_handler_sector_idx = self.msg_id_handler_sector_idx.lock().await;
                    let index = msg_id_handler_sector_idx.get(&msg_ident).cloned();
                    drop(msg_id_handler_sector_idx);

                    if let Some((register_index, _)) = index {
                        // here we don't wait for workers to be ready, we know which worker we need
                        // after free register was found, it might turn out that request has already completed
                        let msg_id_handler_sector_idx = self.msg_id_handler_sector_idx.lock().await;
                        let index = msg_id_handler_sector_idx.get(&msg_ident).cloned();
                        drop(msg_id_handler_sector_idx);

                        if index.is_some() {
                            let cmd_sender = self.cmd_senders[register_index].lock().await;
                            cmd_sender.send(WorkerMessage::System(cmd)).unwrap_or_default();
                        }
                    }
                }
            };
        }

        pub async fn finish_client_command(&self, req_id: u64) {
            let uuid = &Uuid::from_u128(req_id as u128);
            if let Some((_, sector_idx)) = self.msg_id_handler_sector_idx.lock().await.remove(&uuid) {
                let mut sector_counters = self.sector_counters.lock().await;
                let (counter, total_finished) = sector_counters.get_mut(&sector_idx).unwrap();
                *total_finished = *total_finished + 1;
                let notify = self.sector_notifiers[sector_idx as usize].clone();
                notify.notify_waiters();
            }
        }
    }
}