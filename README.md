# Bloop

Bloop is a macOS app for French dictation.
The app uses Rust and native AppKit. Bloop does not depend on one AI provider: Mistral is the default, with Voxtral for speech.
Phase 3 adds an optional correction with a chat model.

Hold Right Command to speak. A small orb shows that Bloop listens.
Bloop streams the audio to the speech provider while you speak.
Release the key to finish transcription. Bloop pastes the text into the focused field.
When the correction is on, Bloop corrects the text before the paste.

The behavior follows the dictation feature of Tabi. Bloop does only this one task.

## Status

Phase 1 and Phase 2 are complete. Bloop transcribes the dictation with Voxtral and pastes the raw transcript.
Phase 3 adds optional correction, one menu toggle, the dictionary, and a daily-use trial.
Phase 4 adds persistent settings and more menu choices when needed.
The [roadmap](docs/ROADMAP.md#priorities) gives the priorities.
Bloop is for personal use on one Mac.

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

## API key

Store the API key of each provider in Keychain. The account is the provider name.
For Mistral, the default provider, run this command from the repository root after the first build:

```sh
security add-generic-password -s bloop -a mistral -T "$PWD/target/release/Bloop.app" -U -w
```

The command asks for the key. Thus the key is not in the shell history.
With `-T`, Bloop reads the key without an access dialog. Then open Bloop again.

## Documents

- [Scope and decisions](docs/SCOPE.md)
- [Roadmap](docs/ROADMAP.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Code structure](docs/STRUCTURE.md)
- [Rust contracts](docs/CONTRACTS.md)
- [Writing rules](docs/WRITING.md)
- [Agent contract](AGENTS.md)
