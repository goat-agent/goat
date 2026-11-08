# iOS

Read [docs/style/swift.md](../../docs/style/swift.md) before writing Swift.

`Goat/` is the app: `GoatApp`, `RootView`, and `Assets.xcassets`. The folder is synchronized with
the target, so new files join it without editing the project file. The target isolates to the main
actor by default.

## Commands

```sh
swift format lint --strict --recursive Goat
swift format format --in-place --recursive Goat
xcodebuild -project Goat.xcodeproj -scheme Goat -destination 'generic/platform=iOS Simulator' build
```

## Previews

Mockups are reviewed as rendered previews ([docs/design/mockup.md](../../docs/design/mockup.md)).
Render them with the Xcode MCP tools, which need Xcode running.

- Open `Goat.xcodeproj` with `XcodeOpenWorkspace`. Then call `RenderPreview` with the workspace
  identifier and the file's path in the project (`Goat/...`, not a file-system path).
  `previewDefinitionIndexInFile` picks a preview by its position in the file.
- It returns the image's path in a temporary directory. Open that file for review, passing the
  path on unchanged: the time in the file name has a narrow no-break space before AM or PM, so
  a retyped path does not match.
- `previewVariantOverrides` renders dark mode and other Dynamic Type sizes,
  `previewLocalizationOverride` renders Korean, and `preferredRenderDestination` picks another
  iPhone or an iPad. The first result lists the values each one accepts.

## Localization

Write UI text in English in code, and keep Korean in a `Localizable.xcstrings` catalog in `Goat/`.
Building in Xcode adds new keys to the catalog; `xcodebuild` does not. A key left untranslated
still builds, so fill in every language before finishing.
