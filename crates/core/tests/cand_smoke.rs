use std::path::PathBuf;
use superbackup_core::git::{self, ScanOptions};

#[tokio::test]
#[ignore]
async fn what_is_not_in_git() {
    let root = PathBuf::from(std::env::var("SB_SCAN_ROOT").unwrap_or_else(|_| ".".into()));
    let inv =
        git::inventory(std::slice::from_ref(&root), &ScanOptions::default()).await.expect("scan");
    eprintln!("{} repos, {} not in git", inv.repos.len(), inv.candidates.len());
    for c in &inv.candidates {
        eprintln!("  {:<28} entries={:<5} project={}", c.name, c.entries, c.looks_like_a_project);
    }
}
