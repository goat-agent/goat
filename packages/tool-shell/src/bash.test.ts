import { afterEach, beforeEach, expect, test } from "bun:test";
import { mkdtemp, readdir, readFile, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Tool, ToolOutput } from "@goat/tool";
import { bash } from "./bash.ts";

let root = "";
let workspace = "";
let outputDir = "";
let tool: Tool;

beforeEach(async () => {
  root = await realpath(await mkdtemp(join(tmpdir(), "goat-bash-")));
  workspace = join(root, "workspace");
  outputDir = join(root, "output");
  await Bun.write(join(workspace, ".keep"), "");
  tool = bash({ cwd: workspace, outputDir });
});

afterEach(async () => {
  await rm(root, { recursive: true, force: true });
});

async function run(
  command: string,
  options: { readonly timeout?: number; readonly signal?: AbortSignal } = {},
): Promise<ToolOutput> {
  const input = options.timeout === undefined ? { command } : { command, timeout: options.timeout };
  return tool.call(input, { signal: options.signal ?? new AbortController().signal });
}

function textOf(output: ToolOutput): string {
  return output.parts.map((part) => (part.type === "text" ? part.text : "")).join("");
}

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function backgroundSurvives(): Promise<boolean> {
  const pid = Number(await readFile(join(workspace, "bg.pid"), "utf8"));
  return survives(pid, performance.now() + 1_000);
}

async function survives(pid: number, deadline: number): Promise<boolean> {
  if (!isAlive(pid)) {
    return false;
  }
  if (performance.now() >= deadline) {
    return true;
  }
  await Bun.sleep(10);
  return survives(pid, deadline);
}

test("returns stdout and stderr in the order they were written, then the exit code", async () => {
  expect(await run("echo one; echo two >&2; echo three")).toEqual({
    parts: [{ type: "text", text: "one\ntwo\nthree\nexit 0" }],
    isError: false,
  });
});

test("reports a failing exit code as output, not as a tool error", async () => {
  const output = await run("echo failed; exit 3");
  expect(textOf(output)).toBe("failed\nexit 3");
  expect(output.isError).toBe(false);
});

test("starts every call in the workspace without carrying over cd", async () => {
  expect(textOf(await run("pwd; cd /; pwd"))).toBe(`${workspace}\n/\nexit 0`);
  expect(textOf(await run("pwd"))).toBe(`${workspace}\nexit 0`);
});

test("gives the command no stdin", async () => {
  expect(textOf(await run('read line; echo "read exited $?"'))).toBe("read exited 1\nexit 0");
});

test("stops the command and its children at the timeout", async () => {
  const started = performance.now();
  const output = await run("sleep 30 & echo $! > bg.pid; sleep 30", { timeout: 200 });
  expect(performance.now() - started).toBeLessThan(5_000);
  expect(output).toEqual({
    parts: [{ type: "text", text: "Stopped after 200 ms." }],
    isError: true,
  });
  expect(await backgroundSurvives()).toBe(false);
});

test("stops the command and its children when the call is aborted", async () => {
  const controller = new AbortController();
  setTimeout(() => {
    controller.abort();
  }, 200);
  const output = await run("echo started; sleep 30 & echo $! > bg.pid; sleep 30", {
    signal: controller.signal,
  });
  expect(output).toEqual({
    parts: [{ type: "text", text: "started\nStopped because the call was aborted." }],
    isError: true,
  });
  expect(await backgroundSurvives()).toBe(false);
});

test("does not run when the call is already aborted", async () => {
  const controller = new AbortController();
  controller.abort();
  const output = await run("touch ran", { signal: controller.signal });
  expect(output.isError).toBe(true);
  expect(await Bun.file(join(workspace, "ran")).exists()).toBe(false);
});

test("stops processes the command leaves running", async () => {
  expect(textOf(await run("sleep 30 & echo $! > bg.pid"))).toBe("exit 0");
  expect(await backgroundSurvives()).toBe(false);
});

test("shortens long output and keeps the full output in a file", async () => {
  const output = textOf(await run("head -c 40000 /dev/zero | tr '\\0' a; echo; echo end"));
  const path = /Full output: (\S+)\]/u.exec(output)?.[1] ?? "";
  expect(output.startsWith("a".repeat(8_000))).toBe(true);
  expect(output.endsWith("end\nexit 0")).toBe(true);
  expect(output).toContain("[24005 bytes omitted. Full output:");
  expect(path.startsWith(outputDir)).toBe(true);
  expect(await readFile(path, "utf8")).toBe(`${"a".repeat(40_000)}\nend\n`);
});

test("leaves no file behind when the output fits", async () => {
  await run("echo short");
  expect(await readdir(outputDir)).toEqual([]);
});

test("reports a command that cannot start as a tool error", async () => {
  const missing = bash({ cwd: join(root, "missing"), outputDir });
  const output = await missing.call({ command: "true" }, { signal: new AbortController().signal });
  expect(output.isError).toBe(true);
  expect(textOf(output)).toStartWith("bash did not start:");
});
