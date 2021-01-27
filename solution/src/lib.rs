mod atomic_register;
mod serialize_deserialize;
mod domain;
mod sectors_manager;
mod stable_storage;

pub use crate::domain::*;
pub use atomic_register_public::*;
pub use register_client_public::*;
pub use sectors_manager_public::*;
pub use stable_storage_public::*;
pub use transfer_public::*;

pub async fn run_register_process(config: Configuration) {
    unimplemented!()
}

pub mod atomic_register_public {
    use crate::atomic_register::atomic_register::AtomicRegisterImplementation;
    use crate::{
        ClientRegisterCommand, OperationComplete, RegisterClient, SectorsManager, StableStorage,
        SystemRegisterCommand,
    };
    use std::sync::Arc;

    #[async_trait::async_trait]
    pub trait AtomicRegister {
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
    use crate::serialize_deserialize::serialize_deserialize::{deserialize_client_command, serialize_client_command, serialize_system_command, deserialize_system_command};
    use crate::RegisterCommand;
    use std::io::{Error, Read, Write};
    use crate::RegisterCommand::{Client, System};

    const MAGIC_NUMBER: [u8; 4] = [0x61, 0x74, 0x64, 0x64];

    fn get_magic_number(data: &mut dyn Read) -> Option<Error>{
        let mut current_numbers: [u8; 4] = [0; 4];
        if let Err(err) = data.read_exact(&mut current_numbers) {
            return Some(err);
        }
        let mut buffer: [u8; 1] = [0];

        loop {
            for i in 0..4 {
                if current_numbers[i] == MAGIC_NUMBER[i] {
                    if i == 3 {
                        return None;
                    }
                } else {
                    break;
                }
            }
            current_numbers[0] = current_numbers[1];
            current_numbers[1] = current_numbers[2];
            current_numbers[2] = current_numbers[3];
            if let Err(err) = data.read_exact(&mut buffer) {
                return Some(err);
            }
            current_numbers[3] = buffer[0];
        }
    }

    pub fn deserialize_register_command(data: &mut dyn Read) -> Result<RegisterCommand, Error> {
        if let Some(err) = get_magic_number(data) {
            return Err(err);
        }
        // getting rid of padding
        let mut buffer: [u8; 4] = [0; 4];
        if let Err(err) = data.read_exact(&mut buffer) {
            return Err(err);
        }
        let process_identifier = buffer[2];
        let msg_type = buffer[3];

        let res = deserialize_client_command(data, msg_type);
        if res.is_none() {
            deserialize_system_command(data, process_identifier, msg_type)
        } else {
            res.unwrap()
        }
    }

    pub fn serialize_register_command(
        cmd: &RegisterCommand,
        writer: &mut dyn Write,
    ) -> Result<(), Error> {
        if let Err(err) = writer.write_all(&MAGIC_NUMBER) {
            return Err(err);
        }
        match cmd {
            Client(client_cmd) => serialize_client_command(client_cmd, writer),
            System(system_cmd) => serialize_system_command(system_cmd, writer),
        }
    }
}

pub mod register_client_public {
    use crate::{SystemCommandHeader, SystemRegisterCommand, SystemRegisterCommandContent};
    use std::sync::Arc;

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
}

pub mod stable_storage_public {
    #[async_trait::async_trait]
    /// A helper trait for small amount of durable metadata needed by the register algorithm
    /// itself. Again, it is only for AtomicRegister definition. StableStorage in unit tests
    /// is durable, as one could expect.
    pub trait StableStorage: Send + Sync {
        async fn put(&mut self, key: &str, value: &[u8]) -> Result<(), String>;

        async fn get(&self, key: &str) -> Option<Vec<u8>>;
    }
}
