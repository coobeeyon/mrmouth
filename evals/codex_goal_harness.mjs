#!/usr/bin/env node
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

const args = parseArgs(process.argv.slice(2));
const cwd = resolve(required(args.cwd, "--cwd"));
const output = resolve(required(args.output, "--output"));
const goal = await readFile(resolve(required(args.goalFile, "--goal-file")), "utf8");
const prompt = args.promptFile
  ? await readFile(resolve(args.promptFile), "utf8")
  : "Work on the active goal until it is complete. When done, mark the goal complete.";
const rawEventsPath = args.rawEvents ? resolve(args.rawEvents) : null;
const timeoutMs = Number(args.timeoutMs || 900000);

const started = Date.now();
const events = [];
const eventCounts = {};
let nextId = 1;
const pending = new Map();
let threadId = null;
let turnId = null;
let finalGoal = null;
let turnCompleted = null;
let turnFailed = null;
let stdoutBytes = 0;
let stderrBytes = 0;
let exited = false;

const proc = spawn(
  "codex",
  [
    "app-server",
    "--stdio",
    "--enable",
    "goals",
    "-c",
    'sandbox="danger-full-access"',
    "-c",
    'approval_policy="never"',
  ],
  {
    cwd,
    env: process.env,
    stdio: ["pipe", "pipe", "pipe"],
  },
);

proc.stderr.on("data", (chunk) => {
  stderrBytes += chunk.length;
  process.stderr.write(chunk);
});

const rl = createInterface({ input: proc.stdout });
rl.on("line", (line) => {
  stdoutBytes += Buffer.byteLength(line) + 1;
  if (!line.trim()) return;
  let message;
  try {
    message = JSON.parse(line);
  } catch (error) {
    record("parse_error", { line, error: String(error) });
    return;
  }

  if (message.method) {
    record(message.method, message.params ?? {});
    if (message.method === "thread/goal/updated") {
      finalGoal = message.params?.goal ?? finalGoal;
    } else if (message.method === "turn/completed") {
      turnCompleted = message.params ?? {};
      turnId = turnCompleted.turn?.id ?? turnId;
    } else if (message.method === "turn/failed") {
      turnFailed = message.params ?? {};
      turnId = turnFailed.turn?.id ?? turnId;
    }
    return;
  }

  if (Object.prototype.hasOwnProperty.call(message, "id")) {
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    if (message.error) {
      waiter.reject(new Error(JSON.stringify(message.error)));
    } else {
      waiter.resolve(message.result);
    }
  }
});

proc.on("exit", () => {
  exited = true;
});

try {
  await request("initialize", {
    clientInfo: {
      name: "mrmouth_eval_goal_harness",
      title: "Mr Mouth Eval Goal Harness",
      version: "0.1.0",
    },
    capabilities: { experimentalApi: true },
  });
  notify("initialized", {});

  const thread = await request("thread/start", {
    cwd,
    sandbox: "danger-full-access",
    approvalPolicy: "never",
    config: { features: { goals: true } },
    threadSource: "mrmouth_eval_goal",
  });
  threadId = thread.thread?.id;
  if (!threadId) {
    throw new Error(`thread/start did not return a thread id: ${JSON.stringify(thread)}`);
  }

  const goalResult = await request("thread/goal/set", {
    threadId,
    objective: goal.trim(),
    status: "active",
  });
  finalGoal = goalResult.goal ?? finalGoal;

  const turn = await request("turn/start", {
    threadId,
    cwd,
    approvalPolicy: "never",
    sandboxPolicy: { type: "dangerFullAccess" },
    input: [{ type: "text", text: prompt }],
  });
  turnId = turn.turn?.id ?? turnId;

  await waitForTurn();
  const currentGoal = await request("thread/goal/get", { threadId });
  finalGoal = currentGoal.goal ?? finalGoal;

  const success = !turnFailed && finalGoal?.status === "complete";
  await writeReport(success, null);
  proc.stdin.end();
  proc.kill("SIGTERM");
  process.exit(success ? 0 : 1);
} catch (error) {
  await writeReport(false, String(error?.stack || error));
  proc.stdin.end();
  if (!exited) proc.kill("SIGTERM");
  process.exit(1);
}

function request(method, params) {
  const id = nextId++;
  const payload = { id, method, params };
  proc.stdin.write(`${JSON.stringify(payload)}\n`);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`timed out waiting for ${method} response`));
    }, timeoutMs);
    pending.set(id, {
      resolve: (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      reject: (error) => {
        clearTimeout(timer);
        reject(error);
      },
    });
  });
}

function notify(method, params) {
  proc.stdin.write(`${JSON.stringify({ method, params })}\n`);
}

function record(type, payload) {
  eventCounts[type] = (eventCounts[type] || 0) + 1;
  events.push({ type, payload });
}

function waitForTurn() {
  return new Promise((resolveWait, rejectWait) => {
    const deadline = Date.now() + timeoutMs;
    const timer = setInterval(() => {
      if (turnCompleted || turnFailed) {
        clearInterval(timer);
        resolveWait();
      } else if (Date.now() > deadline) {
        clearInterval(timer);
        rejectWait(new Error("timed out waiting for turn completion"));
      } else if (exited) {
        clearInterval(timer);
        rejectWait(new Error("codex app-server exited before turn completion"));
      }
    }, 250);
  });
}

async function writeReport(success, failure) {
  const wallMs = Date.now() - started;
  const report = {
    harness: "codex-goal-app-server",
    cwd,
    success,
    failure,
    wall_ms: wallMs,
    stdout_bytes: stdoutBytes,
    stderr_bytes: stderrBytes,
    thread_id: threadId,
    turn_id: turnId,
    goal: {
      objective: goal.trim(),
      final: finalGoal,
    },
    turn: {
      completed: turnCompleted,
      failed: turnFailed,
    },
    app_server: {
      event_counts: eventCounts,
      raw_events_path: rawEventsPath,
    },
  };

  await mkdir(dirname(output), { recursive: true });
  await writeFile(output, `${JSON.stringify(report, null, 2)}\n`);
  if (rawEventsPath) {
    await mkdir(dirname(rawEventsPath), { recursive: true });
    await writeFile(rawEventsPath, `${events.map((e) => JSON.stringify(e)).join("\n")}\n`);
  }
  console.log(`goal eval report: ${output}`);
}

function parseArgs(argv) {
  const parsed = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (!arg.startsWith("--")) {
      throw new Error(`unexpected argument: ${arg}`);
    }
    const key = arg.slice(2).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
    const value = argv[i + 1];
    if (!value || value.startsWith("--")) {
      throw new Error(`missing value for ${arg}`);
    }
    parsed[key] = value;
    i += 1;
  }
  return parsed;
}

function required(value, name) {
  if (!value) {
    throw new Error(`missing ${name}`);
  }
  return value;
}
