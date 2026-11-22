# Releasing tvpn

Releases are fully automated by GitHub Actions.

## Cut a release

1. Bump `version` in `Cargo.toml` (and run `cargo check` so `Cargo.lock` updates).
2. Move the "Unreleased" notes in `CHANGELOG.md` under a new `## [X.Y.Z] - YYYY-MM-DD` heading.
3. Commit, then tag and push:

   ```sh
   git tag vX.Y.Z
   git push origin main vX.Y.Z
   ```

The **Release** workflow then:

1. checks that the tag matches `Cargo.toml` and that `CHANGELOG.md` has the section;
2. builds static binaries for 6 targets (Linux musl x86_64/arm64, macOS
   x86_64/arm64, Windows x64/arm64; Windows zips include `wintun.dll`);
3. builds pip wheels (manylinux, musllinux, macOS, Windows) and an sdist;
4. writes `SHA256SUMS`, dry-runs the npm packages, renders the Homebrew
   formula and winget manifests;
5. creates the GitHub release with all assets and notes from the changelog;
6. calls the **Publish** workflow.

Tags with a suffix (e.g. `v0.2.0-rc.1`) become pre-releases.

Pull requests that touch packaging run steps 1–4 as a dry run.

## Registry tokens (one-time setup)

Add these under **Settings → Secrets and variables → Actions → New repository secret**.
A channel without its secret is skipped (the run summary says so), so you can
enable them one at a time.

| Channel | Secret | How to get it |
|---|---|---|
| npm | `NPM_TOKEN` | npmjs.com → Access Tokens → Generate New Token → *Granular*, read & write, "All packages" (the `tvpn` and `tvpn-<platform>-<arch>` packages are created on first publish). |
| PyPI | `PYPI_API_TOKEN` | pypi.org → Account settings → API tokens → scope "Entire account" for the first upload, then replace it with a token scoped to the `tvpn` project. |
| crates.io | `CARGO_REGISTRY_TOKEN` | crates.io → Account Settings → API Tokens → scopes `publish-new` and `publish-update`. |
| winget | `WINGET_TOKEN` | GitHub → Settings → Developer settings → Personal access tokens (classic) → scope `public_repo`. Used to open a PR on `microsoft/winget-pkgs` from your fork. |
| Homebrew | *(none)* | The formula is committed to `Formula/tvpn.rb` in this repo, which acts as its own tap. |

## Re-publishing an existing release

After adding a token, publish an already-released tag without rebuilding:
**Actions → Publish → Run workflow**, enter the tag (e.g. `v0.1.0`), and
optionally restrict `channels` (e.g. `npm,pypi`). Already-published versions
are skipped, so re-running is safe.

## Notes

- winget submissions are reviewed by Microsoft. The first one usually takes a
  few days. Later versions go through the same PR flow.
- npm packages are published with provenance (signed build attestations).
- If branch protection is enabled on `main`, allow GitHub Actions to push, or
  the Homebrew formula commit will fail.
