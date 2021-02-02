mod atomic_register;
mod serialize_deserialize;
mod domain;
mod sectors_manager;
mod stable_storage;
mod commands_executor;
mod register_client;
mod run_register_process;


pub use crate::domain::*;
pub use atomic_register_public::*;
pub use register_client_public::*;
pub use sectors_manager_public::*;
pub use stable_storage_public::*;
pub use transfer_public::*;
use tokio::net::{TcpListener};
use tokio::sync::mpsc::unbounded_channel;
use crate::run_register_process::run_register_process::{get_commands_executor_and_pending_cmd, handle_external_command,
    handle_internal_commands};
use std::sync::Arc;
use tokio::sync::Mutex;

const REGISTERS_NUMBER: usize = 256;

pub async fn run_register_process(config: Configuration) {
    let self_ident = config.public.self_rank;
    let (host, port) = &config.public.tcp_locations[self_ident as usize - 1];
    let tcp_listener = TcpListener::bind((host.as_str(), *port)).await.unwrap();

    let (sender, receiver) = unbounded_channel();
    let (commands_executor, pending_cmds, pending_cmds_manager) =
        get_commands_executor_and_pending_cmd(&config, sender, REGISTERS_NUMBER).await;

    handle_internal_commands(commands_executor.clone(), receiver);

    // start with executing pending messages
    for cmd in pending_cmds {
        let commands_executor_cloned = commands_executor.clone();
        let callback: Box<dyn FnOnce(OperationComplete) + std::marker::Send + Sync> =
            Box::new(move |op_complete| {
                tokio::spawn(async move {
                    commands_executor_cloned.finish_client_command(op_complete.request_identifier).await;
                });
            });
        commands_executor.execute_client_command(cmd, callback).await;
    }

    loop {
        let accept_res = tcp_listener.accept().await;
        if accept_res.is_err() {
            log::error!("Error while accepting new connection {}", accept_res.unwrap_err());
            continue;
        }
        let (stream, _) = accept_res.unwrap();
        let (read_stream, write) = stream.into_split();
        let write_stream = Arc::new(Mutex::new(write));

        handle_external_command(commands_executor.clone(), pending_cmds_manager.clone(),
            read_stream, write_stream,
            config.hmac_system_key.clone(), config.hmac_client_key.clone(),
            config.public.max_sector);
    }
}

pub mod atomic_register_public {
    use crate::atomic_register::atomic_register::AtomicRegisterImplementation;
    use crate::{
        ClientRegisterCommand, OperationComplete, RegisterClient, SectorsManager, StableStorage,
        SystemRegisterCommand,
    };
    use std::sync::Arc;

    #[async_trait::async_trait]
    pub trait AtomicRegister: Send + Sync {
        /// Send client command to the register. After it is completed, we expect
        /// callback to be called. Note that completion of client command happens after
        /// delivery of multiple system commands to the register, as the algorithm specifies.
        async fn client_command(
            &mut self,
            cmd: ClientRegisterCommand,
            operation_complete: Box<dyn FnOnce(OperationComplete) + Send + Sync>,
        );

        /// Send system command to the register.
        async fn system_command(&mut self, cmd: SystemRegisterCommand);
    }

    /// Idents are numbered starting at 1 (up to the number of processes in the system).
    /// Storage for atomic register algorithm data is separated into StableStorage.
    /// Communication with other processes of the system is to be done by register_client.
    /// And sectors must be stored in the sectors_manager instance.
    pub async fn build_atomic_register(
        self_ident: u8,
        stable_storage: Box<dyn StableStorage>,
        register_client: Arc<dyn RegisterClient>,
        sectors_manager: Arc<dyn SectorsManager>,
        processes_count: usize,
    ) -> (Box<dyn AtomicRegister>, Option<ClientRegisterCommand>) {
        AtomicRegisterImplementation::new(
            self_ident,
            stable_storage,
            register_client,
            sectors_manager,
            processes_count,
        )
        .await
    }
}

pub mod sectors_manager_public {
    use crate::sectors_manager::sectors_manager::SectorsManagerImplementation;
    use crate::{SectorIdx, SectorVec};
    use std::path::PathBuf;
    use std::sync::Arc;

    #[async_trait::async_trait]
    pub trait SectorsManager: Send + Sync {
        /// Returns 4096 bytes of sector data by index.
        async fn read_data(&self, idx: SectorIdx) -> SectorVec;

        /// Returns timestamp and write rank of the process which has saved this data.
        /// Timestamps and ranks are relevant for atomic register algorithm, and are described
        /// there.
        async fn read_metadata(&self, idx: SectorIdx) -> (u64, u8);

        /// Writes a new data, along with timestamp and write rank to some sector.
        async fn write(&self, idx: SectorIdx, sector: &(SectorVec, u64, u8));
    }

    /// Path parameter points to a directory to which this method has exclusive access.
    pub fn build_sectors_manager(path: PathBuf) -> Arc<dyn SectorsManager> {
        SectorsManagerImplementation::new(path)
    }
}

/// Your internal representation of RegisterCommand for ser/de can be anything you want,
/// we just would like some hooks into your solution to asses where the problem is, should
/// there be a problem.
pub mod transfer_public {
    use crate::serialize_deserialize::serialize_deserialize::{deserialize_client_command, serialize_client_command,
        serialize_system_command, deserialize_system_command, READ, WRITE};
    use crate::{RegisterCommand, MAGIC_NUMBER, OperationComplete, OperationReturn, ReadReturn};
    use std::io::{Result, Read, Write};
    use crate::RegisterCommand::{Client, System};

    pub fn deserialize_register_command(data: &mut dyn Read) -> Result<RegisterCommand> {
        // getting rid of padding and magic nuber
        let mut buffer: [u8; 8] = [0; 8];
        data.read_exact(&mut buffer)?;
        let process_identifier = buffer[6];
        let msg_type = buffer[7];

        if msg_type == READ || msg_type == WRITE {
            deserialize_client_command(data, msg_type)
        } else {
            deserialize_system_command(data, process_identifier, msg_type)
        }
    }

    pub fn serialize_register_command(
        cmd: &RegisterCommand,
        writer: &mut dyn Write,
    ) -> Result<()> {
        writer.write_all(&MAGIC_NUMBER)?;
        match cmd {
            Client(client_cmd) => serialize_client_command(client_cmd, writer),
            System(system_cmd) => serialize_system_command(system_cmd, writer),
        }
    }

    pub(crate) fn serialize_response(op_complete: &OperationComplete, msg_type: u8) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(&MAGIC_NUMBER);
        msg.extend_from_slice(&[0, 0, op_complete.status_code as u8, 0x40 + msg_type]);
        msg.extend_from_slice(&op_complete.request_identifier.to_be_bytes());
        if let OperationReturn::Read(ReadReturn { read_data }) = &op_complete.op_return {
            if let Some(data) = read_data {
                msg.extend_from_slice(data.0.as_slice());
            }
        }
        msg
    }
}

pub mod register_client_public {
    use crate::{SystemRegisterCommand};
    use std::sync::Arc;
    use crate::register_client::register_client::{PendingCommandsManager, RegisterClientImplementation};
    use tokio::sync::mpsc::UnboundedSender;

    #[async_trait::async_trait]
    /// We do not need any public implementation of this trait. It is there for use
    /// in AtomicRegister. In our opinion it is a safe bet to say some structure of
    /// this kind must appear in your solution.
    pub trait RegisterClient: core::marker::Send + core::marker::Sync {
        /// Sends a system message to a single process.
        async fn send(&self, msg: Send);

        /// Broadcasts a system message to all processes in the system, including self.
        async fn broadcast(&self, msg: Broadcast);
    }

    pub struct Broadcast {
        pub cmd: Arc<SystemRegisterCommand>,
    }

    pub struct Send {
        pub cmd: Arc<SystemRegisterCommand>,
        /// Identifier of the target process. Those start at 1.
        pub target: usize,
    }

    pub fn build_register_client(self_ident: u8, hmac_system_key: [u8; 64], tcp_locations: Vec<(String, u16)>,
                                 self_sender: UnboundedSender<SystemRegisterCommand>, pending_cmds_manager: Arc<PendingCommandsManager>) -> Arc<dyn RegisterClient> {
        RegisterClientImplementation::new(self_ident, hmac_system_key, tcp_locations, self_sender, pending_cmds_manager)
    }
}

pub mod stable_storage_public {
    use crate::stable_storage::stable_storage::StableStorageImplementation;
    use std::path::PathBuf;

    #[async_trait::async_trait]
    /// A helper trait for small amount of durable metadata needed by the register algorithm
    /// itself. Again, it is only for AtomicRegister definition. StableStorage in unit tests
    /// is durable, as one could expect.
    pub trait StableStorage: Send + Sync {
        async fn put(&mut self, key: &str, value: &[u8]) -> Result<(), String>;

        async fn get(&self, key: &str) -> Option<Vec<u8>>;
    }

    pub async fn build_stable_storage(path: PathBuf) -> Box<dyn StableStorage> {
        StableStorageImplementation::new(path).await
    }
}

pub mod commands_executor_public {
    use crate::commands_executor::commands_executor::CommandsExecutor;
    use crate::AtomicRegister;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    pub fn build_commands_executor(registers: Vec<Mutex<Box<dyn AtomicRegister>>>, max_sector: u64) -> Arc<CommandsExecutor> {
        CommandsExecutor::new(registers, max_sector)
    }
}
