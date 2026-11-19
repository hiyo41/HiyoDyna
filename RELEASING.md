# Releasing

Official releases are created manually from `main` and require write access.

1. Confirm CI passes on `main`.
2. Run the Release workflow from the Actions tab, or use:

```powershell
gh workflow run Release
```

3. The workflow reads the version from `Cargo.toml`, builds the Windows x64 zip plus raw `HiyoDyna.exe` and `HiyoDyna.exe.sha256`, creates an official `v<version>-<run>` tag, and publishes an official release.
4. Verify the release, its raw assets, and its changelog on the Releases page.

Pre-release notes list only the commits since the previous pre-release. Official notes list every commit since the previous official release, including changes already shipped in pre-release builds.

Official tags are `v<Cargo version>-<run number>`. The workflow rejects a build number that is not newer than an existing official tag, so re-running an old workflow cannot publish a downgrade. The in-app Stable channel reads the latest official release; Build reads pre-releases from `main`.

Every push to `main` publishes a pre-release build or refreshes its changelog. Pre-releases never become the latest official release.
