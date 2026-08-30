//! Turning what the user typed into the thing they meant.
//!
//! A person types `dev`. There is one job called `dev-projects`, so that is
//! the one. There are two, so it is **an error** — never a guess, never "the
//! first one", never "the closest". This is a backup tool: choosing the wrong
//! job silently means restoring the wrong folder over the right one, or
//! deleting a job the user still needed. The whole point of a prefix is speed,
//! and speed is worth nothing if it is occasionally wrong.
//!
//! Resolution happens in the client rather than being left to the daemon.
//! `job.get` accepts a prefix too, but its contract is only "a unique prefix",
//! and a client that cannot see the candidate list cannot tell the user which
//! ones collided.

use uuid::Uuid;

use superbackup_core::error::ErrorCode;
use superbackup_core::model::{Destination, Job, StorageProvider};

use super::output::{CliError, CliResult};

/// Something a user can name.
pub trait Entity {
    fn entity_id(&self) -> Uuid;
    fn entity_name(&self) -> &str;
}

impl Entity for Job {
    fn entity_id(&self) -> Uuid {
        self.id
    }
    fn entity_name(&self) -> &str {
        &self.name
    }
}

impl Entity for Destination {
    fn entity_id(&self) -> Uuid {
        self.id
    }
    fn entity_name(&self) -> &str {
        &self.name
    }
}

impl Entity for StorageProvider {
    fn entity_id(&self) -> Uuid {
        self.id
    }
    fn entity_name(&self) -> &str {
        &self.name
    }
}

/// Which noun is being resolved, for the error message and its code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Job,
    Destination,
    Provider,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Job => "job",
            Kind::Destination => "destination",
            Kind::Provider => "provider",
        }
    }

    /// `job_not_found` exists as a distinct code precisely so a caller can
    /// tell "no such job" from "that argument made no sense".
    fn missing_code(self) -> ErrorCode {
        match self {
            Kind::Job => ErrorCode::JobNotFound,
            _ => ErrorCode::Validation,
        }
    }

    fn list_command(self) -> &'static str {
        match self {
            Kind::Job => "superbackup job list",
            Kind::Destination => "superbackup destination list",
            Kind::Provider => "superbackup provider list",
        }
    }
}

/// Find the single thing `needle` names.
///
/// In order: exact id, exact name, case-insensitive exact name, unique
/// case-insensitive name prefix, unique id prefix. Each stage is tried in
/// full before the next, so an exact name always beats a prefix of a longer
/// one — without that, a job called `docs` becomes unreachable the moment
/// somebody adds `docs-archive`.
pub fn one<'a, T: Entity>(needle: &str, items: &'a [T], kind: Kind) -> CliResult<&'a T> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Err(CliError::usage(format!("no {} was named", kind.label())));
    }

    if let Ok(id) = Uuid::parse_str(needle) {
        if let Some(found) = items.iter().find(|i| i.entity_id() == id) {
            return Ok(found);
        }
        return Err(not_found(needle, items, kind));
    }

    if let Some(found) = items.iter().find(|i| i.entity_name() == needle) {
        return Ok(found);
    }

    let lower = needle.to_lowercase();
    let exact_ci: Vec<&T> =
        items.iter().filter(|i| i.entity_name().to_lowercase() == lower).collect();
    match exact_ci.len() {
        1 => return Ok(exact_ci[0]),
        0 => {}
        _ => return Err(ambiguous(needle, &exact_ci, kind)),
    }

    let by_prefix: Vec<&T> =
        items.iter().filter(|i| i.entity_name().to_lowercase().starts_with(&lower)).collect();
    match by_prefix.len() {
        1 => return Ok(by_prefix[0]),
        0 => {}
        _ => return Err(ambiguous(needle, &by_prefix, kind)),
    }

    // An id prefix, for someone pasting the first block of a UUID out of
    // `--json` output. Short prefixes are refused: `1` matching one job today
    // and two tomorrow is the ambiguity this module exists to prevent.
    if needle.len() >= 8 {
        let by_id: Vec<&T> = items
            .iter()
            .filter(|i| i.entity_id().simple().to_string().starts_with(&lower))
            .collect();
        match by_id.len() {
            1 => return Ok(by_id[0]),
            0 => {}
            _ => return Err(ambiguous(needle, &by_id, kind)),
        }
    }

    Err(not_found(needle, items, kind))
}

/// Resolve a repeatable option, keeping the order the user gave and rejecting
/// the whole list if any one entry is wrong.
///
/// All-or-nothing on purpose: a `run --destination a --destination typo` that
/// quietly backed up to `a` only would look like it did what was asked.
pub fn many<'a, T: Entity>(
    needles: &[String],
    items: &'a [T],
    kind: Kind,
) -> CliResult<Vec<&'a T>> {
    let mut out = Vec::with_capacity(needles.len());
    for needle in needles {
        out.push(one(needle, items, kind)?);
    }
    Ok(out)
}

fn ambiguous<T: Entity>(needle: &str, matches: &[&T], kind: Kind) -> CliError {
    let mut names: Vec<String> = matches.iter().map(|m| m.entity_name().to_string()).collect();
    names.sort();
    CliError::new(
        ErrorCode::Validation,
        format!(
            "`{needle}` matches {} {}s: {}",
            names.len(),
            kind.label(),
            names.join(", ")
        ),
    )
    .with_hint("Name it in full, or use its id. superbackup will not guess which one you meant.")
}

fn not_found<T: Entity>(needle: &str, items: &[T], kind: Kind) -> CliError {
    let error = CliError::new(
        kind.missing_code(),
        format!("there is no {} matching `{needle}`", kind.label()),
    );
    if items.is_empty() {
        return error.with_hint(format!("Nothing is configured yet. Run `{}`.", kind.list_command()));
    }
    // Listing every name on a machine with two hundred jobs is not help.
    let mut names: Vec<String> = items.iter().map(|i| i.entity_name().to_string()).collect();
    names.sort();
    if names.len() <= 10 {
        error.with_hint(format!("Known {}s: {}", kind.label(), names.join(", ")))
    } else {
        error.with_hint(format!("Run `{}` to see the {} names.", kind.list_command(), names.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Thing {
        id: Uuid,
        name: String,
    }

    impl Entity for Thing {
        fn entity_id(&self) -> Uuid {
            self.id
        }
        fn entity_name(&self) -> &str {
            &self.name
        }
    }

    fn things(names: &[&str]) -> Vec<Thing> {
        names.iter().map(|n| Thing { id: Uuid::new_v4(), name: (*n).to_string() }).collect()
    }

    #[test]
    fn a_unique_prefix_resolves() {
        let items = things(&["dev-projects", "photos"]);
        assert_eq!(one("dev", &items, Kind::Job).expect("resolves").name, "dev-projects");
        assert_eq!(one("DEV", &items, Kind::Job).expect("case insensitive").name, "dev-projects");
    }

    #[test]
    fn an_ambiguous_prefix_is_an_error_that_lists_the_candidates() {
        let items = things(&["docs", "documents", "photos"]);
        let error = one("doc", &items, Kind::Job).err().expect("must refuse to guess");
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(error.exit_code(), crate::cli::exit::USAGE);
        assert!(error.message.contains("docs"), "{}", error.message);
        assert!(error.message.contains("documents"), "{}", error.message);
        assert!(error.message.contains("matches 2"), "{}", error.message);
    }

    #[test]
    fn an_exact_name_beats_a_longer_one_that_starts_with_it() {
        // Without this rule, adding `docs-archive` makes `docs` unreachable.
        let items = things(&["docs", "docs-archive"]);
        assert_eq!(one("docs", &items, Kind::Job).expect("exact wins").name, "docs");
    }

    #[test]
    fn an_id_resolves_and_a_wrong_id_does_not_fall_through_to_a_name() {
        let items = things(&["docs"]);
        let id = items[0].id;
        assert_eq!(one(&id.to_string(), &items, Kind::Job).expect("by id").name, "docs");
        let other = Uuid::new_v4().to_string();
        assert!(one(&other, &items, Kind::Job).is_err());
    }

    #[test]
    fn an_unknown_name_reports_job_not_found_and_suggests_the_known_ones() {
        let items = things(&["docs", "photos"]);
        let error = one("nope", &items, Kind::Job).err().expect("must fail");
        assert_eq!(error.code, ErrorCode::JobNotFound);
        let hint = error.hint.unwrap_or_default();
        assert!(hint.contains("docs") && hint.contains("photos"), "{hint}");
    }

    #[test]
    fn an_empty_configuration_says_so_rather_than_listing_nothing() {
        let items: Vec<Thing> = Vec::new();
        let error = one("docs", &items, Kind::Destination).err().expect("must fail");
        assert_eq!(error.code, ErrorCode::Validation);
        assert!(error.hint.unwrap_or_default().contains("Nothing is configured"));
    }

    #[test]
    fn a_repeatable_option_fails_as_a_whole_when_one_entry_is_wrong() {
        let items = things(&["local", "offsite"]);
        let asked = vec!["local".to_string(), "typo".to_string()];
        assert!(many(&asked, &items, Kind::Destination).is_err());
        let good = vec!["offsite".to_string(), "local".to_string()];
        let resolved = many(&good, &items, Kind::Destination).expect("both resolve");
        assert_eq!(resolved[0].name, "offsite", "order is preserved");
    }
}
