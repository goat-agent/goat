import { z } from "zod";
import { env } from "../../env";
import type { Integration } from "./integration";

const UserInfo = z.object({
  userId: z.string(),
  email: z.string().optional(),
  fullName: z.string().optional(),
});

export const todoist = {
  name: "Todoist",
  description:
    "To-do lists: tasks, projects, sections, labels, due dates, reminders, comments, completing and rescheduling tasks.",
  auth: {
    type: "oauth",
    issuer: "https://todoist.com",
    client: env.TODOIST_CLIENT_ID ? { id: env.TODOIST_CLIENT_ID } : undefined,
    scopes: ["data:read_write"],
    identify: async ({ callTool }) => {
      const user = UserInfo.parse(await callTool("user-info"));
      return { id: user.userId, name: user.email ?? user.fullName ?? user.userId };
    },
  },
  mcp: {
    url: "https://ai.todoist.net/mcp",
    tools: {
      "user-info": "read",
      "get-overview": "read",
      "find-tasks": "read",
      "find-tasks-by-date": "read",
      "find-completed-tasks": "read",
      "add-tasks": "write",
      "update-tasks": "write",
      "complete-tasks": "write",
      "uncomplete-tasks": "write",
      "reschedule-tasks": "write",
      "find-projects": "read",
      "add-projects": "write",
      "update-projects": "write",
      "find-sections": "read",
      "add-sections": "write",
      "update-sections": "write",
      "find-labels": "read",
      "add-labels": "write",
      "update-labels": "write",
      "find-filters": "read",
      "add-filters": "write",
      "update-filters": "write",
      "find-comments": "read",
      "add-comments": "write",
      "update-comments": "write",
      "find-reminders": "read",
      "add-reminders": "write",
      "update-reminders": "write",
      "delete-object": "write",
    },
  },
} satisfies Integration;
