use goat_provider::Message;

#[derive(Default)]
pub(crate) struct Conversation {
    messages: Vec<Message>,
    db_ids: Vec<Option<i64>>,
}

impl Conversation {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push(&mut self, message: Message, db_id: Option<i64>) {
        self.messages.push(message);
        self.db_ids.push(db_id);
    }

    pub(crate) fn clear(&mut self) {
        self.messages.clear();
        self.db_ids.clear();
    }

    pub(crate) fn replace(&mut self, entries: Vec<(Message, Option<i64>)>) {
        self.clear();
        for (message, db_id) in entries {
            self.push(message, db_id);
        }
    }

    pub(crate) fn set_system(&mut self, text: String) -> bool {
        match self.messages.first() {
            Some(first) if first.role == goat_provider::MessageRole::System => {
                if first.text_content() == text {
                    return false;
                }
                self.messages[0] = Message::text(goat_provider::MessageRole::System, text);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub(crate) fn db_ids(&self) -> &[Option<i64>] {
        &self.db_ids
    }

    pub(crate) fn len(&self) -> usize {
        self.messages.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use goat_provider::{Message, MessageRole};

    use super::Conversation;

    #[test]
    fn push_keeps_messages_and_ids_aligned() {
        let mut conversation = Conversation::new();
        conversation.push(Message::text(MessageRole::System, "sys"), None);
        conversation.push(Message::text(MessageRole::User, "hi"), Some(7));
        assert_eq!(conversation.len(), 2);
        assert_eq!(conversation.db_ids, &[None, Some(7)]);
        assert_eq!(conversation.messages()[1].text_content(), "hi");
    }

    #[test]
    fn replace_swaps_contents() {
        let mut conversation = Conversation::new();
        conversation.push(Message::text(MessageRole::User, "old"), Some(1));
        conversation.replace(vec![
            (Message::text(MessageRole::System, "sys"), None),
            (Message::text(MessageRole::User, "new"), Some(9)),
        ]);
        assert_eq!(conversation.len(), 2);
        assert_eq!(conversation.db_ids, &[None, Some(9)]);
        assert!(!conversation.is_empty());
        conversation.clear();
        assert!(conversation.is_empty());
    }
}
