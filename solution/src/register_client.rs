pub mod register_client {
    use tokio::sync::mpsc::UnboundedSender;
    use crate::{SystemRegisterCommand, RegisterClient, RegisterCommand, serialize_register_command, Broadcast, Send};
    use sha2::Sha256;
    use hmac::{Hmac, Mac, NewMac};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;
    use tokio::sync::Mutex;
    use tokio::time::Duration;
    use std::collections::HashMap;
    use std::sync::Arc;
    use uuid::Uuid;
    use std::io::Result;

    type HmacSha256 = Hmac<Sha256>;

    pub struct PendingCommandsManager {
        /// the most actual command per msg_ident (replaying them might save atomic registers from hanging)
        pending_cmds: Mutex<HashMap<Uuid, SystemRegisterCommand>>
    }

    impl PendingCommandsManager {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                pending_cmds: Mutex::new(HashMap::new()),
            })
        }
        async fn add_pending_cmd(&self, cmd: SystemRegisterCommand) {
            self.pending_cmds.lock().await.insert(cmd.header.msg_ident, cmd);
        }

        pub async fn remove_pending_cmd(&self, msg_ident: Uuid) {
            self.pending_cmds.lock().await.remove(&msg_ident);
        }
    }

    /// RegisterClientImplementation is responsible for sending messages to other processes in the system.
    /// Additionally, it is respnsible for keeping connections alive, which means that if connection to a certain process
    /// is lost, it has to reopen it in the future.
    pub struct RegisterClientImplementation {
        self_ident: u8,
        hmac_system_key: [u8; 64],
        /// vector of TCP streams - None means that connection broke
        tcp_streams: Vec<Mutex<Option<TcpStream>>>,
        tcp_addresses: Vec<(String, u16)>,
        /// channel used for sending messages to itself
        self_sender: UnboundedSender<SystemRegisterCommand>,
        pending_cmds_manager: Arc<PendingCommandsManager>,
    }

    impl RegisterClientImplementation {
        pub fn new(self_ident: u8, hmac_system_key: [u8; 64], tcp_addresses: Vec<(String, u16)>,
                   self_sender: UnboundedSender<SystemRegisterCommand>, pending_cmds_manager: Arc<PendingCommandsManager>) -> Arc<Self> {
            let register_client = Arc::new(Self {
                self_ident,
                hmac_system_key,
                tcp_streams: (0..tcp_addresses.len()).map(|_| Mutex::new(None)).collect(),
                tcp_addresses,
                self_sender,
                pending_cmds_manager,
            });

            let register_client_cloned = register_client.clone();
            tokio::spawn(async move {
                register_client_cloned.monitor_connections().await;
            });

            register_client
        }

        /// monitor_connections is responsible for continuously checking if client is properly connect to other processes.
        /// If stream = None, then it means that connection broke and it has to be reestablished.
        /// Additionally, pending messages are replayed to process when connection reestablished.
         async fn monitor_connections(&self) {
            loop {
                for target in 0..self.tcp_streams.len() {
                    if target as u8 == self.self_ident - 1 {
                        continue;
                    }
                    let mut stream = self.tcp_streams[target].lock().await;
                    if stream.is_none() {
                        let (host, port) = &self.tcp_addresses[target];
                        match tokio::time::timeout(
                            Duration::from_millis(100),
                            tokio::net::TcpStream::connect((host.as_str(), *port))
                        ).await {
                            Ok(result) => {
                                match result {
                                    Ok(mut tcp_stream) => {
                                        let mut error_counter = 0;
                                        for cmd in self.pending_cmds_manager.pending_cmds.lock().await.values() {
                                            let new_msg = crate::Send {
                                                cmd: Arc::new(cmd.clone()),
                                                target: target,
                                            };
                                            if let Err(err) = self.send_stream(&mut tcp_stream, new_msg).await {
                                                log::error!("[target: {}], Error while sending pending command {}", target, err);
                                                error_counter += 1;
                                            }
                                        }
                                        // mark connection as active if no errors occurred
                                        if error_counter == 0 {
                                            *stream = Some(tcp_stream);
                                        }
                                    },
                                    Err(err) => log::error!("[target: {}], Error while trying to connect {}", target, err),
                                }
                            },
                            Err(err) => log::error!("[target: {}], Error while trying to connect {}", target, err),
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }

        async fn send_stream(&self, stream: &mut TcpStream, msg: crate::Send) -> Result<()> {
            let mut data = Vec::new();
            let cmd = RegisterCommand::System(msg.cmd.as_ref().clone());
            serialize_register_command(&cmd, &mut data)?;

            let mut mac = HmacSha256::new_varkey(&self.hmac_system_key).unwrap();
            mac.update(&data);
            data.extend(mac.finalize().into_bytes());
            stream.write_all(data.as_slice()).await
        }
    }

    #[async_trait::async_trait]
    impl RegisterClient for RegisterClientImplementation {
        async fn send(&self, msg: crate::Send) {
            let target = msg.target;
            if target == self.self_ident as usize {
                self.self_sender.send(SystemRegisterCommand {
                    header: msg.cmd.header.clone(),
                    content: msg.cmd.content.clone(),
                }).unwrap_or_else(|err|
                    log::error!("Error while sending to itself {}",  err));
            } else {
                let mut stream = self.tcp_streams[target - 1].lock().await;
                let cmd = msg.cmd.as_ref().clone();
                match stream.as_mut() {
                    Some(tcp_stream) => {
                        // problem with connection - let monitor_connections take care of that
                        if let Err(err) = self.send_stream(tcp_stream, msg).await {
                            log::error!("[target: {}], Error while sending {}", target, err);
                            *stream = None;
                            self.pending_cmds_manager.add_pending_cmd(cmd).await;
                        }
                    },
                    None => {
                        self.pending_cmds_manager.add_pending_cmd(cmd).await;
                    },
                }
            }
        }

        async fn broadcast(&self, msg: Broadcast) {
            let mut sends = vec![];
            for i in 0..self.tcp_addresses.len() {
                let send_future = self.send(Send {
                    cmd: msg.cmd.clone(),
                    target: i + 1,
                });
                sends.push(send_future);
            }
            for s in sends {
                s.await;
            }
        }
    }
}