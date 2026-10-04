# Plugin update ordering

The native Plugin updates panel requires the proxy's `upgrade_allowed: true`
decision before offering an update or reinstall. A missing decision, including
an older proxy that does not support this contract, disables the button. When
available, `upgrade_block_reason` explains why the candidate was rejected.

Every panel install sends the exact version, the existing source query, and
`upgrade_only: true`. The proxy must recheck the installed revision and selected
artifact before download and activation. A changed version string alone does not
prove a release is newer; reviewed commit suffixes cannot be ordered as versions.

The registry and installed manifest may contain positive `revision` values. The
proxy exposes `revision` and `installed_revision` for diagnostics; the GUI does not
compare these JavaScript numbers or infer artifact equality. The backend handles
unknown legacy state, exact reinstalls, and revision floors. Manual rollback
outside this default panel remains an explicit management API operation.

Source matching, already-enabled requirements, platform checks, lifecycle locking
and asynchronous runtime reload verification are unchanged. An install response
alone does not mean that the new library is active.
