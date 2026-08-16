# Formal Release Notes

Every stable release must add `docs/releases/vX.Y.Z.md` to its matching
`release/vX.Y.Z` branch before creating the `vX.Y.Z` tag. The Android release
workflow rejects a tag without this file, a matching `# CloudLedger vX.Y.Z`
title, at least one `##` section, and non-heading body text.

Write for operators and end users. State the concrete updates, fixes,
security-impacting changes, migration or upgrade requirements, and the
validation performed. Do not rely on generated commit lists as the release
description. Never put credentials, internal endpoints, or personal data in
release notes.

Use this outline:

```markdown
# CloudLedger vX.Y.Z

## Updates

- Describe user-visible behavior and its impact.

## Upgrade Notes

- State migrations, compatibility constraints, or `None`.

## Validation

- List the CI, deployment, and recovery checks that passed.
```
