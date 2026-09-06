//! Which kind of git host this is, and creating a repository on it.
//!
//! # Two ways in
//!
//! **GitHub, through `gh`.** If the GitHub CLI is installed and signed in, it
//! already holds a working credential — one the user set up themselves, scoped
//! however they chose, refreshed by `gh` and revocable by them. Using it means
//! superbackup never asks for, stores, or is able to leak a GitHub token. That
//! is a real security property, not a convenience, and it is why `gh` is
//! preferred over the REST API wherever it is available.
//!
//! **Everything else, through the host's API with a token.** Gitea, Forgejo,
//! GitLab and Azure DevOps have no equivalent of `gh`, so creating a
//! repository there needs a personal access token. That token is a secret: it
//! is held in the vault like every other secret, passed in a header and never
//! in a URL or an argument, and never written into `.git/config` — where the
//! obvious implementation (`https://token@host/...` as the remote) would put
//! it in plain text on disk, in every error message, and in the backup.
//!
//! # What creating a repository does *not* do
//!
//! It does not push. Creating an empty repository on a forge is reversible in
//! one click; pushing a tree that turned out to contain a `.env` is not. The
//! remote is added and the first push is left to the user, who can look at
//! what is about to leave the machine first.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::secret::Secret;

/// The kind of host a remote points at.
///
/// Gitea and Forgejo share an API — Forgejo is a fork of Gitea and kept its
/// `/api/v1` surface — so one variant covers both rather than pretending to a
/// distinction that a URL cannot support and that changes nothing about what
/// we send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Forge {
    GitHub,
    GitLab,
    /// Gitea or Forgejo, self-hosted.
    Gitea,
    AzureDevOps,
    Bitbucket,
    /// A host we do not recognise. Almost certainly self-hosted, and the user
    /// can say which software it runs.
    Unknown,
    /// The remote is a path on disk, or there is no remote at all.
    None,
}

impl Forge {
    /// Guess from the hostname.
    ///
    /// Only the public hosts can be identified this way. A self-hosted GitLab
    /// at `git.company.com` is indistinguishable from a Gitea at the same
    /// address, so it is reported as [`Forge::Unknown`] rather than guessed —
    /// sending a GitLab-shaped request to a Gitea produces a 404 that reads
    /// like the user's fault.
    pub fn from_host(host: &str) -> Self {
        let host = host.trim().to_ascii_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host);
        match host {
            "github.com" | "gist.github.com" => Self::GitHub,
            "gitlab.com" => Self::GitLab,
            "bitbucket.org" => Self::Bitbucket,
            "dev.azure.com" | "ssh.dev.azure.com" => Self::AzureDevOps,
            other if other.ends_with(".visualstudio.com") => Self::AzureDevOps,
            // A subdomain of a public host is still that host.
            other if other.ends_with(".github.com") => Self::GitHub,
            other if other.ends_with(".gitlab.com") => Self::GitLab,
            "" => Self::None,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Gitea => "Gitea or Forgejo",
            Self::AzureDevOps => "Azure DevOps",
            Self::Bitbucket => "Bitbucket",
            Self::Unknown => "Self-hosted",
            Self::None => "No remote",
        }
    }

    /// Can superbackup create a repository here?
    pub fn can_create(self) -> bool {
        matches!(self, Self::GitHub | Self::GitLab | Self::Gitea)
    }
}

/// Who a new repository belongs to and whether the world can see it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRepo {
    /// The repository name. Not a path: the owner is separate.
    pub name: String,
    /// The user or organisation to create it under. `None` means the
    /// authenticated account's own namespace.
    pub owner: Option<String>,
    pub description: Option<String>,
    /// **Defaults to private, and every caller must say otherwise
    /// explicitly.** A tool that scans a developer's disk and offers to put
    /// what it finds on the internet must not have "public" as the value that
    /// happens when nobody chose.
    pub private: bool,
}

impl NewRepo {
    pub fn private(name: impl Into<String>) -> Self {
        Self { name: name.into(), owner: None, description: None, private: true }
    }

    /// A repository name the forges will accept.
    ///
    /// Checked here rather than left to the API, because every forge rejects a
    /// bad name with a different message and half of them return a 422 with no
    /// body at all.
    pub fn validate(&self) -> Result<()> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(Error::Validation("a repository needs a name".into()));
        }
        if name.len() > 100 {
            return Err(Error::Validation(
                "a repository name is at most 100 characters".into(),
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(Error::Validation(
                "a repository name can hold letters, digits, dashes, underscores and dots only"
                    .into(),
            ));
        }
        if name.starts_with('.') || name == "." || name == ".." {
            return Err(Error::Validation(
                "a repository name cannot start with a dot".into(),
            ));
        }
        if let Some(owner) = &self.owner {
            if owner.trim().is_empty() {
                return Err(Error::Validation(
                    "leave the owner empty to use your own account, rather than blank".into(),
                ));
            }
            if owner.contains('/') {
                return Err(Error::Validation("an owner is one name, not a path".into()));
            }
        }
        Ok(())
    }

    /// `owner/name`, or just `name` when the owner is the caller.
    pub fn full_name(&self) -> String {
        match &self.owner {
            Some(owner) => format!("{owner}/{}", self.name.trim()),
            None => self.name.trim().to_string(),
        }
    }
}

/// How to reach a forge that is not GitHub-through-`gh`.
#[derive(Debug, Clone)]
pub struct ApiAccess {
    /// The base URL of the host, e.g. `https://gitea.example.com`. No path.
    pub base_url: String,
    /// A personal access token. Held as a [`Secret`] so it cannot be logged,
    /// printed by a derived `Debug`, or serialised into a config file.
    pub token: Secret,
}

impl ApiAccess {
    /// Refuse anything that is not HTTPS to a real host.
    ///
    /// A token sent over plain HTTP is a token given away to every device
    /// between here and the server, and the request that leaks it is the very
    /// first one. `http://localhost` is allowed because a developer testing
    /// against a local Gitea is not crossing a network.
    pub fn validate(&self) -> Result<()> {
        let url = self.base_url.trim().trim_end_matches('/');
        if url.is_empty() {
            return Err(Error::Validation("the host address is empty".into()));
        }
        let local = url.starts_with("http://localhost")
            || url.starts_with("http://127.0.0.1")
            || url.starts_with("http://[::1]");
        if !url.starts_with("https://") && !local {
            return Err(Error::Validation(
                "the host must be reached over https, or the access token is sent in clear text \
                 across the network"
                    .into(),
            ));
        }
        let token = self.token.expose_str().unwrap_or_default();
        if token.trim().is_empty() {
            return Err(Error::Validation("the access token is empty".into()));
        }
        Ok(())
    }

    pub fn base(&self) -> &str {
        self.base_url.trim().trim_end_matches('/')
    }
}

/// A repository that now exists on a forge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedRepo {
    pub full_name: String,
    /// The URL to add as a remote.
    pub clone_url: String,
    /// The page a person would open.
    pub web_url: String,
    pub private: bool,
    /// Which route created it, so the answer can say whether a token was used
    /// or the user's own `gh` login.
    pub via: String,
}

/// The API call for each forge: endpoint, and the JSON body.
///
/// Split out from sending it so the shape of every request is testable without
/// a network, a token, or an account — the part most likely to be wrong is the
/// field names, and those differ on every forge.
pub fn create_request(forge: Forge, access: &ApiAccess, repo: &NewRepo) -> Result<ApiRequest> {
    repo.validate()?;
    access.validate()?;
    let base = access.base();
    let name = repo.name.trim();

    let request = match forge {
        Forge::GitHub => {
            // Creating under an organisation is a different endpoint from
            // creating under yourself, and posting to the wrong one is a 404.
            let url = match &repo.owner {
                Some(org) => format!("{base}/api/v3/orgs/{org}/repos"),
                None => format!("{base}/api/v3/user/repos"),
            };
            ApiRequest {
                url,
                body: serde_json::json!({
                    "name": name,
                    "private": repo.private,
                    "description": repo.description.clone().unwrap_or_default(),
                }),
                auth: AuthStyle::Bearer,
            }
        }
        Forge::Gitea => {
            let url = match &repo.owner {
                Some(org) => format!("{base}/api/v1/orgs/{org}/repos"),
                None => format!("{base}/api/v1/user/repos"),
            };
            ApiRequest {
                url,
                body: serde_json::json!({
                    "name": name,
                    "private": repo.private,
                    "description": repo.description.clone().unwrap_or_default(),
                    "auto_init": false,
                }),
                auth: AuthStyle::Token,
            }
        }
        Forge::GitLab => {
            // GitLab takes the namespace as a numeric id rather than a name,
            // which cannot be guessed from a string — so an owner has to be
            // resolved first, and this refuses rather than creating the
            // repository in the wrong namespace.
            if repo.owner.is_some() {
                return Err(Error::Validation(
                    "GitLab needs a numeric group id rather than a group name. Create the \
                     project in the group from GitLab, then add it as a remote here."
                        .into(),
                ));
            }
            ApiRequest {
                url: format!("{base}/api/v4/projects"),
                body: serde_json::json!({
                    "name": name,
                    "path": name,
                    "visibility": if repo.private { "private" } else { "public" },
                    "description": repo.description.clone().unwrap_or_default(),
                    "initialize_with_readme": false,
                }),
                auth: AuthStyle::PrivateToken,
            }
        }
        Forge::AzureDevOps | Forge::Bitbucket | Forge::Unknown | Forge::None => {
            return Err(Error::Validation(format!(
                "superbackup cannot create a repository on {}. Create it on the host, then add \
                 it as a remote here.",
                forge.label()
            )))
        }
    };
    Ok(request)
}

/// One prepared API call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest {
    pub url: String,
    pub body: serde_json::Value,
    pub auth: AuthStyle,
}

/// How each forge wants the token presented.
///
/// All three are headers. None of them is a query parameter, and that is not
/// an accident: a token in a URL is written to every proxy log, every server
/// access log and the browser history of anyone who pastes the link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStyle {
    /// `Authorization: Bearer <token>` — GitHub.
    Bearer,
    /// `Authorization: token <token>` — Gitea and Forgejo.
    Token,
    /// `PRIVATE-TOKEN: <token>` — GitLab.
    PrivateToken,
}

impl AuthStyle {
    /// The header name and value for a token.
    pub fn header(self, token: &Secret) -> (&'static str, String) {
        // A token that is not valid UTF-8 is not a token any of these forges
        // issued; an empty header fails the request cleanly rather than
        // sending mangled bytes to an authentication endpoint.
        let token = token.expose_str().unwrap_or_default();
        match self {
            Self::Bearer => ("Authorization", format!("Bearer {token}")),
            Self::Token => ("Authorization", format!("token {token}")),
            Self::PrivateToken => ("PRIVATE-TOKEN", token.to_string()),
        }
    }
}

/// The `gh` invocation that creates a repository.
///
/// Built as a list of arguments, never a command line: a repository name is
/// user input, and a name containing `&&` in a shell string would be a command
/// this program ran on the user's machine. [`NewRepo::validate`] already
/// refuses such names, and passing argv means it would be inert even if it
/// did not — two independent reasons it cannot happen, which is the right
/// number for something that would be remote code execution.
pub fn gh_create_args(repo: &NewRepo) -> Result<Vec<String>> {
    repo.validate()?;
    let mut args = vec!["repo".to_string(), "create".to_string(), repo.full_name()];
    args.push(if repo.private { "--private".into() } else { "--public".into() });
    if let Some(description) = &repo.description {
        let description = description.trim();
        if !description.is_empty() {
            args.push("--description".into());
            args.push(description.to_string());
        }
    }
    // No `--push`, no `--source`: creating is reversible and pushing is not.
    // See the module docs.
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_hosts_are_recognised_and_private_ones_are_not_guessed() {
        assert_eq!(Forge::from_host("github.com"), Forge::GitHub);
        assert_eq!(Forge::from_host("GitHub.com"), Forge::GitHub);
        assert_eq!(Forge::from_host("gitlab.com"), Forge::GitLab);
        assert_eq!(Forge::from_host("bitbucket.org"), Forge::Bitbucket);
        assert_eq!(Forge::from_host("dev.azure.com"), Forge::AzureDevOps);
        assert_eq!(Forge::from_host("contoso.visualstudio.com"), Forge::AzureDevOps);
        // A self-hosted GitLab and a self-hosted Gitea look identical from
        // here, so neither is claimed.
        assert_eq!(Forge::from_host("git.company.com"), Forge::Unknown);
        assert_eq!(Forge::from_host(""), Forge::None);
    }

    /// The default that matters most in the whole feature. A backup tool that
    /// reads a developer's disk and offers to create a repository must never
    /// make a public one by accident.
    #[test]
    fn a_new_repository_is_private_unless_someone_said_otherwise() {
        let repo = NewRepo::private("thing");
        assert!(repo.private);
        let args = gh_create_args(&repo).expect("args");
        assert!(args.contains(&"--private".to_string()), "{args:?}");
        assert!(!args.contains(&"--public".to_string()), "{args:?}");

        let public = NewRepo { private: false, ..NewRepo::private("thing") };
        let args = gh_create_args(&public).expect("args");
        assert!(args.contains(&"--public".to_string()), "{args:?}");
    }

    /// Creating is reversible; publishing a tree is not. The `gh` call must
    /// not push, whatever else it does.
    #[test]
    fn creating_a_repository_never_pushes_the_code() {
        let args = gh_create_args(&NewRepo::private("thing")).expect("args");
        for forbidden in ["--push", "--source", "--clone"] {
            assert!(!args.iter().any(|a| a == forbidden), "{forbidden} in {args:?}");
        }
    }

    #[test]
    fn a_repository_name_that_could_be_a_command_is_refused() {
        for bad in [
            "thing && rm -rf /",
            "../escape",
            "with space",
            "with/slash",
            ".hidden",
            "",
            "   ",
        ] {
            let repo = NewRepo::private(bad);
            assert!(repo.validate().is_err(), "{bad:?} must be refused");
            assert!(gh_create_args(&repo).is_err(), "{bad:?} must not reach argv");
        }
        assert!(NewRepo::private("super-backup_2.0").validate().is_ok());
    }

    #[test]
    fn a_token_is_never_sent_over_plain_http_to_a_real_host() {
        let token = Secret::from_str("t0ken");
        let remote = ApiAccess { base_url: "http://gitea.example.com".into(), token: token.clone() };
        let err = remote.validate().expect_err("http is refused");
        assert!(err.to_string().contains("https"), "{err}");

        // Local development is not a network hop.
        let local = ApiAccess { base_url: "http://localhost:3000".into(), token: token.clone() };
        assert!(local.validate().is_ok());

        let secure = ApiAccess { base_url: "https://gitea.example.com/".into(), token };
        assert!(secure.validate().is_ok());
        assert_eq!(secure.base(), "https://gitea.example.com", "the trailing slash is dropped");

        let empty =
            ApiAccess { base_url: "https://gitea.example.com".into(), token: Secret::from_str("  ") };
        assert!(empty.validate().is_err(), "an empty token is not authentication");
    }

    /// Field names differ on every forge and a wrong one silently creates a
    /// *public* repository on some of them, so each body is pinned.
    #[test]
    fn each_forge_gets_the_body_it_actually_documents() {
        let access =
            ApiAccess { base_url: "https://host.example".into(), token: Secret::from_str("t") };

        let gitea = create_request(Forge::Gitea, &access, &NewRepo::private("thing")).expect("ok");
        assert_eq!(gitea.url, "https://host.example/api/v1/user/repos");
        assert_eq!(gitea.body["private"], serde_json::json!(true));
        assert_eq!(gitea.body["auto_init"], serde_json::json!(false));
        assert_eq!(gitea.auth, AuthStyle::Token);

        let gitlab = create_request(Forge::GitLab, &access, &NewRepo::private("thing")).expect("ok");
        assert_eq!(gitlab.url, "https://host.example/api/v4/projects");
        // GitLab has no `private` field; visibility is a string, and getting
        // this wrong makes the project public.
        assert_eq!(gitlab.body["visibility"], serde_json::json!("private"));
        assert_eq!(gitlab.auth, AuthStyle::PrivateToken);

        let public = NewRepo { private: false, ..NewRepo::private("thing") };
        let gitlab_public = create_request(Forge::GitLab, &access, &public).expect("ok");
        assert_eq!(gitlab_public.body["visibility"], serde_json::json!("public"));

        let github = create_request(Forge::GitHub, &access, &NewRepo::private("thing")).expect("ok");
        assert_eq!(github.auth, AuthStyle::Bearer);
        assert_eq!(github.body["private"], serde_json::json!(true));

        // An organisation is a different endpoint on both.
        let org = NewRepo { owner: Some("acme".into()), ..NewRepo::private("thing") };
        let gitea_org = create_request(Forge::Gitea, &access, &org).expect("ok");
        assert_eq!(gitea_org.url, "https://host.example/api/v1/orgs/acme/repos");
        // GitLab wants a numeric id, so it refuses rather than creating the
        // project in the wrong place.
        assert!(create_request(Forge::GitLab, &access, &org).is_err());
    }

    #[test]
    fn hosts_that_cannot_be_created_on_say_so_rather_than_failing_obscurely() {
        let access =
            ApiAccess { base_url: "https://host.example".into(), token: Secret::from_str("t") };
        for forge in [Forge::AzureDevOps, Forge::Bitbucket, Forge::Unknown] {
            let err = create_request(forge, &access, &NewRepo::private("thing"))
                .expect_err("refused");
            let text = err.to_string();
            assert!(text.contains(forge.label()), "{text}");
            assert!(!forge.can_create());
        }
        assert!(Forge::GitHub.can_create());
        assert!(Forge::Gitea.can_create());
        assert!(Forge::GitLab.can_create());
    }

    /// A token in a query string is a token in every log between here and the
    /// server. All three styles must be headers.
    #[test]
    fn a_token_only_ever_travels_in_a_header() {
        let token = Secret::from_str("s3cret");
        for style in [AuthStyle::Bearer, AuthStyle::Token, AuthStyle::PrivateToken] {
            let (name, value) = style.header(&token);
            assert!(!name.is_empty());
            assert!(value.contains("s3cret"), "the header carries the token");
        }
        let access =
            ApiAccess { base_url: "https://host.example".into(), token: Secret::from_str("s3cret") };
        for forge in [Forge::GitHub, Forge::Gitea, Forge::GitLab] {
            let request = create_request(forge, &access, &NewRepo::private("thing")).expect("ok");
            assert!(!request.url.contains("s3cret"), "{forge:?} put the token in the URL");
            assert!(
                !request.body.to_string().contains("s3cret"),
                "{forge:?} put the token in the body"
            );
        }
    }
}
