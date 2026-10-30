import { beforeEach, describe, expect, test } from "bun:test";
import { llm, voiceService } from "./fakes";
import {
  bearer,
  client,
  createAgent,
  listMessages,
  resetDatabase,
  send,
  signIn,
  status,
  waitFor,
  waitForIdle,
} from "./helpers";

let token: string;
let agentId: string;

beforeEach(async () => {
  await resetDatabase();
  llm.reset();
  voiceService.reset();
  token = await signIn("minho@example.com", "민호");
  agentId = await createAgent(token);
});

function call(agent = agentId, as = token, language = "ko-KR") {
  const headers = { ...bearer(as).headers, "accept-language": language };
  return client.agents[":id"].calls.$post(
    { param: { id: agent }, json: { sdp: "offer" } },
    { headers },
  );
}

function voice() {
  const { handlers } = voiceService;
  if (!handlers) throw new Error("no call attached");
  return handlers;
}

describe("calls", () => {
  test("answers the device's offer and sets the voice up with the conversation", async () => {
    llm.reply = () => Promise.resolve("안녕하세요");
    await send(token, agentId, "안녕");
    await waitForIdle(agentId);

    const response = await call();

    if (!response.ok) throw new Error(`call failed with ${response.status}`);
    expect(await response.json()).toEqual({ sdp: "answer to offer" });
    const [setup] = voiceService.setups;
    expect(setup?.locale).toBe("ko");
    expect(setup?.history).toEqual([
      { role: "user", text: "안녕" },
      { role: "assistant", text: "안녕하세요" },
    ]);
    expect(setup?.instructions).toContain("You are Goat, 민호's personal agent");
  });

  test("hands what the user said to the agent and reports its reply", async () => {
    llm.reply = () => Promise.resolve("내일 3시에 회의가 있어요.");
    await call();

    voice().onUserSpeech("내일 ");
    voice().onUserSpeech("일정 알려 줘");
    voice().onTask("task-1");
    await waitFor(() => voiceService.reports.length > 0);

    expect(voiceService.reports).toEqual([{ taskId: "task-1", text: "내일 3시에 회의가 있어요." }]);
    const messages = await listMessages(token, agentId);
    expect(messages.map(({ role, text }) => ({ role, text }))).toEqual([
      { role: "user", text: "내일 일정 알려 줘" },
      { role: "assistant", text: "내일 3시에 회의가 있어요." },
    ]);
  });

  test("asks again when it heard nothing", async () => {
    await call();

    voice().onTask("task-1");
    await waitFor(() => voiceService.reports.length > 0);

    expect(voiceService.reports[0]?.text).toContain("did not come through");
    expect(llm.prompts).toEqual([]);
  });

  test("hides other people's agents", async () => {
    const other = await signIn("other@example.com", "Other");

    expect(status(await call(agentId, other))).toBe(404);
    expect(voiceService.setups).toEqual([]);
  });
});
