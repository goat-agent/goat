import { describe, expect, test } from "bun:test";
import { z } from "zod";
import { gmailTools } from "../src/lib/ai/tools/gmail";
import { googleCalendarTools } from "../src/lib/ai/tools/google-calendar";
import { googleScopes } from "../src/lib/ai/tools/google";
import { googleDriveTools } from "../src/lib/ai/tools/google-drive";
import { youtubeTools } from "../src/lib/ai/tools/youtube";
import { google as workspace } from "../src/lib/integrations/google";
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

describe("youtube", () => {
  const video = {
    id: "v1",
    snippet: { title: "Bun 소개", channelTitle: "Bun", channelId: "c1", description: "설명" },
    contentDetails: { duration: "PT4M13S" },
    statistics: { viewCount: "1200", likeCount: "80" },
  };

  test("searches videos with a small page and marks titles as someone else's words", async () => {
    const google = fakeGoogle(() => ({
      items: [
        { id: { videoId: "v1" }, snippet: { title: "Bun 소개", channelTitle: "Bun" } },
        { id: {}, snippet: { title: "id 없음" } },
      ],
      nextPageToken: "next",
    }));
    const found = await youtubeTools.youtube_search.execute(
      { query: "bun runtime", type: "video", maxResults: 5 },
      google.options,
    );

    const params = google.calls[0]?.url.searchParams;
    expect(google.calls[0]?.url.pathname).toEndWith("/youtube/v3/search");
    expect(params?.get("q")).toBe("bun runtime");
    expect(params?.get("type")).toBe("video");
    expect(params?.get("maxResults")).toBe("5");
    expect(params?.get("part")).toBe("snippet");
    expect(found).toMatchObject({ results: [{ type: "video", id: "v1" }], nextPageToken: "next" });
    expect(JSON.stringify(found)).toMatch(/<google-content id=\\"\w+\\">\\nBun 소개\\n/);
  });

  test("lists liked videos with their counts", async () => {
    const google = fakeGoogle(() => ({ items: [video], nextPageToken: "next" }));
    const liked = await youtubeTools.youtube_list_liked_videos.execute(
      { maxResults: 25 },
      google.options,
    );

    expect(google.calls[0]?.url.searchParams.get("myRating")).toBe("like");
    expect(liked).toMatchObject({
      videos: [{ id: "v1", duration: "PT4M13S", views: 1200, likes: 80 }],
      nextPageToken: "next",
    });
    expect(JSON.stringify(liked)).not.toContain("description");
  });

  test("cuts a long description and says so", async () => {
    const long = { ...video, snippet: { ...video.snippet, description: "가".repeat(3_000) } };
    const google = fakeGoogle(() => ({ items: [long] }));
    const found = await youtubeTools.youtube_get_videos.execute(
      { videoIds: ["v1", "v2"] },
      google.options,
    );

    expect(google.calls[0]?.url.searchParams.get("id")).toBe("v1,v2");
    expect(found).toMatchObject({ videos: [{ descriptionTruncated: true }] });
  });

  test("adds a video to a playlist", async () => {
    const google = fakeGoogle(() => ({ id: "item1" }));
    const added = await youtubeTools.youtube_add_to_playlist.execute(
      { playlistId: "pl1", videoId: "v1" },
      google.options,
    );

    expect(google.calls[0]?.method).toBe("POST");
    expect(google.calls[0]?.url.pathname).toEndWith("/playlistItems");
    expect(google.calls[0]?.body).toEqual({
      snippet: { playlistId: "pl1", resourceId: { kind: "youtube#video", videoId: "v1" } },
    });
    expect(added).toEqual({ itemId: "item1", playlistId: "pl1", videoId: "v1" });
  });

  test("creates a private playlist by default", async () => {
    const google = fakeGoogle(() => ({
      id: "pl1",
      snippet: { title: "나중에 볼 것" },
      status: { privacyStatus: "private" },
    }));
    await youtubeTools.youtube_create_playlist.execute(
      { title: "나중에 볼 것", privacy: "private" },
      google.options,
    );

    expect(google.calls[0]?.body).toEqual({
      snippet: { title: "나중에 볼 것" },
      status: { privacyStatus: "private" },
    });
  });

  test("rates a video", async () => {
    const google = fakeGoogle(() => ({}));
    await youtubeTools.youtube_rate_video.execute(
      { videoId: "v1", rating: "like" },
      google.options,
    );

    expect(google.calls[0]?.method).toBe("POST");
    expect(google.calls[0]?.url.pathname).toEndWith("/videos/rate");
    expect(google.calls[0]?.url.searchParams.get("rating")).toBe("like");
  });

  test("every tool needs the one youtube scope, which Google asks for at connect", () => {
    for (const found of Object.values(youtubeTools)) {
      expect(found.metadata).toMatchObject({ scope: googleScopes.youtube });
    }
    expect(workspace.auth.scopes).toContain(googleScopes.youtube);
  });
});
