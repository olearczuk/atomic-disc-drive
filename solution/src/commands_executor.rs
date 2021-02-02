pub mod commands_executor {
    use std::sync::{Arc};
    use crate::{AtomicRegister, ClientRegisterCommand, SectorIdx, OperationComplete, SystemRegisterCommand,
        SystemRegisterCommandContent};
    use std::collections::{HashMap};
    use uuid::Uuid;
    use tokio::sync::{Mutex, Notify};
    use tokio::sync::mpsc::{UnboundedSender, UnboundedReceiver, unbounded_channel};

    enum WorkerMessage {
        System(SystemRegisterCommand),
        Client(ClientRegisterCommand, Box<dyn FnOnce(OperationComplete) + Send + Sync>)
    }

    /// single_register_thread is responsible for processing commands by a single AtomicRegister.
    /// It conmmunicates with outside world using 2 channels:
    ///   - ready_sender is used for informing that this register is ready for new command
    ///   - cmd_recdeiver is used for receiving commands to handle
    async fn single_register_thread(index: usize, registers: Arc<Vec<Mutex<Box<dyn AtomicRegister>>>>,
                                    ready_sender: UnboundedSender<usize>, mut cmd_receiver: UnboundedReceiver<WorkerMessage>) {
        loop {
            ready_sender.send(index).unwrap_or_else(|err|
                log::error!("[register index: {}] Error while sending ready: {}", index, err)
            );
            let msg = cmd_receiver.recv().await;
            if msg.is_some() {
                let mut register = registers[index].lock().await;
                match msg.unwrap() {
                    WorkerMessage::Client(cmd, callback) =>
                        register.client_command(cmd.clone(), callback).await,
                    WorkerMessage::System(cmd) => register.system_command(cmd.clone()).await,
                };
            }
        }
    }

    /// CommandsExecutor is responsible for managing AtomicRegisters.
    /// To do so, it communiates with running single_register_thread functions (in separate threads).
    pub struct CommandsExecutor {
        /// used for notifying about release of given sector
        sector_notifiers: Vec<Arc<Notify>>,
        /// entries contain (counter, total_finished). Used for races handling.
        sector_counters: Mutex<HashMap<SectorIdx, (u64, u64)>>,
        /// msg_id -> (register that is handling this request, sectoridx)
        msg_id_handler_sector_idx: Mutex<HashMap<Uuid, (usize, SectorIdx)>>,

        /// used for communication with single_register_thread
        ready_register_receiver: Mutex<UnboundedReceiver<usize>>,
        cmd_senders: Vec<Mutex<UnboundedSender<WorkerMessage>>>,
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
                    single_register_thread(index, registers_cloned, ready_register_sender_cloned,
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

           // retrieve your sector-related counter
            let mut sector_counters = self.sector_counters.lock().await;
            if !sector_counters.contains_key(&sector_idx) {
                sector_counters.insert(sector_idx, (0, 0));
            }
            let (counter, _) = sector_counters.get_mut(&sector_idx).unwrap();
            let my_counter = *counter;
            *counter = *counter + 1;
            drop(sector_counters);

            // stay in the loop until you are "owner" of this sector
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

            // get index of ready regsiter
            let mut ready_register_receiver = self.ready_register_receiver.lock().await;
            let register_index = ready_register_receiver.recv().await;
            if register_index.is_none() {
                return;
            }
            let register_index = register_index.unwrap();
            drop(ready_register_receiver);

            // update msg_id_handler_sector_idx info
            let msg_id = Uuid::from_u128(cmd.header.request_identifier as u128);
            let mut msg_id_handler_sector_idx = self.msg_id_handler_sector_idx.lock().await;
            msg_id_handler_sector_idx.insert(msg_id, (register_index, sector_idx));
            drop(msg_id_handler_sector_idx);

            // give register command to execute
            let cmd_sender = self.cmd_senders[register_index].lock().await;
            cmd_sender.send(WorkerMessage::Client(cmd, operation_complete)).unwrap_or_else(|err|
                log::error!("[register index: {}] Error sending Client command: {}", register_index, err)
            );
        }

        pub async fn execute_system_command(&self, cmd: SystemRegisterCommand) {
            match cmd.content {
                SystemRegisterCommandContent::ReadProc | SystemRegisterCommandContent::WriteProc {..} => {
                    // get index of ready register
                    let mut ready_register_receiver = self.ready_register_receiver.lock().await;
                    let register_index = ready_register_receiver.recv().await;
                    if register_index.is_some() {
                        let register_index = register_index.unwrap();
                        drop(ready_register_receiver);

                        // give register command to execute
                        let cmd_sender = self.cmd_senders[register_index].lock().await;
                        cmd_sender.send(WorkerMessage::System(cmd)).unwrap_or_else(|err|
                            log::error!("[register index: {}] Error sending ReadProc/WriteProc command: {}", register_index, err)
                        );
                    }
                }
                SystemRegisterCommandContent::Ack | SystemRegisterCommandContent::Value {..} => {
                    // retrieve register responsible for handling this request
                    let msg_ident = cmd.header.msg_ident;
                    let msg_id_handler_sector_idx = self.msg_id_handler_sector_idx.lock().await;
                    let index = msg_id_handler_sector_idx.get(&msg_ident).cloned();
                    drop(msg_id_handler_sector_idx);

                    // here we don't wait for register to be ready, because we already know register we want
                    if let Some((register_index, _)) = index {
                        let cmd_sender = self.cmd_senders[register_index].lock().await;
                        cmd_sender.send(WorkerMessage::System(cmd)).unwrap_or_else(|err|
                            log::error!("ACK/Value [register index: {}] Error sending Ack/Value command: {}", register_index, err)
                        );
                    }
                }
            };
        }

        pub async fn finish_client_command(&self, req_id: u64) {
            let uuid = &Uuid::from_u128(req_id as u128);
            if let Some((_, sector_idx)) = self.msg_id_handler_sector_idx.lock().await.remove(&uuid) {
                // update total_finished for this sector
                let mut sector_counters = self.sector_counters.lock().await;
                let (_, total_finished) = sector_counters.get_mut(&sector_idx).unwrap();
                *total_finished = *total_finished + 1;

                // notify waiting about the change of total_finished
                let notify = self.sector_notifiers[sector_idx as usize].clone();
                notify.notify_waiters();
            }
        }
    }
}