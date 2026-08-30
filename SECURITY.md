# Security Policy

superbackup holds the keys that make its user's backups recoverable. Security
reports are treated as the highest-priority class of issue.

## Reporting a vulnerability

**Please do not open a public issue.**

Use GitHub's private vulnerability reporting on this repository
(Security → Report a vulnerability), which creates a private advisory visible
only to the maintainers.

Please include:

- what an attacker gains, and what access they need to start
- the affected version (`superbackup version`) and platform
- reproduction steps, or a proof of concept
- anything you already know about the fix

You will get an acknowledgement within 5 working days and an assessment within
10. If a report is valid, you will be kept informed until a fix ships, and
credited in the advisory and the changelog unless you prefer otherwise.

This is a personal open-source project rather than a funded programme: there is
no bug bounty, and timelines depend on maintainer availability. That is stated
up front rather than implied.

## Scope

**In scope** — anything that breaks the guarantees in
[`docs/compliance/THREAT_MODEL.md`](docs/compliance/THREAT_MODEL.md), in
particular:

- extracting secrets from the sealed vault without the master passphrase
- a secret reaching a log, an event, a notification, `argv`, an IPC response,
  or any file outside the vault
- another local user reading secrets or driving the IPC endpoint
- a malicious shared-config repository changing local behaviour without the
  documented decrypt-and-verify step
- path traversal or writes outside a destination root in the mirror engine
- a crash, hang, or memory exhaustion triggerable by malformed input — a
  corrupt vault, hostile kopia output, or a hostile IPC client
- privilege escalation via the service installer or the autostart entry

**Out of scope** — the exclusions in §3 of the threat model, and specifically:

- attacks requiring malware already running as the user with the vault unlocked
- attacks requiring administrator, root, or physical access
- a user choosing a weak master passphrase (though a flaw in how strength is
  *measured* or enforced is in scope)
- that `LocalMirror` destinations are unencrypted — this is documented,
  intentional, and stated in the UI at the point of choice
- vulnerabilities in Kopia itself: report those to
  [the Kopia project](https://github.com/kopia/kopia/security), though we would
  appreciate a heads-up so we can raise our minimum supported version

## Supported versions

During 0.x, only the latest release receives security fixes.

## Disclosure

Coordinated disclosure. We aim to ship a fix before details are published, and
will agree a date with the reporter. If a vulnerability is being exploited, we
will publish mitigation guidance immediately rather than wait for a fix.

## What we will not do

We will not ask you to sign anything to report a bug, and we will not treat a
good-faith report as a hostile act.
