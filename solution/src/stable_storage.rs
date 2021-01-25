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

    #[async_trait::async_trait]
    impl StableStorage for StableStorageImplementation {
        async fn put(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
            let tempdir = temp_dir();
            let tmp_path = tempdir.join(Uuid::new_v4().to_string());
            let fileRes = tokio::fs::File::open(&tmp_path).await;
            if fileRes.is_ok() {
                return Err("error while opening tmp file".to_string());
            }
            let mut file = fileRes.unwrap();
            if file.write_all(value).await.is_err() {
                return Err("error while writing to tmp file".to_string());
            }
            if file.sync_data().await.is_err() {
                return Err("error while syncing tmp file".to_string());
            }

            let renameRes = tokio::fs::rename(&tmp_path, tempdir.join(key)).await;
            if renameRes.is_err() {
                return Err("error while renaming".to_string());
            }

            let dirRes = tokio::fs::File::open(&self.storage_dir).await;
            if dirRes.is_err() {
                return Err("error while opening directory".to_string());
            }
            let dir = dirRes.unwrap();
            if dir.sync_data().await.is_err() {
                return Err("error while syncing directory".to_string());
            }
            Ok(())
        }

        async fn get(&self, key: &str) -> Option<Vec<u8>> {
            let key_path = self.storage_dir.join(key);
            tokio::fs::read(key_path).await.ok()
        }
    }
}
