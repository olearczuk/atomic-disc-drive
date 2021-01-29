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

    pub async fn write(storage_dir: &PathBuf, key: &str, value: &[u8]) -> Result<(), String>{
        let tempdir = temp_dir();
        let tmp_path = tempdir.join(Uuid::new_v4().to_string());
        let file_res = tokio::fs::File::create(&tmp_path).await;
        if file_res.is_err() {
            return Err(file_res.err().unwrap().to_string());
        }
        let mut file = file_res.unwrap();
        let res = file.write_all(value).await;
        if res.is_err() {
            return Err(res.err().unwrap().to_string());
        }
        let res = file.sync_data().await;
        if res.is_err() {
            return Err(res.err().unwrap().to_string());
        }

        let key_path = storage_dir.join(key);
        let rename_res = tokio::fs::rename(&tmp_path, key_path).await;
        if rename_res.is_err() {
            return Err(rename_res.err().unwrap().to_string());
        }

        let dir_res = tokio::fs::File::open(storage_dir).await;
        if dir_res.is_err() {
            return Err(dir_res.err().unwrap().to_string());
        }
        let dir = dir_res.unwrap();
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
