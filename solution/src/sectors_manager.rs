pub mod sectors_manager {
    use crate::stable_storage::stable_storage::{read, write};
    use crate::{SectorIdx, SectorVec, SectorsManager, SECTOR_VEC_SIZE};
    use std::path::PathBuf;
    use std::fs::{create_dir_all};
    use std::convert::TryInto;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    pub struct SectorsManagerImplementation {
        path: PathBuf,
        metadata_path: PathBuf,
        /// metadata_map keeps filename and offset of given sector
        metadata_info_map: Arc<Mutex<HashMap<SectorIdx, (String, usize)>>>,
        /// total size of stored metadata (used for computing file and offset in the file)
        metadata_total_size: Arc<Mutex<usize>>,
    }

    const METADATA_SIZE: usize = 8 + 8 + 1;
    const METADATA: &str = "metadata";
    const METADATAS_PER_FILE: usize = SECTOR_VEC_SIZE / METADATA_SIZE;
    const METADATAS_FILE_SIZE: usize = METADATAS_PER_FILE * METADATA_SIZE;

    fn get_metadata_info(vec: &Vec<u8>, offset: usize) -> (SectorIdx, u64, u8) {
        let sector_idx_vec_res: Result<[u8; 8], _> = vec[offset..offset + 8].try_into();
        let sector_idx_vec = sector_idx_vec_res.unwrap();
        let sector_idx = u64::from_be_bytes(sector_idx_vec);

        let timestamp_vec_res: Result<[u8; 8], _> = vec[offset + 8..offset + 16].try_into();
        let timestamp_vec = timestamp_vec_res.unwrap();
        let timestamp = u64::from_be_bytes(timestamp_vec);

        let write_rank = vec[offset + METADATA_SIZE - 1];
        (sector_idx, timestamp, write_rank)
    }


    impl SectorsManagerImplementation {
        pub fn new(path: PathBuf) -> Arc<dyn SectorsManager> {
            let metadata_path = path.clone().join(METADATA);
            create_dir_all(&path).unwrap();
            create_dir_all(&metadata_path).unwrap();

            // preprocess metadata_info_map and metadata_total_size
            // thanks to that initial linear work, we will be able to access metadata much faster
            let mut file_number = 0;
            let mut metadata_total_size = 0;
            let mut metadata_info_map = HashMap::new();
            loop {
                let file_path = metadata_path.join(file_number.to_string());
                let read_res = std::fs::read(file_path);
                if read_res.is_err() {
                    break;
                }
                let vec = read_res.unwrap();
                metadata_total_size += vec.len();
                for offset in (0..vec.len()).step_by(METADATA_SIZE) {
                    let (sector_idx, _, _) = get_metadata_info(&vec, offset);
                    metadata_info_map.insert(sector_idx, (file_number.to_string(), offset));
                }
                file_number += 1;
            }

            let manager = SectorsManagerImplementation {
                path,
                metadata_path,
                metadata_info_map: Arc::new(Mutex::new(metadata_info_map)),
                metadata_total_size: Arc::new(Mutex::new(metadata_total_size)),
            };
            Arc::new(manager)
        }
    }

    #[async_trait::async_trait]
    impl SectorsManager for SectorsManagerImplementation {
        async fn read_data(&self, idx: SectorIdx) -> SectorVec {
            let metadata_offset_map = self.metadata_info_map.lock().await;
            match metadata_offset_map.get(&idx) {
                // if there is no info in metadata, then write failed after data was written
                None => SectorVec::new(),
                Some(_) => {
                    match read(&self.path, &idx.to_string()).await {
                        None => SectorVec::new(),
                        Some(vec) => SectorVec(vec),
                    }
                },
            }
        }

        async fn read_metadata(&self, idx: SectorIdx) -> (u64, u8) {
            let metadata_offset_map = self.metadata_info_map.lock().await;
            match metadata_offset_map.get(&idx) {
                None => (0, 0),
                Some((filename, offset)) => {
                    match read(&self.metadata_path, filename).await {
                        None => (0, 0),
                        Some(vec) => {
                            let (_, timestamp, write_rank) = get_metadata_info(&vec, *offset);
                            (timestamp, write_rank)
                        },
                    }
                }
            }
        }

        async fn write(&self, idx: SectorIdx, sector: &(SectorVec, u64, u8)) {
            let (data, timestamp, write_rank) = sector;

            if write(&self.path, &idx.to_string(), data.0.as_slice()).await.is_err() {
                return
            }

            let mut metadata_vec = idx.to_be_bytes().to_vec();
            let mut timestamp_vec = timestamp.to_be_bytes().to_vec();
            metadata_vec.append(&mut timestamp_vec);
            metadata_vec.push(*write_rank);
            {
                let mut metadata_info_map = self.metadata_info_map.lock().await;
                let mut metadata_total_size = self.metadata_total_size.lock().await;

                // get file and offset related to this sector
                let (filename, offset) = match metadata_info_map.get(&idx) {
                    None => {
                        let file_number = *metadata_total_size / METADATAS_FILE_SIZE;
                        let offset = *metadata_total_size % METADATAS_FILE_SIZE;
                        (file_number.to_string(), offset)
                    }
                    Some((filename, offset)) => (filename.clone(), *offset)
                };

                // update file value (either by appending or modifying content)
                let mut val = match read(&self.metadata_path, &filename).await {
                    None => vec![],
                    Some(vec) => vec,
                };
                if offset == val.len() {
                    val.append(&mut metadata_vec);
                } else {
                    for i in offset..offset + METADATA_SIZE {
                        val[i] = metadata_vec[i - offset];
                    }
                }

                // write updated value of metadata file, update map if write is successful
                if write(&self.metadata_path, &filename, &val.as_slice()).await.is_err() {
                    return
                }
                if !metadata_info_map.contains_key(&idx) {
                    (*metadata_info_map).insert(idx, (filename, offset));
                    *metadata_total_size += METADATA_SIZE;
                }
            }

        }
    }
}
