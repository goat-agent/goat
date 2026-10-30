import { describe, expect, test } from "bun:test";
import { z } from "zod";
import { gmailTools } from "../src/lib/ai/tools/gmail";
import { googleCalendarTools } from "../src/lib/ai/tools/google-calendar";
import { googleDriveTools } from "../src/lib/ai/tools/google-drive";
import type { Fetch } from "../src/lib/integrations/integration";

type Call = { method: string; url: URL; body: unknown };

function fakeGoogle(answer: (call: Call) => unknown) {
  const calls: Call[] = [];
  const fetch: Fetch = (input, init) => {
    const href = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const text = typeof init?.body === "string" ? init.body : undefined;
    const call: Call = {
      method: init?.method ?? "GET",
      url: new URL(href),
      body: text === undefined ? undefined : JSON.parse(text),
    };
    calls.push(call);
    const reply = answer(call);
    return Promise.resolve(typeof reply === "string" ? new Response(reply) : Response.json(reply));
  };
  return { calls, options: { toolCallId: "call", messages: [], context: { fetch } } };
}

const base64url = (text: string) => Buffer.from(text).toString("base64url");

describe("gmail", () => {
  test("reads the plain text part and marks it as someone else's words", async () => {
    const google = fakeGoogle(() => ({
      id: "m1",
      threadId: "t1",
      labelIds: ["INBOX"],
      payload: {
        mimeType: "multipart/alternative",
        headers: [{ name: "Subject", value: "회의" }],
        parts: [
          { mimeType: "text/plain", body: { data: base64url("내일 3시에 봬요") } },
          { mimeType: "text/html", body: { data: base64url("<p>내일 3시에 봬요</p>") } },
        ],
      },
    }));
    const read = await gmailTools.google_gmail_read.execute({ messageId: "m1" }, google.options);
    expect(read).toMatchObject({ id: "m1", subject: "회의", truncated: false });
    expect(JSON.stringify(read)).toMatch(/<google-content id=\\"\w+\\">\\n내일 3시에 봬요/);
  });

  test("turns an HTML-only body into text", async () => {
    const google = fakeGoogle(() => ({
      id: "m1",
      threadId: "t1",
      payload: {
        mimeType: "text/html",
        body: { data: base64url("<div>안녕&nbsp;하세요<br>민호</div><style>p{}</style>") },
      },
    }));
    const read = await gmailTools.google_gmail_read.execute({ messageId: "m1" }, google.options);
    expect(JSON.stringify(read)).toContain("안녕 하세요\\n민호");
  });

  test("replies in the original thread with its subject and references", async () => {
    const google = fakeGoogle((call) =>
      call.method === "GET"
        ? {
            id: "m1",
            threadId: "t1",
            payload: {
              headers: [
                { name: "Subject", value: "회의" },
                { name: "Message-ID", value: "<abc@mail>" },
              ],
            },
          }
        : { id: "m2", threadId: "t1" },
    );
    await gmailTools.google_gmail_send.execute(
      { to: ["boss@example.com"], body: "네, 갈게요", replyToMessageId: "m1" },
      google.options,
    );

    const sent = google.calls.find((call) => call.method === "POST");
    expect(sent?.url.pathname).toEndWith("/messages/send");
    const body = z.object({ raw: z.string(), threadId: z.string() }).parse(sent?.body);
    const mime = Buffer.from(body.raw, "base64url").toString("utf8");
    expect(body.threadId).toBe("t1");
    expect(mime).toContain("In-Reply-To: <abc@mail>");
    expect(mime).toContain(`Subject: =?UTF-8?B?${Buffer.from("Re: 회의").toString("base64")}?=`);
    expect(mime).toContain(Buffer.from("네, 갈게요").toString("base64"));
  });
});

describe("drive", () => {
  test("exports a Google Doc as text", async () => {
    const google = fakeGoogle((call) =>
      call.url.pathname.endsWith("/export")
        ? "회의록 본문"
        : { id: "d1", name: "회의록", mimeType: "application/vnd.google-apps.document" },
    );
    const read = await googleDriveTools.google_drive_read.execute({ fileId: "d1" }, google.options);
    expect(google.calls[1]?.url.searchParams.get("mimeType")).toBe("text/plain");
    expect(JSON.stringify(read)).toContain("회의록 본문");
  });

  test("does not download a file that is not text", async () => {
    const google = fakeGoogle(() => ({ id: "p1", name: "사진", mimeType: "image/jpeg" }));
    const read = await googleDriveTools.google_drive_read.execute({ fileId: "p1" }, google.options);
    expect(read).toMatchObject({ content: null });
    expect(google.calls).toHaveLength(1);
  });
});

describe("calendar", () => {
  test("deleting an event tells its guests", async () => {
    const google = fakeGoogle(() => ({}));
    await googleCalendarTools.google_calendar_delete_event.execute(
      { eventId: "e1" },
      google.options,
    );
    expect(google.calls[0]?.method).toBe("DELETE");
    expect(google.calls[0]?.url.searchParams.get("sendUpdates")).toBe("all");
  });
});
