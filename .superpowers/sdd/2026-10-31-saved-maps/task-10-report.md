# Task 10 end-to-end verification fixes

## Fixes applied

- Scoped `dead_code` allowances to the native-only session module and the wasm/native helper functions that are intentionally unused on one target. No crate-wide allowance was added.
- Removed the unused `Store::remove` method from the trait and both implementations; map deletion remains in the index page.
- Updated the desktop hint to include `Ctrl+Y` as a redo shortcut.
- Changed corrupt-session parse failures to log serde details with `bevy::log::warn!` while showing `The saved map could not be read.` to the user. Other load error messages are unchanged.

## Verification

- `cd game && cargo test -p party-dungeon` — passed: 146 tests, 0 failed; no `dead_code` warnings.
- `cd game && ./scripts/build-web.sh` — succeeded; wasm release build and web package generation completed with no warnings. The final build took 7m 38s; earlier runs were stopped by the command timeout while compilation was still running.
- `git diff --check` — clean.
