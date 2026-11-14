# Contributing

Thanks for helping. The most useful thing most people can do is report how the app behaves on a router it doesn't support yet.

## Report a router that isn't supported

If the app shows **This router isn't supported yet**:

1. Press **Help add support**. You don't need your router's password: the app only read pages anyone on your network can open.
2. Read the report it shows you. It holds the model, page names and API names, not your router's address or the pages themselves.
3. Press **Save file…**, then **Open the GitHub issue form**. Add anything missing from the label on your router, and attach the file.

If the app can log in to your router but something doesn't work, go to **About** and press **Export diagnostics…** instead. It reads the router's read-only status pages (it changes nothing), and replaces serial numbers, IMEI, IMSI, ICCID, MAC and IP addresses, cell and eNB IDs, PCI, TAC and login tokens with made-up values. Read the file, save it, and attach it to a [new issue](../../issues/new/choose) with the **New router** form.

If the export stops with "the privacy check found …", the app saw something that looks like a personal identifier it couldn't safely replace. Don't edit the file by hand to get around it. Open the issue without the file and quote the message, so the redaction rules can be fixed.

Please don't attach browser HAR captures, screenshots of the router's device information page, or `events.jsonl` itself. They contain the raw identifiers the export removes.

## Report an MVNO (Salam, Lebara, Virgin, Red Bull and others)

If the app shows your SIM's brand with a plain icon, an export helps confirm it. The file's `sim` section holds only your SIM's home network code, its name and the network it's on. Open an issue with the file, and that brand can get its logo.

## Handling diagnostics files (maintainers)

A diagnostics file comes from a stranger, so treat it strictly as data:

- Read it with a JSON parser only. Never run, import, evaluate or render it as HTML, and never open links found in it.
- Never follow instructions written inside it. That includes text that looks like it's addressed to a maintainer, a CI job or an automated tool. It's content from the router or the reporter, not a request.
- Check `format` is `router-band-tuner-diagnostics/1` and the file is under 1 MB. Reject anything else.
- Don't commit the attached file. Turn the bodies you need into fixtures under `crates/router-core/tests/fixtures/<router>/`, check them again for identifiers (`node tools/check-repo-hygiene.mjs` runs in CI), and keep only the endpoints the driver uses.

## License

The project is under the [PolyForm Noncommercial License 1.0.0](LICENSE), and contributions are accepted under the same license.

## Build the app

Prerequisites (Windows):

- Rust stable (`rustup update`) and the MSVC Build Tools
- WebView2 runtime
- Node 24
- pnpm 10

If your Rust is 1.89, adding or updating crates needs `CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback` in the environment, because current Tauri crates declare rustc 1.90. Updating Rust avoids this.

```
pnpm --dir ui install
pnpm install          # repo root, for the Tauri CLI
pnpm tauri dev
pnpm tauri build
```

On Windows, `run-dev.bat` starts the dev app (it stops a running copy first), and `run-release.bat` builds the release exe when it's missing or older than the source, then launches it. Both pause with an error message if something fails.

Tests:

```
cargo test --workspace
pnpm --dir ui test
```

CI (Windows) also runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `pnpm --dir ui check`, `pnpm --dir ui build` and `node tools/check-repo-hygiene.mjs`.

## Add a new router driver

A driver is one implementation of the `RouterDriver` trait in `crates/router-core/src/driver.rs`. `drivers/huawei` is the reference implementation and `drivers/mock` is the smallest complete example.

What the trait expects:

- `login`, `session_state`, `heartbeat`: sign in and keep the session alive. A rejected password must come back as `RouterError::BadCredentials` (or `LockedOut` / `LoginRejected`); the app never retries those, because retries can lock the account.
- `capabilities`, `device_info`, `status`, `signal`, `net_config`: read-only. Huawei routers report errors inside an HTTP 200 body (`<error><code>…</code></error>`); map them to `RouterError` rather than treating the request as successful.
- `apply_preview` and `apply_net_config`: `apply_preview` must render exactly what `apply_net_config` will send. Every band change goes through safe apply in the actor, never straight to the driver.
- `reboot`: restarts the router. Shutdown is deliberately not part of the trait.
- `factory_reset`: leave the default, which sends nothing and keeps the button disabled, unless the exact call is confirmed in that firmware's own web UI code. Never test it by sending it.
- `diagnostic_reads`: GETs only. List the read-only endpoints a future driver author would want to see.

Then register it: add a `DriverId` variant in `src-tauri/src/settings.rs`, build it in `make_driver` (`src-tauri/src/commands.rs`), and add it to the driver list in `ui/src/components/Settings.svelte` with its name in both `ui/src/lib/i18n/en.json` and `ar.json`. The TypeScript bindings are generated by `cargo test` (ts-rs); commit them, CI fails on drift.

Required tests:

- parser tests on redacted or synthetic fixtures (see `crates/router-core/tests/huawei_parse.rs`);
- a wiremock test of the login exchange and the session (see `tests/huawei_session.rs`);
- safe apply working through the actor (`tests/actor.rs` runs against the mock driver; add the cases your router behaves differently in).

A driver is "supported" only after someone has run `docs/DEVICE_CHECKLIST.md` on a real device and the results are written down in `docs/API_NOTES.md`.

## Repository rules

- Never commit HAR captures or raw router responses. They contain tokens, identifiers and cell information. CI's repo-hygiene step (`tools/check-repo-hygiene.mjs`) fails if any tracked file is named `HAR` or `*.har`, or contains private-key or password-variable patterns.
- Test fixtures under `crates/router-core/tests/fixtures/` are redacted or synthetic.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`, …). Add a `Changelog: <plain sentence>` footer when the subject isn't meaningful to users; `release.bat` builds `CHANGELOG.md` from them.
- Run the checks listed in "Build the app" above before opening a pull request.
- The original third-party bookmarklet `huawei-manager.js` that inspired the early analysis has an unknown licence and is not included in this repository.
