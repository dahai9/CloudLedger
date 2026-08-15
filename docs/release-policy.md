# Release channels

CloudLedger has two CI channels:

- Stable releases use a tag such as `v0.1.8` created on the matching
  `release/v0.1.8` branch. The formal workflows verify that the tag points to
  that branch's current tip before publishing the four GHCR images and a GitHub
  Release APK.
- Commits pushed to `main` run the alpha workflow. It publishes only the
  mutable `alpha` and immutable `alpha-<short-sha>` GHCR tags and uploads an
  alpha APK artifact. It never writes `latest` or a formal GitHub Release.

Alpha images are for testing and are intentionally not accepted by the
production operations toolbox, which requires a stable semantic version tag.
