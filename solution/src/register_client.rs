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

    pub struct RegisterClientImplementation {
        self_ident: u8,
        hmac_system_key: [u8; 64],
        tcp_streams: Vec<Mutex<Option<TcpStream>>>,
        tcp_addresses: Vec<(String, u16)>,
        self_sender: UnboundedSender<SystemRegisterCommand>,
        pending_cmds: Mutex<HashMap<Uuid, SystemRegisterCommand>>
    }

    impl RegisterClientImplementation {
        pub fn new(self_ident: u8, hmac_system_key: [u8; 64], tcp_addresses: Vec<(String, u16)>,
                   self_sender: UnboundedSender<SystemRegisterCommand>) -> Arc<Self> {
            let register_client = Arc::new(Self {
                self_ident,
                hmac_system_key,
                tcp_streams: (0..tcp_addresses.len()).map(|_| Mutex::new(None)).collect(),
                tcp_addresses,
                self_sender,
                pending_cmds: Mutex::new(HashMap::new()),
            });

            let register_client_cloned = register_client.clone();
            tokio::spawn(async move {
                register_client_cloned.monitor_connections().await;
            });

            register_client
        }

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
                                        let pending_cmd = self.pending_cmds.lock().await;
                                        let mut error_counter = 0;
                                        for cmd in pending_cmd.values() {
                                            let new_msg = crate::Send {
                                                cmd: Arc::new(cmd.clone()),
                                                target: target,
                                            };
                                            if let Err(err) = self.send_stream(&mut tcp_stream, new_msg).await {
                                                error_counter += 1;
                                            }
                                        }
                                        if error_counter == 0 {
                                            *stream = Some(tcp_stream);
                                        }
                                    },
                                    Err(err) => println!("{:?}", err),
                                }
                            },
                            Err(err) => println!("{:?}", err),
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }

        async fn send_stream(&self, stream: &mut TcpStream, msg: crate::Send) -> Result<()> {
            let mut data = Vec::new();
            let cmd = RegisterCommand::System(msg.cmd.as_ref().clone());
            serialize_register_command(&cmd, &mut data).unwrap();

            let mut mac = HmacSha256::new_varkey(&self.hmac_system_key).unwrap();
            mac.update(&data);
            data.extend(mac.finalize().into_bytes());
            stream.write_all(data.as_slice()).await
        }

        async fn add_pending_cmd(&self, request_identifier: Uuid, cmd: SystemRegisterCommand) {
            self.pending_cmds.lock().await.insert(request_identifier, cmd);
        }
    }

    #[async_trait::async_trait]
    impl RegisterClient for RegisterClientImplementation {
        async fn send(&self, msg: crate::Send) {
            if msg.target == self.self_ident as usize {
                self.self_sender.send(SystemRegisterCommand {
                    header: msg.cmd.header.clone(),
                    content: msg.cmd.content.clone(),
                }).unwrap();
            } else {
                let mut stream = self.tcp_streams[msg.target - 1].lock().await;
                match stream.as_mut() {
                    Some(tcp_stream) => {
                        if let Err(err) = self.send_stream(tcp_stream, msg).await {
                            // problem with connection - let monitor_connections take care of that
                            *stream = None;
                        }
                    },
                    None =>
                        self.add_pending_cmd(msg.cmd.header.msg_ident, msg.cmd.as_ref().clone()).await,
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