import { invoke } from '@tauri-apps/api/core';
import type {
  AdminConfigEditParams, AdminCredentialRemoveParams, AdminCredentialSetParams,
  AgentConversationsOutput, AgentListOutput, AgentSchedulesOutput, AgentSendOutput, ConversationInfo,
  DaemonStatus2, InputAttachment, Op, ResumeMode,
} from './api';

export interface Project { path: string; added_at: string }
export interface Workspace { repo: string | null; branch: string | null; kind: string; pr: { number: number; state: string } | null }
export interface ComputerStatus { accessibility: boolean; screen: boolean; advertised: boolean; halted: boolean; busy: boolean }
export interface DesktopSessionInfo { id: number; cwd: string; daemon: { version: string } }
export interface CommandSpec { name: string; aliases?: string[]; description: string; usage: string }
export type CommandOutcome = { kind: 'show'; screen: string } | { kind: 'done' } | { kind: 'notice'; level: string; text: string } | { kind: 'quit' };
export type AdminRequest =
  | ({ type: 'ConfigEdit' } & AdminConfigEditParams)
  | ({ type: 'CredentialSet' } & AdminCredentialSetParams)
  | ({ type: 'CredentialRemove' } & AdminCredentialRemoveParams)
  | { type: 'ProviderLogin'; provider: string; account: string; method: { type: 'OAuth' } | { type: 'ApiKey'; secret: string } };

export const ipc = {
  projects: () => invoke<Project[]>('projects_list'),
  addProject: (path: string) => invoke<Project[]>('projects_add', { path }),
  removeProject: (path: string) => invoke<Project[]>('projects_remove', { path }),
  conversations: (cwd: string) => invoke<ConversationInfo[]>('conversations', { cwd }),
  workspace: (cwd: string) => invoke<Workspace>('workspace', { cwd }),
  daemon: () => invoke<DaemonStatus2>('daemon_status'),
  computer: () => invoke<ComputerStatus>('computer_status'),
  permission: (kind: 'accessibility' | 'screen') => invoke<void>('computer_request_permission', { kind }),
  resumeComputer: () => invoke<void>('computer_resume'),
  openSession: (cwd: string, resume: ResumeMode) => invoke<DesktopSessionInfo>('session_open', { cwd, resume }),
  listenSession: (id: number) => invoke<void>('session_listen', { id }),
  closeSession: (id: number) => invoke<void>('session_close', { id }),
  op: (id: number, op: Op) => invoke<void>('session_op', { id, op }),
  submit: (id: number, text: string, attachments: InputAttachment[]) => invoke<number>('session_submit', { id, text, attachments }),
  command: (id: number, line: string) => invoke<CommandOutcome>('session_command', { id, line }),
  specs: (id: number) => invoke<CommandSpec[]>('command_specs', { id }),
  admin: (id: number, request: AdminRequest) => invoke<void>('session_admin', { id, request }),
  agents: () => invoke<AgentListOutput>('agents'),
  sendAgent: (slug: string, conversation: string | null, text: string) => invoke<AgentSendOutput>('agent_send', { slug, conversation, text }),
  agentConversations: (slug: string) => invoke<AgentConversationsOutput>('agent_conversations', { slug }),
  openAgent: (slug: string, conversation: string | null) => invoke<void>('agent_chat_open', { slug, conversation }),
  closeAgent: (slug: string) => invoke<void>('agent_chat_close', { slug }),
  openActivity: (slug: string) => invoke<void>('agent_activity_open', { slug }),
  closeActivity: (slug: string) => invoke<void>('agent_activity_close', { slug }),
  schedules: (slug: string) => invoke<AgentSchedulesOutput>('agent_schedules', { slug }),
};

export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : typeof error === 'string' ? error : JSON.stringify(error) ?? String(error);
}
