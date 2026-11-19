(host) => {
  const call = async (request) => {
    const reply = JSON.parse(await host(JSON.stringify(request)));
    if ("error" in reply) throw new Error(reply.error);
    return reply.ok;
  };
  const quote = (value) => `'${String(value).replaceAll("'", `'\\''`)}'`;
  const words = (value) => (Array.isArray(value) ? value.map(quote).join(" ") : quote(value));
  const bash = (command, ...rest) => {
    if (Array.isArray(command) && Array.isArray(command.raw)) {
      const text = command.reduce((text, part, i) => text + words(rest[i - 1]) + part);
      return call({ op: "bash", command: text, timeout: null });
    }
    return call({ op: "bash", command: String(command), timeout: rest[0]?.timeout ?? null });
  };
  const read = (path, { offset = null, limit = null } = {}) =>
    call({ op: "read", path: String(path), offset, limit });
  const edit = (path, edits) => call({ op: "edit", path: String(path), edits });
  const write = (path, content) => call({ op: "write", path: String(path), content: String(content) });

  const show = (value) => {
    if (typeof value === "string") return value;
    if (value instanceof Error) return value.stack ? `${value}\n${value.stack}` : String(value);
    return JSON.stringify(value, null, 2) ?? String(value);
  };
  const logged = [];
  const log = (...values) => {
    logged.push(values.map(show).join(" "));
  };
  globalThis.console = { log, info: log, warn: log, error: log, debug: log };
  Object.assign(globalThis, { bash, read, edit, write });

  return (value) => {
    const parts = logged.splice(0);
    if (value !== undefined) parts.push(show(value));
    return parts.join("\n");
  };
}
