# Signing and verifying releases

Posture for 1.0 (D-032): the checksum file is signed without a key, binary
code signing is wired up but waits for the publisher's accounts, and users
verify downloads with checksums and Sigstore.

## What a release contains

| File | Produced by | Signed? |
|---|---|---|
| `openmapper-<version>-<os>-<arch>.tar.gz` (one per OS) | `cargo xtask dist` in `package.yml` | Binaries unsigned until the accounts below exist |
| `ffmpeg-<tag>-src.tar.gz` | Linux package job | Covered by `SHA256SUMS` |
| `SHA256SUMS` | `publish.yml` (called by `release.yml`), merged from the three OS jobs | **Yes**: `SHA256SUMS.sigstore.json` |
| `PROVENANCE.txt` | `cargo xtask provenance --history` | Covered by the release page only |

## Verifying a download (users)

```
sha256sum -c SHA256SUMS            # macOS: shasum -a 256 -c SHA256SUMS
cosign verify-blob --bundle SHA256SUMS.sigstore.json \
  --certificate-identity https://github.com/bailo167/openmapper/.github/workflows/publish.yml@refs/tags/vX.Y.Z \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  SHA256SUMS
```

`cosign` is a single binary from <https://github.com/sigstore/cosign>. A
successful verification proves the checksum file was produced by this
repository's release pipeline (the `publish.yml` job, called by `release.yml`) at that tag, recorded in the public Sigstore
transparency log; the checksums then cover every archive.

### Running unsigned binaries

- **macOS**: an archive downloaded with a browser is quarantined, and
  Gatekeeper refuses unsigned, un-notarised programs. Either download with
  `curl -LO …` (no quarantine), or after unpacking run
  `xattr -dr com.apple.quarantine openmapper-*/`, or start the app once and
  allow it under System Settings ▸ Privacy & Security ▸ *Open Anyway*. Apple
  Silicon binaries carry the linker's ad-hoc signature, which is all macOS
  needs to execute them once quarantine is cleared.
- **Windows**: SmartScreen shows "Windows protected your PC" for programs
  without a reputation; choose *More info* ▸ *Run anyway*. Verifying
  `SHA256SUMS` first is the substitute for a publisher certificate.
- **Linux**: nothing to do.

## Activating binary code signing (owner)

Everything below is optional for 1.0 and needs money or an account. Once a
secret exists, `package.yml` runs the matching script automatically for
every package and release build; nothing else changes. The scripts have not
been exercised against real credentials: expect to run the `release`
workflow by hand once and fix what Apple or signtool complain about.

### macOS — Developer ID and notarisation

1. Join the Apple Developer Program (US$99/year) and create a **Developer
   ID Application** certificate in Xcode or at developer.apple.com. Export
   it with its private key as a `.p12`.
2. Create an **app-specific password** for the Apple ID at appleid.apple.com.
3. Add repository secrets:

   | Secret | Value |
   |---|---|
   | `APPLE_CERTIFICATE_P12` | `base64 -i certificate.p12` |
   | `APPLE_CERTIFICATE_PASSWORD` | the `.p12` password |
   | `APPLE_SIGNING_IDENTITY` | `Developer ID Application: <Name> (<TEAMID>)` |
   | `APPLE_ID` | the Apple ID e-mail |
   | `APPLE_TEAM_ID` | the 10-character team id |
   | `APPLE_APP_PASSWORD` | the app-specific password |

   `.github/scripts/sign-macos.sh` then signs the FFmpeg libraries and both
   executables with the hardened runtime (plus the JIT entitlements the
   WebAssembly plugin host needs, `entitlements.plist`) and submits the
   staged folder to `notarytool`. Command-line tools cannot be stapled, so
   Gatekeeper checks the notarisation online on first launch.
4. Follow-up once this works: an `OpenMapper.app` bundle and `.dmg`, which
   can be stapled and double-clicked (D-030). The executable already embeds
   the bundle identifier and privacy usage descriptions (`apps/openmapper/
   macos/Info.plist`).

### Windows — Authenticode

1. Obtain a code-signing certificate: an OV certificate from a CA (a
   hardware token is now mandatory, so a CI-friendly option is **Azure
   Trusted Signing**, roughly US$10/month, or a cloud HSM-backed
   certificate), or an EV certificate for immediate SmartScreen reputation.
2. For a `.pfx` file, add `WINDOWS_CERTIFICATE_PFX` (base64) and
   `WINDOWS_CERTIFICATE_PASSWORD`; `.github/scripts/sign-windows.ps1` signs
   both executables with SHA-256 and an RFC 3161 timestamp. For Azure
   Trusted Signing, replace the script body with the
   `azure/trusted-signing-action` step and keep the same `OM_SIGN_COMMAND`
   hook in `package.yml`.
3. SmartScreen reputation for OV certificates builds with downloads over
   weeks; the warning does not vanish on the first signed release.

### A long-term release key (optional)

Keyless Sigstore signing is tied to GitHub. If the project ever wants a
signature independent of GitHub, generate a minisign or GPG key offline,
publish the public key in the repository and documentation, and add a step
in `release.yml` that signs `SHA256SUMS` with the private key from a secret.
Nothing else changes.
