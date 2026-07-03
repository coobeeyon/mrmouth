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
const excerptLimit = Number(args.excerptLimit || 2000);

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
const tokenUsageUpdates = [];
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
    } else if (message.method === "thread/tokenUsage/updated") {
      tokenUsageUpdates.push(message.params ?? {});
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
    token_usage: buildTokenUsage(),
    app_server: {
      event_counts: eventCounts,
      raw_events_path: rawEventsPath,
    },
    evidence: buildEvidence(),
  };

  await mkdir(dirname(output), { recursive: true });
  await writeFile(output, `${JSON.stringify(report, null, 2)}\n`);
  if (rawEventsPath) {
    await mkdir(dirname(rawEventsPath), { recursive: true });
    await writeFile(rawEventsPath, `${events.map((e) => JSON.stringify(e)).join("\n")}\n`);
  }
  console.log(`goal eval report: ${output}`);
}

function buildTokenUsage() {
  const latestUpdate =
    tokenUsageUpdates.length > 0 ? tokenUsageUpdates[tokenUsageUpdates.length - 1] : null;
  const turnUsage =
    normalizeUsage(turnCompleted?.usage) ??
    normalizeUsage(turnCompleted?.turn?.usage) ??
    null;
  const latestNormalized = normalizeUsage(latestUpdate);
  return {
    final_goal_tokens_used: numberOrNull(finalGoal?.tokensUsed),
    update_count: tokenUsageUpdates.length,
    latest_update: latestUpdate,
    latest_update_normalized: latestNormalized,
    turn_completed: turnUsage,
    comparable_total_tokens:
      turnUsage?.total_tokens ??
      latestNormalized?.total_tokens ??
      numberOrNull(finalGoal?.tokensUsed),
    comparable_total_uncached_tokens:
      turnUsage?.total_uncached_tokens ??
      latestNormalized?.total_uncached_tokens ??
      numberOrNull(finalGoal?.tokensUsed),
  };
}

function normalizeUsage(value) {
  if (!value || typeof value !== "object") return null;
  if (value.tokenUsage?.total && typeof value.tokenUsage.total === "object") {
    return normalizeUsage(value.tokenUsage.total);
  }
  if (value.total && typeof value.total === "object") {
    return normalizeUsage(value.total);
  }
  const inputTokens = firstNumber(value, [
    "input_tokens",
    "inputTokens",
    "input",
    "totalInputTokens",
  ]);
  const cachedInputTokens = firstNumber(value, [
    "cached_input_tokens",
    "cachedInputTokens",
    "cachedInput",
    "cacheReadInputTokens",
  ]);
  const outputTokens = firstNumber(value, [
    "output_tokens",
    "outputTokens",
    "output",
    "totalOutputTokens",
  ]);
  const reasoningOutputTokens = firstNumber(value, [
    "reasoning_output_tokens",
    "reasoningOutputTokens",
    "reasoningTokens",
    "reasoning",
  ]);
  const totalTokens = firstNumber(value, [
    "total_tokens",
    "totalTokens",
    "tokensUsed",
    "usedTokens",
  ]);

  if (
    inputTokens == null &&
    cachedInputTokens == null &&
    outputTokens == null &&
    reasoningOutputTokens == null &&
    totalTokens == null
  ) {
    return null;
  }

  const input = inputTokens ?? 0;
  const cached = cachedInputTokens ?? 0;
  const output = outputTokens ?? 0;
  const total = totalTokens ?? input + output;
  return {
    input_tokens: inputTokens,
    cached_input_tokens: cachedInputTokens,
    uncached_input_tokens: inputTokens == null ? null : Math.max(input - cached, 0),
    output_tokens: outputTokens,
    reasoning_output_tokens: reasoningOutputTokens,
    total_tokens: total,
    total_uncached_tokens:
      inputTokens == null || outputTokens == null ? totalTokens : Math.max(input - cached, 0) + output,
  };
}

function firstNumber(value, keys) {
  for (const key of keys) {
    const number = numberOrNull(value[key]);
    if (number != null) return number;
  }
  return null;
}

function numberOrNull(value) {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
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

function buildEvidence() {
  const commandExecutions = [];
  const diffs = [];
  const finalAnswers = [];

  for (const event of events) {
    const payload = event.payload ?? {};
    const item = payload.item;
    if (event.type === "item/completed" && item?.type === "commandExecution") {
      commandExecutions.push({
        id: item.id ?? null,
        cwd: relativePath(item.cwd),
        command: item.command ?? null,
        exit_code: item.exitCode ?? null,
        duration_ms: item.durationMs ?? null,
        output_excerpt: excerpt(item.aggregatedOutput),
      });
    } else if (event.type === "turn/diff/updated" && typeof payload.diff === "string") {
      diffs.push(excerpt(payload.diff));
    } else if (
      event.type === "item/completed" &&
      item?.type === "agentMessage" &&
      item.phase === "final_answer"
    ) {
      finalAnswers.push(excerpt(item.text));
    }
  }

  return {
    command_executions: commandExecutions,
    diffs,
    final_answers: finalAnswers,
  };
}

function relativePath(path) {
  if (!path) return null;
  const absolute = resolve(path);
  if (absolute === cwd) return ".";
  if (absolute.startsWith(`${cwd}/`)) return absolute.slice(cwd.length + 1);
  return absolute;
}

function excerpt(value) {
  if (value == null) return null;
  const text = String(value);
  if (text.length <= excerptLimit) return text;
  return `${text.slice(0, excerptLimit)}\n...[truncated ${text.length - excerptLimit} chars]`;
}
