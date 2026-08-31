//! Building the repository encryption key export — the protocol's one
//! sanctioned path for secret material to leave the daemon.
//!
//! ## Why this exists at all
//!
//! A repository encryption key that is lost is a backup that no longer exists.
//! That is not a bug in kopia; it is the property that makes the encryption
//! worth having. It also means a product that seals a user's only copy of
//! their data behind a key it refuses to let them write down has not protected
//! them — it has taken their data hostage against its own continued
//! correctness.
//!
//! So the export exists, and the document it produces is written to be read by
//! a stranger years from now with nothing but a `kopia` binary: every
//! destination's name, kind, location, algorithms, machine label and key, plus
//! the command that opens it. Nothing in this file refers to superbackup's own
//! formats, because the whole point is that superbackup may not be there.
//!
//! ## What makes it safe enough
//!
//! The bounds are on the *command*, not on this module — see
//! `vault.export_keys` in `crates/core/src/ipc/protocol.rs` for the full list.
//! What this module owes them is narrower:
//!
//! * It is a pure function of a `Store` and its configuration. It opens no
//!   file, writes nothing, and returns the document to its caller. The daemon
//!   may be running as SYSTEM; taking a path from a client and writing to it
//!   would be an arbitrary-file-write primitive far worse than the disclosure
//!   this feature is about.
//! * Nothing it produces is logged. The caller records *that* an export
//!   happened; the contents never reach a tracing span, an event, or an error
//!   message. Every failure path here names a destination, never a key.
//! * A destination it cannot export is **listed as omitted**, never dropped. A
//!   key file that silently lacks one repository is worse than no key file,
//!   because the user will believe they are covered.
//!
//! ## Replicas
//!
//! A replica destination is the same kopia repository as the destination it
//! synchronises from, with the same key. It is exported anyway, with its own
//! location and a line saying whose key it shares, because at recovery time
//! the person holding this paper needs to know that the second copy opens with
//! the first copy's key — that is exactly the fact a replica exists to provide.

use chrono::{DateTime, Utc};
use superbackup_core::config::{destination_passphrase, Store};
use superbackup_core::model::{
    Config, Destination, DestinationKind, PassphraseSource, ProviderKind,
};
use superbackup_core::secret::Secret;

/// The finished document plus the accounting the caller has to report.
pub struct KeyExport {
    pub document: String,
    /// Destinations that carry a key and appear in the document.
    pub exported: u32,
    /// `"<name>: <reason>"` for every destination that does not, in
    /// configuration order.
    pub omitted: Vec<String>,
}

/// Build the export document.
///
/// Takes an already-unlocked [`Store`]; the authorisation — an unlocked vault
/// plus the master passphrase re-presented — happens in the handler, because
/// that is where the rate limit and the audit event live too.
pub fn build(store: &Store, generated_at: DateTime<Utc>) -> KeyExport {
    let config = store.config();
    let mut body = String::new();
    let mut exported = 0u32;
    let mut omitted = Vec::new();

    for destination in &config.destinations {
        match key_for(store, config, destination) {
            Ok(Some(section)) => {
                exported += 1;
                body.push_str(&section);
                body.push_str("\r\n");
            }
            Ok(None) => omitted.push(format!(
                "{}: a folder mirror holds plain copies and has no encryption key",
                destination.name
            )),
            // The reason is this program's own prose about configuration, not
            // anything derived from the key. Checked by a test below.
            Err(reason) => omitted.push(format!("{}: {reason}", destination.name)),
        }
    }

    let mut document = header(config, generated_at, exported);
    if exported == 0 {
        document.push_str(
            "There are no repository encryption keys to write down yet. Once you have\r\n\
             created a repository, export again and keep the result.\r\n\r\n",
        );
    } else {
        document.push_str(&body);
    }
    if !omitted.is_empty() {
        document.push_str("NOT INCLUDED IN THIS FILE\r\n");
        document.push_str("-------------------------\r\n\r\n");
        for line in &omitted {
            document.push_str("  - ");
            document.push_str(line);
            document.push_str("\r\n");
        }
        document.push_str("\r\n");
    }
    document.push_str(&format!(
        "End of file. {} repositor{} listed.\r\n",
        exported,
        if exported == 1 { "y" } else { "ies" }
    ));

    KeyExport { document, exported, omitted }
}

/// The suggested file name. Not a path: the daemon never chooses where this
/// lands.
pub fn suggested_file_name(config: &Config, generated_at: DateTime<Utc>) -> String {
    let slug: String = config
        .machine
        .slug
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let slug = if slug.is_empty() { "superbackup".to_string() } else { slug };
    format!("superbackup-encryption-keys-{slug}-{}.txt", generated_at.format("%Y%m%d"))
}

/// The plain-English preamble. Deliberately blunt: somebody reading this file
/// on a printout has no interface to warn them.
fn header(config: &Config, generated_at: DateTime<Utc>, exported: u32) -> String {
    let _ = exported;
    format!(
        "\
SUPERBACKUP - REPOSITORY ENCRYPTION KEYS\r\n\
=========================================\r\n\
\r\n\
READ THIS FIRST\r\n\
\r\n\
This file contains the encryption keys for the backups made by the computer\r\n\
named below. Anyone who has this file AND can reach the storage listed in it\r\n\
can read every file in those backups. Treat it exactly as you would treat the\r\n\
backed-up files themselves: a locked drawer, a safe, or a password manager.\r\n\
It is not encrypted. Do not email it and do not leave it in your Downloads\r\n\
folder.\r\n\
\r\n\
The other half of the warning matters just as much. If you lose these keys,\r\n\
the backups cannot be opened by anyone, including us. There is no recovery\r\n\
process, no reset link, and no support request that gets them back. That is\r\n\
what the encryption is for.\r\n\
\r\n\
You do not need superbackup to use this. Install kopia from kopia.io, run the\r\n\
`kopia repository connect` command shown under a repository, and give it that\r\n\
repository's encryption key when it asks for a password. Then\r\n\
`kopia snapshot list --all` shows what is there and `kopia restore` gets it\r\n\
back.\r\n\
\r\n\
  Computer:   {label} ({slug})\r\n\
  Written:    {when} UTC\r\n\
  By:         superbackup {version}\r\n\
\r\n\
-----------------------------------------------------------------------------\r\n\
\r\n",
        label = config.machine.label,
        slug = config.machine.slug,
        when = generated_at.format("%Y-%m-%d %H:%M"),
        version = superbackup_core::VERSION,
    )
}

/// One destination's section, or `Ok(None)` when it legitimately has no key.
///
/// `Err` carries a reason written by this program. It must never be built from
/// a secret, and the caller puts it in the "not included" list verbatim.
fn key_for(
    store: &Store,
    config: &Config,
    destination: &Destination,
) -> std::result::Result<Option<String>, String> {
    if !destination.kind.is_repository() {
        return Ok(None);
    }
    let secret: Secret = destination_passphrase(store, destination).map_err(|e| {
        // `Error::Locked` cannot happen here (the handler required an unlocked
        // vault) so anything left is a configuration fault worth naming.
        format!("its encryption key could not be resolved ({})", first_sentence(&e.to_string()))
    })?;
    let Some(key) = secret.expose_str().map(str::to_string) else {
        return Err("its encryption key is not text and cannot be printed".to_string());
    };

    let mut out = String::new();
    out.push_str(&format!("REPOSITORY: {}\r\n", destination.name));
    out.push_str(&format!("{}\r\n\r\n", "-".repeat(12 + destination.name.chars().count())));
    out.push_str(&format!("  Kind:       {}\r\n", destination.kind.label()));
    out.push_str(&format!("  Location:   {}\r\n", location(config, destination)));
    out.push_str(&format!("  Computer:   {}\r\n", config.machine.label));

    let encryption = destination.encryption.clone().unwrap_or_default();
    out.push_str(&format!("  Encryption: {}\r\n", encryption.algorithm.kopia_id()));
    out.push_str(&format!("  Hash:       {}\r\n", encryption.hash.kopia_id()));
    out.push_str(&format!("  Splitter:   {}\r\n", encryption.splitter.kopia_id()));
    if let Some(ecc) = encryption.ecc {
        out.push_str(&format!(
            "  Correction: {} at {}%\r\n",
            ecc.kopia_id(),
            encryption.ecc_overhead_percent
        ));
    }
    out.push_str(&format!("  Key source: {}\r\n", describe_source(encryption.passphrase_source)));
    if let Some(root) = config.replication_root(destination) {
        if root.id != destination.id {
            out.push_str(&format!(
                "  Note:       this is a copy of \"{}\" and opens with the same key.\r\n",
                root.name
            ));
        }
    }
    out.push_str("\r\n  ENCRYPTION KEY (this is the password kopia asks for):\r\n\r\n");
    for line in group(&key) {
        out.push_str("      ");
        out.push_str(&line);
        out.push_str("\r\n");
    }
    out.push_str("\r\n  To open it:\r\n\r\n");
    out.push_str("      ");
    out.push_str(&connect_command(config, destination));
    out.push_str("\r\n\r\n");
    Ok(Some(out))
}

/// Where the repository is, in a form somebody can act on.
fn location(config: &Config, destination: &Destination) -> String {
    match &destination.kind {
        DestinationKind::LocalRepository { path } | DestinationKind::LocalMirror { path } => {
            path.display().to_string()
        }
        DestinationKind::OneDrive { path, account } => match account {
            Some(a) => format!("{} (OneDrive account {a})", path.display()),
            None => path.display().to_string(),
        },
        DestinationKind::S3 { provider_id, bucket, prefix, .. } => {
            let endpoint = match config.provider(provider_id).map(|p| &p.kind) {
                Some(ProviderKind::S3 { endpoint, region, .. }) if !region.is_empty() => {
                    format!("{endpoint} (region {region})")
                }
                Some(ProviderKind::S3 { endpoint, .. }) => endpoint.clone(),
                // The provider is gone from the configuration. Say so rather
                // than printing a bucket with no host, which cannot be used.
                None => "unknown endpoint - the storage provider is missing".to_string(),
            };
            let key = if prefix.is_empty() {
                bucket.clone()
            } else {
                format!("{bucket}/{}", prefix.trim_start_matches('/'))
            };
            format!("{key} on {endpoint}")
        }
    }
}

/// The `kopia repository connect` line for this destination.
///
/// Written out with the real flags rather than "see the docs", because the
/// person reading this may be doing so at the worst moment of their year.
fn connect_command(config: &Config, destination: &Destination) -> String {
    match &destination.kind {
        DestinationKind::LocalRepository { path } | DestinationKind::OneDrive { path, .. } => {
            format!("kopia repository connect filesystem --path=\"{}\"", path.display())
        }
        DestinationKind::S3 { provider_id, bucket, prefix, .. } => {
            let mut cmd = format!("kopia repository connect s3 --bucket=\"{bucket}\"");
            if let Some(ProviderKind::S3 { endpoint, region, .. }) =
                config.provider(provider_id).map(|p| &p.kind)
            {
                let host = superbackup_core::kopia::s3_endpoint_host(endpoint).0;
                if !host.is_empty() {
                    cmd.push_str(&format!(" --endpoint=\"{host}\""));
                }
                if !region.is_empty() {
                    cmd.push_str(&format!(" --region=\"{region}\""));
                }
            }
            if !prefix.is_empty() {
                cmd.push_str(&format!(" --prefix=\"{prefix}\""));
            }
            cmd.push_str(" --access-key=... --secret-access-key=...");
            cmd
        }
        DestinationKind::LocalMirror { .. } => {
            "not a repository - the files are stored as ordinary copies".to_string()
        }
    }
}

fn describe_source(source: PassphraseSource) -> &'static str {
    match source {
        PassphraseSource::Generated => "generated by superbackup and kept in the vault",
        PassphraseSource::UserSupplied => "chosen by you and kept in the vault",
        PassphraseSource::DerivedFromMaster => {
            "worked out from your master passphrase (the key below is the result)"
        }
    }
}

/// Eight-character groups, four to a line, so a key can be typed back in from
/// paper without losing your place. The same grouping the "write it down"
/// screen uses, for the same reason.
fn group(key: &str) -> Vec<String> {
    let chars: Vec<char> = key.chars().collect();
    chars
        .chunks(32)
        .map(|line| {
            line.chunks(8).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("  ")
        })
        .collect()
}

/// Trim a multi-clause error down to something that fits one line of a text
/// file without carrying a path the user does not need here.
fn first_sentence(message: &str) -> String {
    let trimmed = message.trim();
    match trimmed.find(". ") {
        Some(i) => trimmed[..i].to_string(),
        None => trimmed.trim_end_matches('.').to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use superbackup_core::model::{EncryptionSettings, SecretRef};
    use uuid::Uuid;

    fn store_with(destinations: Vec<Destination>) -> Store {
        let dir = std::env::temp_dir().join(format!("sb-keyexport-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let paths = superbackup_core::paths::Paths::rooted_at(&dir, false);
        let mut store = Store::initialise(paths, &Secret::from_str("test-master-passphrase"))
            .expect("a fresh store");
        let mut config = store.config().clone();
        config.machine.label = "Studio".into();
        config.machine.slug = "studio-1a2b3c4d".into();
        config.destinations = destinations;
        store.set_config(config).expect("store the configuration");
        store
    }

    fn repository(name: &str, path: &str) -> Destination {
        let id = Uuid::new_v4();
        Destination {
            id,
            name: name.into(),
            kind: DestinationKind::LocalRepository { path: path.into() },
            encryption: Some(EncryptionSettings::default()),
            passphrase_ref: Some(SecretRef::new("repo-passphrase", &id)),
            retention: Default::default(),
            enabled: true,
            auto_discovered: false,
            bandwidth: None,
            replicate_from: None,
            created_at: Utc::now(),
            last_verified_at: None,
        }
    }

    fn mirror(name: &str, path: &str) -> Destination {
        let mut d = repository(name, path);
        d.kind = DestinationKind::LocalMirror { path: path.into() };
        d.encryption = None;
        d.passphrase_ref = None;
        d
    }

    #[test]
    fn the_document_carries_the_key_and_the_command_to_use_it() {
        let dest = repository("Archive", r"D:\backups\archive");
        let handle = dest.passphrase_ref.clone().expect("a handle");
        let mut store = store_with(vec![dest]);
        store
            .put_secret(handle, Secret::from_str("KEYKEYKEYKEYKEYKEYKEYKEYKEYKEY01"))
            .expect("store the key");

        let export = build(&store, Utc::now());
        assert_eq!(export.exported, 1);
        assert!(export.omitted.is_empty(), "{:?}", export.omitted);
        // The key is present, in groups, and so is the command that uses it.
        assert!(export.document.contains("KEYKEYKE  YKEYKEYK"), "{}", export.document);
        assert!(export.document.contains("kopia repository connect filesystem"));
        assert!(export.document.contains(r"D:\backups\archive"));
        assert!(export.document.contains("AES256-GCM-HMAC-SHA256"));
        assert!(export.document.contains("Studio"));
        // The warning has to be in the file itself, not only in the interface.
        assert!(export.document.contains("can read every file in those backups"));
        assert!(export.document.contains("There is no recovery"));
    }

    #[test]
    fn a_destination_with_no_key_is_listed_rather_than_dropped() {
        // A repository whose secret was never stored, plus a folder mirror.
        let store = store_with(vec![repository("Broken", r"D:\b"), mirror("Copy", r"E:\c")]);
        let export = build(&store, Utc::now());
        assert_eq!(export.exported, 0);
        assert_eq!(export.omitted.len(), 2);
        assert!(export.omitted.iter().any(|o| o.starts_with("Broken:")), "{:?}", export.omitted);
        assert!(export.omitted.iter().any(|o| o.contains("folder mirror")), "{:?}", export.omitted);
        // And the document says so out loud.
        assert!(export.document.contains("NOT INCLUDED IN THIS FILE"));
        assert!(export.document.contains("Broken"));
    }

    #[test]
    fn a_reason_for_omission_never_carries_key_material() {
        let dest = repository("Odd", r"D:\odd");
        let handle = dest.passphrase_ref.clone().expect("a handle");
        let mut store = store_with(vec![dest]);
        // Non-UTF-8 key material: printable text is impossible, and the reason
        // must say that without echoing the bytes.
        store.put_secret(handle, Secret::new(vec![0xff, 0xfe, 0xfd, 0xfc])).expect("store");
        let export = build(&store, Utc::now());
        assert_eq!(export.exported, 0);
        assert_eq!(export.omitted.len(), 1);
        assert!(export.omitted[0].contains("not text"), "{:?}", export.omitted);
        assert!(!export.document.contains('\u{fffd}'), "the raw bytes must not be echoed");
    }

    #[test]
    fn a_replica_says_whose_key_it_shares() {
        let root = repository("Primary", r"D:\primary");
        let root_id = root.id;
        let handle = root.passphrase_ref.clone().expect("a handle");
        let mut replica = repository("Offsite copy", r"E:\offsite");
        replica.replicate_from = Some(root_id);
        replica.passphrase_ref = None;
        // A replica carries no encryption settings either: `sync-to` copies the
        // source's format blob, so the cipher suite and the key are the
        // source's. `config::validate` rejects a replica that declares its own.
        replica.encryption = None;

        let mut store = store_with(vec![root, replica]);
        store.put_secret(handle, Secret::from_str("SHAREDSHAREDSHARED")).expect("store the key");

        let export = build(&store, Utc::now());
        assert_eq!(export.exported, 2, "{:?}", export.omitted);
        assert!(export.document.contains("this is a copy of \"Primary\""), "{}", export.document);
    }

    #[test]
    fn the_suggested_name_is_a_file_name_and_not_a_path() {
        let mut config = Config::default();
        config.machine.slug = "studio/../../etc".into();
        let name = suggested_file_name(&config, Utc::now());
        assert!(!name.contains('/') && !name.contains('\\') && !name.contains(".."), "{name}");
        assert!(name.ends_with(".txt"));
    }

    #[test]
    fn an_s3_destination_lists_bucket_prefix_and_endpoint() {
        use superbackup_core::model::{S3Credentials, StorageProvider};
        let provider_id = Uuid::new_v4();
        let mut dest = repository("Offsite", "");
        dest.kind = DestinationKind::S3 {
            provider_id,
            bucket: "backups".into(),
            prefix: "superbackup/studio/".into(),
            credential_override: None,
        };
        let handle = dest.passphrase_ref.clone().expect("a handle");
        // The provider has to exist before the destination that names it:
        // `Store::set_config` validates, and a dangling provider id is exactly
        // the kind of thing it refuses.
        let mut store = store_with(Vec::new());
        let mut config = store.config().clone();
        config.providers.push(StorageProvider {
            id: provider_id,
            name: "StorJ".into(),
            kind: ProviderKind::S3 {
                endpoint: "https://gateway.storjshare.io".into(),
                region: "eu1".into(),
                credentials: S3Credentials {
                    access_key_ref: SecretRef::new("s3-access-key", &provider_id),
                    secret_key_ref: SecretRef::new("s3-secret-key", &provider_id),
                    session_token_ref: None,
                },
                tls: true,
                path_style: false,
                flavour: Default::default(),
            },
            created_at: Utc::now(),
            last_verified_at: None,
            notes: String::new(),
        });
        config.destinations.push(dest);
        store.set_config(config).expect("store the configuration");
        store.put_secret(handle, Secret::from_str("S3KEY")).expect("store the key");

        let export = build(&store, Utc::now());
        assert!(export.document.contains("backups/superbackup/studio/"), "{}", export.document);
        assert!(export.document.contains("gateway.storjshare.io"));
        assert!(export.document.contains("--endpoint=\"gateway.storjshare.io\""));
        assert!(export.document.contains("--region=\"eu1\""));
    }
}
