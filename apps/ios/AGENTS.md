# iOS

Read [docs/style/swift.md](../../docs/style/swift.md) before writing Swift.

## Map

- `Goat/` — the app target. `App/` (state and root), `Onboarding/`, `Conversation/`,
  `Components/` (shared views), `Assets.xcassets`. The project uses folder-synchronized groups,
  so new files in these folders join the target without editing the project file.
- `GoatAPI/` — Swift package. `openapi.json` is exported by the server (`bun run openapi` in
  `apps/server`); never edit it by hand. Swift OpenAPI Generator turns it into `Client` and the
  `Components`/`Operations` types at build time; generated code is not committed.

## Commands

```sh
swift format lint --strict --recursive Goat GoatAPI/Sources GoatAPI/Package.swift
swift format format --in-place --recursive Goat GoatAPI/Sources GoatAPI/Package.swift
xcodebuild -project Goat.xcodeproj -scheme Goat \
  -destination 'generic/platform=iOS Simulator' -skipPackagePluginValidation build
```

Debug builds talk to `GOAT_SERVER_URL` (`http://localhost:3000`); run the server first.

## Design

- Apple-native: system colors, system navigation, and system components. Do not draw custom
  chrome where a system component exists.
- The violet accent marks only the screen's primary action and the user's own messages.
- Each screen has at most one primary action: `PrimaryButton` in `.safeAreaBar(edge: .bottom)`,
  so it sits above the keyboard. Show errors inline with `FormFooter`, not in alerts.
- Navigation lives in `AppModel` (`signInPath`, `setupPath`), so network results drive it.

## API values

- Enum-like fields decode as `value1` (a case this build knows) or `value2` (any string). Handle
  `value1 == nil` by ignoring the value or showing a neutral fallback; never crash on it.
- Read the agent event stream with `HTTPBody.agentEvents()`. It applies the API's date format,
  which the default JSON decoder does not.

## Concurrency

- The app target isolates to the main actor by default. Types used from other threads, such as
  `TokenStore` inside the client's token closure, are marked `nonisolated`.
- The `GoatAPI` package has no default isolation.
