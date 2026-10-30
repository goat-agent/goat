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
    tools: [
      "user-info",
      "get-overview",
      "find-tasks",
      "find-tasks-by-date",
      "find-completed-tasks",
      "add-tasks",
      "update-tasks",
      "complete-tasks",
      "uncomplete-tasks",
      "reschedule-tasks",
      "find-projects",
      "add-projects",
      "update-projects",
      "find-sections",
      "add-sections",
      "update-sections",
      "find-labels",
      "add-labels",
      "update-labels",
      "find-filters",
      "add-filters",
      "update-filters",
      "find-comments",
      "add-comments",
      "update-comments",
      "find-reminders",
      "add-reminders",
      "update-reminders",
      "delete-object",
    ],
  },
} satisfies Integration;
