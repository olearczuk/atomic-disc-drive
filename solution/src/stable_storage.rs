pub mod stable_storage {
    use crate::StableStorage;
    use std::env::temp_dir;
    use std::path::PathBuf;
    use uuid::Uuid;
    use tokio::prelude::*;
    use tokio::io::SeekFrom;

    pub struct StableStorageImplementation {
        storage_dir: PathBuf,
    }

    impl StableStorageImplementation {
        pub async fn new(storage_dir: PathBuf) -> Box<dyn StableStorage> {
            tokio::fs::create_dir_all(&storage_dir).await.unwrap();
            Box::new(StableStorageImplementation { storage_dir })
        }
    }

    pub async fn write(storage_dir: &PathBuf, key: &str, value: &[u8]) -> Result<(), String>{
        let tempdir = temp_dir();
        let tmp_path = tempdir.join(Uuid::new_v4().to_string());
        let fileRes = tokio::fs::File::create(&tmp_path).await;
        if fileRes.is_err() {
            return Err(fileRes.err().unwrap().to_string());
        }
        let mut file = fileRes.unwrap();
        let res = file.write_all(value).await;
        if res.is_err() {
            return Err(res.err().unwrap().to_string());
        }
        let res = file.sync_data().await;
        if res.is_err() {
            return Err(res.err().unwrap().to_string());
        }

        let key_path = storage_dir.join(key);
        let renameRes = tokio::fs::rename(&tmp_path, key_path).await;
        if renameRes.is_err() {
            return Err(renameRes.err().unwrap().to_string());
        }

        let dirRes = tokio::fs::File::open(storage_dir).await;
        if dirRes.is_err() {
            return Err(dirRes.err().unwrap().to_string());
        }
        let dir = dirRes.unwrap();
        let res = dir.sync_data().await;
        if res.is_err() {
            return Err(res.err().unwrap().to_string());
        }
        Ok(())
    }

    pub async fn read(storage_dir: &PathBuf, key: &str) -> Option<Vec<u8>> {
        let key_path = storage_dir.join(key);
        tokio::fs::read(key_path).await.ok()
    }

    #[async_trait::async_trait]
    impl StableStorage for StableStorageImplementation {
        async fn put(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
            write(&self.storage_dir, key, value).await
        }

        async fn get(&self, key: &str) -> Option<Vec<u8>> {
            read(&self.storage_dir, key).await
        }
    }
}
