import { beforeEach, describe, expect, test } from "bun:test";
import { eq } from "drizzle-orm";
import { db } from "../src/db";
import { users } from "../src/db/schema/auth";
import { saveConnection } from "../src/lib/integrations/connections";
import { llm, transcriber } from "./fakes";
import {
  bearer,
  client,
  createAgent,
  resetDatabase,
  send,
  signIn,
  status,
  waitForIdle,
} from "./helpers";

// The start of an M4A file: a size, then "ftyp".
const M4A = new Uint8Array([0, 0, 0, 0x20, 0x66, 0x74, 0x79, 0x70, 0x4d, 0x34, 0x41, 0x20]);

let token: string;
let agentId: string;

beforeEach(async () => {
  await resetDatabase();
  llm.reset();
  transcriber.reset();
  token = await signIn("minho@example.com", "민호");
  agentId = await createAgent(token);
});

function transcribe(bytes: Uint8Array, agent = agentId, as = token) {
  const audio = new File([bytes], "recording.m4a", { type: "audio/mp4" });
  return client.agents[":id"].transcriptions.$post(
    { param: { id: agent }, form: { audio } },
    bearer(as),
  );
}

describe("transcriptions", () => {
  test("returns what was said", async () => {
    transcriber.text = "노션에 회의록 정리해 줘";

    const response = await transcribe(M4A);

    if (!response.ok) throw new Error(`transcribe failed with ${response.status}`);
    expect(await response.json()).toEqual({ text: "노션에 회의록 정리해 줘" });
  });

  test("hints at the names and the recent conversation", async () => {
    const [user] = await db
      .select({ id: users.id })
      .from(users)
      .where(eq(users.email, "minho@example.com"));
    if (!user) throw new Error("no user");
    await saveConnection(
      user.id,
      "testhub",
      { id: "1", name: "mino_acme" },
      { type: "oauth", accessToken: "token" },
    );
    llm.reply = () => Promise.resolve("PR 목록을 볼게요");
    await send(token, agentId, "깃허브 PR 좀 봐 줘");
    await waitForIdle(agentId);

    await transcribe(M4A);

    expect(transcriber.hints).toEqual([
      {
        context: "깃허브 PR 좀 봐 줘\nPR 목록을 볼게요",
        keywords: ["민호", "Goat", "TestHub", "mino_acme"],
      },
    ]);
  });

  test("refuses what is not a recording", async () => {
    const response = await transcribe(new TextEncoder().encode("hello"));

    expect(status(response)).toBe(400);
    expect(transcriber.hints).toEqual([]);
  });

  test("hides other people's agents", async () => {
    const other = await signIn("other@example.com", "Other");

    expect(status(await transcribe(M4A, agentId, other))).toBe(404);
  });
});
