import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { LauncherConfig } from './config.js';

describe('LauncherConfig', () => {
  let tmpDir;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mxdx-config-test-'));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true });
  });

  it('creates config from CLI args', () => {
    const config = LauncherConfig.fromArgs({
      username: 'belthanior',
      servers: 'matrix.org,mxdx.dev',
      allowedCommands: 'echo,cat',
      telemetry: 'full',
      maxSessions: 10,
    });
    assert.strictEqual(config.username, 'belthanior');
    assert.deepStrictEqual(config.servers, ['matrix.org', 'mxdx.dev']);
    assert.deepStrictEqual(config.allowedCommands, ['echo', 'cat']);
  });

  it('saves and loads TOML config', () => {
    const configPath = path.join(tmpDir, 'launcher.toml');
    const config = LauncherConfig.fromArgs({
      username: 'test-host',
      servers: 'matrix.org',
      allowedCommands: 'echo',
    });
    config.save(configPath);
    assert.ok(fs.existsSync(configPath));

    const loaded = LauncherConfig.load(configPath);
    assert.strictEqual(loaded.username, 'test-host');
    assert.deepStrictEqual(loaded.servers, ['matrix.org']);
  });

  it('returns null when config file does not exist', () => {
    const loaded = LauncherConfig.load(path.join(tmpDir, 'nonexistent.toml'));
    assert.strictEqual(loaded, null);
  });
});
