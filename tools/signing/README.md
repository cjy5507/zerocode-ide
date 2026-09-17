`ensure-local-identity.sh` creates **ZeroCode Local Signing** once in the login
keychain (macOS may ask the person to approve code signing trust and choose
Always Allow for codesign's first private-key access). The import permits
`/usr/bin/codesign`; a keychain partition-list repair additionally needs the
login keychain password and is left to the person in macOS. No login password is
requested or stored by this workflow. Subsequent
builds reuse its certificate fingerprint. `sign-computer-use.sh` pins the helper's
designated requirement to its bundle identifier and `certificate leaf`, so a
new executable hash retains the same TCC identity. Both build.rs and the local
release lane use this script. Keep the certificate and its private key; repair
an unavailable existing identity rather than silently replacing it.

If no local identity is available, signing falls back to ad-hoc with a warning;
the permission report reads the actual signature and returns `identity: "adhoc"`
or `"local"`. Initial migration from ad-hoc may need `permissions --reset`.
Use `ZEROCODE_SIGNING_KEYCHAIN` to select an isolated keychain for testing.

Developer ID signing and notarization for distribution are outside this local signing flow.
