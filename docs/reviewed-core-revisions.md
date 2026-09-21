# Reviewed core channel revisions

A cached channel manifest must not offer an older core as a new update. Commit
IDs and semantic prerelease tags do not express publication order after rebases.

Reviewed manifests may include `revision`, a positive unsigned integer. Use a
strictly increasing publication sequence, such as the release's UTC Unix timestamp
in seconds. A different release needs a greater revision even if the version's
commit suffix sorts before the installed one. Publish a unique tag for different
bytes. Schema version remains 1; older apps that reject the new field must be
updated before publishing a revisioned manifest to their channel.

The installed `reviewed-core.json` also records `revision`. Both update checks and
installation compare it against the manifest after verifying that provenance
matches the installed binary. A different artifact with a missing, equal or lower
revision is rejected before download or stopping the current core. The UI uses
the backend decision. Catalog checks and rollback still run for allowed updates.

Reinstalling the identical repository, tag, commit and archive hash is allowed.
It preserves the greater of the installed and advertised revisions. This allows
an exact current release to seed legacy provenance without lowering a known floor.
Changing a reviewed artifact when installed provenance has no revision is blocked.
An administrator may instead seed that record only after verifying the installed
commit and binary hash against its reviewed release. Missing, unreadable or
mismatched provenance for an identified reviewed installation must be repaired;
it does not reset the revision floor.

The first switch from an official core requires a revisioned manifest. Changing
the manifest URL within the same repository does not reset the floor. To trust a
different repository, explicitly select its distinct manifest URL; that source
starts its own revision sequence. Official updates remain unchanged when the
reviewed channel is disabled. This guard covers the core, not native plugin
registries or app releases.
