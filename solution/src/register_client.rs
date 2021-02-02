pub mod register_client {
    use tokio::sync::mpsc::UnboundedSender;
    use crate::{SystemRegisterCommand, RegisterClient, RegisterCommand, serialize_register_command, Broadcast, Send};
    use sha2::Sha256;
    use hmac::{Hmac, Mac, NewMac};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;
    use tokio::sync::Mutex;
    use tokio::time::Duration;

    type HmacSha256 = Hmac<Sha256>;

    pub struct RegisterClientImplementation {
        self_ident: u8,
        hmac_system_key: [u8; 64],
        tcp_streams: Vec<Mutex<Option<TcpStream>>>,
        tcp_addresses: Vec<(String, u16)>,
        self_sender: UnboundedSender<SystemRegisterCommand>,
    }

    impl RegisterClientImplementation {
        pub fn new(self_ident: u8, hmac_system_key: [u8; 64], tcp_addresses: Vec<(String, u16)>,
                   self_sender: UnboundedSender<SystemRegisterCommand>) -> Self {
            Self {
                self_ident,
                hmac_system_key,
                tcp_streams: (0..tcp_addresses.len()).map(|_| Mutex::new(None)).collect(),
                tcp_addresses,
                self_sender,
            }
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
                let mut data = Vec::new();
                let cmd = RegisterCommand::System(msg.cmd.as_ref().clone());
                serialize_register_command(&cmd, &mut data).unwrap();

                let mut mac = HmacSha256::new_varkey(&self.hmac_system_key).unwrap();
                mac.update(&data);
                data.extend(mac.finalize().into_bytes());

                let mut stream = self.tcp_streams[msg.target - 1].lock().await;
                if stream.is_none() {
                    let (host, port) = &self.tcp_addresses[msg.target - 1];
                    match tokio::time::timeout(
                        Duration::from_millis(100),
                        tokio::net::TcpStream::connect((host.as_str(), *port))
                    ).await {
                        Ok(result) => {
                            match result {
                                Ok(tcp_stream) => {
                                    *stream = Some(tcp_stream);
                                },
                                Err(err) => println!("asdf {:?}", err),
                            }
                        },
                        Err(err) => println!("{:?}", err),
                    }
                }

                // TODO - consider saving last message for every req_id and replaying them after reestablishing connection
                if let Some(tcp_stream) = stream.as_mut() {
                    tcp_stream.write_all(data.as_slice()).await.unwrap();
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