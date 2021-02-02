pub mod stable_storage {
    use crate::StableStorage;
    use std::env::temp_dir;
    use std::path::PathBuf;
    use uuid::Uuid;
    use tokio::prelude::*;

    pub struct StableStorageImplementation {
        storage_dir: PathBuf,
    }

    impl StableStorageImplementation {
        pub async fn new(storage_dir: PathBuf) -> Box<dyn StableStorage> {
            tokio::fs::create_dir_all(&storage_dir).await.unwrap();
            Box::new(StableStorageImplementation { storage_dir })
        }
    }

    pub async fn write(storage_dir: &PathBuf, key: &str, value: &[u8]) -> std::io::Result<()>{
        // create tmp file, write to it and sync data
        let tempdir = temp_dir();
        let tmp_path = tempdir.join(Uuid::new_v4().to_string());
        let mut file = tokio::fs::File::create(&tmp_path).await?;
        file.write_all(value).await?;
        file.sync_data().await?;

        // rename to destination file
        let key_path = storage_dir.join(key);
        tokio::fs::rename(&tmp_path, key_path).await?;

        // sync_data on directory
        let dir = tokio::fs::File::open(storage_dir).await?;
        dir.sync_data().await
    }

    pub async fn read(storage_dir: &PathBuf, key: &str) -> Option<Vec<u8>> {
        let key_path = storage_dir.join(key);
        tokio::fs::read(key_path).await.ok()
    }

    #[async_trait::async_trait]
    impl StableStorage for StableStorageImplementation {
        async fn put(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
            write(&self.storage_dir, key, value).await.map_err(|err| err.to_string())
        }

        async fn get(&self, key: &str) -> Option<Vec<u8>> {
            read(&self.storage_dir, key).await
        }
    }
}
