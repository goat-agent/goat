# iOS

Read [docs/style/swift.md](../../docs/style/swift.md) before writing Swift.

## Map

- `Goat/` — the iPhone's screens: `GoatApp`, `App/RootView`, `Onboarding/`, `Home/` (the agent
  list, the first screen after sign-in), `Conversation/` (the conversation screen, its composer,
  and calls), `Settings/`, `Components/PrimaryButton`, `Info.plist`, and `Assets.xcassets`.
- `../shared/GoatKit/` — what the Mac shares, compiled into this target as a second
  folder-synchronized group: `App/` (`AppModel`, `TokenStore`, `CurrentDevice`), `Conversation/`
  (`ConversationModel`, `Dictation`, and the bubbles and cards both apps draw), `Integrations/`
  (the account list and `Connector`, which runs the sign-in sheet), `Components/`, and the one
  `Localizable.xcstrings` both apps use. Screens that should look different on each device are
  written once per app; only models and the pieces both draw live here. New
  files in either folder join the target without editing the project file.
- `Conversation/` holds the voice features too. `Dictation` records with the system recorder and
  sends the recording to the server, which turns it into text for the composer. `Call` speaks
  plain WebRTC (the `stasel/WebRTC` package; Apple ships no public WebRTC) and only ever talks
  to goat's `startCall`, so the voice service can change without an app update.
- `../shared/GoatAPI/` — Swift package. `openapi.json` is exported by the server
  (`bun run openapi` in `apps/server`); never edit it by hand. Swift OpenAPI Generator turns it
  into `Client` and the `Components`/`Operations` types at build time; generated code is not
  committed.

## Commands

```sh
swift format lint --strict --recursive Goat ../shared/GoatKit ../shared/GoatAPI/Sources
swift format format --in-place --recursive Goat ../shared/GoatKit ../shared/GoatAPI/Sources
xcodebuild -project Goat.xcodeproj -scheme Goat \
  -destination 'generic/platform=iOS Simulator' -skipPackagePluginValidation build
```

Debug builds talk to `GOAT_SERVER_URL` (`http://localhost:3000`); run the server first.

## Design

- Apple-native: system colors, system navigation, and system components. Do not draw custom
  chrome where a system component exists.
- Keep the interface neutral. Color goes only where it carries meaning: the violet accent on the
  user's own messages and a screen's primary action, red for errors and for ending a call. The
  composer's main button is neutral, since its filled shape already marks it.
- Each screen has at most one primary action: `PrimaryButton` in `.safeAreaBar(edge: .bottom)`,
  so it sits above the keyboard. Show errors inline with `FormFooter`, not in alerts.
- Navigation lives in `AppModel` (`signInPath`, `setupPath`, `agentPath`), so network results
  drive it. Launch reopens the last conversation on top of the agent list.
- Connecting an account opens the server's sign-in URL in the system web authentication
  session, which ends at `goat://connected` with a state and code. The app sends those back
  with its own session (`createConnection`), so a sign-in page forwarded to someone else cannot
  connect their account to the sender. Tokens never reach the app. Settings and the
  conversation's connect cards both go through `Connector`.

## Localization

- Write UI text in English in code; `../shared/GoatKit/Localizable.xcstrings` holds the other
  languages for both apps. Text
  built outside a view, such as error messages in a model, goes through `String(localized:)`.
- Building in Xcode adds new keys to the catalog; `xcodebuild` does not. From the command line,
  build both apps, then pass both builds' `.stringsdata` files to `xcrun xcstringstool sync`. A key left
  untranslated still builds, so fill in every language before finishing.

## API values

- Enum-like fields decode as `value1` (a case this build knows) or `value2` (any string). Handle
  `value1 == nil` by ignoring the value or showing a neutral fallback; never crash on it.
- Read the agent event stream with `HTTPBody.agentEvents()`. It applies the API's date format,
  which the default JSON decoder does not.

## Concurrency

- The app target isolates to the main actor by default. Types used from other threads, such as
  `TokenStore` inside the client's token closure, are marked `nonisolated`.
- The `GoatAPI` package has no default isolation.
