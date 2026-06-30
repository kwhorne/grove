# macOS code signing & notarization

To ship a macOS app that opens without the *"Grove is damaged"* / Gatekeeper
warning, the `.app`/`.dmg` must be **code-signed** with a Developer ID and
**notarized** by Apple. The release workflow does this automatically once these
six GitHub secrets are set.

Requires a paid **Apple Developer Program** membership ($99/yr).

## 1. Get a Developer ID Application certificate

In [Apple Developer → Certificates](https://developer.apple.com/account/resources/certificates/list),
create a **Developer ID Application** certificate (or via Xcode → Settings →
Accounts → Manage Certificates → +). Then in **Keychain Access**, find it,
right-click → **Export** as a `.p12`, and set an export password.

Base64-encode it for GitHub:

```bash
base64 -i DeveloperID_Application.p12 | pbcopy   # now in your clipboard
```

## 2. Find your signing identity and Team ID

```bash
security find-identity -v -p codesigning
# → "Developer ID Application: Your Name (TEAMID1234)"
```

The string in quotes is `APPLE_SIGNING_IDENTITY`; the `TEAMID1234` is
`APPLE_TEAM_ID`.

## 3. Create an app-specific password (for notarization)

At [appleid.apple.com](https://appleid.apple.com) → Sign-In & Security →
App-Specific Passwords → generate one (e.g. "grove-notarize"). This is
`APPLE_PASSWORD`; your Apple ID email is `APPLE_ID`.

## 4. Set the GitHub secrets

```bash
gh secret set APPLE_CERTIFICATE            # paste the base64 .p12
gh secret set APPLE_CERTIFICATE_PASSWORD   # the .p12 export password
gh secret set APPLE_SIGNING_IDENTITY --body "Developer ID Application: Your Name (TEAMID1234)"
gh secret set APPLE_ID --body "you@example.com"
gh secret set APPLE_PASSWORD               # the app-specific password
gh secret set APPLE_TEAM_ID --body "TEAMID1234"
```

## 5. Release

Tag a version as usual — the workflow signs + notarizes + staples the macOS
bundles automatically:

```bash
git tag -a vX.Y.Z -m "…" && git push origin vX.Y.Z
```

If the Apple secrets are absent, the build still succeeds but produces an
**unsigned** app (users then need `xattr -dr com.apple.quarantine`).
