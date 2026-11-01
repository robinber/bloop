# Bloop

Bloop is a planned macOS app for French dictation.
The app uses Rust, native AppKit, Mistral Voxtral, and Mistral Small.

Hold Right Command to speak. A small orb shows that Bloop listens.
Release the key to stop. Bloop corrects the text and pastes it into the focused field.

The behavior follows the dictation feature of Tabi. Bloop does only this one task.

## Status

Phase 1 is in progress. The core state machine and the bundle script exist. The app does nothing yet.
The first version is for personal use on one Mac.

## Build

Bloop needs a stable code signing identity. With an ad hoc signature, macOS removes the permissions after each build.

1. Create the identity one time. In Keychain Access, select Keychain Access > Certificate Assistant > Create a Certificate.
   - Name: `Bloop Dev`
   - Identity Type: Self Signed Root
   - Certificate Type: Code Signing
   - Select "Let me override defaults" and set the validity period to 3650 days.
2. Run `scripts/bundle.sh`. The script builds `target/release/Bloop.app` and signs it.
   At the first signature, macOS asks for access to the key. Select "Always Allow".

To use a different identity, set `BLOOP_SIGN_IDENTITY`.

## Documents

- [Scope and decisions](docs/SCOPE.md)
- [Roadmap](docs/ROADMAP.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Code structure](docs/STRUCTURE.md)
- [Rust contracts](docs/CONTRACTS.md)
- [Writing rules](docs/WRITING.md)
- [Agent contract](AGENTS.md)
