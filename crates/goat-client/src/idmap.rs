use std::collections::HashMap;

use goat_protocol::{Event, Op, TaskId};

pub(crate) struct IdMap {
    local_to_daemon: HashMap<u64, u64>,
    daemon_to_local: HashMap<u64, u64>,
    next_local: u64,
}

impl IdMap {
    pub(crate) fn new() -> Self {
        Self {
            local_to_daemon: HashMap::new(),
            daemon_to_local: HashMap::new(),
            next_local: 1,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.local_to_daemon.clear();
        self.daemon_to_local.clear();
        self.next_local = 1;
    }

    fn bind(&mut self, local: u64, daemon: u64) {
        self.local_to_daemon.insert(local, daemon);
        self.daemon_to_local.insert(daemon, local);
        if local >= self.next_local {
            self.next_local = local + 1;
        }
    }

    pub(crate) fn record_correlation(&mut self, correlation: u64, daemon: TaskId) {
        self.bind(correlation, daemon.0);
    }

    fn local_for_daemon(&mut self, daemon: u64) -> u64 {
        if let Some(local) = self.daemon_to_local.get(&daemon) {
            return *local;
        }
        let local = self.next_local;
        self.next_local += 1;
        self.bind(local, daemon);
        local
    }

    fn daemon_for_local(&self, local: u64) -> Option<u64> {
        self.local_to_daemon.get(&local).copied()
    }

    pub(crate) fn translate_inbound(&mut self, event: &mut Event) {
        for id in event_ids_mut(event) {
            *id = TaskId(self.local_for_daemon(id.0));
        }
    }

    pub(crate) fn translate_outbound(&self, op: &mut Op) {
        if let Some(id) = op_id_mut(op)
            && let Some(daemon) = self.daemon_for_local(id.0)
        {
            *id = TaskId(daemon);
        }
    }
}

fn op_id_mut(op: &mut Op) -> Option<&mut TaskId> {
    match op {
        Op::Interrupt { id } | Op::Answer { id, .. } | Op::DequeueMessage { id } => Some(id),
        _ => None,
    }
}

fn event_ids_mut(event: &mut Event) -> Vec<&mut TaskId> {
    let mut ids = Vec::new();
    match event {
        Event::TaskStarted { id }
        | Event::TextDelta { id, .. }
        | Event::TextDone { id, .. }
        | Event::ToolStarted { id, .. }
        | Event::ToolDone { id, .. }
        | Event::ShellDone { id, .. }
        | Event::TaskDone { id, .. }
        | Event::ThinkingDelta { id, .. }
        | Event::AskStarted { id, .. }
        | Event::AskDismissed { id, .. }
        | Event::Usage { id, .. }
        | Event::Retrying { id, .. }
        | Event::UserMessage { id, .. }
        | Event::MessageDequeued { id, .. }
        | Event::CompactionStarted { id }
        | Event::CompactionDone { id, .. }
        | Event::AgentDone { id, .. }
        | Event::Error { id: Some(id), .. } => ids.push(id),
        Event::AgentStarted { id, parent, .. } => {
            ids.push(id);
            ids.push(parent);
        }
        _ => {}
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::IdMap;
    use goat_protocol::{Event, Op, TaskId};

    #[test]
    fn correlation_binds_local_to_daemon_both_ways() {
        let mut map = IdMap::new();
        map.record_correlation(5, TaskId(100));

        let mut started = Event::TaskStarted { id: TaskId(100) };
        map.translate_inbound(&mut started);
        assert_eq!(started, Event::TaskStarted { id: TaskId(5) });

        let mut interrupt = Op::Interrupt { id: TaskId(5) };
        map.translate_outbound(&mut interrupt);
        assert_eq!(interrupt, Op::Interrupt { id: TaskId(100) });
    }

    #[test]
    fn unknown_daemon_id_mints_stable_local_id() {
        let mut map = IdMap::new();
        let mut a = Event::TextDelta {
            id: TaskId(4242),
            chunk: "x".to_owned(),
        };
        let mut b = Event::TaskDone {
            id: TaskId(4242),
            interrupted: false,
        };
        map.translate_inbound(&mut a);
        map.translate_inbound(&mut b);
        let Event::TextDelta { id: local_a, .. } = a else {
            unreachable!()
        };
        let Event::TaskDone { id: local_b, .. } = b else {
            unreachable!()
        };
        assert_eq!(local_a, local_b, "same daemon id maps to same local id");
    }

    #[test]
    fn agent_started_translates_both_id_and_parent() {
        let mut map = IdMap::new();
        map.record_correlation(2, TaskId(7));
        let mut ev = Event::AgentStarted {
            id: TaskId(900),
            parent: TaskId(7),
            agent_type: "explore".to_owned(),
            label: "x".to_owned(),
        };
        map.translate_inbound(&mut ev);
        let Event::AgentStarted { parent, id, .. } = ev else {
            unreachable!()
        };
        assert_eq!(parent, TaskId(2));
        assert_ne!(id, TaskId(900));
    }
}
