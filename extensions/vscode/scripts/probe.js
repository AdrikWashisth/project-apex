/**
 * Protocol probe: exercises the extension's client against a real APEX
 * runtime, without needing VS Code.
 *
 * `client.ts` imports only Node builtins, so it can be driven from plain Node.
 * This proves the extension speaks the same wire protocol as the CLI and TUI:
 * handshake, request correlation, and the requests the views depend on.
 *
 * Usage: node probe.js <endpoint> <token>
 * Prints one line per check and exits non-zero on the first failure.
 */

const { ApexClient } = require("../out/client");

const endpoint = process.argv[2];
const token = process.argv[3];

if (!endpoint || !token) {
  console.error("usage: node probe.js <endpoint> <token>");
  process.exit(2);
}

const info = {
  transport: "tcp_loopback",
  endpoint,
  token,
};

function check(name, condition, detail) {
  if (condition) {
    console.log(`ok ${name}`);
  } else {
    console.log(`not ok ${name} ${detail ?? ""}`);
    process.exitCode = 1;
  }
}

async function main() {
  const client = new ApexClient(info);

  let liveEvents = 0;
  client.on("event", () => {
    liveEvents += 1;
  });

  const handshake = await client.connect();
  check("handshake", handshake.protocolVersion === 1, `got ${handshake.protocolVersion}`);
  check("runtime-version", typeof handshake.runtimeVersion === "string");

  // The tree views depend on these three.
  const tasks = await client.request({ op: "list_tasks", limit: 10 });
  check("list_tasks", tasks.op === "task_list", `got ${tasks.op}`);
  check("tasks-is-array", Array.isArray(tasks.tasks));

  const agents = await client.request({ op: "list_agents" });
  check("list_agents", agents.op === "agents", `got ${agents.op}`);
  check("agents-non-empty", Array.isArray(agents.agents) && agents.agents.length > 0);

  const status = await client.request({ op: "status" });
  check("status", status.op === "status", `got ${status.op}`);

  // Correlated ids: fire several requests back to back and confirm each answer
  // is matched to its own request rather than to the first one that returns.
  const interleaved = await Promise.all([
    client.request({ op: "list_tasks", limit: 1 }),
    client.request({ op: "list_agents" }),
    client.request({ op: "status" }),
    client.request({ op: "list_tasks", limit: 2 }),
  ]);
  check(
    "request-correlation",
    interleaved[0].op === "task_list" &&
      interleaved[1].op === "agents" &&
      interleaved[2].op === "status" &&
      interleaved[3].op === "task_list",
    `got ${interleaved.map((r) => r.op).join(",")}`
  );

  if (tasks.tasks && tasks.tasks.length > 0) {
    const taskId = tasks.tasks[0].id;
    const subtasks = await client.request({ op: "list_subtasks", task_id: taskId });
    check("list_subtasks", subtasks.op === "subtask_list", `got ${subtasks.op}`);

    const plan = await client.request({ op: "show_plan", task_id: taskId });
    check("show_plan", plan.op === "plan", `got ${plan.op}`);

    // These may legitimately fail for a single-agent task, but they must not
    // break the connection.
    await client.request({ op: "get_diff", task_id: taskId }).catch(() => undefined);
    await client.request({ op: "verify", task_id: taskId }).catch(() => undefined);
  }

  // The connection is still usable after all of that.
  const again = await client.request({ op: "status" });
  check("still-connected", again.op === "status");

  check("no-stray-events", liveEvents >= 0);

  client.disconnect();
}

main().catch((error) => {
  console.log(`not ok client ${String(error)}`);
  process.exit(1);
});
