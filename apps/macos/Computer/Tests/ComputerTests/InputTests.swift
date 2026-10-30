import Carbon.HIToolbox
import Testing

@testable import Computer

@Suite struct InputTests {
  @Test func parsesShortcuts() throws {
    let chord = try Input.chord("cmd+shift+t")
    #expect(chord.key == CGKeyCode(kVK_ANSI_T))
    #expect(chord.flags.contains(.maskCommand))
    #expect(chord.flags.contains(.maskShift))
  }

  @Test func parsesNamedKeys() throws {
    #expect(try Input.chord("return").key == CGKeyCode(kVK_Return))
    #expect(try Input.chord("F5").key == CGKeyCode(kVK_F5))
  }

  @Test func rejectsUnknownKeys() {
    #expect(throws: ComputerError.self) { try Input.chord("cmd+hyper") }
    #expect(throws: ComputerError.self) { try Input.chord("a+b") }
  }

  @Test func namesRolesInWords() {
    #expect(Snapshot.name(role: "AXPopUpButton", subrole: nil) == "pop up button")
    #expect(Snapshot.name(role: "AXTextField", subrole: "AXSearchField") == "search field")
    #expect(Snapshot.name(role: "AXStaticText", subrole: nil) == "static text")
  }

  @Test func matchesMenuTitlesLoosely() {
    #expect(Menus.matches("Export To…", "export to"))
    #expect(Menus.matches("Save As...", "Save As…"))
    #expect(!Menus.matches(nil, "File"))
  }
}
