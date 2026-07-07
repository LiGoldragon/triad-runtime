# Non-ideal agent guidance — triad-runtime

This file names required temporary debt. Treat each item as a future fix target, not as a pattern to copy.

- `MultiListenerDaemon` is the legacy synchronous ordinary/meta listener shell. It remains only for consumers that have not yet migrated to `AsyncMultiListenerDaemon`; new schema-emitted daemon work must not target the polling shell.
