use assignment_2_solution::{build_sectors_manager, SectorVec};
use tempfile::tempdir;

#[tokio::test]
async fn read_before_write() {
    // given
    let root_drive_dir = tempdir().unwrap();
    let sectors_manager = build_sectors_manager(root_drive_dir.into_path());

    // when
    let data = sectors_manager.read_data(1).await;

    // then
    assert_eq!(data.0.len(), 4096);
    assert_eq!(data.0, vec![0; 4096]);
    assert_eq!(sectors_manager.read_metadata(1).await, (0, 0));
}

#[tokio::test]
async fn drive_can_store_data() {
    // given
    let root_drive_dir = tempdir().unwrap();
    let sectors_manager = build_sectors_manager(root_drive_dir.into_path());

    // when
    for i in 1..100 {
        sectors_manager
            .write(i, &(SectorVec(vec![2; 4096]), i, 1))
            .await;
    }

    // then
    for i in 1..100 {
        let data = sectors_manager.read_data(i).await;
        assert_eq!(data.0.len(), 4096);
        assert_eq!(data.0, vec![2; 4096]);
        assert_eq!(sectors_manager.read_metadata(i).await, (i, 1));
    }
}
