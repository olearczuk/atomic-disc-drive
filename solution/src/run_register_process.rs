pub mod run_register_process {
    use crate::{Configuration, build_register_client, build_sectors_manager, build_atomic_register, build_stable_storage, ClientRegisterCommand, SystemRegisterCommand, deserialize_register_command, RegisterCommand, OperationComplete, MAGIC_NUMBER, serialize_response, StatusCode, OperationReturn, SECTOR_VEC_SIZE};
    use tokio::sync::mpsc::{UnboundedSender, UnboundedReceiver};
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use crate::commands_executor_public::build_commands_executor;
    use crate::commands_executor::commands_executor::CommandsExecutor;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
    use std::io::{Error};
    use crate::serialize_deserialize::serialize_deserialize::{READ, WRITE, READ_PROC, WRITE_PROC, VALUE, ACK};
    use hmac::{Hmac, Mac, NewMac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    pub fn handle_external_command(commands_executor: Arc<CommandsExecutor>, mut read_stream: OwnedReadHalf,
                                   write_stream: Arc<Mutex<OwnedWriteHalf>>, hmac_system_key: [u8; 64],
                                   hmac_client_key: [u8; 32], max_sector: u64) {
        tokio::spawn(async move {
            let mut stream_open_checker = [0];
            let mut buffer = [0; 4];
            loop {
                match read_stream.peek(&mut stream_open_checker).await {
                    Ok(0) => {
                        break;
                    },
                    Ok(_) => {
                        // TODO - error
                        get_magic_number(&mut read_stream).await.unwrap();

                        read_stream.peek(&mut buffer).await.unwrap();
                        let msg_type = buffer[3];
                        let msg_len = match msg_type {
                            READ => 24,
                            WRITE => 24 + SECTOR_VEC_SIZE,
                            READ_PROC => 40,
                            VALUE => 40 + 16 + SECTOR_VEC_SIZE,
                            WRITE_PROC => 40 + 16 + SECTOR_VEC_SIZE,
                            ACK => 40,
                            _ => 0,
                        };
                        if msg_len == 0 {
                            continue;
                        }

                        let hmac_key: &[u8] = if msg_type < READ_PROC { &hmac_client_key } else { &hmac_system_key };

                        // +HMAC tag, - already read MAGIC_NUMBER
                        let mut msg = vec![0; msg_len + 32 - 4];
                        read_stream.read_exact(&mut msg).await.unwrap();
                        let mut data = MAGIC_NUMBER.to_vec();
                        data.append(&mut msg);

                        let mut mac = HmacSha256::new_varkey(hmac_key).unwrap();
                        mac.update(&data[..msg_len]);
                        let is_verified = mac.verify(&data[msg_len..]).is_ok();

                        // TODO - err
                        let command = deserialize_register_command(&mut &data[..]).unwrap();
                        match command {
                            RegisterCommand::System(cmd) => {
                                if is_verified {
                                    commands_executor.clone().execute_system_command(cmd).await;
                                }
                            },
                            RegisterCommand::Client(cmd) => {
                                if !is_verified || cmd.header.sector_idx >= max_sector {
                                    respond_err(
                                        commands_executor.clone(),
                                        &mut *write_stream.lock().await,
                                        msg_type,
                                        hmac_client_key,
                                        if !is_verified {StatusCode::AuthFailure} else {StatusCode::InvalidSectorIndex},
                                        cmd.header.request_identifier,
                                    ).await;
                                } else {
                                    let write_stream_callback = write_stream.clone();
                                    let commands_executor_callback = commands_executor.clone();
                                    let callback: Box<dyn FnOnce(OperationComplete) + Send + Sync> =
                                        Box::new(move |op_complete| {
                                            tokio::spawn(async move {
                                                respond(
                                                    commands_executor_callback,
                                                    &op_complete,
                                                    &mut *write_stream_callback.lock().await,
                                                    msg_type,
                                                    hmac_client_key).await;
                                            });
                                        });
                                    commands_executor.execute_client_command(cmd, callback).await;
                                }
                            }
                        }
                    }
                    Err(err) => println!("{:?}", err),
                }
            }
        });
    }

    pub fn handle_internal_commands(commands_executor: Arc<CommandsExecutor>, mut receiver: UnboundedReceiver<SystemRegisterCommand>) {
        tokio::spawn(async move {
            while let Some(cmd) = receiver.recv().await {
                commands_executor.execute_system_command(cmd).await;
            }
        });
    }

    async fn get_magic_number(data: &mut OwnedReadHalf) -> Result<(), Error>{
        let mut current_numbers: [u8; 4] = [0; 4];
        if let Err(err) = data.read_exact(&mut current_numbers).await {
            return Err(err);
        }
        let mut buffer: [u8; 1] = [0];

        loop {
            for i in 0..4 {
                if current_numbers[i] == MAGIC_NUMBER[i] {
                    if i == 3 {
                        return Ok(());
                    }
                } else {
                    break;
                }
            }
            current_numbers[0] = current_numbers[1];
            current_numbers[1] = current_numbers[2];
            current_numbers[2] = current_numbers[3];
            if let Err(err) = data.read_exact(&mut buffer).await {
                return Err(err);
            }
            current_numbers[3] = buffer[0];
        }
    }

    async fn respond_err(commands_executor: Arc<CommandsExecutor>, write_stream: &mut OwnedWriteHalf, msg_type: u8, hmac_client_key: [u8; 32],
        status_code: StatusCode, request_identifier: u64) {
        let op_complete = OperationComplete {
            status_code,
            request_identifier,
            op_return: OperationReturn::Write,
        };
        respond(commands_executor, &op_complete, write_stream, msg_type, hmac_client_key).await;
    }

    async fn respond(commands_executor: Arc<CommandsExecutor>, op_complete: &OperationComplete, write_stream: &mut OwnedWriteHalf,
                     msg_type: u8, hmac_client_key: [u8; 32]) {
        let mut msg = serialize_response(&op_complete, msg_type);
        let mut mac = HmacSha256::new_varkey(&hmac_client_key).unwrap();
        mac.update(&msg);
        msg.extend(mac.finalize().into_bytes());

        write_stream.write_all(&msg).await.unwrap();
        commands_executor.finish_client_command(op_complete.request_identifier).await;
    }

    pub async fn get_commands_executor_and_pending_cmd(config: &Configuration, sender: UnboundedSender<SystemRegisterCommand>,
        registers_number: usize) -> (Arc<CommandsExecutor>, Vec<ClientRegisterCommand>) {
        let processes_count = config.public.tcp_locations.len();
        let self_ident = config.public.self_rank;
        let storage_dir = config.public.storage_dir.clone();

        let register_client = build_register_client(
            self_ident,
            config.hmac_system_key.clone(),
            config.public.tcp_locations.clone(),
            sender,
        );
        let sectors_manager = build_sectors_manager(storage_dir.clone());

        let mut registers = vec![];
        let mut pending_cmds = vec![];
        for i in 0..registers_number {
            let path = storage_dir.clone().join(format!("register{}", i));
            let stable_storage = build_stable_storage(path).await;

            let (register, pending_cmd) = build_atomic_register(
                self_ident,
                stable_storage,
                register_client.clone(),
                sectors_manager.clone(),
                processes_count,
            ).await;
            registers.push(Mutex::new(register));

            if pending_cmd.is_some() {
                pending_cmds.push(pending_cmd.unwrap());
            }
        }
        let commands_executor = build_commands_executor(registers, config.public.max_sector);
        (commands_executor, pending_cmds)
    }
}