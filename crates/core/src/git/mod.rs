//! What is in these folders, and is any of it only here?
//!
//! # Why a backup tool cares about git
//!
//! superbackup exists because a developer machine holds source trees that
//! OneDrive cannot cope with. But a source tree that is committed *and pushed*
//! is already safe somewhere else — the backup of it is a convenience, not the
//! only copy. A tree with uncommitted work, or with commits that were never
//! pushed, is the opposite: the backup is the only thing standing between the
//! user and losing it.
//!
//! So the question this module answers is not "which folders use git". It is
//! **"which of these folders contain work that exists nowhere but this
//! disk"** — and that is a question with a different answer for every folder,
//! which nobody is going to check by hand across forty repositories.
//!
//! # What is trusted, and what is asked
//!
//! Everything local is read straight from git and is exact. The remote side is
//! deliberately *not* read from `.git`: `ahead 0, behind 0` there means "in
//! sync as of the last fetch", and a tree last fetched in March will report
//! itself up to date all summer. [`RemoteCheck`] asks the remote instead, with
//! `ls-remote`, which is a read-only network call that changes nothing — no
//! fetch, no ref update, nothing written into a repository the user did not
//! ask us to touch.

pub mod cli;
pub mod forge;
pub mod parse;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use cli::{Git, LOCAL_TIMEOUT, REMOTE_TIMEOUT};

pub use forge::Forge;
pub use parse::{LastCommit, Remote, RemoteLocation, Status};

/// Folders never descended into while looking for repositories.
///
/// Not an optimisation — a correctness bound. This program was written because
/// a Next.js tree holds millions of cache files; walking one to find a `.git`
/// that is not in it would take longer than the backup. None of these ever
/// contains a repository the user is working in, and a repository vendored
/// inside `node_modules` is a dependency, not their work.
pub const SKIPPED: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    "bower_components",
    "Pods",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".gradle",
    ".terraform",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".tox",
    ".cache",
    ".idea",
    ".vs",
    "obj",
    "bin",
    "DerivedData",
    "$RECYCLE.BIN",
    "System Volume Information",
];

/// How far to look, and how hard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanOptions {
    /// How many levels below each root to search. Four covers
    /// `workspace/client/project/repo` without walking a whole drive.
    pub max_depth: usize,
    /// Ask each remote where it actually is. Off by default because it is the
    /// only part that touches the network.
    pub check_remotes: bool,
    /// Repositories examined at once. Each one is three short git invocations,
    /// so this is process count, not thread count.
    pub concurrency: usize,
    /// A stop, so a root accidentally set to `C:\` cannot walk forever.
    pub max_folders: u64,
    pub max_repos: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_depth: 4,
            check_remotes: false,
            concurrency: 8,
            max_folders: 40_000,
            max_repos: 500,
        }
    }
}

/// Where a repository's work lives, in one word.
///
/// Ordered by how much of it exists only on this disk, because that is what
/// the list is sorted and coloured by. `Clean` is last for the same reason: a
/// repository that is committed and pushed is the one the user does not need
/// to look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoState {
    /// git could not read it — a broken index, a permissions problem.
    Unreadable,
    /// git refuses to read it because the folder belongs to another account.
    /// Common on Windows, where anything created from an elevated shell is
    /// owned by `Administrators` rather than by the user.
    NotTrusted,
    /// Commits are being made on no branch, and the next checkout orphans them.
    Detached,
    /// Local and remote have both moved. Needs a person.
    Diverged,
    /// Edits that are not committed anywhere.
    Uncommitted,
    /// Committed, but the commits are on this disk only.
    Unpushed,
    /// A repository with no remote at all: *everything* in it is only here.
    NoRemote,
    /// A branch that was never pushed, so it has nothing to be measured
    /// against even though the repository has a remote.
    NoUpstream,
    /// The remote has commits this tree does not.
    PullRecommended,
    /// Committed, pushed, and level with the remote.
    Clean,
    /// Marked by the user as somebody else's code: a clone they read and never
    /// commit to. Sorted last and never counted as at risk.
    External,
}

impl RepoState {
    /// Is the work here at risk if this disk is lost?
    ///
    /// The whole point of the screen: these are the repositories where the
    /// backup is doing real work rather than duplicating a forge.
    pub fn only_on_this_disk(self) -> bool {
        matches!(
            self,
            Self::Detached
                | Self::Diverged
                | Self::Uncommitted
                | Self::Unpushed
                | Self::NoRemote
                | Self::NoUpstream
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unreadable => "Unreadable",
            Self::NotTrusted => "Not trusted by git",
            Self::Detached => "Detached HEAD",
            Self::Diverged => "Diverged",
            Self::Uncommitted => "Uncommitted changes",
            Self::Unpushed => "Not pushed",
            Self::NoRemote => "No remote",
            Self::NoUpstream => "Never pushed",
            Self::PullRecommended => "Pull recommended",
            Self::Clean => "Up to date",
            Self::External => "External",
        }
    }

    /// One sentence saying what it means for this user's data.
    pub fn explanation(self) -> &'static str {
        match self {
            Self::Unreadable => "git could not read this folder, so nothing here is known.",
            Self::NotTrusted => {
                "This folder belongs to a different account — usually because it was created \
                 from an administrator shell — so git will not read it. Until it is marked as \
                 trusted, superbackup cannot tell whether there is unsaved work here."
            }
            Self::Detached => {
                "Commits here are on no branch. A checkout will leave them unreachable."
            }
            Self::Diverged => {
                "This branch and its remote have both moved on. Merging or rebasing is a \
                 decision, so superbackup will not make it for you."
            }
            Self::Uncommitted => {
                "There are changes that are not committed. They exist on this disk and in this \
                 backup, and nowhere else."
            }
            Self::Unpushed => {
                "Commits here have never reached the remote. This backup is the only other copy."
            }
            Self::NoRemote => {
                "This repository has no remote, so its whole history exists only on this disk."
            }
            Self::NoUpstream => {
                "This branch has never been pushed, so the remote has no copy of it."
            }
            Self::PullRecommended => {
                "The remote has commits this folder does not. Nothing is at risk; you are just \
                 behind."
            }
            Self::Clean => "Committed and pushed. The remote has everything this folder has.",
            Self::External => {
                "Marked as somebody else's code — a clone you read rather than work in. It is                  still backed up; it is just not counted as work you could lose."
            }
        }
    }
}

/// How the local branch stands against the remote *right now*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// The remote is at the same commit as this branch.
    Same,
    /// The remote's commit is an ancestor of ours: we have more.
    LocalAhead,
    /// Ours is an ancestor of the remote's: they have more.
    LocalBehind,
    /// Neither contains the other.
    Diverged,
    /// The remote answered, but its commit is not in this clone, so the
    /// relationship cannot be worked out without fetching.
    NeedsFetch,
}

/// The answer from asking the remote, rather than from reading `.git`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCheck {
    pub remote: String,
    /// The ref asked about, e.g. `refs/heads/main`.
    pub reference: String,
    pub remote_head: Option<String>,
    pub relation: Option<Relation>,
    /// Why the remote could not be reached — an unreachable host, a
    /// credential that is not stored, a repository that has been deleted.
    pub error: Option<String>,
    pub checked_at: DateTime<Utc>,
}

/// One repository found under a job's sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRepo {
    pub path: PathBuf,
    /// The folder's own name, which is what the user calls this project.
    pub name: String,
    /// Which scanned root it was found under, so a list covering several
    /// sources can say where each entry came from.
    pub root: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicted: u32,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub last_commit_summary: Option<String>,
    pub last_commit_author: Option<String>,
    pub remotes: Vec<RepoRemote>,
    pub remote_check: Option<RemoteCheck>,
    /// Every branch, not only the one checked out. A branch nobody has looked
    /// at in months is exactly where unpushed work hides.
    #[serde(default)]
    pub branches: Vec<parse::Branch>,
    /// Linked working trees. A repository with three of them has three sets of
    /// uncommitted changes, and a scan that reported only the main one would
    /// be quietly wrong about the thing this page is for.
    #[serde(default)]
    pub worktrees: Vec<parse::Worktree>,
    /// Marked by the user as somebody else's: a plain clone they read and
    /// never commit to. Excluded from the at-risk count, because "you have
    /// not pushed your changes" is not true of a repository you have no
    /// changes in and no intention of making any.
    #[serde(default)]
    pub external: bool,
    pub error: Option<String>,
    /// git refused this folder over its ownership rather than failing to read
    /// it. Separated from `error` because it has an exact, one-line fix and
    /// the generic failure does not.
    #[serde(default)]
    pub untrusted: bool,
}

/// A configured remote, with what can be told about where it points.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRemote {
    pub name: String,
    pub url: String,
    pub host: Option<String>,
    pub forge: Forge,
    /// The page a person would open. Absent for a remote that is a path.
    #[serde(default)]
    pub web_url: Option<String>,
    /// How this remote authenticates — the mechanism, never the secret.
    #[serde(default = "auth_none")]
    pub auth: parse::AuthMethod,
}

fn auth_none() -> parse::AuthMethod {
    parse::AuthMethod::None
}

impl GitRepo {
    /// The remote a person means when they say "the remote": `origin` if there
    /// is one, otherwise whichever is configured.
    pub fn primary_remote(&self) -> Option<&RepoRemote> {
        self.remotes
            .iter()
            .find(|r| r.name == "origin")
            .or_else(|| self.remotes.first())
    }

    /// Boil everything down to the one word the list shows.
    ///
    /// Ordered by risk, not by how git would describe it: uncommitted work
    /// outranks being behind the remote, because one of those is a copy that
    /// does not exist anywhere else and the other is a merge waiting to happen.
    pub fn state(&self) -> RepoState {
        // A repository the user has marked as somebody else's. Whatever its
        // git state, none of it is work they will lose — so it reports as
        // external rather than as a warning they would learn to ignore.
        if self.external {
            return RepoState::External;
        }
        if self.untrusted {
            return RepoState::NotTrusted;
        }
        if self.error.is_some() {
            return RepoState::Unreadable;
        }
        if self.branch.is_none() {
            return RepoState::Detached;
        }

        // The live answer beats the cached one wherever we have it: `.git`'s
        // ahead/behind is only as fresh as the last fetch.
        let live = self.remote_check.as_ref().and_then(|c| c.relation);
        let diverged = live == Some(Relation::Diverged) || (self.ahead > 0 && self.behind > 0);
        if diverged {
            return RepoState::Diverged;
        }
        if self.staged + self.unstaged + self.untracked + self.conflicted > 0 {
            return RepoState::Uncommitted;
        }
        if self.remotes.is_empty() {
            return RepoState::NoRemote;
        }
        if self.upstream.is_none() {
            return RepoState::NoUpstream;
        }
        match live {
            Some(Relation::LocalAhead) => RepoState::Unpushed,
            Some(Relation::LocalBehind) | Some(Relation::NeedsFetch) => {
                RepoState::PullRecommended
            }
            Some(Relation::Same) => RepoState::Clean,
            Some(Relation::Diverged) => RepoState::Diverged,
            // No live check: fall back to what the last fetch recorded.
            None if self.ahead > 0 => RepoState::Unpushed,
            None if self.behind > 0 => RepoState::PullRecommended,
            None => RepoState::Clean,
        }
    }
}

/// Everything found under one set of roots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub roots: Vec<PathBuf>,
    pub repos: Vec<GitRepo>,
    pub scanned_at: DateTime<Utc>,
    pub folders_scanned: u64,
    /// A limit was hit, so this list is not everything. Said out loud rather
    /// than left for the user to notice, because a silently short list of
    /// "repositories at risk" is worse than no list.
    pub truncated: bool,
    pub git_available: bool,
    pub notes: Vec<String>,
}

impl Inventory {
    /// Repositories holding work that exists nowhere else, worst first.
    pub fn at_risk(&self) -> Vec<&GitRepo> {
        let mut risky: Vec<&GitRepo> =
            self.repos.iter().filter(|r| r.state().only_on_this_disk()).collect();
        risky.sort_by_key(|r| (r.state(), r.name.to_lowercase()));
        risky
    }
}

/// Find every git repository under `roots` and report on each.
pub async fn inventory(roots: &[PathBuf], options: &ScanOptions) -> Result<Inventory> {
    let scanned_at = Utc::now();
    if !Git::available() {
        return Ok(Inventory {
            roots: roots.to_vec(),
            repos: Vec::new(),
            scanned_at,
            folders_scanned: 0,
            truncated: false,
            git_available: false,
            notes: vec![
                "git is not installed, or is not on this account's PATH, so no repository \
                 information is available."
                    .into(),
            ],
        });
    }
    let git = Git::locate()?;

    let mut notes = Vec::new();
    let mut found: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut folders_scanned = 0u64;
    let mut truncated = false;

    for root in roots {
        if !root.is_dir() {
            notes.push(format!("{} is not a folder that can be read.", root.display()));
            continue;
        }
        let discovered = discover(root, options, &mut folders_scanned);
        if discovered.truncated {
            truncated = true;
        }
        for path in discovered.repos {
            if found.len() >= options.max_repos {
                truncated = true;
                break;
            }
            found.push((path, root.clone()));
        }
    }

    // Bounded concurrency. Each repository is a handful of short-lived
    // processes, and letting four hundred of them start at once would put the
    // machine on its knees for no gain — the work is process startup, not CPU.
    let mut repos = Vec::with_capacity(found.len());
    for chunk in found.chunks(options.concurrency.max(1)) {
        let mut tasks = Vec::with_capacity(chunk.len());
        for (path, root) in chunk {
            let git = git.clone();
            let path = path.clone();
            let root = root.clone();
            let check_remotes = options.check_remotes;
            tasks.push(async move { examine(&git, &path, &root, check_remotes).await });
        }
        repos.extend(futures_join_all(tasks).await);
    }

    repos.sort_by_key(|r| (r.state(), r.name.to_lowercase()));
    if truncated {
        notes.push(format!(
            "The scan stopped at {} folders or {} repositories, so this list may be incomplete. \
             Narrow the sources, or lower the depth, to see all of them.",
            options.max_folders, options.max_repos
        ));
    }

    Ok(Inventory {
        roots: roots.to_vec(),
        repos,
        scanned_at,
        folders_scanned,
        truncated,
        git_available: true,
        notes,
    })
}

/// `join_all` without pulling in `futures` for one call.
async fn futures_join_all<F>(tasks: Vec<F>) -> Vec<GitRepo>
where
    F: std::future::Future<Output = GitRepo> + Send + 'static,
{
    let mut set = tokio::task::JoinSet::new();
    // The futures borrow nothing, so they can be spawned; ordering is restored
    // by the caller's sort, which is by state and name rather than by
    // discovery order anyway.
    let mut out = Vec::with_capacity(tasks.len());
    for task in tasks {
        set.spawn(task);
    }
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(repo) => out.push(repo),
            // A panic in one repository's examination must not lose the other
            // thirty-nine.
            Err(_) => continue,
        }
    }
    out
}

struct Discovered {
    repos: Vec<PathBuf>,
    truncated: bool,
}

/// Walk for `.git`, breadth-first, stopping at each repository found.
///
/// Stopping matters: a repository's own `node_modules` is skipped by name, but
/// its submodules and vendored checkouts are not, and listing forty
/// dependencies of one project as forty projects would bury the four the user
/// actually works in.
fn discover(root: &Path, options: &ScanOptions, folders_scanned: &mut u64) -> Discovered {
    let mut repos = Vec::new();
    let mut truncated = false;
    let mut queue = VecDeque::from([(root.to_path_buf(), 0usize)]);

    while let Some((dir, depth)) = queue.pop_front() {
        if *folders_scanned >= options.max_folders {
            truncated = true;
            break;
        }
        *folders_scanned += 1;

        if dir.join(".git").exists() {
            repos.push(dir);
            continue;
        }
        if depth >= options.max_depth {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else { continue };
            // Not `is_dir()`: that follows the link, and a symlink pointing at
            // an ancestor turns the walk into an infinite one.
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if SKIPPED.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                continue;
            }
            queue.push_back((entry.path(), depth + 1));
        }
    }

    Discovered { repos, truncated }
}

/// Read one repository.
async fn examine(git: &Git, path: &Path, root: &Path, check_remotes: bool) -> GitRepo {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let mut repo = GitRepo {
        path: path.to_path_buf(),
        name,
        root: root.to_path_buf(),
        branch: None,
        head: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        staged: 0,
        unstaged: 0,
        untracked: 0,
        conflicted: 0,
        last_commit_at: None,
        last_commit_summary: None,
        last_commit_author: None,
        remotes: Vec::new(),
        remote_check: None,
        branches: Vec::new(),
        worktrees: Vec::new(),
        external: false,
        error: None,
        untrusted: false,
    };

    let status = git
        .run(
            path,
            &["status", "--porcelain=v2", "--branch", "--untracked-files=normal"],
            LOCAL_TIMEOUT,
        )
        .await;
    match status {
        Ok(out) if out.ok() => {
            let s = parse::parse_status(&out.stdout);
            repo.branch = s.branch;
            repo.head = s.head;
            repo.upstream = s.upstream;
            repo.ahead = s.ahead;
            repo.behind = s.behind;
            repo.staged = s.staged;
            repo.unstaged = s.unstaged;
            repo.untracked = s.untracked;
            repo.conflicted = s.conflicted;
        }
        Ok(out) => {
            repo.error = Some(out.failure());
            repo.untrusted = is_dubious_ownership(&out.stderr);
            return repo;
        }
        Err(e) => {
            repo.error = Some(e.to_string());
            return repo;
        }
    }

    if let Ok(out) = git.run(path, &["log", "-1", parse::LOG_FORMAT], LOCAL_TIMEOUT).await {
        if out.ok() {
            if let Some(commit) = parse::parse_last_commit(&out.stdout) {
                repo.last_commit_at = Some(commit.at);
                repo.last_commit_summary = Some(commit.summary);
                repo.last_commit_author = Some(commit.author);
            }
        }
        // A repository with no commits fails this and has no last commit,
        // which the `None`s already say.
    }

    if let Ok(out) = git
        .run(path, &["config", "--get-regexp", r"^remote\..*\.url"], LOCAL_TIMEOUT)
        .await
    {
        if out.ok() {
            repo.remotes = parse::parse_remotes(&out.stdout)
                .into_iter()
                .map(|r| {
                    let location = parse::parse_remote_url(&r.url);
                    RepoRemote {
                        name: r.name,
                        forge: location
                            .as_ref()
                            .map(|l| Forge::from_host(&l.host))
                            .unwrap_or(Forge::None),
                        web_url: location.as_ref().and_then(parse::web_url),
                        host: location.map(|l| l.host),
                        auth: parse::AuthMethod::None,
                        url: r.url,
                    }
                })
                .collect();
        }
    }

    // Every branch, not only the checked-out one. Unpushed work hides on a
    // branch nobody has looked at in months, which is precisely the thing a
    // list of "the current branch is fine" would never show.
    if let Ok(out) = git
        .run(
            path,
            &["branch", "--list", "--no-color", parse::BRANCH_FORMAT],
            LOCAL_TIMEOUT,
        )
        .await
    {
        if out.ok() {
            repo.branches = parse::parse_branches(&out.stdout);
        }
    }

    // Linked working trees, each with its own uncommitted changes. Reporting
    // only the main one would be quietly wrong about the whole question.
    if let Ok(out) = git.run(path, &["worktree", "list", "--porcelain"], LOCAL_TIMEOUT).await {
        if out.ok() {
            repo.worktrees = parse::parse_worktrees(&out.stdout);
        }
    }

    // How each remote authenticates. Configuration only — never a key, a
    // token, or a password.
    for index in 0..repo.remotes.len() {
        let url = repo.remotes[index].url.clone();
        repo.remotes[index].auth = auth_method(git, path, &url).await;
    }

    if check_remotes {
        repo.remote_check = check_remote(git, &repo).await;
    }
    repo
}

/// Ask the remote where it is, without fetching.
///
/// `ls-remote` is the only read that does not write into the repository. A
/// fetch would update remote-tracking refs and reflogs in a tree the user did
/// not ask us to modify — a small change, but this application's whole promise
/// is that it copies and does not alter.
async fn check_remote(git: &Git, repo: &GitRepo) -> Option<RemoteCheck> {
    let remote = repo.primary_remote()?.name.clone();
    let branch = repo.branch.clone()?;
    let reference = format!("refs/heads/{branch}");
    let checked_at = Utc::now();

    let listed = git
        .run(&repo.path, &["ls-remote", &remote, &reference], REMOTE_TIMEOUT)
        .await;
    let out = match listed {
        Ok(out) if out.ok() => out,
        Ok(out) => {
            return Some(RemoteCheck {
                remote,
                reference,
                remote_head: None,
                relation: None,
                error: Some(out.failure()),
                checked_at,
            })
        }
        Err(e) => {
            return Some(RemoteCheck {
                remote,
                reference,
                remote_head: None,
                relation: None,
                error: Some(e.to_string()),
                checked_at,
            })
        }
    };

    let Some(remote_head) = parse::parse_ls_remote(&out.stdout, &reference) else {
        // The remote answered and does not have this branch: it has never been
        // pushed, whatever `.git` believes.
        return Some(RemoteCheck {
            remote,
            reference,
            remote_head: None,
            relation: None,
            error: Some("the remote does not have this branch".into()),
            checked_at,
        });
    };

    let relation = relation_to(git, &repo.path, repo.head.as_deref(), &remote_head).await;
    Some(RemoteCheck {
        remote,
        reference,
        remote_head: Some(remote_head),
        relation: Some(relation),
        error: None,
        checked_at,
    })
}

/// Work out how two commits relate, using only what is already in the clone.
async fn relation_to(
    git: &Git,
    path: &Path,
    local_head: Option<&str>,
    remote_head: &str,
) -> Relation {
    let Some(local) = local_head else { return Relation::NeedsFetch };
    if local == remote_head {
        return Relation::Same;
    }
    // Do we even have the remote's commit? A clone that has not fetched since
    // the remote moved does not, and no amount of ancestry checking will
    // invent it.
    let have = git
        .run(path, &["cat-file", "-e", &format!("{remote_head}^{{commit}}")], LOCAL_TIMEOUT)
        .await;
    if !matches!(have, Ok(ref out) if out.ok()) {
        return Relation::NeedsFetch;
    }

    let remote_is_ancestor = git
        .run(path, &["merge-base", "--is-ancestor", remote_head, local], LOCAL_TIMEOUT)
        .await;
    let local_is_ancestor = git
        .run(path, &["merge-base", "--is-ancestor", local, remote_head], LOCAL_TIMEOUT)
        .await;
    let remote_behind = matches!(remote_is_ancestor, Ok(ref o) if o.ok());
    let local_behind = matches!(local_is_ancestor, Ok(ref o) if o.ok());
    match (remote_behind, local_behind) {
        (true, false) => Relation::LocalAhead,
        (false, true) => Relation::LocalBehind,
        (false, false) => Relation::Diverged,
        // Both ancestors of each other means the same commit, already handled.
        (true, true) => Relation::Same,
    }
}

/// Did git refuse this folder over who owns it?
///
/// Git's message is `fatal: detected dubious ownership in repository at ...`,
/// followed by four lines telling the user to run a `git config` command they
/// are not looking at a terminal to run. Recognising it here is what turns a
/// dead row reading "Unreadable" into one with a button.
fn is_dubious_ownership(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("dubious ownership") || lower.contains("safe.directory")
}

/// A path in the form git writes and compares `safe.directory` entries in.
///
/// Two transformations, both required for the entry to match. Windows'
/// canonical form carries the `\\?\` extended-length prefix, which git has
/// never heard of, and git normalises separators to `/` in its config — so an
/// entry written with backslashes silently never matches and the button
/// appears to do nothing.
fn normalise_for_git_config(path: &Path) -> String {
    let text = path.display().to_string();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.replace('\\', "/")
}

/// Tell git this folder is safe to read, for this user, permanently.
///
/// Exactly what git's own error message instructs, written to the user's
/// global config: `git config --global --add safe.directory <path>`.
///
/// This is a real security decision and it is the user's to make, which is why
/// it is a separate action behind a button rather than something the scan does
/// on its own. Git's check exists because reading a repository *executes*
/// configuration from it — `core.fsmonitor` names a program git will run — so
/// a repository planted by another account could run code as this user.
/// Marking one folder trusted after being told what that means is reasonable;
/// silently bypassing the check for every folder scanned is not, and a scan
/// that did it would be a way to get code running by leaving a repository
/// somewhere a backup job looks.
pub async fn trust(path: &Path) -> Result<ActionOutcome> {
    let git = Git::locate()?;
    if !path.is_dir() {
        return Err(Error::Validation(format!("{} is not a folder", path.display())));
    }
    // The absolute path, because a relative one in the global config would
    // match a different folder from wherever git happened to be run.
    let absolute = std::fs::canonicalize(path)
        .map_err(|e| Error::io(format!("resolving {}", path.display()), e))?;
    // `\\?\C:\...` is how Windows returns a canonical path, and git does not
    // match its own repositories against that form — nor against backslashes,
    // which it writes as forward slashes in its own config.
    let text = normalise_for_git_config(&absolute);

    let out = git
        .run(path, &["config", "--global", "--add", "safe.directory", &text], LOCAL_TIMEOUT)
        .await?;
    Ok(ActionOutcome {
        path: path.to_path_buf(),
        action: "trust".into(),
        ok: out.ok(),
        detail: if out.ok() {
            format!("git will now read {text}.")
        } else {
            out.failure()
        },
    })
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// What an action did, in the words the user should see.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionOutcome {
    pub path: PathBuf,
    /// `pull`, `commit`, `push`.
    pub action: String,
    pub ok: bool,
    /// git's own summary, trimmed. Shown rather than paraphrased: a user who
    /// knows git wants the real message, and one who does not is no worse off.
    pub detail: String,
}

/// Work out how a remote authenticates, from its URL and git's own config.
///
/// Reads configuration only — `credential.helper`, `core.sshCommand` — and
/// never a key, a token or a password. See [`parse::AuthMethod`].
async fn auth_method(git: &Git, path: &Path, url: &str) -> parse::AuthMethod {
    let Some(location) = parse::parse_remote_url(url) else {
        return parse::AuthMethod::Local;
    };
    let ssh = url.starts_with("ssh://")
        || (!url.contains("://") && url.contains(':'))
        || url.starts_with("git+ssh://");

    if ssh {
        let user = url
            .split('@')
            .next()
            .filter(|_| url.contains('@'))
            .map(|u| u.rsplit('/').next().unwrap_or(u).to_string());
        // `core.sshCommand` is where a per-repository key is usually pinned.
        // Anything more than that lives in ~/.ssh/config, which only ssh
        // itself resolves — and saying so is better than guessing.
        let key_path = match git
            .run(path, &["config", "--get", "core.sshCommand"], LOCAL_TIMEOUT)
            .await
        {
            Ok(out) if out.ok() => extract_identity_file(out.stdout.trim()),
            _ => None,
        };
        let key_path = match key_path {
            Some(key) => Some(key),
            None => default_ssh_key(),
        };
        return parse::AuthMethod::SshKey { key_path, user };
    }

    let _ = location;
    match git.run(path, &["config", "--get", "credential.helper"], LOCAL_TIMEOUT).await {
        Ok(out) if out.ok() && !out.stdout.trim().is_empty() => {
            parse::AuthMethod::CredentialHelper { helper: out.stdout.trim().to_string() }
        }
        _ => parse::AuthMethod::NoneConfigured,
    }
}

/// Pull `-i <path>` out of a `core.sshCommand`.
fn extract_identity_file(command: &str) -> Option<String> {
    let mut parts = command.split_whitespace();
    while let Some(part) = parts.next() {
        if part == "-i" {
            return parts.next().map(|p| p.trim_matches('"').to_string());
        }
        if let Some(rest) = part.strip_prefix("-i") {
            if !rest.is_empty() {
                return Some(rest.trim_matches('"').to_string());
            }
        }
    }
    None
}

/// The key ssh would offer by default, if exactly one obvious candidate
/// exists.
///
/// Reported as the *likely* key and never as a certainty: ssh consults
/// `~/.ssh/config` and the agent, and neither is read here. With several keys
/// present there is no single answer, so none is given.
fn default_ssh_key() -> Option<String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    let ssh = home.join(".ssh");
    let candidates: Vec<PathBuf> = ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|name| ssh.join(name))
        .filter(|p| p.is_file())
        .collect();
    match candidates.len() {
        1 => Some(candidates[0].display().to_string()),
        _ => None,
    }
}

/// The trailer that says a commit was made from here.
///
/// A `Co-authored-by`-style trailer rather than a rewritten author: the commit
/// is still the *user's*, made with their identity, and claiming otherwise
/// would put a name they never chose on their history. This records the tool
/// that pressed the button, which is what someone looking at an unfamiliar
/// commit six months later actually wants to know.
pub const COMMIT_TRAILER: &str = "Committed-with: superbackup";

/// A commit message to start from, which the user is expected to edit.
///
/// It says what is being committed and why it was offered — a folder whose
/// work existed nowhere else — because "wip" and "changes" are what get typed
/// when a box is empty, and neither is any use later. Nothing here is forced:
/// this is a default in a text field, not a template with holes.
pub fn suggested_commit_message(repo: &str, changes: u32, at: DateTime<Utc>) -> String {
    let what = if changes == 1 { "1 change" } else { &format!("{changes} changes") };
    format!(
        "Save uncommitted work in {repo}

         {what} committed from superbackup on {}, because this folder held work that existed 
         nowhere but this machine.

         {COMMIT_TRAILER}",
        at.format("%-d %B %Y")
    )
}

/// `git pull --ff-only`.
///
/// Fast-forward only, always. A plain `pull` on a diverged branch either
/// creates a merge commit or starts a rebase, and doing either on the user's
/// behalf — from a backup tool, with one button, in a repository they have not
/// looked at — is not a decision this program gets to make. When it cannot
/// fast-forward it says so, and the user opens their own terminal.
pub async fn pull(path: &Path) -> Result<ActionOutcome> {
    let git = Git::locate()?;
    ensure_repository(&git, path).await?;
    let out = git.run(path, &["pull", "--ff-only"], REMOTE_TIMEOUT * 4).await?;
    Ok(ActionOutcome {
        path: path.to_path_buf(),
        action: "pull".into(),
        ok: out.ok(),
        detail: if out.ok() {
            first_line(&out.stdout, "Already up to date.")
        } else {
            format!(
                "{} Superbackup only fast-forwards, so nothing was changed.",
                out.failure()
            )
        },
    })
}

/// Stage everything and commit it.
///
/// `--all` includes untracked files, because the state this is meant to rescue
/// — a folder of work that exists nowhere else — is usually untracked as well
/// as uncommitted. `.gitignore` still applies, so a `node_modules` is not
/// swept in by it.
pub async fn commit(path: &Path, message: &str, include_untracked: bool) -> Result<ActionOutcome> {
    let message = message.trim();
    if message.is_empty() {
        return Err(Error::Validation("a commit needs a message".into()));
    }
    let git = Git::locate()?;
    ensure_repository(&git, path).await?;

    let stage = if include_untracked {
        git.run(path, &["add", "--all", "--", "."], LOCAL_TIMEOUT).await?
    } else {
        git.run(path, &["add", "--update", "--", "."], LOCAL_TIMEOUT).await?
    };
    if !stage.ok() {
        return Ok(ActionOutcome {
            path: path.to_path_buf(),
            action: "commit".into(),
            ok: false,
            detail: stage.failure(),
        });
    }

    let out = git.run(path, &["commit", "-m", message], LOCAL_TIMEOUT).await?;
    if out.ok() {
        return Ok(ActionOutcome {
            path: path.to_path_buf(),
            action: "commit".into(),
            ok: true,
            detail: first_line(&out.stdout, "Committed."),
        });
    }
    // "nothing to commit" is git's exit 1, and is not a failure: the tree was
    // already clean, which is the state the caller wanted.
    let combined = format!("{}{}", out.stdout, out.stderr);
    if combined.contains("nothing to commit") || combined.contains("nothing added to commit") {
        return Ok(ActionOutcome {
            path: path.to_path_buf(),
            action: "commit".into(),
            ok: true,
            detail: "Nothing to commit; the working tree is already clean.".into(),
        });
    }
    Ok(ActionOutcome {
        path: path.to_path_buf(),
        action: "commit".into(),
        ok: false,
        detail: identity_hint(&out.failure()),
    })
}

/// `git push`, setting an upstream the first time if the branch has none.
pub async fn push(path: &Path) -> Result<ActionOutcome> {
    let git = Git::locate()?;
    ensure_repository(&git, path).await?;

    let status = git
        .run(path, &["status", "--porcelain=v2", "--branch"], LOCAL_TIMEOUT)
        .await?;
    let parsed = parse::parse_status(&status.stdout);
    let Some(branch) = parsed.branch else {
        return Err(Error::Validation(
            "this repository has no branch checked out, so there is nothing to push".into(),
        ));
    };

    let out = if parsed.upstream.is_some() {
        git.run(path, &["push"], REMOTE_TIMEOUT * 4).await?
    } else {
        // A branch that has never been pushed needs to be told where to go,
        // and `--set-upstream` means the next push does not.
        git.run(path, &["push", "--set-upstream", "origin", &branch], REMOTE_TIMEOUT * 4)
            .await?
    };
    Ok(ActionOutcome {
        path: path.to_path_buf(),
        action: "push".into(),
        ok: out.ok(),
        detail: if out.ok() {
            first_line(&out.stderr, "Pushed.")
        } else {
            out.failure()
        },
    })
}

/// Refuse to act on a folder that is not a repository.
///
/// The path arrives from a client, and `git commit -a` in the wrong folder
/// walks *up* to whatever repository contains it — so a mistyped path could
/// commit in a parent tree the user never named. This pins the action to a
/// repository root.
async fn ensure_repository(git: &Git, path: &Path) -> Result<()> {
    if !path.is_dir() {
        return Err(Error::Validation(format!("{} is not a folder", path.display())));
    }
    let out = git
        .run(path, &["rev-parse", "--show-toplevel"], LOCAL_TIMEOUT)
        .await?;
    if !out.ok() {
        return Err(Error::Validation(format!(
            "{} is not a git repository",
            path.display()
        )));
    }
    let top = PathBuf::from(out.stdout.trim());
    let same = std::fs::canonicalize(&top)
        .ok()
        .zip(std::fs::canonicalize(path).ok())
        .map(|(a, b)| a == b)
        .unwrap_or(false);
    if !same {
        return Err(Error::Validation(format!(
            "{} is inside the repository at {}, not its root. Superbackup acts on whole \
             repositories only.",
            path.display(),
            top.display()
        )));
    }
    Ok(())
}

/// Turn git's identity complaint into an instruction.
///
/// `*** Please tell me who you are` is four lines of shell that a user staring
/// at a backup application has no way to run, and the failure is otherwise
/// completely opaque.
fn identity_hint(failure: &str) -> String {
    if failure.contains("Please tell me who you are") || failure.contains("empty ident name") {
        return "git does not know who you are, so it will not record a commit. Set it once with \
                `git config --global user.name \"Your Name\"` and `git config --global user.email \
                \"you@example.com\"`."
            .to_string();
    }
    failure.to_string()
}

fn first_line(text: &str, fallback: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn repo(name: &str) -> GitRepo {
        GitRepo {
            path: PathBuf::from(format!("/w/{name}")),
            name: name.into(),
            root: PathBuf::from("/w"),
            branch: Some("main".into()),
            head: Some("aaaa".into()),
            upstream: Some("origin/main".into()),
            ahead: 0,
            behind: 0,
            staged: 0,
            unstaged: 0,
            untracked: 0,
            conflicted: 0,
            last_commit_at: None,
            last_commit_summary: None,
            last_commit_author: None,
            remotes: vec![RepoRemote {
                name: "origin".into(),
                url: "git@github.com:me/thing.git".into(),
                host: Some("github.com".into()),
                forge: Forge::GitHub,
                web_url: Some("https://github.com/me/thing".into()),
                auth: parse::AuthMethod::SshKey { key_path: None, user: Some("git".into()) },
            }],
            remote_check: None,
            branches: Vec::new(),
            worktrees: Vec::new(),
            external: false,
            error: None,
            untrusted: false,
        }
    }

    /// The ordering is the feature. A backup tool's question is "what is only
    /// here", so uncommitted work must outrank being behind the remote, and
    /// a repository with no remote at all must not read as fine.
    #[test]
    fn risk_is_what_orders_the_list() {
        let mut clean = repo("clean");
        assert_eq!(clean.state(), RepoState::Clean);
        assert!(!clean.state().only_on_this_disk());

        clean.behind = 3;
        assert_eq!(clean.state(), RepoState::PullRecommended);
        assert!(!clean.state().only_on_this_disk(), "being behind risks nothing");

        let mut dirty = repo("dirty");
        dirty.behind = 3;
        dirty.untracked = 1;
        assert_eq!(dirty.state(), RepoState::Uncommitted, "unsaved work outranks being behind");
        assert!(dirty.state().only_on_this_disk());

        let mut ahead = repo("ahead");
        ahead.ahead = 2;
        assert_eq!(ahead.state(), RepoState::Unpushed);
        assert!(ahead.state().only_on_this_disk());

        let mut local = repo("local-only");
        local.remotes.clear();
        local.upstream = None;
        assert_eq!(local.state(), RepoState::NoRemote);
        assert!(local.state().only_on_this_disk(), "a repo with no remote is entirely at risk");

        let mut never = repo("never-pushed");
        never.upstream = None;
        assert_eq!(never.state(), RepoState::NoUpstream);
        assert!(never.state().only_on_this_disk());

        let mut detached = repo("detached");
        detached.branch = None;
        assert_eq!(detached.state(), RepoState::Detached);

        let mut broken = repo("broken");
        broken.error = Some("bad index".into());
        assert_eq!(broken.state(), RepoState::Unreadable);
    }

    /// `.git` says "up to date" for as long as nobody fetches. When the live
    /// check disagrees with the cache, the live answer is the true one — this
    /// is the entire reason `ls-remote` is worth a network call.
    #[test]
    fn a_live_check_overrules_a_stale_fetch() {
        let mut stale = repo("stale");
        // Everything local says level with the remote.
        stale.ahead = 0;
        stale.behind = 0;
        assert_eq!(stale.state(), RepoState::Clean);

        stale.remote_check = Some(RemoteCheck {
            remote: "origin".into(),
            reference: "refs/heads/main".into(),
            remote_head: Some("bbbb".into()),
            relation: Some(Relation::LocalBehind),
            error: None,
            checked_at: Utc::now(),
        });
        assert_eq!(stale.state(), RepoState::PullRecommended, "the remote has moved since March");

        // And a remote whose commit we do not even have is the same advice.
        stale.remote_check.as_mut().expect("check").relation = Some(Relation::NeedsFetch);
        assert_eq!(stale.state(), RepoState::PullRecommended);

        // Ahead, live: the commits are on this disk only.
        stale.remote_check.as_mut().expect("check").relation = Some(Relation::LocalAhead);
        assert_eq!(stale.state(), RepoState::Unpushed);
    }

    /// The whole point of the marking: a clone you read and never commit to
    /// stops being reported as work you could lose. It is still backed up —
    /// the setting changes what is *said* about it, not what is copied.
    #[test]
    fn an_external_repository_is_not_counted_as_work_at_risk() {
        let mut clone = repo("some-dependency");
        clone.untracked = 3;
        clone.upstream = None;
        assert_eq!(clone.state(), RepoState::Uncommitted);
        assert!(clone.state().only_on_this_disk(), "before marking, it is a warning");

        clone.external = true;
        assert_eq!(clone.state(), RepoState::External);
        assert!(!clone.state().only_on_this_disk(), "after marking, it is not");
        assert!(clone.state().explanation().contains("still backed up"));

        // It outranks even the states that cannot be read, because the user
        // has said they do not care what is in there.
        let mut broken = repo("vendored");
        broken.external = true;
        broken.untrusted = true;
        broken.error = Some("dubious ownership".into());
        assert_eq!(broken.state(), RepoState::External);
    }

    /// External sorts last, so marking a repository moves it out of the way
    /// rather than leaving it at the top of a list of problems.
    #[test]
    fn external_sorts_below_everything_that_needs_attention() {
        assert!(RepoState::External > RepoState::Clean);
        assert!(RepoState::External > RepoState::Uncommitted);
        assert!(RepoState::Uncommitted < RepoState::PullRecommended);
    }

    /// The auth report names the mechanism and never the secret. A screen that
    /// showed a token would be a screen that put one in a screenshot.
    #[test]
    fn the_auth_report_names_the_mechanism_and_never_the_secret() {
        let ssh = parse::AuthMethod::SshKey {
            key_path: Some("/home/a/.ssh/id_ed25519".into()),
            user: Some("git".into()),
        };
        assert_eq!(ssh.label(), "SSH · id_ed25519", "the file name, not the key");
        assert!(ssh.detail().contains("never reads the key"));

        let unknown = parse::AuthMethod::SshKey { key_path: None, user: None };
        assert_eq!(unknown.label(), "SSH");
        assert!(unknown.detail().contains("cannot be named"), "honest about not knowing");

        let helper = parse::AuthMethod::CredentialHelper { helper: "manager".into() };
        assert_eq!(helper.label(), "HTTPS · manager");
        assert!(helper.detail().contains("never sees it"));

        // The state that makes a private HTTPS remote fail under a scan.
        assert!(parse::AuthMethod::NoneConfigured.detail().contains("fails rather than prompting"));
    }

    /// `-i <path>` is where a per-repository key is pinned, and it is the one
    /// place the key can be named without running ssh itself.
    #[test]
    fn a_pinned_ssh_key_is_read_out_of_the_configured_command() {
        assert_eq!(
            extract_identity_file("ssh -i /home/a/.ssh/work_ed25519"),
            Some("/home/a/.ssh/work_ed25519".to_string())
        );
        assert_eq!(
            extract_identity_file("ssh -o BatchMode=yes -i \"C:/Users/a/.ssh/k\" -F none"),
            Some("C:/Users/a/.ssh/k".to_string())
        );
        assert_eq!(extract_identity_file("ssh -o BatchMode=yes"), None);
        assert_eq!(extract_identity_file(""), None);
    }

    /// A URL a person can click, for the hosts whose web layout is the same
    /// as their clone path — which is every forge this recognises.
    #[test]
    fn a_remote_becomes_a_link_a_person_can_open() {
        let ssh = parse::parse_remote_url("git@github.com:andreaswiren/superbackup.git")
            .expect("parsed");
        assert_eq!(
            parse::web_url(&ssh).as_deref(),
            Some("https://github.com/andreaswiren/superbackup"),
            "an ssh remote still has a web page"
        );

        let azure =
            parse::parse_remote_url("https://dev.azure.com/org/project/_git/repo").expect("parsed");
        assert_eq!(
            parse::web_url(&azure).as_deref(),
            Some("https://dev.azure.com/org/project/_git/repo")
        );

        // A remote with a host but no path names no repository.
        let bare = parse::RemoteLocation { host: "github.com".into(), path: String::new() };
        assert_eq!(parse::web_url(&bare), None);
    }

    #[test]
    fn origin_is_the_remote_a_person_means() {
        let mut many = repo("many");
        many.remotes.insert(
            0,
            RepoRemote {
                name: "upstream".into(),
                url: "https://gitlab.com/them/thing".into(),
                host: Some("gitlab.com".into()),
                forge: Forge::GitLab,
                web_url: Some("https://gitlab.com/them/thing".into()),
                auth: parse::AuthMethod::NoneConfigured,
            },
        );
        assert_eq!(many.primary_remote().expect("one").name, "origin");

        // With no origin, whatever is configured is better than nothing.
        many.remotes.retain(|r| r.name != "origin");
        assert_eq!(many.primary_remote().expect("one").name, "upstream");
    }

    /// The bound that keeps a scan from walking a Next.js cache. Losing this
    /// makes the feature unusable on exactly the machines it was written for.
    #[test]
    fn the_folders_that_made_this_program_necessary_are_never_walked() {
        for skipped in ["node_modules", ".next", "target", ".turbo", "__pycache__"] {
            assert!(
                SKIPPED.iter().any(|s| s.eq_ignore_ascii_case(skipped)),
                "{skipped} must not be descended into"
            );
        }
    }

    /// A `safe.directory` entry that git will not match is a button that
    /// appears to do nothing. Both transformations are required: git has never
    /// heard of the extended-length prefix, and it compares with `/`.
    #[test]
    fn a_trusted_path_is_written_in_the_form_git_compares_against() {
        assert_eq!(
            normalise_for_git_config(Path::new(r"\\?\C:\Users\Andreas\workspace\deathwar")),
            "C:/Users/Andreas/workspace/deathwar"
        );
        assert_eq!(
            normalise_for_git_config(Path::new(r"C:\Users\Andreas\workspace\deathwar")),
            "C:/Users/Andreas/workspace/deathwar"
        );
        assert_eq!(normalise_for_git_config(Path::new("/home/a/work/thing")), "/home/a/work/thing");
    }

    /// The real message from a folder created by an elevated shell, which is
    /// three of the nineteen repositories on the machine this was written on.
    /// Left as a generic failure it reads "Unreadable" and offers nothing.
    #[test]
    fn gits_ownership_refusal_is_told_apart_from_a_real_failure() {
        let real = "fatal: detected dubious ownership in repository at '/w/deathwar'\n\
                    '/w/deathwar' is owned by:\n\tBUILTIN/Administrators (S-1-5-32-544)\n";
        assert!(is_dubious_ownership(real));
        assert!(is_dubious_ownership("fatal: add an exception with safe.directory"));
        assert!(!is_dubious_ownership("fatal: not a git repository"));
        assert!(!is_dubious_ownership(""));

        let mut untrusted = repo("elevated");
        untrusted.untrusted = true;
        untrusted.error = Some("detected dubious ownership".into());
        assert_eq!(untrusted.state(), RepoState::NotTrusted);
        // And it must never read as safe: nothing at all is known about it.
        assert!(untrusted.state().explanation().contains("cannot tell"));

        let mut broken = repo("broken");
        broken.error = Some("index file corrupt".into());
        assert_eq!(broken.state(), RepoState::Unreadable);
    }

    /// The suggestion is a starting point, and it must say enough to be worth
    /// keeping: what happened, where, when, and that superbackup made it. A
    /// message nobody can interpret six months later is what an empty box
    /// produces, which is the whole reason this exists.
    #[test]
    fn the_suggested_message_says_what_happened_and_who_made_it() {
        let at = Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).single().expect("a date");
        let message = suggested_commit_message("wasm-openra", 71, at);

        let subject = message.lines().next().expect("a subject");
        assert!(subject.contains("wasm-openra"), "{subject}");
        assert!(subject.len() < 72, "a subject line is short: {subject}");
        assert!(message.contains("71 changes"));
        assert!(message.contains("2026"));
        assert!(message.contains(COMMIT_TRAILER), "the trailer records the tool");
        // A blank line after the subject, or git treats the whole thing as one.
        assert_eq!(message.lines().nth(1), Some(""), "{message}");

        // Singular reads as English rather than "1 changes".
        let one = suggested_commit_message("notes", 1, at);
        assert!(one.contains("1 change committed"), "{one}");
        assert!(!one.contains("1 changes"), "{one}");
    }

    /// The trailer names the tool; it must not claim to be the author. The
    /// commit is the user's, made under their own git identity, and putting a
    /// name they never chose on their history would be a lie in their log.
    #[test]
    fn the_trailer_records_the_tool_rather_than_replacing_the_author() {
        assert!(COMMIT_TRAILER.contains("superbackup"));
        for authorship in ["Author:", "Signed-off-by", "Co-authored-by"] {
            assert!(
                !COMMIT_TRAILER.contains(authorship),
                "{COMMIT_TRAILER} must not claim authorship"
            );
        }
    }

    #[test]
    fn an_empty_commit_message_is_refused_before_anything_is_staged() {
        let dir = std::env::current_dir().expect("cwd");
        let err = tokio_test_block(commit(&dir, "   ", true)).expect_err("refused");
        assert!(err.to_string().contains("message"), "{err}");
    }

    fn tokio_test_block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }
}
