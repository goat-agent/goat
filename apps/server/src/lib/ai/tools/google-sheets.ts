import { tool } from "ai";
import { z } from "zod";
import { callGoogle, GoogleConnection, googleScopes, untrusted, UNTRUSTED_NOTE } from "./google";

const API = "https://sheets.googleapis.com/v4/spreadsheets";
const metadata = { scope: googleScopes.spreadsheets };
const readMetadata = { ...metadata, effect: "read" };

const Values = z.object({ range: z.string(), values: z.array(z.array(z.unknown())).default([]) });
const Spreadsheet = z.object({ spreadsheetId: z.string(), spreadsheetUrl: z.string() });

const range = z.string().describe("A1 notation, such as Sheet1!A1:D20");
const rows = z.array(z.array(z.union([z.string(), z.number(), z.boolean()])));

export const googleSheetsTools = {
  google_sheets_read: tool({
    description: `Google Sheets: read cell values in a range. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object({ spreadsheetId: z.string(), range }),
    contextSchema: GoogleConnection,
    metadata: readMetadata,
    execute: async ({ spreadsheetId, range: cells }, { context, abortSignal }) => {
      const url = `${API}/${encodeURIComponent(spreadsheetId)}/values/${encodeURIComponent(cells)}`;
      const values = Values.parse(await callGoogle(context.fetch, url, { signal: abortSignal }));
      return { range: values.range, values: untrusted(JSON.stringify(values.values)) };
    },
  }),
  google_sheets_update: tool({
    description: "Google Sheets: write rows into a range, replacing what is there.",
    inputSchema: z.object({ spreadsheetId: z.string(), range, rows }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ spreadsheetId, range: cells, rows: values }, { context, abortSignal }) => {
      const url = `${API}/${encodeURIComponent(spreadsheetId)}/values/${encodeURIComponent(cells)}?valueInputOption=USER_ENTERED`;
      await callGoogle(context.fetch, url, {
        method: "PUT",
        body: { values },
        signal: abortSignal,
      });
      return { updated: cells };
    },
  }),
  google_sheets_append: tool({
    description: "Google Sheets: add rows after the last row of a table.",
    inputSchema: z.object({ spreadsheetId: z.string(), range, rows }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ spreadsheetId, range: cells, rows: values }, { context, abortSignal }) => {
      const url = `${API}/${encodeURIComponent(spreadsheetId)}/values/${encodeURIComponent(cells)}:append?valueInputOption=USER_ENTERED&insertDataOption=INSERT_ROWS`;
      await callGoogle(context.fetch, url, {
        method: "POST",
        body: { values },
        signal: abortSignal,
      });
      return { appended: values.length };
    },
  }),
  google_sheets_create: tool({
    description: "Google Sheets: create a spreadsheet.",
    inputSchema: z.object({ title: z.string() }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ title }, { context, abortSignal }) => {
      const sheet = Spreadsheet.parse(
        await callGoogle(context.fetch, API, {
          method: "POST",
          body: { properties: { title } },
          signal: abortSignal,
        }),
      );
      return { spreadsheetId: sheet.spreadsheetId, link: sheet.spreadsheetUrl };
    },
  }),
};
