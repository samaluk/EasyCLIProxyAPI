# Reviewed core updates

A patched core can lose required model metadata or plugin behavior when the official updater replaces it. The optional reviewed channel points the existing core update button at a publisher's manifest and checks compatibility before accepting the new binary. Official releases remain the default.

In Version management, enter the HTTPS manifest URL and select **Use reviewed channel**. Start the existing core, check for updates, then use the core update button. Downloads use only that manifest's source. Download failure never selects an official build automatically. The publisher is trusted to review the binary; the checksum proves it matches the manifest, not that its code is safe.

A manifest has this format:

```json
{
  "schema_version": 1,
  "repository": "example/CLIProxyAPI",
  "tag": "v7.3.9-review.abc1234",
  "commit": "0123456789abcdef0123456789abcdef01234567",
  "assets": [
    {
      "os": "darwin",
      "arch": "arm64",
      "url": "https://github.com/example/CLIProxyAPI/releases/download/v7.3.9-review.abc1234/cli-proxy-api_darwin_arm64.tar.gz",
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "size": 42000000
    }
  ]
}
```

Replace the example commit, checksum and size with the actual values. Each platform occurs once. Supported OS values are `darwin`, `linux`, and `windows`; architectures are `arm64` and `amd64`. Windows uses ZIP; other platforms use tar.gz. Asset URLs must name the declared GitHub repository and exact release tag. SHA-256 and a positive size up to 512 MiB are required. The manifest rejects unknown fields.

The app downloads, hashes and extracts the candidate while the existing core runs. It snapshots the visible rich model catalog and registered plugins, stops the core, saves a unique backup of the previous binary and installation metadata, and replaces only the executable. Configuration, OAuth files and plugin binaries remain in place. The new core starts using the existing configuration without applying GUI configuration defaults. A bounded health check verifies that existing model routes and their declared limits, input/output types and reasoning options remain, and that registered plugins keep their enabled state. These checks make no inference requests. A core that intentionally changes these capabilities needs a separate reviewed configuration migration.

If replacement, startup or health validation fails, the updater stops the candidate, restores the previous binary and metadata, and restarts the previous core. If the candidate cannot be stopped or restoration fails, the error identifies the retained backup directory. Backups are retained after successful updates too. This is runtime failure recovery, not a journal for power loss or forced termination of the updater. The installer requires an already running core so it can capture a baseline without changing a stopped service.

Installed provenance records the manifest URL, declared repository/tag/commit, archive and installed-binary hashes, timestamp and backup location. The UI shows this record only while its binary hash matches the deployed executable.

This channel updates only the core. Plugins still use the plugin store and can select a reviewed custom registry there. Official app self-update and bundled-core replacement are blocked while the reviewed channel is selected because they may remove compatibility patches. macOS signature and Gatekeeper validation remain unchanged. Automatic reviewed app replacement still requires a trusted signed app distribution, or the patches to be included upstream. Selecting **Use official channel** explicitly removes this guard.
