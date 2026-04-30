# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |

## Reporting a Vulnerability

**Do not open a public issue for security vulnerabilities.**

Email **mingyel2@illinois.edu** with:

- A description of the vulnerability and its impact.
- Steps to reproduce or a proof of concept.
- The version of acrawl affected.
- Any suggested fix, if you have one.

We will acknowledge your report within 48 hours and aim to release a fix within 90 days of confirmation.

## Security Considerations

**Credential storage.** LLM provider credentials (API keys, OAuth tokens) are stored in plaintext in `~/.acrawl/credentials.json`. Protect this file with appropriate filesystem permissions. Do not commit it to version control.

**Browser automation.** acrawl runs a headless Chromium instance that navigates to, renders, and interacts with web pages. Be aware of what sites and content you direct it toward — the browser executes JavaScript on those pages.

**Permission model.** acrawl enforces a three-tier permission model (`read-only`, `workspace-write`, `danger-full-access`) that gates which tools the LLM agent can invoke. Use the most restrictive mode that meets your needs.
