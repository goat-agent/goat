import Testing

@testable import Computer

struct KeyComboTests {
  @Test func readsModifiersAndCharacterKeys() throws {
    let combo = try #require(KeyCombo("cmd+shift+t"))
    #expect(combo.code == 17)
    #expect(combo.character == "T")
    #expect(combo.modifiers == [.command, .shift])
  }

  @Test func readsNamedKeysWithoutACharacter() throws {
    let combo = try #require(KeyCombo("Return"))
    #expect(combo.code == 36)
    #expect(combo.character == nil)
    #expect(combo.modifiers.isEmpty)
  }

  @Test func acceptsSpellingsOfModifiers() {
    #expect(KeyCombo("command+option+esc")?.modifiers == [.command, .option])
    #expect(KeyCombo("ctrl+alt+delete")?.modifiers == [.control, .option])
  }

  @Test func refusesWhatIsNotAKey() {
    #expect(KeyCombo("cmd+") == nil)
    #expect(KeyCombo("hyper+a") == nil)
    #expect(KeyCombo("cmd+banana") == nil)
  }
}
