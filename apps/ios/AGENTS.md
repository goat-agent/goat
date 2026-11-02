# iOS

Read [docs/style/swift.md](../../docs/style/swift.md) before writing Swift.

## Map

- `Goat/` — the iPhone's screens: `GoatApp`, `App/RootView`, `Onboarding/`, `Home/` (the agent
  list, the first screen after sign-in), `Agents/` (an agent's settings sheet), `Conversation/`
  (the conversation screen), `Settings/`, `Components/PrimaryButton`, `Info.plist`, and
  `Assets.xcassets`.
- `../shared/GoatKit/` — what the Mac shares, compiled into this target as a second
  folder-synchronized group: `App/` (`AppModel`, `TokenStore`, `CurrentDevice`,
  `AccountSection`), `Agents/` (`AvatarView`, `AgentRow`, `AgentTitle`, `AgentForm` with the
  new-agent sheet and the settings form), `Conversation/` (`ConversationModel`, `Dictation`,
  `ConversationMessages`, `Composer`, `Call` and `CallView`, and the bubbles and cards),
  `Integrations/` (`IntegrationsSection`, the catalog, and `Connector`, which runs the sign-in
  sheet), `UI/` (the design system: `Palette`, control sizes, and one file per part, as in
  apps/web's `shared/ui`), `Components/`, and the one `Localizable.xcstrings` both apps use. Screens that should look different on each device are
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

- Screens draw only with `../shared/GoatKit/UI`: `Palette` for color and the parts named as in
  apps/web (`.buttonStyle(.primary)`, `.inputStyle()`, `InputOTP`, `Switch`, `.alertDialog`,
  `DropdownMenu`, `ItemGroup`). Do not put a system-styled control, color, or dialog on a
  screen; a missing part is added to `UI/` first. System navigation and sheets stay.
- Dropdown menus and alert dialogs draw in a portal, as on the web, so a narrow panel does not
  clip them: every window's root view and every sheet's root view ends in `.portalHost()`, and
  the innermost host draws what opens inside it.
- Keep the interface neutral. Color and character belong to the content: agent pictures and the
  user's own messages. Controls are filled and monochrome. Violet (`Palette.primary`) means
  "you": your messages and the one next action you take (the composer's main button, a screen's
  primary action). Red is for errors and for ending a call. Links in replies keep the text color
  and are underlined.
- List rows are rounded rectangles; a standalone control is a capsule; a surface holding rows
  is their radius plus its inset.
- One task is one shape. Everything for writing a message, including attaching, dictating,
  calling, and its errors, lives inside the composer's glass.
- An agent always shows an avatar: its photo, or the picture the server drew from its parts. Making an agent and editing one
  use the same `AgentForm`; edits save as they are made.
- Each screen has at most one primary action: `PrimaryButton` in `.safeAreaBar(edge: .bottom)`,
  so it sits above the keyboard. Show errors inline with `FormFooter`, not in alerts.
- Navigation lives in `AppModel` (`signInPath`, and `agentPath` of agent ids), so network
  results drive it. Launch reopens the last conversation on top of the agent list. Signing up
  ends with the first agent; the user's name is optional and set in Settings. `AppModel` keeps
  the agent list live from `streamAgents`.
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
- Read event streams with `HTTPBody.agentEvents()` and `agentListEvents()`. They apply the
  API's date format, which the default JSON decoder does not.

## Concurrency

- The app target isolates to the main actor by default. Types used from other threads, such as
  `TokenStore` inside the client's token closure, are marked `nonisolated`.
- The `GoatAPI` package has no default isolation.
