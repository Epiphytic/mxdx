import { describe, it, expect } from "vitest";
import {
  CommandEvent,
  CommandAction,
  OutputEvent,
  OutputStream,
  ResultEvent,
  ResultStatus,
  HostTelemetryEvent,
  SecretRequestEvent,
  SecretResponseEvent,
} from "../index.js";

describe("CommandEvent", () => {
  it("parses a valid command event", () => {
    const json = {
      uuid: "550e8400-e29b-41d4-a716-446655440000",
      action: "exec",
      cmd: "cargo build --release",
      args: ["--features", "gpu"],
      env: { RUST_LOG: "info" },
      cwd: "/workspace",
      timeout_seconds: 3600,
    };
    const result = CommandEvent.parse(json);
    expect(result.uuid).toBe("550e8400-e29b-41d4-a716-446655440000");
    expect(result.action).toBe("exec");
    expect(result.args).toEqual(["--features", "gpu"]);
    expect(result.cwd).toBe("/workspace");
    expect(result.timeout_seconds).toBe(3600);
  });

  it("accepts optional fields as null", () => {
    const json = {
      uuid: "x",
      action: "kill",
      cmd: "x",
      args: [],
      env: {},
      cwd: null,
      timeout_seconds: null,
    };
    const result = CommandEvent.parse(json);
    expect(result.cwd).toBeNull();
    expect(result.timeout_seconds).toBeNull();
  });

  it("accepts missing optional fields", () => {
    const json = {
      uuid: "x",
      action: "signal",
      cmd: "x",
      args: [],
      env: {},
    };
    const result = CommandEvent.parse(json);
    expect(result.cwd).toBeUndefined();
    expect(result.timeout_seconds).toBeUndefined();
  });

  it("rejects unknown action", () => {
    const json = {
      uuid: "x",
      action: "fly_to_moon",
      cmd: "x",
      args: [],
      env: {},
    };
    expect(() => CommandEvent.parse(json)).toThrow();
  });

  it("rejects missing required fields", () => {
    const json = { uuid: "x" };
    expect(() => CommandEvent.parse(json)).toThrow();
  });
});

describe("OutputEvent", () => {
  it("parses stdout output event", () => {
    const json = {
      uuid: "test-1",
      stream: "stdout",
      data: "aGVsbG8=",
      encoding: "raw+base64",
      seq: 0,
    };
    const result = OutputEvent.parse(json);
    expect(result.stream).toBe("stdout");
    expect(result.data).toBe("aGVsbG8=");
    expect(result.seq).toBe(0);
  });

  it("parses stderr output event", () => {
    const json = {
      uuid: "test-1",
      stream: "stderr",
      data: "ZXJyb3I=",
      encoding: "raw+base64",
      seq: 1,
    };
    const result = OutputEvent.parse(json);
    expect(result.stream).toBe("stderr");
  });

  it("rejects invalid stream value", () => {
    const json = {
      uuid: "x",
      stream: "stdwhat",
      data: "",
      encoding: "raw",
      seq: 0,
    };
    expect(() => OutputEvent.parse(json)).toThrow();
  });
});

describe("ResultEvent", () => {
  it("parses exit result", () => {
    const json = {
      uuid: "test-result-1",
      status: "exit",
      exit_code: 0,
      summary: "Build succeeded",
    };
    const result = ResultEvent.parse(json);
    expect(result.status).toBe("exit");
    expect(result.exit_code).toBe(0);
    expect(result.summary).toBe("Build succeeded");
  });

  it("parses timeout result", () => {
    const json = {
      uuid: "test-result-2",
      status: "timeout",
      exit_code: null,
      summary: "Command timed out after 3600s",
    };
    const result = ResultEvent.parse(json);
    expect(result.status).toBe("timeout");
    expect(result.exit_code).toBeNull();
  });

  it("rejects unknown status", () => {
    const json = {
      uuid: "x",
      status: "exploded",
      exit_code: null,
      summary: null,
    };
    expect(() => ResultEvent.parse(json)).toThrow();
  });
});

describe("HostTelemetryEvent", () => {
  const baseTelemetry = {
    timestamp: "2026-03-05T12:00:00Z",
    hostname: "worker-01",
    os: "linux",
    arch: "x86_64",
    uptime_seconds: 86400,
    load_avg: [0.5, 0.3, 0.1],
    cpu: { cores: 8, usage_percent: 45.2 },
    memory: { total_bytes: 16_000_000_000, used_bytes: 8_000_000_000 },
    disk: { total_bytes: 500_000_000_000, used_bytes: 200_000_000_000 },
  };

  it("parses telemetry event without optional fields", () => {
    const result = HostTelemetryEvent.parse(baseTelemetry);
    expect(result.hostname).toBe("worker-01");
    expect(result.cpu.cores).toBe(8);
    expect(result.memory.total_bytes).toBe(16_000_000_000);
    expect(result.uptime_seconds).toBe(86400);
    expect(result.load_avg).toEqual([0.5, 0.3, 0.1]);
  });

  it("parses telemetry event with network info", () => {
    const json = {
      ...baseTelemetry,
      network: { rx_bytes: 1_000_000, tx_bytes: 500_000 },
    };
    const result = HostTelemetryEvent.parse(json);
    expect(result.network?.rx_bytes).toBe(1_000_000);
    expect(result.network?.tx_bytes).toBe(500_000);
  });

  it("rejects missing required fields", () => {
    const json = { timestamp: "2026-03-05T12:00:00Z", hostname: "x" };
    expect(() => HostTelemetryEvent.parse(json)).toThrow();
  });

  it("rejects load_avg with wrong length", () => {
    const json = { ...baseTelemetry, load_avg: [0.5, 0.3] };
    expect(() => HostTelemetryEvent.parse(json)).toThrow();
  });
});

describe("SecretRequestEvent", () => {
  it("parses a valid secret request", () => {
    const json = {
      request_id: "req-001",
      scope: "github.token",
      ttl_seconds: 3600,
      reason: "CI deployment",
    };
    const result = SecretRequestEvent.parse(json);
    expect(result.request_id).toBe("req-001");
    expect(result.scope).toBe("github.token");
    expect(result.ttl_seconds).toBe(3600);
    expect(result.reason).toBe("CI deployment");
  });

  it("rejects missing fields", () => {
    const json = { request_id: "req-001" };
    expect(() => SecretRequestEvent.parse(json)).toThrow();
  });
});

describe("SecretResponseEvent", () => {
  it("parses a granted response", () => {
    const json = {
      request_id: "req-001",
      granted: true,
      value: "ghp_secret_token_value",
      error: null,
    };
    const result = SecretResponseEvent.parse(json);
    expect(result.granted).toBe(true);
    expect(result.value).toBe("ghp_secret_token_value");
    expect(result.error).toBeNull();
  });

  it("parses a denied response", () => {
    const json = {
      request_id: "req-002",
      granted: false,
      value: null,
      error: "Unauthorized scope",
    };
    const result = SecretResponseEvent.parse(json);
    expect(result.granted).toBe(false);
    expect(result.value).toBeNull();
    expect(result.error).toBe("Unauthorized scope");
  });

  it("rejects missing fields", () => {
    const json = { request_id: "req-001" };
    expect(() => SecretResponseEvent.parse(json)).toThrow();
  });
});
