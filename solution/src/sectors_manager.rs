pub mod sectors_manager {
    use crate::stable_storage::stable_storage::{StableStorageImplementation, read, write};
    use crate::{SectorIdx, SectorVec, SectorsManager, StableStorage, SECTOR_VEC_SIZE};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::fs::create_dir_all;
    use std::convert::TryInto;

    pub struct SectorsManagerImplementation {
        path: PathBuf,
    }

    const METADATA_SIZE: usize = 8 + 1;

    impl SectorsManagerImplementation {
        pub fn new(path: PathBuf) -> Arc<dyn SectorsManager> {
            create_dir_all(&path).unwrap();
            let manager = SectorsManagerImplementation { path };

            Arc::new(manager)
        }
    }

    // TODO: avoid > 10% overhead
    #[async_trait::async_trait]
    impl SectorsManager for SectorsManagerImplementation {
        async fn read_data(&self, idx: SectorIdx) -> SectorVec {
            match read(&self.path, &idx.to_string()).await {
                None => SectorVec(vec![0; SECTOR_VEC_SIZE]),
                Some(vec) => SectorVec(vec[METADATA_SIZE..].to_vec()),
            }
        }

        async fn read_metadata(&self, idx: SectorIdx) -> (u64, u8) {
            match read(&self.path, &idx.to_string()).await {
                None => (0, 0),
                Some(vec) => {
                    let write_rank = vec[METADATA_SIZE - 1];
                    let timestamp_vec_res: Result<[u8; 8], _> = vec[..METADATA_SIZE - 1].try_into();
                    let timestamp_vec = timestamp_vec_res.unwrap();
                    let timestamp = u64::from_be_bytes(timestamp_vec);
                    (timestamp, write_rank)
                },
            }
        }

        async fn write(&self, idx: SectorIdx, sector: &(SectorVec, u64, u8)) {
            let (data, timestamp, write_rank) = sector;
            let mut metadata_vec = timestamp.to_be_bytes().to_vec();
            metadata_vec.push(*write_rank);

            let mut vec = data.0.clone();
            metadata_vec.append(&mut vec);
            write(&self.path, &idx.to_string(), metadata_vec.as_slice()).await.unwrap();

        }
    }
}
