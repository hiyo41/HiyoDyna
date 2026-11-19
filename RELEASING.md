# Releasing

Official releases are created manually from `main` and require write access.

1. Confirm CI passes on `main`.
2. Run the Release workflow from the Actions tab, or use:

```powershell
gh workflow run Release
```

3. The workflow reads the version from `Cargo.toml`, builds the Windows x64 zip, creates the tag, and publishes an official release.
4. Verify the release and its changelog on the Releases page.

Every push to `main` publishes a pre-release build or refreshes its changelog. Pre-releases never become the latest official release.
