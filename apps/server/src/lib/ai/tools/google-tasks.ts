import { tool } from "ai";
import { z } from "zod";
import { callGoogle, GoogleConnection, googleScopes } from "./google";

const API = "https://tasks.googleapis.com/tasks/v1";
const metadata = { scope: googleScopes.tasks };

const Task = z.object({
  id: z.string(),
  title: z.string().default(""),
  notes: z.string().optional(),
  due: z.string().optional(),
  status: z.string(),
});

const Tasks = z.object({ items: z.array(Task).default([]) });
const Lists = z.object({
  items: z.array(z.object({ id: z.string(), title: z.string() })).default([]),
});

const listId = z
  .string()
  .default("@default")
  .describe("A list id from google_tasks_list_lists; the default list when omitted");
const due = z.iso.date().optional().describe("Due date, YYYY-MM-DD");

export const googleTasksTools = {
  google_tasks_list_lists: tool({
    description: "Google Tasks: list the user's task lists.",
    inputSchema: z.object({}),
    contextSchema: GoogleConnection,
    metadata,
    execute: async (_, { context, abortSignal }) =>
      Lists.parse(
        await callGoogle(context.fetch, `${API}/users/@me/lists`, { signal: abortSignal }),
      ),
  }),
  google_tasks_list: tool({
    description: "Google Tasks: list the to-dos in a task list.",
    inputSchema: z.object({ listId, showCompleted: z.boolean().default(false) }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ listId: list, showCompleted }, { context, abortSignal }) => {
      const url = new URL(`${API}/lists/${encodeURIComponent(list)}/tasks`);
      url.search = new URLSearchParams({
        showCompleted: String(showCompleted),
        showHidden: String(showCompleted),
      }).toString();
      return Tasks.parse(await callGoogle(context.fetch, url, { signal: abortSignal }));
    },
  }),
  google_tasks_create: tool({
    description: "Google Tasks: add a to-do.",
    inputSchema: z.object({ listId, title: z.string(), notes: z.string().optional(), due }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ listId: list, ...task }, { context, abortSignal }) =>
      Task.parse(
        await callGoogle(context.fetch, `${API}/lists/${encodeURIComponent(list)}/tasks`, {
          method: "POST",
          body: taskBody(task),
          signal: abortSignal,
        }),
      ),
  }),
  google_tasks_update: tool({
    description: "Google Tasks: change a to-do, or mark it done or not done.",
    inputSchema: z.object({
      listId,
      taskId: z.string(),
      title: z.string().optional(),
      notes: z.string().optional(),
      due,
      completed: z.boolean().optional(),
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ listId: list, taskId, ...changes }, { context, abortSignal }) => {
      const url = `${API}/lists/${encodeURIComponent(list)}/tasks/${encodeURIComponent(taskId)}`;
      return Task.parse(
        await callGoogle(context.fetch, url, {
          method: "PATCH",
          body: taskBody(changes),
          signal: abortSignal,
        }),
      );
    },
  }),
  google_tasks_delete: tool({
    description: "Google Tasks: delete a to-do.",
    inputSchema: z.object({ listId, taskId: z.string() }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ listId: list, taskId }, { context, abortSignal }) => {
      const url = `${API}/lists/${encodeURIComponent(list)}/tasks/${encodeURIComponent(taskId)}`;
      await callGoogle(context.fetch, url, { method: "DELETE", signal: abortSignal });
      return { deleted: taskId };
    },
  }),
};

function taskBody(task: {
  title?: string | undefined;
  notes?: string | undefined;
  due?: string | undefined;
  completed?: boolean | undefined;
}) {
  return {
    ...(task.title === undefined ? {} : { title: task.title }),
    ...(task.notes === undefined ? {} : { notes: task.notes }),
    // Google Tasks keeps only the date of `due`, but takes it as an RFC 3339 timestamp.
    ...(task.due === undefined ? {} : { due: `${task.due}T00:00:00.000Z` }),
    ...(task.completed === undefined
      ? {}
      : { status: task.completed ? "completed" : "needsAction" }),
  };
}
