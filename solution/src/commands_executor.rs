pub mod commands_executor {
    use std::sync::{Arc, Mutex, Condvar};
    use crate::{AtomicRegister, ClientRegisterCommand, SectorIdx, OperationComplete, SystemRegisterCommand, SystemRegisterCommandContent};
    use std::collections::{HashMap, HashSet};
    use uuid::Uuid;

    enum RegisterIndex {
        Any,
        Some(usize),
    }

    pub struct CommandsExecutor {
        registers: Vec<Mutex<Box<dyn AtomicRegister>>>,

        busy_sectors: Mutex<HashSet<SectorIdx>>,
        busy_sectors_cvar: Condvar,

        busy_registers: Mutex<HashSet<usize>>,
        busy_registers_cvar: Condvar,

        msg_id_handler: Mutex<HashMap<Uuid, usize>>,
    }

    impl CommandsExecutor {
        pub fn new(registers: Vec<Mutex<Box<dyn AtomicRegister>>>) -> Arc<Self> {
            Arc::new(CommandsExecutor {
                registers: registers,
                busy_sectors: Mutex::new(HashSet::new()),
                busy_sectors_cvar: Condvar::new(),
                busy_registers: Mutex::new(HashSet::new()),
                busy_registers_cvar: Condvar::new(),
                msg_id_handler: Mutex::new(HashMap::new()),
            })
        }

        pub async fn execute_client_command(&self, cmd: ClientRegisterCommand,
                                        operation_complete: Box<dyn FnOnce(OperationComplete) + Send + Sync>) {
            let sector_idx = cmd.header.sector_idx;
            let guard = self.busy_sectors_cvar.wait_while(self.busy_sectors.lock().unwrap(), |busy_sectors| {
                !busy_sectors.insert(sector_idx)
            }).unwrap();
            drop(guard);

            let register_index = self.acquire_register_index(RegisterIndex::Any);

            let req_id = Uuid::from_u128(cmd.header.request_identifier as u128);
            let mut msg_id_handler = self.msg_id_handler.lock().unwrap();
            msg_id_handler.insert(req_id, register_index);
            drop(msg_id_handler);

            let mut register = self.registers[register_index].lock().unwrap();
            register.client_command(cmd, operation_complete).await;

            // release register and notify waiting candidates
            self.busy_registers.lock().unwrap().remove(&register_index);
            self.busy_registers_cvar.notify_all();
        }

        pub async fn execute_system_command(&self, cmd: SystemRegisterCommand) {
            let register_index = match cmd.content {
                SystemRegisterCommandContent::ReadProc | SystemRegisterCommandContent::WriteProc {..} => {
                    let register_index = self.acquire_register_index(RegisterIndex::Any);
                    let mut register = self.registers[register_index].lock().unwrap();
                    register.system_command(cmd).await;
                    Some(register_index)
                }
                SystemRegisterCommandContent::Ack | SystemRegisterCommandContent::Value {..} => {
                    let msg_ident = cmd.header.msg_ident;
                    let msg_id_handler = self.msg_id_handler.lock().unwrap();
                    let index = msg_id_handler.get(&msg_ident).cloned();
                    drop(msg_id_handler);

                    if let Some(register_index) = index {
                        self.acquire_register_index(RegisterIndex::Some(register_index));

                        // after waking up request related to given Ack/Value might have finished already
                        let msg_id_handler = self.msg_id_handler.lock().unwrap();
                        let index = msg_id_handler.get(&msg_ident).cloned();
                        drop(msg_id_handler);
                        if index.is_some() {
                            let mut register = self.registers[register_index].lock().unwrap();
                            register.system_command(cmd).await;
                        }
                        Some(register_index)
                    } else {
                        None
                    }
                }
            };
            // release register and notify waiting candidates
            if let Some(register_index) = register_index {
                self.busy_registers.lock().unwrap().remove(&register_index);
                self.busy_registers_cvar.notify_all();
            }
        }

        pub fn finish_client_command(&self, req_id: u64, sector_idx: SectorIdx) {
            self.msg_id_handler.lock().unwrap().remove(&Uuid::from_u128(req_id as u128));
            self.busy_sectors.lock().unwrap().remove(&sector_idx);
            self.busy_sectors_cvar.notify_all();
        }

        fn acquire_register_index(&self, register: RegisterIndex) -> usize {
            let mut register_index = 0;
            let _ = self.busy_registers_cvar.wait_while(
                self.busy_registers.lock().unwrap(),
                |busy_registers| {
                    match register {
                        RegisterIndex::Any => {
                            for index in 0..self.registers.len() {
                                if !busy_registers.contains(&index) {
                                    busy_registers.insert(index);
                                    register_index = index;
                                    return false;
                                }
                            }
                            true
                        },
                        RegisterIndex::Some(index) => {
                            if !busy_registers.contains(&index) {
                                busy_registers.insert(index);
                                register_index = index;
                                return false;
                            }
                            true
                        },
                    }
                }).unwrap();
            register_index
        }
    }
}