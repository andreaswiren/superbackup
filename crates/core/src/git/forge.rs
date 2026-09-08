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
        matches!(self, Self::GitHub | Self::GitLab | Self::Gitea | Self::AzureDevOps)
    }

    /// What the host calls the credential, and the scope it needs — the two
    /// facts somebody has to know before they can make one, and the two the
    /// error message cannot supply after the fact.
    pub fn credential_hint(self) -> &'static str {
        match self {
            Self::GitHub => {
                "The GitHub CLI (`gh auth login`) is the best option: superbackup then holds no                  token at all. Otherwise a fine-grained personal access token with Administration                  write on the repositories it may create."
            }
            Self::Gitea => {
                "A personal access token from Settings › Applications, with the                  `write:repository` scope. Gitea fixes a token's scopes when it is created, so                  this needs a new token rather than an edit to an existing one."
            }
            Self::GitLab => {
                "A personal access token with the `api` scope, from Preferences › Access tokens."
            }
            Self::AzureDevOps => {
                "A personal access token with `Code (read & write)`, from User settings ›                  Personal access tokens. Azure sends it as an HTTP Basic password with an empty                  username, which superbackup does for you."
            }
            Self::Bitbucket | Self::Unknown | Self::None => {
                "Superbackup cannot create a repository here."
            }
        }
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
            return Err(Error::Validation("a repository name is at most 100 characters".into()));
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
            return Err(Error::Validation(
                "a repository name can hold letters, digits, dashes, underscores and dots only"
                    .into(),
            ));
        }
        if name.starts_with('.') || name == "." || name == ".." {
            return Err(Error::Validation("a repository name cannot start with a dot".into()));
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
        Forge::AzureDevOps => {
            // Azure has no account-level endpoint at all: a repository there
            // lives inside a project, so it needs an organisation and a
            // project rather than an owner. `create_with_token` takes those.
            return Err(Error::Validation(
                "an Azure DevOps repository is created inside a project, so it needs the \
                 organisation and the project rather than an owner. See `create_with_token`."
                    .into(),
            ));
        }
        Forge::Bitbucket | Forge::Unknown | Forge::None => {
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

/// Create a repository with the GitHub CLI.
///
/// Preferred over the REST API wherever `gh` is signed in, and the reason is
/// not convenience. `gh` already holds a credential the user set up, scoped
/// however they chose, refreshed by `gh` and revocable by them in one place.
/// Borrowing it means superbackup never asks for, stores, or is capable of
/// leaking a GitHub token — which is a better security property than any
/// amount of care taken with one we held.
pub async fn create_with_gh(repo: &NewRepo) -> crate::error::Result<CreatedRepo> {
    use crate::error::Error;

    let args = gh_create_args(repo)?;
    let program = locate_gh().ok_or_else(|| {
        Error::Validation(
            "the GitHub CLI is not installed, or is not on this account's PATH. Install it from \
             https://cli.github.com and sign in with `gh auth login`."
                .into(),
        )
    })?;

    let mut command = tokio::process::Command::new(program);
    command.args(&args);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    // `gh` will happily open a browser and wait for a login. In a tray
    // application that is an application that has stopped responding.
    command.env("GH_PROMPT_DISABLED", "1");
    command.env("GH_NO_UPDATE_NOTIFIER", "1");
    crate::kopia::harden_child(&mut command);

    let output = tokio::time::timeout(std::time::Duration::from_secs(60), command.output())
        .await
        .map_err(|_| Error::Config("the GitHub CLI did not answer within a minute".into()))?
        .map_err(|e| Error::io("running the GitHub CLI", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        let reason = stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("the GitHub CLI failed without saying why");
        return Err(Error::Config(reason.to_string()));
    }

    // `gh repo create` prints the URL of what it made.
    let web_url = stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .find(|l| l.starts_with("https://"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://github.com/{}", repo.full_name()));
    let full_name = web_url
        .strip_prefix("https://github.com/")
        .map(str::to_string)
        .unwrap_or_else(|| repo.full_name());

    Ok(CreatedRepo {
        clone_url: format!("git@github.com:{full_name}.git"),
        web_url,
        full_name,
        private: repo.private,
        via: "the GitHub CLI".into(),
    })
}

fn locate_gh() -> Option<std::path::PathBuf> {
    let exe = if cfg!(windows) { "gh.exe" } else { "gh" };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(exe)).find(|c| c.is_file())
}

/// Azure DevOps needs a project, and its API path is not `owner/repo`.
///
/// `POST https://dev.azure.com/{organisation}/{project}/_apis/git/repositories`
/// with `?api-version=7.1`. The organisation and the project are both
/// required and are different things — which is why Azure gets its own type
/// rather than being squeezed into an `owner` field that means something else
/// everywhere else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AzureTarget {
    /// `dev.azure.com/<organisation>`.
    pub organisation: String,
    /// The project the repository goes in. Azure has no repositories outside
    /// projects.
    pub project: String,
}

/// How each host wants to be authenticated, and what it is called there.
///
/// Written down because every one of these is a different word for the same
/// thing, and getting it wrong produces a 401 that says nothing useful.
///
/// | Host | Credential | Header |
/// |---|---|---|
/// | GitHub | fine-grained PAT, or the `gh` CLI | `Authorization: Bearer` |
/// | Gitea / Forgejo | PAT with `write:repository` | `Authorization: token` |
/// | GitLab | PAT with the `api` scope | `PRIVATE-TOKEN` |
/// | Azure DevOps | PAT with `Code (read & write)` | `Authorization: Basic` |
///
/// Azure is the odd one: it uses HTTP Basic with an **empty username** and the
/// PAT as the password, which is why it cannot share the others' header shape.
/// OAuth and Entra app registrations exist for it and for GitHub, and are the
/// better answer for a service; for one person creating their own repositories
/// from their own machine, a PAT is what the documentation steers you to and
/// is revocable in one click, so it is what this supports.
pub async fn create_with_token(
    forge: Forge,
    access: &ApiAccess,
    repo: &NewRepo,
    azure: Option<&AzureTarget>,
) -> crate::error::Result<CreatedRepo> {
    use crate::error::Error;

    repo.validate()?;
    access.validate()?;

    let (url, body, header, value) = match forge {
        Forge::AzureDevOps => {
            let target = azure.ok_or_else(|| {
                Error::Validation(
                    "Azure DevOps needs the organisation and the project: a repository there \
                     lives inside a project, and there is no account-level place to put one."
                        .into(),
                )
            })?;
            if target.organisation.trim().is_empty() || target.project.trim().is_empty() {
                return Err(Error::Validation(
                    "both the organisation and the project are needed".into(),
                ));
            }
            // The PAT goes in as the *password* of an empty-username Basic
            // credential. Azure documents it exactly that way.
            let token = access
                .token
                .expose_str()
                .ok_or_else(|| Error::Internal("the token is not text".into()))?;
            let encoded = {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(format!(":{token}"))
            };
            (
                format!(
                    "{}/{}/{}/_apis/git/repositories?api-version=7.1",
                    access.base(),
                    target.organisation.trim(),
                    target.project.trim()
                ),
                serde_json::json!({ "name": repo.name.trim() }),
                "Authorization",
                format!("Basic {encoded}"),
            )
        }
        _ => {
            let request = create_request(forge, access, repo)?;
            let (header, value) = request.auth.header(&access.token);
            (request.url, request.body, header, value)
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("superbackup/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Config(format!("could not start an HTTPS client: {e}")))?;

    let response = client
        .post(&url)
        .header(header, value)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            // The URL is safe to show — the token is in a header, deliberately,
            // and never in the address.
            Error::Config(format!("{url} could not be reached: {e}"))
        })?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(Error::Config(explain_failure(forge, status.as_u16(), &text)));
    }

    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    Ok(read_created(forge, repo, &json, access.base()))
}

/// Turn a status code into something a person can act on.
///
/// A bare "403" from four different hosts means four different things, and
/// every one of them has a specific fix.
fn explain_failure(forge: Forge, status: u16, body: &str) -> String {
    let scope = match forge {
        Forge::Gitea => "write:repository",
        Forge::GitLab => "api",
        Forge::AzureDevOps => "Code (read & write)",
        _ => "repository write",
    };
    let detail = body.chars().take(300).collect::<String>();
    match status {
        401 => format!(
            "{} rejected the token. Check that it has not expired and that it was pasted whole.",
            forge.label()
        ),
        403 => format!(
            "{} accepted the token but refused the request. It most likely lacks the `{scope}` \
             scope — a token is created with the scopes it will ever have, so this needs a new \
             one rather than an edit.",
            forge.label()
        ),
        404 => format!(
            "{} has no such place to create a repository in. For Azure DevOps that usually means \
             the project name; elsewhere, the organisation.",
            forge.label()
        ),
        409 | 422 => format!("{} already has a repository with that name.", forge.label()),
        other => format!("{} answered {other}. {detail}", forge.label()),
    }
}

/// Read the clone address out of whatever shape the host answered with.
fn read_created(forge: Forge, repo: &NewRepo, json: &serde_json::Value, base: &str) -> CreatedRepo {
    let string = |key: &str| json.get(key).and_then(|v| v.as_str()).map(str::to_string);

    // Every one of these calls the same two things something different.
    let (clone_url, web_url, full_name) = match forge {
        Forge::AzureDevOps => (
            string("sshUrl").or_else(|| string("remoteUrl")).unwrap_or_default(),
            string("webUrl").unwrap_or_default(),
            string("name").unwrap_or_else(|| repo.name.trim().to_string()),
        ),
        Forge::GitLab => (
            string("ssh_url_to_repo").unwrap_or_default(),
            string("web_url").unwrap_or_default(),
            string("path_with_namespace").unwrap_or_else(|| repo.full_name()),
        ),
        // Gitea, Forgejo and the GitHub API agree on these names.
        _ => (
            string("ssh_url").or_else(|| string("clone_url")).unwrap_or_default(),
            string("html_url").unwrap_or_default(),
            string("full_name").unwrap_or_else(|| repo.full_name()),
        ),
    };

    CreatedRepo {
        clone_url,
        web_url: if web_url.is_empty() { format!("{base}/{full_name}") } else { web_url },
        full_name,
        private: repo.private,
        via: format!("the {} API", forge.label()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Azure DevOps is the one that does not fit the others' shape, and every
    /// part of that is a thing you find out by getting a 401 or a 404.
    #[test]
    fn azure_devops_is_addressed_as_a_project_and_authenticated_as_basic() {
        // It has no account-level endpoint, so the shared builder refuses it
        // rather than inventing a URL that would 404.
        let access =
            ApiAccess { base_url: "https://dev.azure.com".into(), token: Secret::from_str("t") };
        let err = create_request(Forge::AzureDevOps, &access, &NewRepo::private("thing"))
            .expect_err("has no account-level endpoint");
        assert!(err.to_string().contains("project"), "{err}");

        // The credential hint names the scope and the mechanism, because the
        // failure afterwards cannot.
        let hint = Forge::AzureDevOps.credential_hint();
        assert!(hint.contains("Code (read & write)"), "{hint}");
        assert!(hint.contains("Basic"), "{hint}");
    }

    /// Each host calls the same two things something different, and reading
    /// the wrong field yields an empty remote that silently points nowhere.
    #[test]
    fn the_clone_address_is_read_from_the_field_each_host_actually_uses() {
        let repo = NewRepo::private("thing");

        // Gitea and Forgejo, and GitHub's REST shape.
        let gitea = serde_json::json!({
            "full_name": "andreas/thing",
            "ssh_url": "git@gitea.example.com:andreas/thing.git",
            "clone_url": "https://gitea.example.com/andreas/thing.git",
            "html_url": "https://gitea.example.com/andreas/thing",
        });
        let made = read_created(Forge::Gitea, &repo, &gitea, "https://gitea.example.com");
        assert_eq!(made.clone_url, "git@gitea.example.com:andreas/thing.git", "ssh over https");
        assert_eq!(made.full_name, "andreas/thing");
        assert_eq!(made.web_url, "https://gitea.example.com/andreas/thing");

        // GitLab renames all three.
        let gitlab = serde_json::json!({
            "path_with_namespace": "group/thing",
            "ssh_url_to_repo": "git@gitlab.com:group/thing.git",
            "web_url": "https://gitlab.com/group/thing",
        });
        let made = read_created(Forge::GitLab, &repo, &gitlab, "https://gitlab.com");
        assert_eq!(made.clone_url, "git@gitlab.com:group/thing.git");
        assert_eq!(made.full_name, "group/thing");

        // Azure renames them again, and its `name` is the repository alone.
        let azure = serde_json::json!({
            "name": "thing",
            "sshUrl": "git@ssh.dev.azure.com:v3/org/project/thing",
            "remoteUrl": "https://org@dev.azure.com/org/project/_git/thing",
            "webUrl": "https://dev.azure.com/org/project/_git/thing",
        });
        let made = read_created(Forge::AzureDevOps, &repo, &azure, "https://dev.azure.com");
        assert_eq!(made.clone_url, "git@ssh.dev.azure.com:v3/org/project/thing");
        assert_eq!(made.web_url, "https://dev.azure.com/org/project/_git/thing");

        // A host that answers with nothing useful still yields a web address
        // rather than an empty string the interface would show as a blank.
        let empty =
            read_created(Forge::Gitea, &repo, &serde_json::Value::Null, "https://x.example");
        assert_eq!(empty.web_url, "https://x.example/thing");
        assert!(empty.clone_url.is_empty(), "and the clone address is honestly absent");
    }

    /// A bare status code from four hosts means four things, each with a
    /// different fix. The scope named has to be the one that host uses.
    #[test]
    fn a_refusal_names_the_scope_that_host_actually_requires() {
        assert!(explain_failure(Forge::Gitea, 403, "").contains("write:repository"));
        assert!(explain_failure(Forge::GitLab, 403, "").contains("api"));
        assert!(explain_failure(Forge::AzureDevOps, 403, "").contains("Code (read & write)"));

        // 401 is a different problem from 403 and gets different advice.
        let unauthorised = explain_failure(Forge::Gitea, 401, "");
        assert!(unauthorised.contains("expired"), "{unauthorised}");
        assert!(!unauthorised.contains("scope"), "{unauthorised}");

        assert!(explain_failure(Forge::Gitea, 409, "").contains("already has a repository"));
        // An unrecognised code still carries the host's own words, trimmed.
        let odd = explain_failure(Forge::Gitea, 500, "internal error");
        assert!(odd.contains("500") && odd.contains("internal error"), "{odd}");
    }

    /// Every host that can be created on says what credential it needs and
    /// where to get it — the two facts a 403 afterwards cannot supply.
    #[test]
    fn every_supported_host_says_what_credential_it_needs() {
        for forge in [Forge::GitHub, Forge::Gitea, Forge::GitLab, Forge::AzureDevOps] {
            let hint = forge.credential_hint();
            assert!(hint.len() > 40, "{forge:?}: {hint}");
            assert!(
                hint.contains("token") || hint.contains("CLI"),
                "{forge:?} must name the credential: {hint}"
            );
        }
    }

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
        for bad in
            ["thing && rm -rf /", "../escape", "with space", "with/slash", ".hidden", "", "   "]
        {
            let repo = NewRepo::private(bad);
            assert!(repo.validate().is_err(), "{bad:?} must be refused");
            assert!(gh_create_args(&repo).is_err(), "{bad:?} must not reach argv");
        }
        assert!(NewRepo::private("super-backup_2.0").validate().is_ok());
    }

    #[test]
    fn a_token_is_never_sent_over_plain_http_to_a_real_host() {
        let token = Secret::from_str("t0ken");
        let remote =
            ApiAccess { base_url: "http://gitea.example.com".into(), token: token.clone() };
        let err = remote.validate().expect_err("http is refused");
        assert!(err.to_string().contains("https"), "{err}");

        // Local development is not a network hop.
        let local = ApiAccess { base_url: "http://localhost:3000".into(), token: token.clone() };
        assert!(local.validate().is_ok());

        let secure = ApiAccess { base_url: "https://gitea.example.com/".into(), token };
        assert!(secure.validate().is_ok());
        assert_eq!(secure.base(), "https://gitea.example.com", "the trailing slash is dropped");

        let empty = ApiAccess {
            base_url: "https://gitea.example.com".into(),
            token: Secret::from_str("  "),
        };
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

        let gitlab =
            create_request(Forge::GitLab, &access, &NewRepo::private("thing")).expect("ok");
        assert_eq!(gitlab.url, "https://host.example/api/v4/projects");
        // GitLab has no `private` field; visibility is a string, and getting
        // this wrong makes the project public.
        assert_eq!(gitlab.body["visibility"], serde_json::json!("private"));
        assert_eq!(gitlab.auth, AuthStyle::PrivateToken);

        let public = NewRepo { private: false, ..NewRepo::private("thing") };
        let gitlab_public = create_request(Forge::GitLab, &access, &public).expect("ok");
        assert_eq!(gitlab_public.body["visibility"], serde_json::json!("public"));

        let github =
            create_request(Forge::GitHub, &access, &NewRepo::private("thing")).expect("ok");
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
        for forge in [Forge::Bitbucket, Forge::Unknown] {
            let err =
                create_request(forge, &access, &NewRepo::private("thing")).expect_err("refused");
            let text = err.to_string();
            assert!(text.contains(forge.label()), "{text}");
            assert!(!forge.can_create());
        }
        assert!(Forge::GitHub.can_create());
        assert!(Forge::Gitea.can_create());
        assert!(Forge::GitLab.can_create());
        assert!(Forge::AzureDevOps.can_create(), "through create_with_token");
        assert!(!Forge::Bitbucket.can_create());
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
        let access = ApiAccess {
            base_url: "https://host.example".into(),
            token: Secret::from_str("s3cret"),
        };
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
