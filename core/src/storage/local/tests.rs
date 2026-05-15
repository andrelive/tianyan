use super::*;
use crate::common::types::ContextNamespace;
use tempfile::tempdir;

#[tokio::test]
async fn test_local_storage_initialize() {
    let dir = tempdir().unwrap();
    let mut config = StorageConfig::default();
    config.data_dir = dir.path().into();
    let storage = LocalStorageBackend::new(config);

    storage.initialize().await.unwrap();

    assert!(dir.path().join("user").exists());
    assert!(dir.path().join("session").exists());
    assert!(dir.path().join("memory").exists());
    assert!(dir.path().join("knowledge").exists());
    assert!(dir.path().join("agent").exists());
    assert!(dir.path().join("skill").exists());
}

#[tokio::test]
async fn test_local_storage_write_read_entry() {
    let dir = tempdir().unwrap();
    let mut config = StorageConfig::default();
    config.data_dir = dir.path().into();
    let storage = LocalStorageBackend::new(config);
    storage.initialize().await.unwrap();

    let uri = TianyanUri::new(
        ContextNamespace::User,
        vec!["profile".to_string(), "basic_info".to_string()],
    );

    let mut entry = ContextEntry::new_file(uri.clone());
    entry.abstract_content = Some("测试摘要".to_string());
    entry.overview_content = Some("测试概览".to_string());
    entry.detail_content = Some("测试详细内容".to_string());

    storage.write_entry(&entry).await.unwrap();

    let read_entry = storage.read_entry(&uri).await.unwrap();
    assert_eq!(read_entry.metadata.uri, uri);
    assert_eq!(read_entry.abstract_content, Some("测试摘要".to_string()));
    assert_eq!(read_entry.overview_content, Some("测试概览".to_string()));
    assert_eq!(read_entry.detail_content, Some("测试详细内容".to_string()));
}

#[tokio::test]
async fn test_local_storage_delete_entry() {
    let dir = tempdir().unwrap();
    let mut config = StorageConfig::default();
    config.data_dir = dir.path().into();
    let storage = LocalStorageBackend::new(config);
    storage.initialize().await.unwrap();

    let uri = TianyanUri::new(
        ContextNamespace::User,
        vec!["profile".to_string(), "test".to_string()],
    );

    let entry = ContextEntry::new_file(uri.clone());
    storage.write_entry(&entry).await.unwrap();

    assert!(storage.exists(&uri).await.unwrap());

    storage.delete_entry(&uri).await.unwrap();
    assert!(!storage.exists(&uri).await.unwrap());
}

#[tokio::test]
async fn test_local_storage_list_directory() {
    let dir = tempdir().unwrap();
    let mut config = StorageConfig::default();
    config.data_dir = dir.path().into();
    let storage = LocalStorageBackend::new(config);
    storage.initialize().await.unwrap();

    for i in 0..3 {
        let uri = TianyanUri::new(
            ContextNamespace::User,
            vec!["profile".to_string(), format!("entry_{}", i)],
        );
        let entry = ContextEntry::new_file(uri);
        storage.write_entry(&entry).await.unwrap();
    }

    let parent_uri = TianyanUri::new(ContextNamespace::User, vec!["profile".to_string()]);
    let entries = storage.list_directory(&parent_uri).await.unwrap();

    assert_eq!(entries.len(), 3);
}
