//! Turning `git`'s output into values, with no process and no filesystem.
//!
//! Every function here is pure, so the parsing can be tested against captured
//! real output rather than against a live repository whose state changes under
//! the test. The formats are the machine-readable ones — `--porcelain=v2`, an
//! explicit `--format` — never the human output, which git is free to change
//! and which is localised.

use chrono::{DateTime, TimeZone, Utc};

/// The record separator asked for in `git log --format`.
///
/// A commit subject can contain almost anything and an author name can contain
/// a tab or a pipe, so the separator has to be a character that cannot appear
/// in either. US (0x1f) is the conventional choice and git emits it verbatim
/// through `%x1f`.
pub const FIELD_SEP: char = '\u{1f}';

/// `git log -1` in the format [`LOG_FORMAT`] asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastCommit {
    pub id: String,
    pub at: DateTime<Utc>,
    pub author: String,
    pub summary: String,
}

/// The `--format` string [`parse_last_commit`] expects: hash, commit time as a
/// Unix timestamp, author name, subject.
pub const LOG_FORMAT: &str = "--format=%H%x1f%ct%x1f%an%x1f%s";

/// Parse one line of [`LOG_FORMAT`] output.
///
/// The *commit* date, not the author date: a rebased or cherry-picked commit
/// keeps its original author date, which would report a branch touched an hour
/// ago as three months stale — exactly backwards for a screen whose question is
/// "when was this last worked on".
pub fn parse_last_commit(output: &str) -> Option<LastCommit> {
    let line = output.lines().next()?.trim_end_matches('\r');
    let mut fields = line.split(FIELD_SEP);
    let id = fields.next()?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let at = Utc.timestamp_opt(fields.next()?.trim().parse::<i64>().ok()?, 0).single()?;
    let author = fields.next().unwrap_or_default().to_string();
    // The subject is last and may itself contain the separator only if the
    // author put one there, so the remainder is taken whole rather than split.
    let summary = fields.collect::<Vec<_>>().join(&FIELD_SEP.to_string());
    Some(LastCommit { id, at, author, summary })
}

/// What the working tree and the branch look like, from one `git status`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// `None` on a detached HEAD, which git reports as the literal
    /// `(detached)`.
    pub branch: Option<String>,
    /// `None` in a repository with no commits yet, reported as `(initial)`.
    pub head: Option<String>,
    /// The tracking branch, e.g. `origin/main`. Absent until one is set, which
    /// is the state a branch created locally and never pushed is left in.
    pub upstream: Option<String>,
    /// Commits on this branch that the tracking ref does not have, and
    /// vice versa — as of the last fetch, which is why
    /// [`crate::git::RemoteCheck`] exists.
    pub ahead: u32,
    pub behind: u32,
    /// Files with changes added to the index.
    pub staged: u32,
    /// Files changed in the working tree and not added.
    pub unstaged: u32,
    /// Files git has never been told about.
    pub untracked: u32,
    /// Files in a conflicted merge.
    pub conflicted: u32,
}

impl Status {
    /// Is there work here that exists nowhere but this disk?
    pub fn has_local_changes(&self) -> bool {
        self.staged + self.unstaged + self.untracked + self.conflicted > 0
    }
}

/// Parse `git status --porcelain=v2 --branch --untracked-files=normal`.
///
/// Format, from `git-status(1)`:
///
/// ```text
/// # branch.oid <commit> | (initial)
/// # branch.head <branch> | (detached)
/// # branch.upstream <upstream>
/// # branch.ab +<ahead> -<behind>
/// 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>        ordinary change
/// 2 <XY> … <path><TAB><origPath>                      rename or copy
/// u <XY> …                                            unmerged
/// ? <path>                                            untracked
/// ! <path>                                            ignored
/// ```
///
/// `X` is the state in the index and `Y` the state in the working tree; `.`
/// means unchanged. A file can be both — staged edits plus later unstaged ones
/// — so it counts once in each column rather than being forced into one.
pub fn parse_status(output: &str) -> Status {
    let mut status = Status::default();
    for raw in output.lines() {
        let line = raw.trim_end_matches('\r');
        if let Some(header) = line.strip_prefix("# ") {
            parse_status_header(header, &mut status);
            continue;
        }
        let Some((kind, rest)) = line.split_once(' ') else { continue };
        match kind {
            "1" | "2" => {
                let mut xy = rest.chars();
                let index = xy.next().unwrap_or('.');
                let worktree = xy.next().unwrap_or('.');
                if index != '.' {
                    status.staged += 1;
                }
                if worktree != '.' {
                    status.unstaged += 1;
                }
            }
            "u" => status.conflicted += 1,
            "?" => status.untracked += 1,
            // `!` is an ignored file, which is only ever listed when explicitly
            // asked for, and is never a change.
            _ => {}
        }
    }
    status
}

fn parse_status_header(header: &str, status: &mut Status) {
    let Some((key, value)) = header.split_once(' ') else { return };
    match key {
        "branch.oid" if value != "(initial)" => status.head = Some(value.to_string()),
        "branch.head" if value != "(detached)" => status.branch = Some(value.to_string()),
        "branch.upstream" => status.upstream = Some(value.to_string()),
        "branch.ab" => {
            // `+3 -0`. Absent entirely when there is no upstream, so a missing
            // header means "unknown", which the zeroed default already says.
            for part in value.split_whitespace() {
                match part.split_at(1) {
                    ("+", n) => status.ahead = n.parse().unwrap_or(0),
                    ("-", n) => status.behind = n.parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// One configured remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

/// Parse `git config --get-regexp ^remote\..*\.url`, whose lines are
/// `remote.<name>.url <url>`.
///
/// `git remote -v` would do as well, but it prints each remote twice — once
/// for fetch and once for push — and the two can legitimately differ, which
/// makes deduplicating it a guess. The config is the source of truth.
pub fn parse_remotes(output: &str) -> Vec<Remote> {
    let mut remotes = Vec::new();
    for raw in output.lines() {
        let line = raw.trim_end_matches('\r');
        let Some((key, url)) = line.split_once(' ') else { continue };
        let Some(name) = key.strip_prefix("remote.").and_then(|k| k.strip_suffix(".url")) else {
            continue;
        };
        if name.is_empty() || url.is_empty() {
            continue;
        }
        remotes.push(Remote { name: name.to_string(), url: url.to_string() });
    }
    remotes
}

/// Pull the object id for one ref out of `git ls-remote` output.
///
/// Lines are `<sha>\t<ref>`. The ref is matched exactly, so `refs/heads/main`
/// is never satisfied by `refs/heads/main-2` or by the `refs/tags/main^{}`
/// that a tag of the same name would add.
pub fn parse_ls_remote(output: &str, want: &str) -> Option<String> {
    for raw in output.lines() {
        let line = raw.trim_end_matches('\r');
        let Some((sha, name)) = line.split_once('\t') else { continue };
        if name.trim() == want {
            let sha = sha.trim();
            if !sha.is_empty() {
                return Some(sha.to_string());
            }
        }
    }
    None
}

/// Where a remote URL points, split into the parts a forge API needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteLocation {
    /// `github.com`, `gitea.example.com:2222` reduced to `gitea.example.com`.
    pub host: String,
    /// The path with any `.git` suffix removed: `owner/repo`, or
    /// `org/project/_git/repo` on Azure DevOps.
    pub path: String,
}

impl RemoteLocation {
    /// `owner`, `repo` for the hosts that use a two-segment path.
    ///
    /// Azure DevOps is the exception and is handled by its own accessor,
    /// because its path is four segments and splitting it as `owner/repo`
    /// would name a project as though it were a user.
    pub fn owner_repo(&self) -> Option<(&str, &str)> {
        let (owner, repo) = self.path.rsplit_once('/')?;
        if owner.is_empty() || repo.is_empty() || owner.contains("/_git") {
            return None;
        }
        Some((owner, repo))
    }
}

/// Parse any of the shapes git accepts for a remote.
///
/// * `https://github.com/owner/repo.git`
/// * `http://gitea.example.com:3000/owner/repo`
/// * `ssh://git@example.com:2222/owner/repo.git`
/// * `git@github.com:owner/repo.git` — the scp-like form, which is not a URL
///   and has to be recognised by the colon rather than by a scheme
/// * `https://org@dev.azure.com/org/project/_git/repo`
///
/// A local path (`/srv/git/repo`, `C:\repos\thing`, `../sibling`) is not a
/// host and returns `None`: reporting `C:` as the forge would be worse than
/// reporting nothing.
pub fn parse_remote_url(url: &str) -> Option<RemoteLocation> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let after_scheme = match url.split_once("://") {
        Some((scheme, rest)) => {
            // `file://` is a local path wearing a URL, and has no forge.
            if scheme.eq_ignore_ascii_case("file") {
                return None;
            }
            rest
        }
        None => {
            // The scp-like form: `[user@]host:path`, where the part before the
            // colon has no slash. `C:\repos\thing` also matches "colon", which
            // is why a single-character host is rejected — a drive letter is
            // never a hostname.
            let (before, after) = url.split_once(':')?;
            if before.contains('/') || before.contains('\\') || after.starts_with('\\') {
                return None;
            }
            let host_part = before.rsplit('@').next().unwrap_or(before);
            if host_part.chars().count() < 2 || !host_part.contains('.') {
                return None;
            }
            return Some(RemoteLocation {
                host: host_part.to_ascii_lowercase(),
                path: clean_repo_path(after),
            });
        }
    };

    let (authority, path) = match after_scheme.split_once('/') {
        Some((a, p)) => (a, p),
        None => (after_scheme, ""),
    };
    // Drop `user@` and any `:port`.
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        return None;
    }
    Some(RemoteLocation { host: host.to_ascii_lowercase(), path: clean_repo_path(path) })
}

fn clean_repo_path(path: &str) -> String {
    path.trim_matches('/').trim_end_matches(".git").trim_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a real repository with staged, unstaged and untracked
    /// work at once — the case a single "dirty" flag cannot describe, and the
    /// case a developer's machine is actually in.
    const PORCELAIN: &str = "\
# branch.oid 8f2a1c9e4b7d3a5f6c8e0b2d4f6a8c0e2b4d6f80
# branch.head feature/inventory
# branch.upstream origin/feature/inventory
# branch.ab +2 -3
1 M. N... 100644 100644 100644 aaaa bbbb staged-only.rs
1 .M N... 100644 100644 100644 cccc dddd unstaged-only.rs
1 MM N... 100644 100644 100644 eeee ffff both.rs
2 R. N... 100644 100644 100644 1111 2222 R100 new.rs\told.rs
u UU N... 100644 100644 100644 100644 3333 4444 5555 conflict.rs
? untracked.rs
? also-untracked.rs
! ignored.rs
";

    #[test]
    fn a_file_can_be_staged_and_unstaged_at_once() {
        let s = parse_status(PORCELAIN);
        assert_eq!(s.branch.as_deref(), Some("feature/inventory"));
        assert_eq!(s.upstream.as_deref(), Some("origin/feature/inventory"));
        assert_eq!(s.head.as_deref(), Some("8f2a1c9e4b7d3a5f6c8e0b2d4f6a8c0e2b4d6f80"));
        assert_eq!((s.ahead, s.behind), (2, 3));
        // staged-only, both, and the rename.
        assert_eq!(s.staged, 3, "index column");
        // unstaged-only and both.
        assert_eq!(s.unstaged, 2, "worktree column");
        assert_eq!(s.untracked, 2);
        assert_eq!(s.conflicted, 1);
        // The ignored file is not a change.
        assert!(s.has_local_changes());
    }

    #[test]
    fn a_fresh_repository_has_no_head_and_a_detached_one_has_no_branch() {
        let initial = parse_status("# branch.oid (initial)\n# branch.head main\n");
        assert_eq!(initial.head, None);
        assert_eq!(initial.branch.as_deref(), Some("main"));
        assert!(!initial.has_local_changes());

        let detached = parse_status("# branch.oid abc123\n# branch.head (detached)\n");
        assert_eq!(detached.branch, None);
        assert_eq!(detached.head.as_deref(), Some("abc123"));
        // No `branch.ab` at all, which must read as zero rather than panicking.
        assert_eq!((detached.ahead, detached.behind), (0, 0));
    }

    #[test]
    fn the_commit_line_survives_a_subject_full_of_punctuation() {
        let line = "9c1f\u{1f}1717171717\u{1f}Andreas Wirén\u{1f}fix: don't split on | or \t";
        let c = parse_last_commit(line).expect("parsed");
        assert_eq!(c.id, "9c1f");
        assert_eq!(c.author, "Andreas Wirén");
        assert_eq!(c.summary, "fix: don't split on | or \t");
        assert_eq!(c.at.timestamp(), 1_717_171_717);

        // An empty repository logs nothing at all.
        assert_eq!(parse_last_commit(""), None);
        assert_eq!(parse_last_commit("\n"), None);
    }

    #[test]
    fn remotes_come_from_config_rather_than_the_doubled_v_output() {
        let remotes = parse_remotes(
            "remote.origin.url git@github.com:andreaswiren/superbackup.git\n\
             remote.upstream.url https://gitea.example.com/team/superbackup\n\
             remote.broken.url \n\
             not-a-remote-line\n",
        );
        assert_eq!(
            remotes,
            vec![
                Remote {
                    name: "origin".into(),
                    url: "git@github.com:andreaswiren/superbackup.git".into()
                },
                Remote {
                    name: "upstream".into(),
                    url: "https://gitea.example.com/team/superbackup".into()
                },
            ]
        );
    }

    /// A tag sharing a branch's name adds `refs/tags/<name>` and
    /// `refs/tags/<name>^{}` to the same listing. Matching on a prefix would
    /// return the tag's object and report a pull that is not needed.
    #[test]
    fn a_ref_is_matched_whole_and_never_by_prefix() {
        let out = "aaa\trefs/heads/main\n\
                   bbb\trefs/heads/main-2\n\
                   ccc\trefs/tags/main\n\
                   ddd\trefs/tags/main^{}\n";
        assert_eq!(parse_ls_remote(out, "refs/heads/main").as_deref(), Some("aaa"));
        assert_eq!(parse_ls_remote(out, "refs/heads/main-2").as_deref(), Some("bbb"));
        assert_eq!(parse_ls_remote(out, "refs/heads/absent"), None);
    }

    #[test]
    fn every_shape_of_remote_url_git_accepts() {
        let cases = [
            ("https://github.com/andreaswiren/superbackup.git", "github.com", "andreaswiren/superbackup"),
            ("https://github.com/andreaswiren/superbackup", "github.com", "andreaswiren/superbackup"),
            ("git@github.com:andreaswiren/superbackup.git", "github.com", "andreaswiren/superbackup"),
            ("ssh://git@gitea.example.com:2222/team/thing.git", "gitea.example.com", "team/thing"),
            ("http://gitea.example.com:3000/team/thing", "gitea.example.com", "team/thing"),
            ("https://GitLab.com/group/sub/project.git", "gitlab.com", "group/sub/project"),
            (
                "https://org@dev.azure.com/org/project/_git/repo",
                "dev.azure.com",
                "org/project/_git/repo",
            ),
        ];
        for (url, host, path) in cases {
            let parsed = parse_remote_url(url).unwrap_or_else(|| panic!("{url} did not parse"));
            assert_eq!(parsed.host, host, "{url}");
            assert_eq!(parsed.path, path, "{url}");
        }
    }

    /// A repository whose remote is another folder has no forge, and saying it
    /// is hosted on `C` would be worse than saying nothing.
    #[test]
    fn a_local_path_is_not_a_host() {
        for local in [
            "C:\\repos\\thing",
            "/srv/git/thing.git",
            "../sibling",
            "./mirror",
            "file:///srv/git/thing",
            "",
        ] {
            assert_eq!(parse_remote_url(local), None, "{local} is not hosted anywhere");
        }
    }

    #[test]
    fn owner_and_repo_are_only_claimed_where_they_mean_that() {
        let gh = parse_remote_url("https://github.com/andreaswiren/superbackup").expect("gh");
        assert_eq!(gh.owner_repo(), Some(("andreaswiren", "superbackup")));

        // Azure's path names an organisation, a project and a repository, so
        // the last two segments are not an owner and a repo.
        let az = parse_remote_url("https://dev.azure.com/org/project/_git/repo").expect("az");
        assert_eq!(az.owner_repo(), None);

        // A GitLab subgroup: the owner is everything above the repository.
        let gl = parse_remote_url("https://gitlab.com/group/sub/project").expect("gl");
        assert_eq!(gl.owner_repo(), Some(("group/sub", "project")));
    }
}
