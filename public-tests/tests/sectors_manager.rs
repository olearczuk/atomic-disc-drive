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
    let path = root_drive_dir.into_path();
    let sectors_manager = build_sectors_manager(path);

    // when
    for i in 1..350 {
        sectors_manager
            .write(i, &(SectorVec(vec![2; 4096]), i, 1))
            .await;
    }

    // then
    for i in 1..350 {
        let data = sectors_manager.read_data(i).await;
        assert_eq!(data.0.len(), 4096);
        assert_eq!(data.0, vec![2; 4096]);
        assert_eq!(sectors_manager.read_metadata(i).await, (i, 1));
    }
}

#[tokio::test]
async fn drive_can_restore_data() {
    // given
    let root_drive_dir = tempdir().unwrap();
    let path = root_drive_dir.into_path();
    {
        let sectors_manager = build_sectors_manager(path.clone());
        // when
        for i in 1..20 {
            sectors_manager
                .write(i, &(SectorVec(vec![2; 4096]), i, 1))
                .await;
        }
    }

    // next sectors manager
    let sectors_manager2 = build_sectors_manager(path.clone());


    // when
    for i in 10..30 {
        sectors_manager2
            .write(i, &(SectorVec(vec![3; 4096]), i + 10, 2))
            .await;
    }

    // then
    // old values
    for i in 1..10 {
        let data = sectors_manager2.read_data(i).await;
        assert_eq!(data.0.len(), 4096);
        assert_eq!(data.0, vec![2; 4096]);
        assert_eq!(sectors_manager2.read_metadata(i).await, (i, 1));
    }

    // assert_eq!(1, 2);
    // changed values
    for i in 10..30 {
        let data = sectors_manager2.read_data(i).await;
        assert_eq!(data.0.len(), 4096);
        assert_eq!(data.0, vec![3; 4096]);
        assert_eq!(sectors_manager2.read_metadata(i).await, (i + 10, 2));
    }
}
