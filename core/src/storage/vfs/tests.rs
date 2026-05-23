use super::VirtualFileSystemImpl;
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::storage::test_utils::create_test_vfs;
use crate::storage::{ContentStore, VfsCore, VfsSearch, VirtualFileSystem};

#[tokio::test]
async fn test_vfs_initialize() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();
}

#[tokio::test]
async fn test_vfs_create_directory() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let uri = TianyanUri::new(ContextNamespace::User, vec!["test_dir".to_string()]);
    let entry = vfs.create_directory(&uri).await.unwrap();

    assert!(entry.is_directory());
    assert!(vfs.exists(&uri).await.unwrap());
}

#[tokio::test]
async fn test_vfs_create_file() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let uri = TianyanUri::new(
        ContextNamespace::User,
        vec!["test_dir".to_string(), "test_file".to_string()],
    );
    let entry = vfs.create_file(&uri).await.unwrap();

    assert!(!entry.is_directory());
    assert!(vfs.exists(&uri).await.unwrap());
}

#[tokio::test]
async fn test_vfs_write_read_content() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
    vfs.create_file(&uri).await.unwrap();

    vfs.write(&uri, ContentLevel::Detail, "测试详情")
        .await
        .unwrap();

    let detail_content = vfs.read(&uri, ContentLevel::Detail).await.unwrap();
    assert_eq!(detail_content, "测试详情");
}

#[tokio::test]
async fn test_vfs_delete() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
    vfs.create_file(&uri).await.unwrap();

    assert!(vfs.exists(&uri).await.unwrap());

    vfs.delete(&uri).await.unwrap();
    assert!(!vfs.exists(&uri).await.unwrap());
}

#[tokio::test]
async fn test_vfs_list() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let parent_uri = TianyanUri::new(ContextNamespace::User, vec!["parent".to_string()]);
    vfs.create_directory(&parent_uri).await.unwrap();

    for i in 0..3 {
        let child_uri = parent_uri.append(&format!("child_{}", i));
        vfs.create_file(&child_uri).await.unwrap();
    }

    let entries = vfs.list(&parent_uri).await.unwrap();
    assert_eq!(entries.len(), 3);
}

#[tokio::test]
async fn test_vfs_move() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let source = TianyanUri::new(ContextNamespace::User, vec!["source".to_string()]);
    let dest = TianyanUri::new(ContextNamespace::User, vec!["dest".to_string()]);

    vfs.create_file(&source).await.unwrap();
    vfs.write(&source, ContentLevel::Detail, "测试内容")
        .await
        .unwrap();

    vfs.move_entry(&source, &dest).await.unwrap();

    assert!(!vfs.exists(&source).await.unwrap());
    assert!(vfs.exists(&dest).await.unwrap());

    let content = vfs.read(&dest, ContentLevel::Detail).await.unwrap();
    assert_eq!(content, "测试内容");
}

#[tokio::test]
async fn test_vfs_copy() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let source = TianyanUri::new(ContextNamespace::User, vec!["source".to_string()]);
    let dest = TianyanUri::new(ContextNamespace::User, vec!["dest".to_string()]);

    vfs.create_file(&source).await.unwrap();
    vfs.write(&source, ContentLevel::Detail, "测试内容")
        .await
        .unwrap();

    vfs.copy_entry(&source, &dest).await.unwrap();

    assert!(vfs.exists(&source).await.unwrap());
    assert!(vfs.exists(&dest).await.unwrap());

    let content = vfs.read(&dest, ContentLevel::Detail).await.unwrap();
    assert_eq!(content, "测试内容");
}

#[tokio::test]
async fn test_content_loader() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
    vfs.create_file(&uri).await.unwrap();
    vfs.write(&uri, ContentLevel::Detail, "测试内容")
        .await
        .unwrap();

    let content = vfs.read(&uri, ContentLevel::Detail).await.unwrap();
    assert_eq!(content, "测试内容");

    let has_content = vfs.has_content(&uri, ContentLevel::Detail).await.unwrap();
    assert!(has_content);
}

#[test]
fn test_validate_uri() {
    let valid_uri = TianyanUri::new(ContextNamespace::User, vec!["test".to_string()]);
    assert!(VirtualFileSystemImpl::validate_uri(&valid_uri).is_ok());

    let root_uri = TianyanUri::new(ContextNamespace::User, vec![]);
    assert!(VirtualFileSystemImpl::validate_uri(&root_uri).is_ok());
}

#[tokio::test]
async fn test_search_session() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let session_uri = TianyanUri::new(ContextNamespace::Session, vec!["session_test".to_string()]);
    vfs.create_file(&session_uri).await.unwrap();
    vfs.write(&session_uri, ContentLevel::Detail, "会话搜索测试")
        .await
        .unwrap();

    let content = vfs.read(&session_uri, ContentLevel::Detail).await.unwrap();
    assert_eq!(content, "会话搜索测试");

    let results = vfs
        .search("搜索", 10, Some(ContextNamespace::Session))
        .await;
    assert!(results.is_err());
}

#[tokio::test]
async fn test_search_memory() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let memory_uri = TianyanUri::new(ContextNamespace::Memory, vec!["memory_test".to_string()]);
    vfs.create_file(&memory_uri).await.unwrap();
    vfs.write(&memory_uri, ContentLevel::Detail, "记忆搜索测试")
        .await
        .unwrap();

    let results = vfs.search("搜索", 10, Some(ContextNamespace::Memory)).await;
    assert!(results.is_err());
}

#[tokio::test]
async fn test_search_knowledge() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let knowledge_uri = TianyanUri::new(
        ContextNamespace::Knowledge,
        vec!["knowledge_test".to_string()],
    );
    vfs.create_file(&knowledge_uri).await.unwrap();
    vfs.write(&knowledge_uri, ContentLevel::Detail, "知识搜索测试")
        .await
        .unwrap();

    let results = vfs
        .search("搜索", 10, Some(ContextNamespace::Knowledge))
        .await;
    assert!(results.is_err());
}

#[tokio::test]
async fn test_search_skill() {
    let vfs = create_test_vfs().await;
    vfs.initialize().await.unwrap();

    let skill_uri = TianyanUri::new(ContextNamespace::Skill, vec!["skill_test".to_string()]);
    vfs.create_file(&skill_uri).await.unwrap();
    vfs.write(&skill_uri, ContentLevel::Detail, "技能搜索测试")
        .await
        .unwrap();

    let results = vfs.search("搜索", 10, Some(ContextNamespace::Skill)).await;
    assert!(results.is_err());
}
