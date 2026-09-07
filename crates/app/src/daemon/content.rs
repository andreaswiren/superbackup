//! The daemon's answer to "what goes in a vault or keys backup".
//!
//! [`superbackup_core::engine::protected`] defines the shape and does the
//! sealing; this supplies the two things it cannot reach from the engine
//! crate: the unlocked vault's master passphrase, and the configuration that
//! says which keys the user ticked.
//!
//! # Why this needs the passphrase and not just an unlocked vault
//!
//! Sealing a key bundle means encrypting *under the master passphrase*, so
//! that the machine which restores it can open it with the thing its owner
//! knows. Derived keys from the running vault would produce a bundle that only
//! this installation could read, which defeats the purpose. So the run needs
//! the passphrase, and a locked daemon cannot make one of these backups at
//! all — it fails the run and says so, rather than writing an empty snapshot.
//!
//! That is also why a vault or keys job is worth scheduling on a machine where
//! the vault is unlocked at boot from the OS credential store, and worth
//! running by hand on one where it is not.

use std::path::PathBuf;
use std::sync::Arc;

use superbackup_core::engine::clock::BoxFuture;
use superbackup_core::engine::protected::{
    self, ContentProvider, StagedContent, Staging,
};
use superbackup_core::error::{Error, Result};
use superbackup_core::model::{JobContent, Source};
use uuid::Uuid;

use super::runtime::Runtime;

/// Builds vault and key payloads from the daemon's own state.
#[derive(Debug)]
pub struct DaemonContent {
    runtime: Arc<Runtime>,
}

impl DaemonContent {
    pub fn new(runtime: Arc<Runtime>) -> Arc<DaemonContent> {
        Arc::new(DaemonContent { runtime })
    }

    /// The keys ticked for backup, each with its public half.
    ///
    /// The `.pub` travels with the private key because a key without its
    /// public half is one ssh can still use and a person cannot identify — and
    /// identifying which of four restored keys is the one a server knows is
    /// exactly the problem you have on the day you use this backup.
    fn key_paths(config: &superbackup_core::model::Config) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for key in &config.settings.backed_up_keys {
            paths.push(key.clone());
            let public = key.with_extension("pub");
            if public.is_file() {
                paths.push(public);
            }
        }
        paths
    }
}

impl ContentProvider for DaemonContent {
    fn stage<'a>(
        &'a self,
        content: JobContent,
        run_id: Uuid,
    ) -> BoxFuture<'a, Result<StagedContent>> {
        Box::pin(async move {
            // `master()` returns `Locked` rather than `None`, so a locked
            // daemon cannot fall through into building an empty bundle.
            let passphrase = self.runtime.master().map_err(|_| {
                Error::Locked
            })?;
            let config = { self.runtime.store.lock().await.config().clone() };
            let machine = config.machine.label.clone();
            let vault_file = self.runtime.paths.vault_file();
            let dir = protected::staging_dir(&self.runtime.paths.data_dir, run_id);

            // Reading keys and writing the sealed bundle are blocking file
            // operations on a runtime that is also servicing IPC.
            tokio::task::spawn_blocking(move || {
                let staging = Staging::create(dir)?;
                let names = match content {
                    JobContent::Keys => {
                        let keys = DaemonContent::key_paths(&config);
                        protected::stage_keys(staging.dir(), &machine, &keys, &passphrase)?
                    }
                    JobContent::Vault => protected::stage_vault(staging.dir(), &vault_file)?,
                    JobContent::Files => {
                        // Unreachable: the runner only calls this for a
                        // prepared content. Refused rather than defaulted,
                        // because the default would be to back up the staging
                        // folder, which is empty.
                        return Err(Error::Internal(
                            "a files job asked for a prepared payload".into(),
                        ));
                    }
                };
                Ok(StagedContent {
                    sources: vec![Source::new(staging.dir().to_path_buf())],
                    notes: names,
                    staging,
                })
            })
            .await
            .map_err(|e| Error::Internal(format!("preparing the backup did not finish: {e}")))?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The public half is picked up automatically, and only for keys that
    /// have one. A missing `.pub` must not put a path that does not exist into
    /// the bundle, which `stage_keys` would then refuse the whole run over.
    #[test]
    fn a_keys_payload_carries_each_public_half_that_exists() {
        let dir = std::env::temp_dir().join(format!("sb-content-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("create");
        let paired = dir.join("id_ed25519");
        let lonely = dir.join("id_rsa");
        std::fs::write(&paired, b"k").expect("write");
        std::fs::write(dir.join("id_ed25519.pub"), b"p").expect("write");
        std::fs::write(&lonely, b"k").expect("write");

        let mut config = superbackup_core::model::Config::default();
        config.settings.backed_up_keys = vec![paired.clone(), lonely.clone()];

        let paths = DaemonContent::key_paths(&config);
        assert_eq!(paths, vec![paired.clone(), dir.join("id_ed25519.pub"), lonely]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
