import type {
  AccountEntry, ConversationSummary, Event, InputAttachment, LoginProvider, Mode,
  ModelEntry, ModelTarget, ProcessInfo, ProcessState, RateLimitEntry, RewindPoint,
  SkillInfo, SubagentGroupMember, TaskId, ToolCall, ToolOutcome, TranscriptEntry, UsageEntry,
} from './api';

export interface TextRow { kind: 'user' | 'system' | 'text' | 'thinking'; key: number; text: string; task?: TaskId; streaming?: boolean; attachments?: InputAttachment[] }
export interface ToolRow { kind: 'tool'; key: number; call: ToolCall; outcome?: ToolOutcome }
export interface GroupMember { member: SubagentGroupMember; run?: TaskId; outcome?: ToolOutcome; done?: boolean }
export interface GroupRow { kind: 'group'; key: number; group: string; parent: TaskId; members: GroupMember[] }
export interface ShellRow { kind: 'shell'; key: number; task: TaskId; command: string; output: string; done: boolean }
export interface ProcessRow { kind: 'process'; key: number; process?: string; command: string; output: string; state: ProcessState; exitCode?: number | null; reason?: string }
export interface NoticeRow { kind: 'notice'; key: number; level: string; text: string; hint?: string | null }
export interface CompactionRow { kind: 'compaction'; key: number; before: number; after: number }
export type Row = TextRow | ToolRow | GroupRow | ShellRow | ProcessRow | NoticeRow | CompactionRow;
export interface SubagentRun { id: TaskId; parent: TaskId; call: string; type: string; label: string; rows: Row[]; done: boolean | null; tokens: number; tools: number }
export interface QueuedMessage { id: TaskId; text: string; attachments: InputAttachment[] }
export interface SessionView {
  rows: Row[];
  serial: number;
  revision: number;
  active: TaskId | null;
  busy: boolean;
  thinking: boolean;
  compacting: boolean;
  model: ModelTarget | null;
  models: ModelEntry[];
  accounts: AccountEntry[];
  loginProviders: LoginProvider[];
  loginStatus: Extract<Event, { type: 'LoginStatus' }>[];
  mode: Mode;
  planPath: string | null;
  conversation: number | null;
  conversations: ConversationSummary[];
  rewind: RewindPoint[];
  files: string[];
  skills: SkillInfo[];
  asks: Extract<Event, { type: 'AskStarted' }>[];
  plan: Extract<Event, { type: 'PlanProposed' }> | null;
  processes: ProcessInfo[];
  usage: UsageEntry[];
  totals: Record<string, { input: number; output: number }>;
  rates: RateLimitEntry[];
  turnTokens: number;
  contextTokens: number | null;
  contextWindow: number | null;
  compactionThreshold: number | null;
  retry: (Extract<Event, { type: 'Retrying' }> & { until: number }) | null;
  subagents: Record<string, SubagentRun>;
  streams: Record<string, number>;
  thoughts: Record<string, number>;
  queued: QueuedMessage[];
  draft: { text: string; attachments: InputAttachment[]; sequence: number };
  notices: { key: number; level: string; text: string }[];
}

export function createSession(): SessionView {
  return {
    rows: [], serial: 0, revision: 0, active: null, busy: false, thinking: false, compacting: false,
    model: null, models: [], accounts: [], loginProviders: [], loginStatus: [], mode: 'normal', planPath: null,
    conversation: null, conversations: [], rewind: [], files: [], skills: [], asks: [], plan: null,
    processes: [], usage: [], totals: {}, rates: [], turnTokens: 0, contextTokens: null, contextWindow: null,
    compactionThreshold: null, retry: null, subagents: {}, streams: {}, thoughts: {}, queued: [],
    draft: { text: '', attachments: [], sequence: 0 }, notices: [],
  };
}

function rowsFor(state: SessionView, task?: TaskId): Row[] {
  return task && state.subagents[task] ? state.subagents[task].rows : state.rows;
}

function finishThought(state: SessionView, task: TaskId) {
  const row = rowsFor(state, task).find(row => row.key === state.thoughts[task]);
  if (row && row.kind === 'thinking') row.streaming = false;
  delete state.thoughts[task];
}

function finishTask(state: SessionView, task: TaskId) {
  finishThought(state, task);
  const row = rowsFor(state, task).find(row => row.key === state.streams[task]);
  if (row?.kind === 'text') row.streaming = false;
  delete state.streams[task];
}

function memberFor(state: SessionView, parent: TaskId, call: string): GroupMember | undefined {
  for (const row of rowsFor(state, parent)) {
    if (row.kind !== 'group' || row.parent !== parent) continue;
    const member = row.members.find(member => member.member.call === call);
    if (member) return member;
  }
}

function processFor(state: SessionView, process: string, command = ''): ProcessRow {
  let row = state.rows.find((row): row is ProcessRow => row.kind === 'process' && row.process === process);
  if (!row) {
    row = { kind: 'process', key: ++state.serial, process, command, output: '', state: 'running' };
    state.rows.push(row);
    row = state.rows[state.rows.length - 1] as ProcessRow;
  }
  if (command) row.command = command;
  return row;
}

function restored(state: SessionView, entry: TranscriptEntry): Row {
  const key = ++state.serial;
  switch (entry.type) {
    case 'User': return { key, kind: entry.system ? 'system' : 'user', text: entry.system ? entry.display ?? entry.text : entry.text, attachments: entry.attachments ?? [] };
    case 'Assistant': return { key, kind: 'text', text: entry.text };
    case 'Thinking': return { key, kind: 'thinking', text: entry.text };
    case 'Tool': return { key, kind: 'tool', call: entry.call, outcome: entry.outcome };
    case 'SubagentGroup': return { key, kind: 'group', group: entry.group, parent: '0', members: entry.members.map(member => ({ member: member.member, outcome: member.outcome, done: member.outcome.ok })) };
    case 'Compaction': return { key, kind: 'compaction', before: entry.tokens_before, after: entry.tokens_after };
    case 'Shell': return { key, kind: 'shell', task: '0', command: entry.command, output: entry.output, done: true };
    case 'Process': return { key, kind: 'process', command: entry.command, output: entry.output, state: 'exited' };
  }
}

function restoreQueue(state: SessionView) {
  if (!state.queued.length) return;
  state.draft = {
    text: state.queued.map(message => message.text).join('\n'),
    attachments: state.queued.flatMap(message => message.attachments),
    sequence: state.draft.sequence + 1,
  };
  state.queued = [];
}

export function applyEvent(state: SessionView, event: Event): void {
  state.revision++;
  if (event.type === 'UserMessage' || event.type === 'MessageDequeued') {
    const attachments = event.attachments ?? [];
    const index = state.queued.findIndex(message => message.id === event.id || (
      message.text === event.text && message.attachments.length === attachments.length
      && message.attachments.every((image, position) => image.data === attachments[position].data && image.media_type === attachments[position].media_type)
    ));
    if (index >= 0) state.queued.splice(index, 1);
  }
  switch (event.type) {
    case 'ConversationRestored':
      state.rows = event.entries.map(entry => restored(state, entry));
      state.subagents = {};
      state.streams = {};
      state.thoughts = {};
      state.active = null;
      state.busy = false;
      state.thinking = false;
      state.compacting = false;
      state.retry = null;
      state.asks = [];
      state.plan = null;
      state.model = event.target;
      state.contextTokens = event.context_tokens ?? null;
      state.compactionThreshold = event.compaction_threshold ?? null;
      state.turnTokens = 0;
      state.queued = [];
      state.usage = [];
      state.contextWindow = state.models.find(model => model.provider === event.target.provider && model.model === event.target.model)?.context_window ?? null;
      break;
    case 'TaskStarted':
      if (!state.subagents[event.id]) {
        state.active = event.id;
        state.busy = true;
        state.thinking = false;
        state.turnTokens = 0;
      }
      break;
    case 'UserMessage':
      state.rows.push({ kind: event.system ? 'system' : 'user', key: ++state.serial, task: event.id, text: event.display ?? event.text, attachments: event.attachments ?? [] });
      break;
    case 'ThinkingDelta': {
      if (!state.subagents[event.id]) state.thinking = true;
      const rows = rowsFor(state, event.id);
      const row = rows.find(row => row.key === state.thoughts[event.id]);
      if (row?.kind === 'thinking') row.text += event.chunk;
      else {
        const key = ++state.serial;
        rows.push({ kind: 'thinking', key, text: event.chunk, task: event.id, streaming: true });
        state.thoughts[event.id] = key;
      }
      break;
    }
    case 'TextDelta': {
      finishThought(state, event.id);
      if (!state.subagents[event.id]) { state.thinking = false; state.retry = null; }
      const rows = rowsFor(state, event.id);
      const row = rows.find(row => row.key === state.streams[event.id]);
      if (row?.kind === 'text') row.text += event.chunk;
      else {
        const key = ++state.serial;
        rows.push({ kind: 'text', key, text: event.chunk, task: event.id, streaming: true });
        state.streams[event.id] = key;
      }
      break;
    }
    case 'TextDone': {
      finishThought(state, event.id);
      const rows = rowsFor(state, event.id);
      const row = rows.find(row => row.key === state.streams[event.id]);
      if (row?.kind === 'text') { row.text = event.text; row.streaming = false; }
      else if (event.text) rows.push({ kind: 'text', key: ++state.serial, text: event.text, task: event.id });
      delete state.streams[event.id];
      break;
    }
    case 'ToolStarted':
      finishThought(state, event.id);
      if (!state.subagents[event.id]) { state.thinking = false; state.retry = null; }
      else state.subagents[event.id].tools++;
      if (!memberFor(state, event.id, event.call.id)) rowsFor(state, event.id).push({ kind: 'tool', key: ++state.serial, call: event.call });
      break;
    case 'ToolDone': {
      const member = memberFor(state, event.id, event.call);
      if (member) {
        if (!member.member.background) { member.outcome = event.outcome; member.done = event.outcome.ok; }
      } else {
        const row = rowsFor(state, event.id).find(row => row.kind === 'tool' && row.call.id === event.call);
        if (row?.kind === 'tool') row.outcome = event.outcome;
      }
      break;
    }
    case 'SubagentGroupStarted':
      rowsFor(state, event.id).push({ kind: 'group', key: ++state.serial, group: event.group, parent: event.id, members: event.members.map(member => ({ member })) });
      break;
    case 'SubagentStarted': {
      state.subagents[event.id] = { id: event.id, parent: event.parent, call: event.call, type: event.subagent_type, label: event.label, rows: [], done: null, tokens: 0, tools: 0 };
      let member = memberFor(state, event.parent, event.call);
      if (!member) {
        rowsFor(state, event.parent).push({ kind: 'group', key: ++state.serial, group: event.call, parent: event.parent, members: [{ member: { call: event.call, subagent_type: event.subagent_type, label: event.label, background: false }, run: event.id }] });
        member = memberFor(state, event.parent, event.call);
      }
      if (member) member.run = event.id;
      break;
    }
    case 'SubagentDone': {
      const run = state.subagents[event.id];
      if (run) {
        run.done = event.ok;
        finishTask(state, event.id);
        const member = memberFor(state, run.parent, run.call);
        if (member) member.done = event.ok;
      }
      break;
    }
    case 'TaskDone':
      finishTask(state, event.id);
      if (!state.subagents[event.id]) {
        state.busy = false; state.active = null; state.thinking = false; state.compacting = false; state.retry = null;
        state.asks = state.asks.filter(ask => ask.id !== event.id);
        if (state.plan?.id === event.id) state.plan = null;
        if (event.interrupted) {
          state.rows.push({ kind: 'notice', key: ++state.serial, level: 'info', text: 'Turn interrupted' });
          restoreQueue(state);
        }
      }
      break;
    case 'ShellDone': {
      const row = state.rows.find(row => row.kind === 'shell' && row.task === event.id)
        ?? state.rows.find(row => row.kind === 'shell' && row.task.startsWith('local-') && !row.done);
      if (row?.kind === 'shell') { row.task = event.id; row.output = event.output; row.done = true; }
      else state.rows.push({ kind: 'shell', key: ++state.serial, task: event.id, command: '', output: event.output, done: true });
      break;
    }
    case 'ProcessStarted': processFor(state, event.process, event.command); break;
    case 'ProcessOutput': processFor(state, event.process).output += event.chunk; break;
    case 'ProcessExited': {
      const row = processFor(state, event.process);
      row.state = 'exited'; row.exitCode = event.code; row.reason = event.reason;
      break;
    }
    case 'ProcessListChanged':
      state.processes = event.processes;
      for (const process of event.processes) {
        const row = processFor(state, process.id, process.command);
        row.state = process.state; row.exitCode = process.exit_code;
      }
      break;
    case 'CompactionStarted': if (!state.subagents[event.id]) state.compacting = true; break;
    case 'CompactionDone':
      if (!state.subagents[event.id]) state.compacting = false;
      if (event.ok) {
        rowsFor(state, event.id).push({ kind: 'compaction', key: ++state.serial, before: event.tokens_before, after: event.tokens_after });
        const tokens = event.usage.input_tokens + event.usage.output_tokens;
        if (state.subagents[event.id]) state.subagents[event.id].tokens += tokens;
        else {
          state.contextTokens = event.tokens_after;
          state.turnTokens += tokens;
          if (state.model) {
            const key = `${state.model.provider}/${state.model.account}`;
            const total = state.totals[key] ?? { input: 0, output: 0 };
            state.totals[key] = { input: total.input + event.usage.input_tokens, output: total.output + event.usage.output_tokens };
          }
        }
      }
      break;
    case 'Retrying': {
      const rows = rowsFor(state, event.id);
      const index = rows.findIndex(row => row.key === state.streams[event.id]);
      if (index >= 0) rows.splice(index, 1);
      delete state.streams[event.id];
      finishThought(state, event.id);
      if (!state.subagents[event.id]) { state.thinking = false; state.retry = { ...event, until: Date.now() + event.delay_ms }; }
      else rows.push({ kind: 'notice', key: ++state.serial, level: 'info', text: `Retry ${event.attempt}/${event.max_attempts}: ${event.reason}` });
      break;
    }
    case 'Error':
      rowsFor(state, event.id ?? undefined).push({ kind: 'notice', key: ++state.serial, level: 'error', text: event.message, hint: event.hint });
      if (event.id) finishTask(state, event.id);
      if (!event.id || !state.subagents[event.id]) {
        state.busy = false; state.active = null; state.retry = null; state.thinking = false; state.compacting = false;
        restoreQueue(state);
      }
      break;
    case 'Notify': state.notices.push({ key: ++state.serial, level: event.kind, text: event.message }); break;
    case 'AskStarted': state.asks = [...state.asks.filter(ask => ask.call !== event.call), event]; break;
    case 'AskDismissed': state.asks = state.asks.filter(ask => ask.call !== event.call); break;
    case 'PlanProposed': state.plan = event; break;
    case 'ModeChanged': state.mode = event.mode; state.planPath = event.plan_path ?? null; break;
    case 'ModelListChanged':
      state.models = event.entries;
      if (state.model) state.contextWindow = event.entries.find(model => model.provider === state.model?.provider && model.model === state.model?.model)?.context_window ?? state.contextWindow;
      break;
    case 'ModelSelected': {
      state.model = event.target;
      const usage = state.usage.find(usage => usage.provider === event.target.provider && usage.account === event.target.account);
      const model = state.models.find(model => model.provider === event.target.provider && model.model === event.target.model);
      if (usage) state.contextTokens = usage.usage.input_tokens;
      state.contextWindow = usage?.context_window ?? model?.context_window ?? null;
      break;
    }
    case 'AccountsChanged': state.accounts = event.providers; break;
    case 'LoginProviders': state.loginProviders = event.providers; break;
    case 'LoginStatus': state.loginStatus = [...state.loginStatus.filter(status => status.provider !== event.provider), event]; break;
    case 'SkillsChanged': state.skills = event.skills; break;
    case 'ConversationsListed': state.conversations = event.conversations; break;
    case 'RewindPointsListed': state.rewind = event.points; break;
    case 'FilesListed': state.files = event.entries; break;
    case 'ConversationBound': state.conversation = event.conversation_id; break;
    case 'ConversationRewound': state.draft = { text: event.draft.text, attachments: event.draft.attachments ?? [], sequence: state.draft.sequence + 1 }; break;
    case 'MessageDequeued':
      state.draft = { text: event.text, attachments: event.attachments ?? [], sequence: state.draft.sequence + 1 };
      break;
    case 'Usage': {
      const tokens = event.usage.input_tokens + event.usage.output_tokens;
      if (state.subagents[event.id]) { state.subagents[event.id].tokens += tokens; break; }
      state.turnTokens += tokens;
      const key = `${event.provider}/${event.account}`;
      const total = state.totals[key] ?? { input: 0, output: 0 };
      state.totals[key] = { input: total.input + event.usage.input_tokens, output: total.output + event.usage.output_tokens };
      state.usage = [...state.usage.filter(usage => usage.provider !== event.provider || usage.account !== event.account), { provider: event.provider, account: event.account, usage: event.usage, context_window: event.context_window, compaction_threshold: event.compaction_threshold }];
      if (!state.model || (state.model.provider === event.provider && state.model.account === event.account)) {
        state.contextTokens = event.usage.input_tokens;
        if (event.context_window != null) state.contextWindow = event.context_window;
        if (event.compaction_threshold != null) state.compactionThreshold = event.compaction_threshold;
      }
      break;
    }
    case 'RateLimits': state.rates = [...state.rates.filter(rate => rate.provider !== event.provider || rate.account !== event.account), { provider: event.provider, account: event.account, snapshot: event.snapshot, cached_at: event.cached_at }]; break;
    default: {
      const unhandled: never = event;
      throw new Error(`Unhandled session event: ${JSON.stringify(unhandled)}`);
    }
  }
}
