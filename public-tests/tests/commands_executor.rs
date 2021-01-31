use assignment_2_solution::commands_executor_public::build_commands_executor;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use assignment_2_solution::{SectorIdx, AtomicRegister, OperationComplete, ClientRegisterCommand, SystemRegisterCommand,
    StatusCode, OperationReturn, ClientCommandHeader, ClientRegisterCommandContent, SystemRegisterCommandContent, SystemCommandHeader};
use std::convert::TryInto;
use ntest::timeout;

struct MockAtomicRegister {
    commands: Arc<Mutex<Vec<(u64, SectorIdx, String)>>>,
    callback: Option<Box<dyn FnOnce(OperationComplete) + Send + Sync>>,
}

impl MockAtomicRegister {
    fn new(commands: Arc<Mutex<Vec<(u64, SectorIdx, String)>>>) -> Mutex<Box<dyn AtomicRegister>> {
        Mutex::new(Box::new(MockAtomicRegister { commands, callback: None }))
    }
}

#[async_trait::async_trait]
impl AtomicRegister for MockAtomicRegister {
    async fn client_command(&mut self, cmd: ClientRegisterCommand, operation_complete: Box<dyn FnOnce(OperationComplete) + Send + Sync>) {
        {
            let mut commands = self.commands.lock().unwrap();
            commands.push((cmd.header.request_identifier, cmd.header.sector_idx, "client".parse().unwrap()));
        }
        self.callback = Some(operation_complete);
    }

    async fn system_command(&mut self, cmd: SystemRegisterCommand) {
        let req_id = cmd.header.msg_ident.as_u128() as u64;
        {
            let mut commands = self.commands.lock().unwrap();
            commands.push((req_id, cmd.header.sector_idx, "system".parse().unwrap()));
        }

        match cmd.content {
            SystemRegisterCommandContent::Ack => {
                if self.callback.is_some() {
                    let op_complete = OperationComplete {
                        status_code: StatusCode::Ok,
                        request_identifier: req_id,
                        op_return: OperationReturn::Write,
                    };
                    (self.callback.take().unwrap())(op_complete);
                    self.callback = None;
                }
            },
            _ => {},
        }
    }
}


#[tokio::test]
#[timeout(200)]
async fn single_register() {
    let commands = Arc::new(Mutex::new(vec![]));
    let reg1 = MockAtomicRegister::new(Arc::clone(&commands));

    let executor = build_commands_executor(vec![reg1]);


    // sending first client command
    let cmd = ClientRegisterCommand {
        header: ClientCommandHeader {
            request_identifier: 1,
            sector_idx: 2,
        },
        content: ClientRegisterCommandContent::Read,
    };
    let cloned_exec = executor.clone();
    let op_complete: Box<dyn FnOnce(OperationComplete) + core::marker::Send + Sync> =
        Box::new(move |_| { cloned_exec.finish_client_command(1, 2) });
    let f1 = executor.execute_client_command(cmd, op_complete);


    // sending second client command
    let cmd = ClientRegisterCommand {
        header: ClientCommandHeader {
            request_identifier: 2,
            sector_idx: 3,
        },
        content: ClientRegisterCommandContent::Read,
    };
    let cloned_exec = executor.clone();
    let op_complete: Box<dyn FnOnce(OperationComplete) + core::marker::Send + Sync> =
        Box::new(move |_| { cloned_exec.finish_client_command(1, 2) });
    let f2 = executor.execute_client_command(cmd, op_complete);

    f1.await;
    f2.await;
    let cmds = commands.lock().unwrap();
    assert_eq!(vec![(1, 2, "client".try_into().unwrap()), (2, 3, "client".try_into().unwrap())], *cmds);
}

#[tokio::test]
#[timeout(200)]
async fn two_registers() {
    let commands = Arc::new(Mutex::new(vec![]));
    let reg1 = MockAtomicRegister::new(Arc::clone(&commands));
    let reg2 = MockAtomicRegister::new(Arc::clone(&commands));

    let executor = build_commands_executor(vec![reg1, reg2]);


    // sending first client command
    let cmd = ClientRegisterCommand {
        header: ClientCommandHeader {
            request_identifier: 1,
            sector_idx: 2,
        },
        content: ClientRegisterCommandContent::Read,
    };
    let cloned_exec = executor.clone();
    let op_complete: Box<dyn FnOnce(OperationComplete) + core::marker::Send + Sync> =
        Box::new(move |_| {
            cloned_exec.finish_client_command(1, 2);
        });
    executor.execute_client_command(cmd, op_complete).await;

    // sending client command to a different sector - does not block
    let cmd = ClientRegisterCommand {
        header: ClientCommandHeader {
            request_identifier: 2,
            sector_idx: 15,
        },
        content: ClientRegisterCommandContent::Read,
    };
    let cloned_exec = executor.clone();
    let op_complete: Box<dyn FnOnce(OperationComplete) + core::marker::Send + Sync> =
        Box::new(move |_| {
            cloned_exec.finish_client_command(1, 2);
        });
    executor.execute_client_command(cmd, op_complete).await;

    // sending some external system message - even though it operates on busy sector it does not have to wait
    let cmd = SystemRegisterCommand {
        header: SystemCommandHeader {
            process_identifier: 3,
            msg_ident: Uuid::from_u128(3),
            read_ident: 12,
            sector_idx: 2,
        },
        content: SystemRegisterCommandContent::ReadProc
    };
    executor.execute_system_command(cmd).await;

    // finishing first client command by sending Ack
    let cmd = SystemRegisterCommand {
        header: SystemCommandHeader {
            process_identifier: 3,
            msg_ident: Uuid::from_u128(1),
            read_ident: 12,
            sector_idx: 2,
        },
        content: SystemRegisterCommandContent::Ack
    };
    executor.execute_system_command(cmd).await;

    // sending second client command - this does not block, because previous request on the same sector finished
    let cmd = ClientRegisterCommand {
        header: ClientCommandHeader {
            request_identifier: 2,
            sector_idx: 2,
        },
        content: ClientRegisterCommandContent::Read,
    };
    let cloned_exec = executor.clone();
    let op_complete: Box<dyn FnOnce(OperationComplete) + core::marker::Send + Sync> =
        Box::new(move |_| { cloned_exec.finish_client_command(1, 2) });
    executor.execute_client_command(cmd, op_complete).await;


    let cmds = commands.lock().unwrap();
    let client: String = "client".try_into().unwrap();
    let system: String = "system".try_into().unwrap();
    // first client request, ReadProc message, Ack for first client that frees sector, second client request
    assert_eq!(vec![(1, 2, client.clone()), (2, 15, client.clone()), (3, 2, system.clone()), (1, 2, system.clone()), (2, 2, client.clone())], *cmds);
}

#[tokio::test]
#[timeout(200)]
#[should_panic]
async fn two_registers_blocking_on_same_sector() {
    let commands = Arc::new(Mutex::new(vec![]));
    let reg1 = MockAtomicRegister::new(Arc::clone(&commands));
    let reg2 = MockAtomicRegister::new(Arc::clone(&commands));

    let executor = build_commands_executor(vec![reg1, reg2]);


    // sending first client command
    let cmd = ClientRegisterCommand {
        header: ClientCommandHeader {
            request_identifier: 1,
            sector_idx: 2,
        },
        content: ClientRegisterCommandContent::Read,
    };
    let cloned_exec = executor.clone();
    let op_complete: Box<dyn FnOnce(OperationComplete) + core::marker::Send + Sync> =
        Box::new(move |_| {
            cloned_exec.finish_client_command(1, 2);
        });
    executor.execute_client_command(cmd, op_complete).await;


    // sending second client command - this blocks because request on the same sector has not finished yet
    let cmd = ClientRegisterCommand {
        header: ClientCommandHeader {
            request_identifier: 2,
            sector_idx: 2,
        },
        content: ClientRegisterCommandContent::Read,
    };
    let cloned_exec = executor.clone();
    let op_complete: Box<dyn FnOnce(OperationComplete) + core::marker::Send + Sync> =
        Box::new(move |_| { cloned_exec.finish_client_command(1, 2) });
    executor.execute_client_command(cmd, op_complete).await;
}