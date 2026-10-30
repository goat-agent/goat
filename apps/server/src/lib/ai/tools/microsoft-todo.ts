import { tool } from "ai";
import { z } from "zod";
import { callGraph, MicrosoftConnection } from "./microsoft";

const API = "https://graph.microsoft.com/v1.0/me/todo/lists";
const PAGE = 50;

const Task = z.object({
  id: z.string(),
  title: z.string().default(""),
  body: z.object({ content: z.string().default("") }).optional(),
  dueDateTime: z.object({ dateTime: z.string() }).optional(),
  status: z.string(),
});

const Tasks = z.object({ value: z.array(Task).default([]) });
const Lists = z.object({
  value: z.array(z.object({ id: z.string(), displayName: z.string() })).default([]),
});

const listId = z.string().describe("A list id from microsoft_todo_list_lists");
const due = z.iso.date().optional().describe("Due date, YYYY-MM-DD");

export const microsoftTodoTools = {
  microsoft_todo_list_lists: tool({
    description: "Microsoft To Do: list the user's task lists.",
    inputSchema: z.object({}),
    contextSchema: MicrosoftConnection,
    execute: async (_, { context, abortSignal }) =>
      Lists.parse(await callGraph(context.fetch, API, { signal: abortSignal })),
  }),
  microsoft_todo_list: tool({
    description: "Microsoft To Do: list the tasks in a task list.",
    inputSchema: z.object({ listId, showCompleted: z.boolean().default(false) }),
    contextSchema: MicrosoftConnection,
    execute: async ({ listId: list, showCompleted }, { context, abortSignal }) => {
      const url = new URL(`${API}/${encodeURIComponent(list)}/tasks`);
      url.searchParams.set("$top", String(PAGE));
      if (!showCompleted) url.searchParams.set("$filter", "status ne 'completed'");
      const { value } = Tasks.parse(await callGraph(context.fetch, url, { signal: abortSignal }));
      return { tasks: value.map(publicTask) };
    },
  }),
  microsoft_todo_create: tool({
    description: "Microsoft To Do: add a task.",
    inputSchema: z.object({ listId, title: z.string(), notes: z.string().optional(), due }),
    contextSchema: MicrosoftConnection,
    execute: async ({ listId: list, ...task }, { context, abortSignal }) =>
      publicTask(
        Task.parse(
          await callGraph(context.fetch, `${API}/${encodeURIComponent(list)}/tasks`, {
            method: "POST",
            body: taskBody(task),
            signal: abortSignal,
          }),
        ),
      ),
  }),
  microsoft_todo_update: tool({
    description: "Microsoft To Do: change a task, or mark it done or not done.",
    inputSchema: z.object({
      listId,
      taskId: z.string(),
      title: z.string().optional(),
      notes: z.string().optional(),
      due,
      completed: z.boolean().optional(),
    }),
    contextSchema: MicrosoftConnection,
    execute: async ({ listId: list, taskId, ...changes }, { context, abortSignal }) => {
      const url = `${API}/${encodeURIComponent(list)}/tasks/${encodeURIComponent(taskId)}`;
      return publicTask(
        Task.parse(
          await callGraph(context.fetch, url, {
            method: "PATCH",
            body: taskBody(changes),
            signal: abortSignal,
          }),
        ),
      );
    },
  }),
  microsoft_todo_delete: tool({
    description: "Microsoft To Do: delete a task.",
    inputSchema: z.object({ listId, taskId: z.string() }),
    contextSchema: MicrosoftConnection,
    execute: async ({ listId: list, taskId }, { context, abortSignal }) => {
      const url = `${API}/${encodeURIComponent(list)}/tasks/${encodeURIComponent(taskId)}`;
      await callGraph(context.fetch, url, { method: "DELETE", signal: abortSignal });
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
    ...(task.notes === undefined ? {} : { body: { contentType: "text", content: task.notes } }),
    // To Do keeps only the date of the due time, but takes it as a date-time with a zone.
    ...(task.due === undefined
      ? {}
      : { dueDateTime: { dateTime: `${task.due}T00:00:00`, timeZone: "UTC" } }),
    ...(task.completed === undefined
      ? {}
      : { status: task.completed ? "completed" : "notStarted" }),
  };
}

function publicTask(task: z.infer<typeof Task>) {
  return {
    id: task.id,
    title: task.title,
    notes: task.body?.content,
    due: task.dueDateTime?.dateTime.slice(0, 10),
    status: task.status,
  };
}
