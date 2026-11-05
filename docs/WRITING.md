# Writing rules

The repository uses Markdown and ASD-STE100 Simplified Technical English.
Use [ASD-STE100 Issue 9](https://www.asd-ste100.org/) as the language reference.
The current documents are drafts. A complete STE vocabulary review remains open.

## Review checks

- Limit an instruction sentence to 20 words.
- Limit a descriptive sentence to 25 words.
- Give each sentence one purpose.
- Name the component that performs the action.
- Prefer short sentences with simple verb forms.
- Put a condition before its required action.
- Keep terminology consistent across documents.
- Use dictionary words with their approved meaning and part of speech.
- Preserve official names, API fields, and Rust identifiers.
- Define project terms before their first use.
- Identify proposals and test results separately.

## Project terms

| Term | Meaning |
| --- | --- |
| Dictation key | The key that the user holds during speech input |
| Hold threshold | The minimum hold time before a session shows the orb and starts the capture |
| Session | One dictation, from key press to paste, fallback card, or cancellation |
| Capture | The first part of a session: microphone recording until key release, cancellation, or failure |
| Job | The part after capture: transcription completion, correction, and paste, without a visible indicator |
| Streaming | Transfer of audio parts to a provider while capture continues |
| Audio chunk | One ordered part of audio sent to the speech provider |
| Transcription session | The provider connection that receives audio chunks and returns one complete transcript |
| Generation | The number that identifies a session; the core ignores results with an old generation |
| Orb | The small pill panel that shows the target application icon and the voice level |
| Fallback card | The orb view that shows the final text when a paste is not possible |
| Transcript | The complete text that the speech model returns after the audio ends |
| Partial transcript | Text received before transcription completes; Bloop never inserts it |
| Final text | The text that Bloop pastes: the corrected text or the raw transcript |
| Focused field | The text field that has keyboard focus at key release |
| Target check | The checks before the paste: same application, no new input, no secure input |
| Landed | The result when the target reads the clipboard item after the paste command |
| Core | The pure Rust state machine that receives events and returns commands |
| Provider | An external AI service, for example Mistral |
| Port | A trait in `bloop-core` that an adapter implements |
| Adapter | The component that implements a port for a provider or a system API |
| Text step | One change to the text after the transcription, for example the correction |
| Correction | The optional text step that a chat model applies to the transcript |
| Prompt | The instructions that the correction sends to the chat model with the transcript |
| Step report | The result of each text step of one job, for the log |
| Settings snapshot | The settings, and the objects built from them, that one generation uses |
| Status menu | The menu of the Bloop item in the menu bar |
| Paste command | The Command-V event that Bloop posts |
| Dictionary | The user list of written forms, with an optional form that Bloop writes now |
| Suggestion | A dictionary entry that Bloop proposes after the user edits a pasted text |
| Driver | The `bloop-app` component that sends events to the core and executes its commands |
| MVP | Minimum viable product; the smallest version that proves the complete process |

This term list supports a later vocabulary review. It does not replace the STE dictionary.
