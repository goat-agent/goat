import { beforeEach, describe, expect, test } from "bun:test";
import { eq, sql } from "drizzle-orm";
import { db } from "../src/db";
import { agents } from "../src/db/schema/agents";
import { files } from "../src/db/schema/files";
import { sweep } from "../src/lib/files";
import { llm, storedObjects } from "./fakes";
import {
  bearer,
  client,
  createAgent,
  listMessages,
  resetDatabase,
  signIn,
  status,
  waitForIdle,
} from "./helpers";

const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]);

let token: string;
let agentId: string;

beforeEach(async () => {
  await resetDatabase();
  storedObjects.clear();
  llm.reset();
  token = await signIn("minho@example.com", "민호");
  agentId = await createAgent(token);
});

function upload(file: File, agent = agentId, as = token) {
  return client.agents[":id"].files.$post({ param: { id: agent }, form: { file } }, bearer(as));
}

async function uploaded(file: File): Promise<string> {
  const response = await upload(file);
  if (!response.ok) throw new Error(`upload failed with ${response.status}`);
  return (await response.json()).file.id;
}

function send(json: { text?: string; files?: string[] }) {
  return client.agents[":id"].messages.$post({ param: { id: agentId }, json }, bearer(token));
}

/** What the model saw in the user message at `index`, the newest last. */
function seen(prompt: number, index: number) {
  const users = llm.prompts[prompt]?.filter((message) => message.role === "user") ?? [];
  return users
    .at(index)
    ?.content.map((part) =>
      part.type === "text" ? part.text : { file: part.mediaType, filename: part.filename },
    );
}

describe("uploads", () => {
  test("stores a supported file and serves it back", async () => {
    const id = await uploaded(new File([PNG], "screen.png", { type: "image/png" }));

    const response = await client.agents[":id"].files[":fileId"].$get(
      { param: { id: agentId, fileId: id } },
      bearer(token),
    );
    expect(status(response)).toBe(200);
    expect(response.headers.get("content-type")).toBe("image/png");
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(PNG);
  });

  test("rejects content the agent cannot read, whatever type the client claims", async () => {
    const zip = new File([new Uint8Array([0x50, 0x4b, 3, 4, 0, 0])], "a.png", {
      type: "image/png",
    });
    expect(status(await upload(zip))).toBe(400);

    const binary = new File([new Uint8Array([0xff, 0xfe, 0xfd])], "a.txt", { type: "text/plain" });
    expect(status(await upload(binary))).toBe(400);
    expect(status(await upload(new File([], "empty.txt")))).toBe(400);
    expect(storedObjects.size).toBe(0);
  });

  test("reads the type from the bytes and the name, not from the client", async () => {
    const png = await upload(new File([PNG], "screen", { type: "text/plain" }));
    const csv = await upload(
      new File(["a,b\n1,2"], "data.csv", { type: "application/octet-stream" }),
    );
    if (!png.ok || !csv.ok) throw new Error("upload failed");
    expect((await png.json()).file.mediaType).toBe("image/png");
    expect((await csv.json()).file.mediaType).toBe("text/csv");
  });

  test("hides another user's files", async () => {
    const id = await uploaded(new File([PNG], "screen.png", { type: "image/png" }));
    const other = await signIn("other@example.com", "다른 사람");

    const response = await client.agents[":id"].files[":fileId"].$get(
      { param: { id: agentId, fileId: id } },
      bearer(other),
    );
    expect(status(response)).toBe(404);
    expect(
      status(await upload(new File([PNG], "x.png", { type: "image/png" }), agentId, other)),
    ).toBe(404);
  });
});

describe("messages with files", () => {
  test("shows the files with the message and gives the model the whole file in that turn", async () => {
    const id = await uploaded(new File([PNG], "screen.png", { type: "image/png" }));
    llm.reply = () => Promise.resolve("스크린샷이네요");

    expect(status(await send({ text: "이거 봐 줘", files: [id] }))).toBe(202);
    await waitForIdle(agentId);

    const [sent] = await listMessages(token, agentId);
    expect(sent?.text).toBe("이거 봐 줘");
    expect(sent?.files).toEqual([{ id, name: "screen.png", mediaType: "image/png" }]);
    expect(seen(0, -1)?.slice(-2)).toEqual([
      { file: "image/png", filename: "screen.png" },
      "이거 봐 줘",
    ]);
  });

  test("sends a file without text", async () => {
    const id = await uploaded(new File(["회의록"], "notes.txt", { type: "text/plain" }));

    expect(status(await send({ files: [id] }))).toBe(202);
    await waitForIdle(agentId);

    const [sent] = await listMessages(token, agentId);
    expect(sent?.text).toBe("");
    expect(sent?.files?.map((file) => file.name)).toEqual(["notes.txt"]);
    expect(seen(0, -1)?.at(-1)).toBe('File "notes.txt":\n\n회의록');
  });

  test("turns earlier files into a note the agent can open with read_file", async () => {
    const id = await uploaded(new File([PNG], "screen.png", { type: "image/png" }));
    await send({ text: "이거 봐 줘", files: [id] });
    await waitForIdle(agentId);

    llm.reply = () =>
      Promise.resolve(
        llm.prompts.length === 2 ? { toolName: "read_file", input: { id } } : "다시 봤어요",
      );
    await send({ text: "아까 그거 다시" });
    await waitForIdle(agentId);

    const earlier = seen(1, 0)?.find((part) => typeof part === "string" && part.includes(id));
    expect(earlier).toContain("read_file");
    const toolResult = llm.prompts[2]
      ?.find((message) => message.role === "tool")
      ?.content.find((part) => part.type === "tool-result");
    expect(toolResult?.type === "tool-result" ? toolResult.output : undefined).toMatchObject({
      type: "content",
      value: [{ type: "file", mediaType: "image/png", data: { type: "data" } }],
    });
  });

  test("refuses files that are not uploads to this agent", async () => {
    const otherAgent = await createAgent(token);
    const elsewhere = await uploaded(new File([PNG], "a.png", { type: "image/png" }));
    await db.update(files).set({ agentId: otherAgent }).where(eq(files.id, elsewhere));

    expect(status(await send({ files: [elsewhere] }))).toBe(400);
    expect(status(await send({ files: [crypto.randomUUID()] }))).toBe(400);
    expect(status(await send({}))).toBe(400);
  });
});

describe("sweep", () => {
  test("deletes files never sent after a day and files of deleted agents", async () => {
    const unsent = await uploaded(new File([PNG], "a.png", { type: "image/png" }));
    const fresh = await uploaded(new File([PNG], "b.png", { type: "image/png" }));
    const sent = await uploaded(new File([PNG], "c.png", { type: "image/png" }));
    await send({ files: [sent] });
    await waitForIdle(agentId);
    await db
      .update(files)
      .set({ createdAt: sql`now() - interval '2 days'` })
      .where(eq(files.id, unsent));

    await sweep();
    expect([...storedObjects.keys()].toSorted()).toEqual([fresh, sent].toSorted());

    await db.delete(agents).where(eq(agents.id, agentId));
    await sweep();
    expect(storedObjects.size).toBe(0);
    expect(await db.select().from(files)).toEqual([]);
  });
});
