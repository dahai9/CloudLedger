# Release channels

CloudLedger has two CI channels:

- Stable releases use a tag such as `v0.1.8` created on the matching
  `release/v0.1.8` branch. The formal workflows verify that the tag points to
  that branch's current tip before publishing the four GHCR images and a GitHub
  Release APK.
- Commits pushed to `main` run the alpha workflow. The workflow chooses the
  next prerelease tag from the app base version, starting with
  `v0.1.8.alpha.1`, and publishes that tag, the mutable `alpha` tag, and the
  immutable `alpha-<short-sha>` tag for all four GHCR images. It also creates a
  GitHub prerelease with the signed alpha APK.

Alpha tags are excluded from the stable workflows and are intended for testing;
formal production releases still require a stable semantic version tag.
