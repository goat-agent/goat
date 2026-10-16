import { randomUUID } from "node:crypto";
import { closeSync, openSync } from "node:fs";
import { mkdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { defineTool, type Tool, type ToolOutput } from "@goat/tool";
import { z } from "zod";

export interface BashOptions {
  readonly cwd: string;
  readonly outputDir: string;
}

const defaultTimeout = 120_000;
const inlineBytes = 16_000;

export function bash(options: BashOptions): Tool {
  return defineTool({
    name: "bash",
    description: [
      "Runs a command with `bash -c` in the workspace directory and returns its stdout and stderr, interleaved as they were written, followed by the exit code.",
      "Each call starts a new shell. `cd`, variables and background processes do not carry over, so chain dependent commands with `&&`. Processes that the command leaves running are stopped when it exits.",
      "The command has no stdin, so a command that waits for input fails instead of waiting.",
      `Output longer than ${inlineBytes} bytes is shortened to its start and end, and the full output is saved to a file whose path is given.`,
    ].join("\n\n"),
    input: z.object({
      command: z.string().min(1).describe("The command to run."),
      timeout: z
        .number()
        .int()
        .positive()
        .optional()
        .describe(
          `Milliseconds before the command and its child processes are stopped. Defaults to ${defaultTimeout}.`,
        ),
    }),
    run: async ({ command, timeout }, { signal }) =>
      execute(options, command, timeout ?? defaultTimeout, signal),
  });
}

async function execute(
  options: BashOptions,
  command: string,
  timeout: number,
  signal: AbortSignal,
): Promise<ToolOutput> {
  if (signal.aborted) {
    return text("The command did not run because the call was aborted.", true);
  }
  const started = await start(options, command);
  if (typeof started === "string") {
    return text(`bash did not start: ${started}`, true);
  }
  const { child, path } = started;
  const stoppedBy = await supervise(child.pid, child.exited, timeout, signal);
  const output = await readOutput(path);
  const status =
    stoppedBy === "timeout"
      ? `Stopped after ${timeout} ms.`
      : stoppedBy === "abort"
        ? "Stopped because the call was aborted."
        : `exit ${child.exitCode ?? child.signalCode ?? "unknown"}`;
  const separator = output === "" || output.endsWith("\n") ? "" : "\n";
  return text(`${output}${separator}${status}`, stoppedBy !== undefined);
}

interface Started {
  readonly child: Bun.Subprocess;
  readonly path: string;
}

async function start(options: BashOptions, command: string): Promise<Started | string> {
  await mkdir(options.outputDir, { recursive: true });
  const path = join(options.outputDir, `bash-${randomUUID()}.log`);
  const fd = openSync(path, "w");
  try {
    const child = Bun.spawn(["bash", "-c", command], {
      cwd: options.cwd,
      stdin: "ignore",
      stdout: fd,
      stderr: fd,
      detached: true,
    });
    return { child, path };
  } catch (error) {
    await rm(path, { force: true });
    return error instanceof Error ? error.message : String(error);
  } finally {
    closeSync(fd);
  }
}

async function supervise(
  pid: number,
  exited: Promise<number>,
  timeout: number,
  signal: AbortSignal,
): Promise<"timeout" | "abort" | undefined> {
  let stoppedBy: "timeout" | "abort" | undefined;
  const stop = (reason: "timeout" | "abort"): void => {
    stoppedBy = reason;
    killGroup(pid);
  };
  const timer = setTimeout(() => {
    stop("timeout");
  }, timeout);
  const onAbort = (): void => {
    stop("abort");
  };
  signal.addEventListener("abort", onAbort, { once: true });
  await exited;
  clearTimeout(timer);
  signal.removeEventListener("abort", onAbort);
  killGroup(pid);
  return stoppedBy;
}

function killGroup(pid: number): void {
  try {
    process.kill(-pid, "SIGKILL");
  } catch {
    // ESRCH: every process in the group has already exited.
  }
}

async function readOutput(path: string): Promise<string> {
  const file = Bun.file(path);
  if (file.size <= inlineBytes) {
    const output = await file.text();
    await rm(path);
    return output;
  }
  const half = inlineBytes / 2;
  const head = await file.slice(0, half).text();
  const tail = await file.slice(file.size - half).text();
  return `${head}\n[${file.size - inlineBytes} bytes omitted. Full output: ${path}]\n${tail}`;
}

function text(value: string, isError: boolean): ToolOutput {
  return { parts: [{ type: "text", text: value }], isError };
}
